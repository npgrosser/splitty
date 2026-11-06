//! `viewer.read`: a viewer pane's document in chunks, over the connection of
//! the window that shows it (`viewer-panes`, Reading A Document). The server
//! keeps nothing between two chunks: every answer names the document's
//! version, and a later chunk names the version of the first, so that a file
//! changed in between fails the read instead of mixing two versions. Reads
//! run off the connection's message loop, at most four at a time.
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use splitty_core::ids::PaneId;
use splitty_core::mux::{viewer_path_is_valid, Store, ViewerSource, VIEWER_TEXT_LIMIT};
use splitty_transport::budget::{ConnectionBudget, Lease};
use tokio::sync::Semaphore;

use super::files::{self, OpenError};
use super::sender::ClientSender;

/// The largest file a viewer pane shows.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// The most text a text document holds, in bytes of UTF-8, the most
/// characters of its title, and the longest reply in a thread: the limits the
/// CLI checks its input against before it sends it.
pub const MAX_TEXT_BYTES: usize = VIEWER_TEXT_LIMIT;
pub const MAX_TITLE_CHARS: usize = splitty_core::mux::VIEWER_TITLE_LIMIT;
pub const MAX_REPLY_BYTES: usize = splitty_core::mux::ANNOTATION_TEXT_LIMIT;
/// The most bytes of a document one `viewer.chunk` carries, decoded.
pub const CHUNK_BYTES: u64 = 1024 * 1024;
/// Reads of one connection in flight at once; `busy` beyond.
const MAX_IN_FLIGHT: usize = 4;
/// The longest `requestId` a read may carry.
const MAX_REQUEST_ID: usize = 128;

/// The `code` of `viewer.failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    UnknownPane,
    NotFound,
    NotAFile,
    TooLarge,
    Unreadable,
    Changed,
    Busy,
    Invalid,
}

impl Code {
    fn message(self) -> &'static str {
        match self {
            Code::UnknownPane => "No viewer pane has this ID",
            Code::NotFound => "The file does not exist",
            Code::NotAFile => "Only regular files can be shown",
            Code::TooLarge => "The document is too large to show",
            Code::Unreadable => "The file cannot be read",
            Code::Changed => "The document changed during the read",
            Code::Busy => "Too many reads are in flight; read again later",
            Code::Invalid => "The read is malformed",
        }
    }
}

/// Where a read starts and which version it expects.
#[derive(Debug)]
struct Position {
    offset: u64,
    /// The version of the first chunk; required after it.
    version: Option<String>,
    /// At offset 0: answer `viewer.unchanged` when the document has it.
    if_version: Option<String>,
}

#[derive(Debug)]
struct ReadRequest {
    request_id: String,
    pane_id: String,
    at: Position,
}

impl ReadRequest {
    /// The read, or the `requestId` to answer `invalid` with.
    fn parse(value: &Value) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Incoming {
            request_id: String,
            pane_id: String,
            offset: u64,
            #[serde(default)]
            version: Option<String>,
            #[serde(default)]
            if_version: Option<String>,
        }
        let echo = value["requestId"]
            .as_str()
            .filter(|id| id.len() <= MAX_REQUEST_ID)
            .unwrap_or_default()
            .to_owned();
        match Incoming::deserialize(value) {
            Ok(read)
                if read.request_id.len() <= MAX_REQUEST_ID
                    && (read.offset == 0 || read.version.is_some()) =>
            {
                Ok(Self {
                    request_id: read.request_id,
                    pane_id: read.pane_id,
                    at: Position {
                        offset: read.offset,
                        version: read.version,
                        if_version: read.if_version,
                    },
                })
            }
            _ => Err(echo),
        }
    }
}

