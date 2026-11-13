//! Files between this computer and the server of one source connection.
//!
//! A pane's process offers a file with `splitty download`; nothing is read
//! before the user's Save, and the file is written only below this user's
//! download directory. Pasted images and files go the other way: each is
//! stored on the pane's own server, which names the path to paste.
use super::*;
use crate::Client;
use base64::Engine as _;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// The largest file either direction carries.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// The largest image `save_image` takes; a larger one is uploaded as a file.
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// The most one `upload` message or `file.chunk` carries, decoded.
const CHUNK_BYTES: usize = 1024 * 1024;
/// Offers asked or read at once on one connection.
const MAX_OPEN_OFFERS: usize = 3;
/// Offer ids remembered so that a repeated offer is never asked twice.
const MAX_SEEN_OFFERS: usize = 256;
/// How long a prompt waits at most, whatever the offer says.
const MAX_PROMPT: Duration = Duration::from_secs(60);
const MAX_NAME_BYTES: usize = 255;
const MAX_PATH_BYTES: usize = 4096;
/// How long the answer to one read or one sent chunk may take.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// Store requests waiting behind the one in progress.
const MAX_QUEUED_STORES: usize = 8;
const UNKNOWN_SAVE_IMAGE: &str = "unknown message type: save_image";
const UNKNOWN_UPLOAD: &str = "unknown message type: upload";

/// One pasted item on its way to a pane's server.
#[derive(Debug)]
pub enum StoreItem {
    /// Clipboard image data; `extension` names its format (`png`, `jpg`, …).
    Image {
        extension: &'static str,
        bytes: Vec<u8>,
    },
    /// A file of this computer.
    File(PathBuf),
}

/// What the user is asked or told about files of one source connection.
#[derive(Debug)]
pub enum FileUpdate {
    /// A pane's process offers a file; nothing is read before `AnswerFileOffer`.
    Offered {
        source: SourceToken,
        offer_id: String,
        name: String,
        size: u64,
        expires_in: Duration,
    },
    /// The offer ended before this window saved it.
    OfferClosed {
        source: SourceToken,
        offer_id: String,
    },
    DownloadProgress {
        source: SourceToken,
        offer_id: String,
        received: u64,
        size: u64,
    },
    Downloaded {
        source: SourceToken,
        offer_id: String,
        path: PathBuf,
    },
    DownloadFailed {
        source: SourceToken,
        offer_id: String,
        reason: String,
    },
    StoreProgress {
        source: SourceToken,
        request_id: uuid::Uuid,
        stored: u64,
        total: u64,
    },
    /// Every item is stored; `paths` are the server's, in item order.
    Stored {
        source: SourceToken,
        request_id: uuid::Uuid,
        paths: Vec<String>,
    },
    StoreFailed {
        source: SourceToken,
        request_id: uuid::Uuid,
        reason: String,
        /// The server's Splitty is older than this message.
        needs_update: bool,
    },
}

/// The name a download is saved under: the last path component as plain
/// text, without characters file systems refuse. As in the web client,
/// controls and the bidirectional embeddings, overrides and isolates go: an
/// offered name must not display in another order than it is saved in.
/// Current servers offer no such name; an older or a hostile one may.
fn download_name(name: &str) -> Option<String> {
    if name.len() > MAX_NAME_BYTES {
        return None;
    }
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            other => other,
        })
        .collect();
    let cleaned = cleaned.trim_start_matches('.').trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

/// A path a server names for a stored file: absolute, plain and bounded.
fn stored_path(value: &Value) -> Option<String> {
    let path = value.as_str()?;
    (path.starts_with('/') && path.len() <= MAX_PATH_BYTES && !path.chars().any(char::is_control))
        .then(|| path.to_owned())
}

/// How files pasted or dropped into a terminal or a scratchpad reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRoute {
    /// Insert the files' own paths: the pane's server reads this computer's
    /// files where they are.
    OwnPaths,
    /// Store the files on the pane's server and insert the stored paths,
    /// without asking: they stay on this computer.
    Store,
    /// Ask before the files leave this computer, then store them.
    AskThenStore,
}

/// The route for files into a pane. `local` is a pane of this computer's
/// own server; `shares_filesystem` says whether that server reads this
/// app's paths as they are, which a server in WSL does not: a Windows path
/// means nothing to its shell. `all_paths` is false when an item is only
/// data, as a clipboard image is.
pub fn file_route(
    local: bool,
    shares_filesystem: bool,
    all_paths: bool,
    dropped: bool,
) -> FileRoute {
    match (local, dropped) {
        (true, _) if shares_filesystem && all_paths => FileRoute::OwnPaths,
        (true, _) | (false, false) => FileRoute::Store,
        (false, true) => FileRoute::AskThenStore,
    }
}

/// What a local pane of a server in WSL gets for one file of this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WslFile {
    /// The file lives in the server's distribution: insert this Linux path.
    Linux(String),
    /// Store the file on the server, as any file of this computer.
    Store,
}

/// The way into a local pane for the file at `path` (as this computer names
/// it), when the local server runs in the WSL distribution `distro`. Explorer
/// names that distribution's files `\\wsl.localhost\<distro>\<rest>` or
/// `\\wsl$\<distro>\<rest>`; such a path of `distro`, in any case, is the
/// Linux path `/<rest>`. Every other path, a drive letter's included, is
/// stored.
pub fn wsl_file(path: &str, distro: Option<&str>) -> WslFile {
    let separator = |c: char| c == '\\' || c == '/';
    let Some(distro) = distro.filter(|distro| !distro.is_empty()) else {
        return WslFile::Store;
    };
    let Some(unc) = path
        .strip_prefix(separator)
        .and_then(|path| path.strip_prefix(separator))
    else {
        return WslFile::Store;
    };
    let mut parts = unc.split(separator);
    let host = parts.next().unwrap_or_default();
    if !host.eq_ignore_ascii_case("wsl.localhost") && !host.eq_ignore_ascii_case("wsl$") {
        return WslFile::Store;
    }
    if parts.next().map(str::to_lowercase) != Some(distro.to_lowercase()) {
        return WslFile::Store;
    }
    let rest: Vec<&str> = parts.filter(|part| !part.is_empty()).collect();
    WslFile::Linux(format!("/{}", rest.join("/")))
}

/// Keep plain paths; quote anything a shell would split or expand.
pub fn quote_path(path: &str) -> String {
    if !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
    {
        path.to_owned()
    } else {
        format!("'{}'", path.replace('\'', r#"'"'"'"#))
    }
}

fn plain(text: Option<&str>, fallback: &str) -> String {
    let text: String = text
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    if text.is_empty() {
        fallback.to_owned()
    } else {
        text
    }
}

/// Where downloads go: the user's download directory, else their home.
fn download_dir() -> io::Result<PathBuf> {
    let dir = dirs::download_dir()
        .filter(|dir| dir.is_dir())
        .or_else(dirs::home_dir)
        .ok_or_else(|| io::Error::other("This computer has no download folder"))?;
    Ok(dir)
}

struct Download {
    file: std::fs::File,
    part: PathBuf,
    dir: PathBuf,
    received: u64,
    finished: bool,
}
impl Drop for Download {
    fn drop(&mut self) {
        if !self.finished {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}
impl Download {
    fn begin(dir: PathBuf, name: &str) -> io::Result<Self> {
        let part = dir.join(format!(
            ".{name}.{}.splitty-part",
            uuid::Uuid::new_v4().simple()
        ));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)?;
        Ok(Self {
            file,
            part,
            dir,
            received: 0,
            finished: false,
        })
    }

    /// Give the finished file a name of its own; nothing is ever replaced.
    fn finish(mut self, name: &str) -> io::Result<PathBuf> {
        self.file.flush()?;
        let (stem, extension) = match name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
            _ => (name, String::new()),
        };
        for attempt in 0..1000u32 {
            let candidate = self.dir.join(if attempt == 0 {
                name.to_owned()
            } else {
                format!("{stem} ({attempt}){extension}")
            });
            match std::fs::hard_link(&self.part, &candidate) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&self.part);
                    self.finished = true;
                    return Ok(candidate);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) if !candidate.exists() => {
                    // A file system without hard links.
                    std::fs::rename(&self.part, &candidate)?;
                    self.finished = true;
                    return Ok(candidate);
                }
                Err(_) => continue,
            }
        }
        Err(io::Error::other("No free file name in the download folder"))
    }
}

struct Offer {
    name: String,
    size: u64,
    download: Option<Download>,
    /// When the answer to the last `file.read` is overdue.
    deadline: Option<Instant>,
}

enum Body {
    Memory(Vec<u8>),
    File(std::fs::File),
}
impl Body {
    fn chunk(&mut self, offset: u64, size: u64) -> io::Result<Vec<u8>> {
        let len = usize::try_from((size - offset).min(CHUNK_BYTES as u64)).expect("bounded chunk");
        match self {
            Body::Memory(bytes) => {
                let start = usize::try_from(offset).expect("bounded offset");
                Ok(bytes[start..start + len].to_vec())
            }
            Body::File(file) => {
                let mut chunk = vec![0; len];
                file.read_exact(&mut chunk)
                    .map_err(|_| io::Error::other("The file changed while it was sent"))?;
                Ok(chunk)
            }
        }
    }
}