/// A read's answer before its `requestId` is added.
#[derive(Debug)]
enum Answer {
    Chunk {
        offset: u64,
        size: u64,
        version: String,
        data: String,
        eof: bool,
        /// The read's bytes and their Base64, until the answer is queued.
        _scratch: Lease,
    },
    Unchanged {
        version: String,
    },
    Failed(Code),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum Wire<'a> {
    #[serde(rename = "viewer.chunk", rename_all = "camelCase")]
    Chunk {
        request_id: &'a str,
        offset: u64,
        size: u64,
        version: &'a str,
        data: &'a str,
        eof: bool,
    },
    #[serde(rename = "viewer.unchanged", rename_all = "camelCase")]
    Unchanged {
        request_id: &'a str,
        version: &'a str,
    },
    #[serde(rename = "viewer.failed", rename_all = "camelCase")]
    Failed {
        request_id: &'a str,
        code: Code,
        error: &'static str,
    },
}

/// The viewer reads of one connection.
pub(super) struct Reads {
    store: Arc<Store>,
    sender: Arc<ClientSender>,
    in_flight: Arc<Semaphore>,
}

impl Reads {
    pub(super) fn new(store: Arc<Store>, sender: Arc<ClientSender>) -> Self {
        Self {
            store,
            sender,
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        }
    }

    /// Answer `viewer.read` off the message loop; a malformed read, and one
    /// beyond the reads in flight, at once.
    pub(super) fn read(&self, value: &Value) {
        let request = match ReadRequest::parse(value) {
            Ok(request) => request,
            Err(request_id) => {
                return send(&self.sender, &request_id, Answer::Failed(Code::Invalid))
            }
        };
        let Ok(permit) = self.in_flight.clone().try_acquire_owned() else {
            return send(
                &self.sender,
                &request.request_id,
                Answer::Failed(Code::Busy),
            );
        };
        let store = self.store.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let ReadRequest {
                request_id,
                pane_id,
                at,
            } = request;
            let budget = sender.context().budget;
            let answer = match store.viewer_source(&PaneId::from(pane_id)) {
                None => Answer::Failed(Code::UnknownPane),
                Some(ViewerSource::Text { text, .. }) => read_text(&at, &text, &budget),
                Some(ViewerSource::File { path }) => {
                    tokio::task::spawn_blocking(move || read_file(&at, &path, &budget))
                        .await
                        .unwrap_or(Answer::Failed(Code::Unreadable))
                }
            };
            send(&sender, &request_id, answer);
            drop(permit);
        });
    }
}

/// Queue `answer`; a connection without room for it is closed by the sender.
fn send(sender: &ClientSender, request_id: &str, answer: Answer) {
    let wire = match &answer {
        Answer::Chunk {
            offset,
            size,
            version,
            data,
            eof,
            ..
        } => Wire::Chunk {
            request_id,
            offset: *offset,
            size: *size,
            version,
            data,
            eof: *eof,
        },
        Answer::Unchanged { version } => Wire::Unchanged {
            request_id,
            version,
        },
        Answer::Failed(code) => Wire::Failed {
            request_id,
            code: *code,
            error: code.message(),
        },
    };
    if let Err(error) = sender.try_send_json(&wire) {
        tracing::debug!(%error, "a viewer answer was not queued");
    }
}

/// One chunk of the file at `path`, opened anew without waiting on a FIFO or
/// a device. Blocking: call it off the runtime's workers.
fn read_file(at: &Position, path: &str, budget: &ConnectionBudget) -> Answer {
    // `open_viewer` admits no other path; checked again for records that
    // did not pass it.
    if !viewer_path_is_valid(path) {
        return Answer::Failed(Code::Unreadable);
    }
    let (file, metadata) = match files::open_regular(Path::new(path)) {
        Ok(opened) => opened,
        Err(OpenError::Missing) => return Answer::Failed(Code::NotFound),
        Err(OpenError::Unreadable) => return Answer::Failed(Code::Unreadable),
        Err(OpenError::NotAFile) => return Answer::Failed(Code::NotAFile),
    };
    if metadata.len() > MAX_FILE_BYTES {
        return Answer::Failed(Code::TooLarge);
    }
    let version = file_version(&metadata);
    let expected = version.clone();
    chunk(at, metadata.len(), version, budget, |offset, len| {
        let bytes = files::read_at(&file, offset, len).map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => Code::Changed,
            _ => Code::Unreadable,
        })?;
        // A write during the read shows in the opened file's metadata.
        match file.metadata() {
            Ok(after) if file_version(&after) == expected => Ok(bytes),
            Ok(_) => Err(Code::Changed),
            Err(_) => Err(Code::Unreadable),
        }
    })
}