enum Sending {
    Image {
        id: String,
        size: u64,
    },
    Upload {
        id: String,
        body: Body,
        size: u64,
        /// The end of the chunk whose answer is awaited.
        sent: u64,
    },
}

enum Prepared {
    Image(Vec<u8>),
    Upload { name: String, body: Body, size: u64 },
}

struct StoreJob {
    request_id: uuid::Uuid,
    items: VecDeque<Prepared>,
    paths: Vec<String>,
    total: u64,
    stored: u64,
    sending: Option<Sending>,
    /// When the answer to what was last sent is overdue.
    deadline: Option<Instant>,
    /// The directory the user chose for the files, else temporary storage.
    directory: Option<String>,
}

#[derive(Default)]
pub(super) struct FileTransfers {
    offers_supported: bool,
    upload_control: bool,
    offers: HashMap<String, Offer>,
    seen: VecDeque<String>,
    jobs: VecDeque<StoreJob>,
}

fn image_extension(extension: &str) -> bool {
    matches!(extension, "png" | "jpg" | "jpeg" | "gif" | "webp")
}

fn prepare(item: StoreItem) -> Result<Prepared, String> {
    match item {
        StoreItem::Image { extension, bytes } => {
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err("The image is larger than 512 MB.".into());
            }
            if image_extension(extension) && bytes.len() <= MAX_IMAGE_BYTES {
                Ok(Prepared::Image(bytes))
            } else {
                let size = bytes.len() as u64;
                Ok(Prepared::Upload {
                    name: format!("image.{extension}"),
                    body: Body::Memory(bytes),
                    size,
                })
            }
        }
        StoreItem::File(path) => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let unreadable = |error: io::Error| format!("{name} could not be read: {error}");
            let not_a_file =
                || format!("{name} is not a file. Folders cannot be pasted; archive them first.");
            // Checked before opening: Windows refuses to open a directory
            // ("Access is denied"), and opening a FIFO would block.
            if !std::fs::metadata(&path).map_err(unreadable)?.is_file() {
                return Err(not_a_file());
            }
            let file = std::fs::File::open(&path).map_err(unreadable)?;
            let metadata = file.metadata().map_err(unreadable)?;
            if !metadata.is_file() {
                return Err(not_a_file());
            }
            if metadata.len() > MAX_FILE_BYTES {
                return Err(format!("{name} is larger than 512 MB."));
            }
            Ok(Prepared::Upload {
                name,
                body: Body::File(file),
                size: metadata.len(),
            })
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveImage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: &'a str,
    data: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadChunk<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    request_id: &'a str,
    offset: u64,
    data: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    /// With the first chunk: an existing absolute directory of the server
    /// that takes the file instead of temporary storage.
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<&'a str>,
}

fn send<T: serde::Serialize>(client: &Client, message: &T) -> io::Result<()> {
    client
        .sender()
        .json(splitty_transport::wire::Kind::Text, message)
}

impl FileTransfers {
    pub fn welcome(&mut self, client: &Client, json: &Value) {
        self.upload_control = capability::offered(json, capability::UPLOAD_CONTROL);
        self.offers_supported = capability::offered(json, capability::FILE_OFFER)
            && send(client, &json!({"type":"file.register"})).is_ok();
    }

    fn remember(&mut self, id: &str) -> bool {
        if self.seen.iter().any(|seen| seen == id) {
            return false;
        }
        if self.seen.len() >= MAX_SEEN_OFFERS {
            self.seen.pop_front();
        }
        self.seen.push_back(id.to_owned());
        true
    }

    /// One server message about a file; `attached` are this connection's
    /// sessions. True when an `error` answered a file request.
    pub fn event(
        &mut self,
        client: &Client,
        source: SourceToken,
        attached: &HashSet<String>,
        json: &Value,
        out: &mut Vec<Update>,
    ) -> bool {
        self.observe(client, source, attached, json, out).is_some()
    }

    fn observe(
        &mut self,
        client: &Client,
        source: SourceToken,
        attached: &HashSet<String>,
        json: &Value,
        out: &mut Vec<Update>,
    ) -> Option<()> {
        match json["type"].as_str() {
            Some("file.offer") => self.offered(client, source, attached, json, out),
            Some("file.chunk") => self.chunk(client, source, json, out),
            Some("file.closed") => {
                let id = json["offerId"].as_str()?;
                let offer = self.offers.remove(id)?;
                out.push(Update::Files(if offer.download.is_some() {
                    FileUpdate::DownloadFailed {
                        source,
                        offer_id: id.to_owned(),
                        reason: "The server ended the download.".into(),
                    }
                } else {
                    FileUpdate::OfferClosed {
                        source,
                        offer_id: id.to_owned(),
                    }
                }));
            }
            Some("image_saved") | Some("upload_saved") => {
                let image = json["type"] == "image_saved";
                if !self.answers(json, image) {
                    return None;
                }
                match stored_path(&json["path"]) {
                    Some(path) => self.item_stored(client, source, path, out),
                    None => self.fail(
                        client,
                        source,
                        "The server named no usable file.".into(),
                        false,
                        out,
                    ),
                }
            }
            Some("image_failed") | Some("upload_failed") => {
                if self.answers(json, json["type"] == "image_failed") {
                    let reason = plain(json["error"].as_str(), "The server refused the file.");
                    self.fail(client, source, reason, false, out);
                }
            }
            Some("upload_progress") => {
                if !self.answers(json, false) {
                    return None;
                }
                let expected = match self.jobs.front().and_then(|job| job.sending.as_ref()) {
                    Some(Sending::Upload { sent, size, .. }) if sent < size => Some(*sent),
                    _ => None,
                };
                if json["received"].as_u64().is_some() && json["received"].as_u64() == expected {
                    let received = expected.expect("matched progress");
                    if let Some(job) = self.jobs.front() {
                        out.push(Update::Files(FileUpdate::StoreProgress {
                            source,
                            request_id: job.request_id,
                            stored: job.stored + received,
                            total: job.total,
                        }));
                    }
                    match self.send_chunk(client) {
                        Ok(()) => {
                            if let Some(job) = self.jobs.front_mut() {
                                job.deadline = Some(Instant::now() + ANSWER_TIMEOUT);
                            }
                        }
                        Err(error) => self.fail(client, source, error.to_string(), false, out),
                    }
                } else {
                    self.fail(
                        client,
                        source,
                        "The server lost track of the upload.".into(),
                        false,
                        out,
                    );
                }
            }
            Some("error") if json["sessionId"].is_null() => {
                let image = match json["error"].as_str() {
                    Some(UNKNOWN_SAVE_IMAGE) => true,
                    Some(UNKNOWN_UPLOAD) => false,
                    _ => return None,
                };
                let sending_image = matches!(
                    self.jobs.front().and_then(|job| job.sending.as_ref()),
                    Some(Sending::Image { .. })
                );
                if self.jobs.front().is_some_and(|job| job.sending.is_some())
                    && image == sending_image
                {
                    let reason = if image {
                        "This server's Splitty is too old to take images."
                    } else {
                        "This server's Splitty is too old to take files."
                    };
                    self.fail(client, source, reason.into(), true, out);
                    return Some(());
                }
                return None;
            }
            _ => {}
        }
        None
    }