/// One chunk of a `text` document from the pane record.
fn read_text(at: &Position, text: &str, budget: &ConnectionBudget) -> Answer {
    if text.len() > VIEWER_TEXT_LIMIT {
        return Answer::Failed(Code::TooLarge);
    }
    let Ok(size) = u64::try_from(text.len()) else {
        return Answer::Failed(Code::TooLarge);
    };
    let digest = Sha256::digest(text.as_bytes());
    let version = format!("t-{size}-{}", hex::encode(&digest[..8]));
    chunk(at, size, version, budget, |offset, len| {
        let start = usize::try_from(offset).map_err(|_| Code::Invalid)?;
        let len = usize::try_from(len).map_err(|_| Code::Invalid)?;
        start
            .checked_add(len)
            .and_then(|end| text.as_bytes().get(start..end))
            .map(<[u8]>::to_vec)
            .ok_or(Code::Invalid)
    })
}

/// `f-<size>-<mtime ns>-<inode>` of an opened file.
fn file_version(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(metadata);
    #[cfg(not(unix))]
    let inode = 0_u64;
    format!("f-{}-{modified}-{inode}", metadata.len())
}

/// The answer to a read of a document of `size` bytes at `version`, with
/// `read` giving `len` bytes at an offset.
fn chunk(
    at: &Position,
    size: u64,
    version: String,
    budget: &ConnectionBudget,
    read: impl FnOnce(u64, u64) -> Result<Vec<u8>, Code>,
) -> Answer {
    if at.offset == 0 && at.if_version.as_deref() == Some(version.as_str()) {
        return Answer::Unchanged { version };
    }
    if at
        .version
        .as_ref()
        .is_some_and(|expected| expected != &version)
    {
        return Answer::Failed(Code::Changed);
    }
    let Some(left) = size.checked_sub(at.offset) else {
        return Answer::Failed(Code::Invalid);
    };
    let len = left.min(CHUNK_BYTES);
    // Reserve the raw read and its Base64 before either is allocated; a
    // connection whose output is full is answered `busy`.
    let scratch = usize::try_from(len).ok().and_then(|length| {
        length
            .div_ceil(3)
            .checked_mul(4)
            .and_then(|encoded| length.checked_add(encoded))
    });
    let Some(scratch) = scratch.and_then(|bytes| budget.reserve(bytes, false).ok()) else {
        return Answer::Failed(Code::Busy);
    };
    match read(at.offset, len) {
        Ok(bytes) => Answer::Chunk {
            offset: at.offset,
            size,
            version,
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
            eof: len == left,
            _scratch: scratch,
        },
        Err(code) => Answer::Failed(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_session::Message;
    use serde_json::json;
    use splitty_core::ids::ScreenId;
    use splitty_core::mux::Op;
    use splitty_transport::queue::Receiver;
    use std::time::Duration;

    /// Screen `s1` with the terminal `p1` and a viewer pane per document.
    fn store_showing(documents: &[(&str, Value)]) -> Arc<Store> {
        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("s1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .expect("screen");
        for (pane, source) in documents {
            let op = serde_json::from_value(json!({
                "type": "open_viewer", "paneId": "p1", "placement": "tab",
                "newPaneId": pane, "source": source,
            }))
            .expect("wire op");
            store
                .dispatch_screen(&ScreenId::from("s1"), op)
                .expect("viewer");
        }
        Arc::new(store)
    }

    fn file(path: &Path) -> Value {
        json!({"kind": "file", "path": path.to_str().expect("UTF-8 path")})
    }

    fn reads(store: Arc<Store>) -> (Reads, Receiver<Message>) {
        let (sender, rx) = ClientSender::channel();
        (Reads::new(store, Arc::new(sender)), rx)
    }

    async fn answer(rx: &mut Receiver<Message>) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("an answer in time")
            .expect("the connection is open");
        let Message::Text(text) = message.value else {
            panic!("text frame")
        };
        serde_json::from_str(&text).expect("JSON")
    }

    fn decoded(chunk: &Value) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(chunk["data"].as_str().expect("data"))
            .expect("base64")
    }

    /// viewer-panes, Reading A Document (A file in two chunks, Nothing
    /// changed): 1 MiB, then the rest at the next offset with the version.
    #[tokio::test]
    async fn a_file_is_read_in_two_chunks_and_then_unchanged() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("PLAN.md");
        let bytes: Vec<u8> = (0..CHUNK_BYTES + CHUNK_BYTES / 2)
            .map(|i| u8::try_from(i % 251).expect("byte"))
            .collect();
        std::fs::write(&path, &bytes).expect("file");
        let (reads, mut rx) = reads(store_showing(&[("v1", file(&path))]));

        reads.read(&json!({"type": "viewer.read", "requestId": "r1", "paneId": "v1", "offset": 0}));
        let first = answer(&mut rx).await;
        assert_eq!(first["type"], "viewer.chunk", "{first}");
        assert_eq!(first["requestId"], "r1");
        assert_eq!(first["offset"], 0);
        assert_eq!(first["size"], bytes.len());
        assert_eq!(first["eof"], false);
        let version = first["version"].as_str().expect("version").to_owned();
        assert!(
            version.starts_with(&format!("f-{}-", bytes.len())),
            "{version}"
        );
        let mut received = decoded(&first);
        assert_eq!(u64::try_from(received.len()).ok(), Some(CHUNK_BYTES));

        reads.read(&json!({
            "type": "viewer.read", "requestId": "r2", "paneId": "v1",
            "offset": CHUNK_BYTES, "version": version,
        }));
        let second = answer(&mut rx).await;
        assert_eq!(second["type"], "viewer.chunk", "{second}");
        assert_eq!(second["offset"], CHUNK_BYTES);
        assert_eq!(second["eof"], true);
        assert_eq!(second["version"], version.as_str());
        received.extend(decoded(&second));
        assert_eq!(received, bytes);

        reads.read(&json!({
            "type": "viewer.read", "requestId": "r3", "paneId": "v1", "offset": 0, "ifVersion": version,
        }));
        assert_eq!(
            answer(&mut rx).await,
            json!({"type": "viewer.unchanged", "requestId": "r3", "version": version})
        );
    }

    /// viewer-panes, Reading A Document (Changed between two chunks).
    #[tokio::test]
    async fn a_file_replaced_between_two_chunks_fails_the_read() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("PLAN.md");
        std::fs::write(&path, vec![b'a'; 1_500_000]).expect("file");
        let (reads, mut rx) = reads(store_showing(&[("v1", file(&path))]));

        reads.read(&json!({"type": "viewer.read", "requestId": "r1", "paneId": "v1", "offset": 0}));
        let first = answer(&mut rx).await;
        assert_eq!(first["eof"], false, "{first}");
        let replacement = dir.path().join("PLAN.md.new");
        std::fs::write(&replacement, vec![b'b'; 1_500_000]).expect("file");
        std::fs::rename(&replacement, &path).expect("replace");

        reads.read(&json!({
            "type": "viewer.read", "requestId": "r2", "paneId": "v1",
            "offset": CHUNK_BYTES, "version": first["version"],
        }));
        let failed = answer(&mut rx).await;
        assert_eq!(failed["type"], "viewer.failed", "{failed}");
        assert_eq!(failed["requestId"], "r2");
        assert_eq!(failed["code"], "changed");
        assert!(failed.get("data").is_none());

        // A stale `ifVersion` reads the new content in full.
        reads.read(&json!({
            "type": "viewer.read", "requestId": "r3", "paneId": "v1", "offset": 0, "ifVersion": first["version"],
        }));
        let fresh = answer(&mut rx).await;
        assert_eq!(fresh["type"], "viewer.chunk", "{fresh}");
        assert_ne!(fresh["version"], first["version"]);
        assert!(decoded(&fresh).iter().all(|byte| *byte == b'b'));
    }

    /// viewer-panes, Reading A Document (Too large, A FIFO) and the other
    /// file failures, each without data.
    #[tokio::test]
    async fn files_a_viewer_cannot_show_fail_with_their_code() {
        let dir = tempfile::tempdir().expect("dir");
        let large = dir.path().join("app.log");
        std::fs::File::create(&large)
            .and_then(|file| file.set_len(9 * 1024 * 1024))
            .expect("sparse file");
        let missing = dir.path().join("gone.md");
        let at_limit = dir.path().join("limit.log");
        std::fs::File::create(&at_limit)
            .and_then(|file| file.set_len(MAX_FILE_BYTES))
            .expect("sparse file");
        let mut documents = vec![
            ("large", file(&large)),
            ("missing", file(&missing)),
            ("folder", file(dir.path())),
            ("limit", file(&at_limit)),
        ];
        let mut cases = vec![
            ("large", "too_large"),
            ("missing", "not_found"),
            ("folder", "not_a_file"),
            ("dotted", "unreadable"),
            ("p1", "unknown_pane"),
            ("absent", "unknown_pane"),
        ];
        #[cfg(unix)]
        {
            // Answered at once: nothing waits for a writer.
            let fifo = dir.path().join("pipe");
            nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).expect("FIFO");
            documents.push(("fifo", file(&fifo)));
            cases.push(("fifo", "not_a_file"));
        }
        // A path `open_viewer` refuses, in a record that did not pass it, as
        // an edited state.json can hold: the read checks the path itself.
        let mut workspace = store_showing(&documents).get_workspace_state();
        if let Some(screen) = workspace.screens.get_mut(&ScreenId::from("s1")) {
            let mut pane = screen.panes[&PaneId::from("missing")].clone();
            pane.id = PaneId::from("dotted");
            pane.viewer = pane.viewer.map(|mut viewer| {
                viewer.source = ViewerSource::File {
                    path: format!("{}/./gone.md", dir.path().display()),
                };
                viewer
            });
            screen.panes.insert(pane.id.clone(), pane);
        }
        let (reads, mut rx) = reads(Arc::new(Store::with_state(workspace)));
        for (pane, code) in cases {
            reads.read(
                &json!({"type": "viewer.read", "requestId": pane, "paneId": pane, "offset": 0}),
            );
            let failed = answer(&mut rx).await;
            assert_eq!(failed["type"], "viewer.failed", "{pane}: {failed}");
            assert_eq!(failed["requestId"], pane);
            assert_eq!(failed["code"], code, "{pane}: {failed}");
            assert!(failed["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()));
            assert!(failed.get("data").is_none());
        }
        reads.read(
            &json!({"type": "viewer.read", "requestId": "limit", "paneId": "limit", "offset": 0}),
        );
        let chunk = answer(&mut rx).await;
        assert_eq!(chunk["type"], "viewer.chunk", "8 MiB itself is shown");
        assert_eq!(chunk["size"], MAX_FILE_BYTES);
    }

    /// viewer-panes, Reading A Document: a `text` document from the record.
    #[tokio::test]
    async fn a_text_document_is_read_from_the_pane_record() {
        let text = "## Answer\n\n- a list";
        let store = store_showing(&[(
            "v1",
            json!({"kind": "text", "title": "Selection from claude", "text": text}),
        )]);
        let (reads, mut rx) = reads(store);
        reads.read(&json!({"type": "viewer.read", "requestId": "r1", "paneId": "v1", "offset": 0}));
        let chunk = answer(&mut rx).await;
        assert_eq!(chunk["type"], "viewer.chunk", "{chunk}");
        assert_eq!(chunk["size"], text.len());
        assert_eq!(chunk["eof"], true);
        assert_eq!(decoded(&chunk), text.as_bytes());
        let version = chunk["version"].as_str().expect("version");
        assert!(
            version.starts_with(&format!("t-{}-", text.len())),
            "{version}"
        );
        assert_eq!(version.len(), format!("t-{}-", text.len()).len() + 16);

        reads.read(&json!({"type": "viewer.read", "requestId": "r2", "paneId": "v1", "offset": 0, "ifVersion": version}));
        assert_eq!(answer(&mut rx).await["type"], "viewer.unchanged");
        reads.read(&json!({
            "type": "viewer.read", "requestId": "r3", "paneId": "v1", "offset": 4, "version": "t-1-0000000000000000",
        }));
        assert_eq!(answer(&mut rx).await["code"], "changed");
    }

    /// viewer-panes, Reading A Document: `invalid` for a malformed read and a
    /// later chunk without `version`, echoing a `requestId` that fits.
    #[tokio::test]
    async fn malformed_reads_are_invalid() {
        let (reads, mut rx) = reads(store_showing(&[]));
        let long = "r".repeat(MAX_REQUEST_ID + 1);
        for (read, echo) in [
            (
                json!({"type": "viewer.read", "requestId": "r1", "paneId": "v1"}),
                "r1",
            ),
            (
                json!({"type": "viewer.read", "requestId": "r2", "paneId": "v1", "offset": -1}),
                "r2",
            ),
            (
                json!({"type": "viewer.read", "requestId": "r3", "paneId": "v1", "offset": 1.5}),
                "r3",
            ),
            (
                json!({"type": "viewer.read", "requestId": "r4", "paneId": "v1", "offset": 10}),
                "r4",
            ),
            (
                json!({"type": "viewer.read", "requestId": "r5", "offset": 0}),
                "r5",
            ),
            (
                json!({"type": "viewer.read", "requestId": 7, "paneId": "v1", "offset": 0}),
                "",
            ),
            (
                json!({"type": "viewer.read", "requestId": long, "paneId": "v1", "offset": 0}),
                "",
            ),
        ] {
            reads.read(&read);
            let failed = answer(&mut rx).await;
            assert_eq!(failed["code"], "invalid", "{read}: {failed}");
            assert_eq!(failed["requestId"], echo, "{read}");
        }
    }

    /// viewer-panes, Reading A Document: `busy` while four reads of the
    /// connection are in flight, and reads again once one is done.
    #[tokio::test]
    async fn a_fifth_read_in_flight_is_busy() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("PLAN.md");
        std::fs::write(&path, "plan").expect("file");
        let (reads, mut rx) = reads(store_showing(&[("v1", file(&path))]));
        let in_flight = reads
            .in_flight
            .clone()
            .try_acquire_many_owned(u32::try_from(MAX_IN_FLIGHT).expect("permits"))
            .expect("four reads");
        let read = json!({"type": "viewer.read", "requestId": "r5", "paneId": "v1", "offset": 0});
        reads.read(&read);
        let busy = answer(&mut rx).await;
        assert_eq!(busy["code"], "busy", "{busy}");
        assert_eq!(busy["requestId"], "r5");
        drop(in_flight);
        reads.read(&read);
        assert_eq!(answer(&mut rx).await["type"], "viewer.chunk");
    }

    /// A connection without room for a chunk's bytes is answered `busy`, and
    /// its budget is free again afterwards.
    #[tokio::test]
    async fn a_read_without_room_in_the_connection_budget_is_busy() {
        use splitty_transport::{
            budget::{ConnectionBudget, ProcessBudget},
            queue::Context,
        };
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("PLAN.md");
        std::fs::write(&path, vec![b'x'; 4096]).expect("file");
        let process = ProcessBudget::new(64 * 1024, 1024);
        let budget = ConnectionBudget::new(process.clone(), 64 * 1024, 1024);
        let (sender, mut rx) = ClientSender::with_context(Context::new(budget.clone()));
        let reads = Reads::new(store_showing(&[("v1", file(&path))]), Arc::new(sender));
        let pressure = budget.reserve(60 * 1024, false).expect("pressure");
        let read = json!({"type": "viewer.read", "requestId": "r1", "paneId": "v1", "offset": 0});
        reads.read(&read);
        assert_eq!(answer(&mut rx).await["code"], "busy");
        drop(pressure);
        reads.read(&read);
        assert_eq!(answer(&mut rx).await["type"], "viewer.chunk");
        assert_eq!(budget.used(), 0, "scratch and frames released");
    }
}