    fn offered(
        &mut self,
        client: &Client,
        source: SourceToken,
        attached: &HashSet<String>,
        json: &Value,
        out: &mut Vec<Update>,
    ) {
        let Some(id) = json["offerId"].as_str().filter(|id| {
            id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }) else {
            return;
        };
        if !self.offers_supported || !self.remember(id) {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let remaining = json["expiresAt"]
            .as_u64()
            .and_then(|expires| expires.checked_sub(now))
            .filter(|remaining| *remaining > 0)
            .map(|remaining| Duration::from_millis(remaining).min(MAX_PROMPT));
        let name = json["name"].as_str().and_then(download_name);
        let size = json["size"].as_u64().filter(|size| *size <= MAX_FILE_BYTES);
        let shown = json["sessionId"]
            .as_str()
            .is_some_and(|session| attached.contains(session));
        let (Some(expires_in), Some(name), Some(size), true) = (remaining, name, size, shown)
        else {
            let _ = send(client, &json!({"type":"file.decline","offerId":id}));
            return;
        };
        if self.offers.len() >= MAX_OPEN_OFFERS {
            let _ = send(client, &json!({"type":"file.decline","offerId":id}));
            return;
        }
        self.offers.insert(
            id.to_owned(),
            Offer {
                name: name.clone(),
                size,
                download: None,
                deadline: None,
            },
        );
        out.push(Update::Files(FileUpdate::Offered {
            source,
            offer_id: id.to_owned(),
            name,
            size,
            expires_in,
        }));
    }

    /// The user's answer to an offer; declining an accepted one stops it.
    pub fn answer(
        &mut self,
        client: &Client,
        source: SourceToken,
        offer_id: &str,
        save: bool,
        out: &mut Vec<Update>,
    ) {
        let Some(offer) = self.offers.get_mut(offer_id) else {
            return;
        };
        if !save {
            self.offers.remove(offer_id);
            let _ = send(client, &json!({"type":"file.decline","offerId":offer_id}));
            return;
        }
        if offer.download.is_some() {
            return;
        }
        let started = download_dir()
            .and_then(|dir| Download::begin(dir, &offer.name))
            .and_then(|download| {
                send(client, &json!({"type":"file.accept","offerId":offer_id}))?;
                send(
                    client,
                    &json!({"type":"file.read","offerId":offer_id,"offset":0}),
                )?;
                Ok(download)
            });
        match started {
            Ok(download) => {
                let size = offer.size;
                offer.download = Some(download);
                offer.deadline = Some(Instant::now() + ANSWER_TIMEOUT);
                out.push(Update::Files(FileUpdate::DownloadProgress {
                    source,
                    offer_id: offer_id.to_owned(),
                    received: 0,
                    size,
                }));
            }
            Err(error) => {
                self.offers.remove(offer_id);
                let _ = send(client, &json!({"type":"file.decline","offerId":offer_id}));
                out.push(Update::Files(FileUpdate::DownloadFailed {
                    source,
                    offer_id: offer_id.to_owned(),
                    reason: error.to_string(),
                }));
            }
        }
    }

    fn chunk(&mut self, client: &Client, source: SourceToken, json: &Value, out: &mut Vec<Update>) {
        let Some(id) = json["offerId"].as_str() else {
            return;
        };
        let Some(offer) = self.offers.get_mut(id) else {
            return;
        };
        let size = offer.size;
        offer.deadline = Some(Instant::now() + ANSWER_TIMEOUT);
        let Some(download) = offer.download.as_mut() else {
            return;
        };
        if json["offset"].as_u64() != Some(download.received) {
            return;
        }
        let written = json["data"]
            .as_str()
            .filter(|data| data.len() <= CHUNK_BYTES.div_ceil(3) * 4)
            .and_then(|data| base64::engine::general_purpose::STANDARD.decode(data).ok())
            .filter(|bytes| download.received + bytes.len() as u64 <= size)
            .ok_or_else(|| "The server sent more than it offered.".to_owned())
            .and_then(|bytes| {
                let complete = download.received + bytes.len() as u64 == size;
                if json["eof"].as_bool() != Some(complete) || (!complete && bytes.is_empty()) {
                    return Err("The server sent a file of another size than it offered.".into());
                }
                download
                    .file
                    .write_all(&bytes)
                    .map_err(|error| format!("The file could not be written: {error}"))?;
                download.received += bytes.len() as u64;
                Ok(complete)
            });
        let offer_id = id.to_owned();
        match written {
            Ok(false) => {
                let received = download.received;
                let _ = send(
                    client,
                    &json!({"type":"file.read","offerId":id,"offset":received}),
                );
                out.push(Update::Files(FileUpdate::DownloadProgress {
                    source,
                    offer_id,
                    received,
                    size,
                }));
            }
            Ok(true) => {
                let offer = self.offers.remove(id).expect("current offer");
                let download = offer.download.expect("current download");
                out.push(Update::Files(match download.finish(&offer.name) {
                    Ok(path) => FileUpdate::Downloaded {
                        source,
                        offer_id,
                        path,
                    },
                    Err(error) => FileUpdate::DownloadFailed {
                        source,
                        offer_id,
                        reason: format!("The file could not be saved: {error}"),
                    },
                }));
            }
            Err(reason) => {
                self.offers.remove(id);
                let _ = send(client, &json!({"type":"file.decline","offerId":id}));
                out.push(Update::Files(FileUpdate::DownloadFailed {
                    source,
                    offer_id,
                    reason,
                }));
            }
        }
    }

    /// Store `items` one after the other; nothing is sent when one is refused.
    #[allow(clippy::too_many_arguments)] // mirrors the fields of `CommandKind::StoreFiles`
    pub fn store(
        &mut self,
        client: &Client,
        source: SourceToken,
        request_id: uuid::Uuid,
        items: Vec<StoreItem>,
        dropped: bool,
        directory: Option<String>,
        out: &mut Vec<Update>,
    ) {
        if directory.as_deref().is_some_and(|dir| {
            !dir.starts_with('/') || dir.len() > MAX_PATH_BYTES || dir.chars().any(char::is_control)
        }) {
            out.push(Update::Files(FileUpdate::StoreFailed {
                source,
                request_id,
                reason: "The destination is not an absolute directory.".into(),
                needs_update: false,
            }));
            return;
        }
        if (dropped || directory.is_some()) && !self.upload_control {
            out.push(Update::Files(FileUpdate::StoreFailed {
                source,
                request_id,
                reason: "Update this host's Splitty to send dropped files.".into(),
                needs_update: true,
            }));
            return;
        }
        let refused = |reason: String, out: &mut Vec<Update>| {
            out.push(Update::Files(FileUpdate::StoreFailed {
                source,
                request_id,
                reason,
                needs_update: false,
            }));
        };
        if self.jobs.len() > MAX_QUEUED_STORES {
            return refused("Too many files are waiting to be stored.".into(), out);
        }
        let mut prepared = VecDeque::new();
        let mut total = 0u64;
        for item in items {
            match prepare(item) {
                Ok(item) => {
                    total += match &item {
                        Prepared::Image(bytes) => bytes.len() as u64,
                        Prepared::Upload { size, .. } => *size,
                    };
                    prepared.push_back(item);
                }
                Err(reason) => return refused(reason, out),
            }
        }
        self.jobs.push_back(StoreJob {
            request_id,
            items: prepared,
            paths: Vec::new(),
            total,
            stored: 0,
            sending: None,
            deadline: None,
            directory,
        });
        if self.jobs.len() == 1 {
            self.advance(client, source, out);
        }
    }

    /// Give up on answers that are overdue: a server that stopped answering
    /// must not hold a download or a paste open forever.
    pub fn expire(&mut self, client: &Client, source: SourceToken, out: &mut Vec<Update>) {
        let now = Instant::now();
        let stalled: Vec<String> = self
            .offers
            .iter()
            .filter(|(_, offer)| {
                offer.download.is_some() && offer.deadline.is_some_and(|deadline| now >= deadline)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for offer_id in stalled {
            self.offers.remove(&offer_id);
            let _ = send(client, &json!({"type":"file.decline","offerId":offer_id}));
            out.push(Update::Files(FileUpdate::DownloadFailed {
                source,
                offer_id,
                reason: "The server stopped sending the file.".into(),
            }));
        }
        if self
            .jobs
            .front()
            .is_some_and(|job| job.deadline.is_some_and(|deadline| now >= deadline))
        {
            self.fail(
                client,
                source,
                "The server did not answer.".into(),
                false,
                out,
            );
        }
    }

    /// The user gave up on a store request; what was stored stays.
    pub fn cancel(
        &mut self,
        client: &Client,
        source: SourceToken,
        request_id: uuid::Uuid,
        out: &mut Vec<Update>,
    ) {
        if self
            .jobs
            .front()
            .is_none_or(|job| job.request_id != request_id)
        {
            self.jobs.retain(|job| job.request_id != request_id);
            return;
        }
        let job = self.jobs.pop_front().expect("current job");
        if let (Some(Sending::Upload { id, .. }), true) = (&job.sending, self.upload_control) {
            let _ = send(client, &json!({"type":"upload_cancel","requestId":id}));
        }
        self.advance(client, source, out);
    }

    /// Whether `json` answers the message the current job waits for.
    fn answers(&self, json: &Value, image: bool) -> bool {
        match self.jobs.front().and_then(|job| job.sending.as_ref()) {
            Some(Sending::Image { id, .. }) => image && json["requestId"] == id.as_str(),
            Some(Sending::Upload { id, .. }) => !image && json["requestId"] == id.as_str(),
            None => false,
        }
    }

    fn item_stored(
        &mut self,
        client: &Client,
        source: SourceToken,
        path: String,
        out: &mut Vec<Update>,
    ) {
        let Some(job) = self.jobs.front_mut() else {
            return;
        };
        match job.sending.take() {
            Some(Sending::Image { size, .. }) => job.stored += size,
            Some(Sending::Upload { size, sent, .. }) if sent == size => job.stored += size,
            Some(Sending::Upload { .. }) => {
                return self.fail(
                    client,
                    source,
                    "The server stored the file before all of it was sent.".into(),
                    false,
                    out,
                );
            }
            None => return,
        }
        job.paths.push(path);
        out.push(Update::Files(FileUpdate::StoreProgress {
            source,
            request_id: job.request_id,
            stored: job.stored,
            total: job.total,
        }));
        self.advance(client, source, out);
    }

    /// Send the current job's next item, or finish it and start the next job.
    fn advance(&mut self, client: &Client, source: SourceToken, out: &mut Vec<Update>) {
        loop {
            let Some(job) = self.jobs.front_mut() else {
                return;
            };
            let Some(item) = job.items.pop_front() else {
                let job = self.jobs.pop_front().expect("current job");
                out.push(Update::Files(FileUpdate::Stored {
                    source,
                    request_id: job.request_id,
                    paths: job.paths,
                }));
                continue;
            };
            let sent = match item {
                Prepared::Image(bytes) => {
                    let id = uuid::Uuid::new_v4().to_string();
                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    let size = bytes.len() as u64;
                    drop(bytes);
                    job.sending = Some(Sending::Image {
                        id: id.clone(),
                        size,
                    });
                    send(
                        client,
                        &SaveImage {
                            kind: "save_image",
                            request_id: &id,
                            data: &data,
                        },
                    )
                }
                Prepared::Upload { name, body, size } => {
                    job.sending = Some(Sending::Upload {
                        id: format!("up-{}", uuid::Uuid::new_v4()),
                        body,
                        size,
                        sent: 0,
                    });
                    self.send_first_chunk(client, &name)
                }
            };
            if let Err(error) = sent {
                self.fail(client, source, error.to_string(), false, out);
                continue;
            }
            if let Some(job) = self.jobs.front_mut() {
                job.deadline = Some(Instant::now() + ANSWER_TIMEOUT);
            }
            return;
        }
    }

    fn send_first_chunk(&mut self, client: &Client, name: &str) -> io::Result<()> {
        self.send_upload(client, Some(name))
    }

    fn send_chunk(&mut self, client: &Client) -> io::Result<()> {
        self.send_upload(client, None)
    }

    fn send_upload(&mut self, client: &Client, name: Option<&str>) -> io::Result<()> {
        let directory = self
            .jobs
            .front()
            .and_then(|job| job.directory.clone())
            .filter(|_| name.is_some());
        let Some(Sending::Upload {
            id,
            body,
            size,
            sent,
        }) = self.jobs.front_mut().and_then(|job| job.sending.as_mut())
        else {
            return Ok(());
        };
        let offset = *sent;
        let chunk = body.chunk(offset, *size)?;
        let data = base64::engine::general_purpose::STANDARD.encode(&chunk);
        *sent = offset + chunk.len() as u64;
        send(
            client,
            &UploadChunk {
                kind: "upload",
                request_id: id,
                offset,
                data: &data,
                name,
                size: name.map(|_| *size),
                dir: directory.as_deref(),
            },
        )
    }

    /// End the current job without pasting anything and start the next.
    fn fail(
        &mut self,
        client: &Client,
        source: SourceToken,
        reason: String,
        needs_update: bool,
        out: &mut Vec<Update>,
    ) {
        let Some(job) = self.jobs.pop_front() else {
            return;
        };
        if let (Some(Sending::Upload { id, .. }), true) = (&job.sending, self.upload_control) {
            let _ = send(client, &json!({"type":"upload_cancel","requestId":id}));
        }
        out.push(Update::Files(FileUpdate::StoreFailed {
            source,
            request_id: job.request_id,
            reason,
            needs_update,
        }));
        self.advance(client, source, out);
    }
}

#[cfg(test)]
mod tests {
    use super::{download_name, quote_path, Download};
    use std::io::Write as _;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "splitty-files-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn finished_downloads_never_replace_a_file_and_unfinished_ones_leave_nothing() {
        let dir = scratch_dir("download");
        std::fs::write(dir.join("report.pdf"), b"mine").unwrap();
        let mut download = Download::begin(dir.clone(), "report.pdf").unwrap();
        download.file.write_all(b"offered").unwrap();
        let saved = download.finish("report.pdf").unwrap();
        assert_eq!(saved, dir.join("report (1).pdf"));
        assert_eq!(std::fs::read(&saved).unwrap(), b"offered");
        assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"mine");

        let abandoned = Download::begin(dir.clone(), "notes").unwrap();
        let part = abandoned.part.clone();
        assert!(part.exists());
        drop(abandoned);
        assert!(
            !part.exists(),
            "a cancelled download leaves no partial file"
        );
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["report (1).pdf", "report.pdf"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A local pane takes this computer's paths only where its server reads
    /// them; a pane of a server in WSL gets the files stored, as a host's
    /// pane does, but without the question a host's drop asks.
    #[test]
    fn files_reach_a_pane_where_its_server_can_read_them() {
        use super::{file_route, FileRoute};
        for dropped in [false, true] {
            assert_eq!(file_route(true, true, true, dropped), FileRoute::OwnPaths);
            assert_eq!(file_route(true, true, false, dropped), FileRoute::Store);
            assert_eq!(file_route(true, false, true, dropped), FileRoute::Store);
            assert_eq!(file_route(true, false, false, dropped), FileRoute::Store);
        }
        for shares in [false, true] {
            assert_eq!(file_route(false, shares, true, false), FileRoute::Store);
            assert_eq!(
                file_route(false, shares, true, true),
                FileRoute::AskThenStore
            );
        }
    }

    /// A folder gets the folder message, also where it cannot be opened:
    /// Windows refuses to open any directory as a file, and Unix one without
    /// read permission.
    #[test]
    fn a_folder_is_refused_as_a_folder_before_it_is_opened() {
        use super::{prepare, StoreItem};
        let dir = scratch_dir("folder");
        let open = dir.join("open");
        let closed = dir.join("closed");
        std::fs::create_dir(&open).unwrap();
        std::fs::create_dir(&closed).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        for (path, name) in [(open, "open"), (closed.clone(), "closed")] {
            let Err(message) = prepare(StoreItem::File(path)) else {
                panic!("a folder was prepared for upload");
            };
            assert_eq!(
                message,
                format!("{name} is not a file. Folders cannot be pasted; archive them first.")
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn download_names_keep_only_the_last_plain_component() {
        assert_eq!(
            download_name("build/report.pdf").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(download_name("..\\..\\.ssh").as_deref(), Some("ssh"));
        assert_eq!(download_name("a:b?.txt").as_deref(), Some("a_b_.txt"));
        assert_eq!(download_name("dir/"), None);
        assert_eq!(download_name(".."), None);
        assert_eq!(download_name(&"x".repeat(300)), None);
        // A name that would display reversed is saved as it reads.
        assert_eq!(
            download_name("invoice\u{202e}fdp.exe").as_deref(),
            Some("invoicefdp.exe")
        );
        assert_eq!(
            download_name("\u{2066}report\u{2069}.pdf\u{202c}").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(download_name("\u{202e}\u{2067}"), None);
    }

    #[test]
    fn files_of_the_local_distribution_are_referenced_by_their_linux_path() {
        use super::{wsl_file, WslFile};
        let linux = |path: &str| WslFile::Linux(path.to_owned());
        let ubuntu = Some("Ubuntu-24.04");
        assert_eq!(
            wsl_file(r"\\wsl.localhost\Ubuntu-24.04\home\user\spec.pdf", ubuntu),
            linux("/home/user/spec.pdf")
        );
        assert_eq!(
            wsl_file(r"\\wsl$\Ubuntu-24.04\home\user\spec.pdf", ubuntu),
            linux("/home/user/spec.pdf")
        );
        assert_eq!(
            wsl_file(r"\\WSL.LocalHost\ubuntu-24.04\Home\User\Spec.PDF", ubuntu),
            linux("/Home/User/Spec.PDF"),
            "host and distribution in any case, the rest as it is"
        );
        assert_eq!(
            wsl_file(r"\\Wsl$\UBUNTU-24.04\tmp\x", ubuntu),
            linux("/tmp/x")
        );
        assert_eq!(
            wsl_file(
                r"\\wsl.localhost\Ubuntu-24.04\home\user\My Report 1.pdf",
                ubuntu
            ),
            linux("/home/user/My Report 1.pdf")
        );
        assert_eq!(
            wsl_file(r"\\wsl$\Ubuntu-24.04\home\jörg\Übersicht 日本.md", ubuntu),
            linux("/home/jörg/Übersicht 日本.md")
        );
        assert_eq!(
            wsl_file("//wsl.localhost/Ubuntu-24.04/home/user/a", ubuntu),
            linux("/home/user/a")
        );
        // The share root is the distribution's root.
        assert_eq!(wsl_file(r"\\wsl$\Ubuntu-24.04", ubuntu), linux("/"));
        assert_eq!(
            wsl_file(r"\\wsl.localhost\Ubuntu-24.04\", ubuntu),
            linux("/")
        );
        for stored in [
            r"\\wsl.localhost\Debian\home\user\spec.pdf",
            r"\\wsl$\Ubuntu\home\user\spec.pdf",
            r"\\wsl$\Ubuntu-24.04.1\home",
            r"C:\Users\user\spec.pdf",
            r"C:\Users\user\My Report.pdf",
            r"\\fileserver\Ubuntu-24.04\home",
            r"\\?\UNC\wsl.localhost\Ubuntu-24.04\home",
            r"\\wsl$\",
            r"\\wsl$",
            r"\\\Ubuntu-24.04\home",
            "/home/user/spec.pdf",
            "",
        ] {
            assert_eq!(wsl_file(stored, ubuntu), WslFile::Store, "{stored}");
        }
        assert_eq!(
            wsl_file(r"\\wsl$\Ubuntu-24.04\home", None),
            WslFile::Store,
            "a server outside WSL"
        );
        assert_eq!(wsl_file(r"\\wsl$\\home", Some("")), WslFile::Store);
    }

    #[test]
    fn stored_paths_stay_literal_shell_arguments() {
        assert_eq!(quote_path("/tmp/paste-1.png"), "/tmp/paste-1.png");
        assert_eq!(quote_path("/tmp/My Report.pdf"), "'/tmp/My Report.pdf'");
        assert_eq!(quote_path("/tmp/it's"), r#"'/tmp/it'"'"'s'"#);
        assert_eq!(quote_path("/tmp/$(id)"), "'/tmp/$(id)'");
    }
}
