//! The store of files dropped into a pane (`upload`).
//!
//! A client sends a file in chunks of at most [`MAX_CHUNK_BYTES`] as base64
//! and waits for the answer to each before it sends the next. The server keeps
//! every upload in a new directory `upload-<UTC yyyymmdd-hhmmss>-<8 random
//! hex>/` of the image store's directory (`paste`), as a file only its owner
//! can read, under the client's name made safe ([`sanitize_name`]): the name
//! never selects a directory. A failed or abandoned upload is removed.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use rand::Rng;

use super::paste::{self, PasteError, MAX_AGE, MAX_KEPT, NAME_ATTEMPTS};

/// The most bytes of a file one `upload` message carries, decoded.
pub const MAX_CHUNK_BYTES: usize = 1024 * 1024;

/// The longest base64 text of one chunk. Longer data is refused before it is
/// decoded.
pub const MAX_CHUNK_ENCODED_LEN: usize = MAX_CHUNK_BYTES.div_ceil(3) * 4;

/// The largest file a client may upload.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// How many uploads one connection may have under way at once.
pub const MAX_IN_FLIGHT: usize = 4;

/// The start of every upload directory's name, and of only those.
const PREFIX: &str = "upload-";

/// The longest file name an upload keeps, in bytes.
const MAX_NAME_BYTES: usize = 120;

/// The longest extension [`sanitize_name`] keeps when it shortens a name.
const MAX_EXTENSION_BYTES: usize = 16;

/// Why an upload failed. The message goes to the client.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("the chunk is not valid base64")]
    InvalidBase64,
    #[error("a chunk holds more than 1 MiB")]
    ChunkTooLarge,
    #[error("the file is larger than 512 MiB")]
    TooLarge,
    #[error("the first chunk names no size")]
    NoSize,
    #[error("the chunk does not continue the upload at byte {0}")]
    OutOfOrder(u64),
    #[error("the chunk does not start an upload")]
    UnknownUpload,
    #[error("the upload holds more bytes than its size")]
    Overrun,
    #[error("too many uploads at once")]
    TooMany,
    #[error("the server has no cache directory for uploads")]
    NoDirectory,
    #[error("{0}")]
    Directory(#[from] PasteError),
    #[error("could not store the file: {0}")]
    Io(#[from] io::Error),
}

/// Decode one chunk's base64 text.
pub fn decode_chunk(encoded: &[u8]) -> Result<Vec<u8>, UploadError> {
    if encoded.len() > MAX_CHUNK_ENCODED_LEN {
        return Err(UploadError::ChunkTooLarge);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| UploadError::InvalidBase64)?;
    if bytes.len() > MAX_CHUNK_BYTES {
        return Err(UploadError::ChunkTooLarge);
    }
    Ok(bytes)
}

/// The name an upload is stored under: the last component of `name`, every
/// character that is not a letter, a digit, `.`, `-` or `_` replaced by `_`,
/// without leading dots, at most [`MAX_NAME_BYTES`] long with its extension
/// kept, and `file` when nothing is left. A pasted path then needs no quoting.
pub fn sanitize_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let safe: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_start_matches('.');
    if safe.is_empty() {
        return "file".to_owned();
    }
    if safe.len() <= MAX_NAME_BYTES {
        return safe.to_owned();
    }
    let extension = safe
        .rfind('.')
        .map(|dot| &safe[dot..])
        .filter(|ext| ext.len() > 1 && ext.len() <= MAX_EXTENSION_BYTES)
        .unwrap_or("");
    let stem = &safe[..safe.len() - extension.len()];
    format!("{}{extension}", cut(stem, MAX_NAME_BYTES - extension.len()))
}

/// `text` cut to at most `bytes` bytes at a character boundary.
fn cut(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// An upload under way: its directory and file, which it removes when it is
/// dropped before it is [finished](Upload::finish).
#[derive(Debug)]
pub struct Upload {
    dir: PathBuf,
    path: PathBuf,
    file: Option<fs::File>,
    size: u64,
    received: u64,
}

impl Upload {
    /// Start an upload of `size` bytes named `name` in `store`, after removing
    /// old uploads there. Blocking: call it off the async runtime's workers.
    pub fn begin(store: &Path, name: &str, size: u64) -> Result<Self, UploadError> {
        if size > MAX_FILE_BYTES {
            return Err(UploadError::TooLarge);
        }
        let store = paste::prepare_dir(store)?;
        remove_old(&store, SystemTime::now());
        let dir = create_dir(&store)?;
        let path = dir.join(sanitize_name(name));
        let mut options = fs::OpenOptions::new();
        // `create_new` never follows a link; the directory is new anyway.
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) => {
                remove_dir(&dir);
                return Err(error.into());
            }
        };
        Ok(Self {
            dir,
            path,
            file: Some(file),
            size,
            received: 0,
        })
    }

    /// Write `bytes`, which start at `offset` of the file. Blocking.
    pub fn append(&mut self, offset: u64, bytes: &[u8]) -> Result<(), UploadError> {
        if offset != self.received {
            return Err(UploadError::OutOfOrder(self.received));
        }
        let len = bytes.len() as u64;
        if self.received + len > self.size {
            return Err(UploadError::Overrun);
        }
        let file = self.file.as_mut().ok_or(UploadError::Overrun)?;
        file.write_all(bytes)?;
        self.received += len;
        Ok(())
    }

    /// The bytes received so far.
    pub fn received(&self) -> u64 {
        self.received
    }

    /// Whether every byte of the file arrived.
    pub fn is_complete(&self) -> bool {
        self.received == self.size
    }

    /// Close the file and keep it; returns its absolute path.
    pub fn finish(mut self) -> PathBuf {
        self.file = None;
        std::mem::take(&mut self.path)
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        // `finish` takes the path; an upload that still has one is abandoned.
        if !self.path.as_os_str().is_empty() {
            self.file = None;
            remove_dir(&self.dir);
        }
    }
}

/// A new directory for one upload in `store`, mode 0700; a name that is
/// taken is never reused.
fn create_dir(store: &Path) -> io::Result<PathBuf> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    let mut attempt = 1;
    loop {
        let dir = store.join(dir_name(SystemTime::now()));
        match builder.create(&dir) {
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists && attempt < NAME_ATTEMPTS =>
            {
                attempt += 1;
            }
            result => return result.map(|()| dir),
        }
    }
}

/// `upload-<UTC yyyymmdd-hhmmss>-<8 random hex digits>`.
fn dir_name(now: SystemTime) -> String {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut random = [0u8; 4];
    rand::thread_rng().fill(&mut random);
    format!("{PREFIX}{}-{}", paste::utc_stamp(secs), hex::encode(random))
}

/// Remove the upload directories in `store` that are older than
/// [`MAX_AGE`], then all but the newest [`MAX_KEPT`]. Only directories (not
/// links to them) whose names start with [`PREFIX`] count. A failure is
/// logged and does not stop the upload.
fn remove_old(store: &Path, now: SystemTime) {
    let entries = match fs::read_dir(store) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(dir = %store.display(), %error, "cannot list the uploads");
            return;
        }
    };
    let mut kept: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(PREFIX.as_bytes())
        {
            continue;
        }
        let path = entry.path();
        // Does not follow a link, so a link is not a directory here.
        let modified = match entry.metadata() {
            Ok(meta) if !meta.is_dir() => continue,
            Ok(meta) => meta.modified(),
            Err(error) => Err(error),
        };
        match modified {
            Ok(modified) if now.duration_since(modified).unwrap_or_default() > MAX_AGE => {
                remove_dir(&path);
            }
            Ok(modified) => kept.push((modified, path)),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot read the age of an upload");
            }
        }
    }
    if kept.len() > MAX_KEPT {
        // Newest first; the name, which starts with the time, breaks ties.
        kept.sort_unstable_by(|a, b| b.cmp(a));
        for (_, path) in kept.drain(MAX_KEPT..) {
            remove_dir(&path);
        }
    }
}

fn remove_dir(dir: &Path) {
    if let Err(error) = fs::remove_dir_all(dir) {
        if error.kind() != io::ErrorKind::NotFound {
            tracing::warn!(dir = %dir.display(), %error, "cannot remove an upload");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn uploads(store: &Path) -> Vec<String> {
        let Ok(entries) = fs::read_dir(store) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with(PREFIX))
            .collect();
        names.sort();
        names
    }

    /// terminal-sessions, Chunked File Uploads, Upload in two chunks: the
    /// bytes land in a new `upload-` directory under the file's name, owner
    /// only.
    #[test]
    fn upload_in_two_chunks() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("splitty").join("paste");
        let first = vec![1u8; MAX_CHUNK_BYTES];
        let second = vec![2u8; MAX_CHUNK_BYTES / 2];
        let size = (first.len() + second.len()) as u64;

        let mut upload = Upload::begin(&store, "notes.txt", size).unwrap();
        upload.append(0, &first).unwrap();
        assert_eq!(upload.received(), MAX_CHUNK_BYTES as u64);
        assert!(!upload.is_complete());
        upload.append(MAX_CHUNK_BYTES as u64, &second).unwrap();
        assert!(upload.is_complete());
        let path = upload.finish();

        assert!(path.is_absolute(), "{path:?}");
        assert_eq!(path.file_name().unwrap(), "notes.txt");
        let dir = path.parent().unwrap();
        assert_eq!(dir.parent(), Some(store.as_path()));
        assert!(dir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(PREFIX));
        assert_eq!(fs::read(&path).unwrap(), [first, second].concat());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(dir), 0o700);
            assert_eq!(mode(&store), 0o700);
        }
    }

    /// terminal-sessions, Chunked File Uploads, Name with spaces and a path,
    /// and access-control, Upload name with a path.
    #[test]
    fn names_keep_only_their_last_component_made_safe() {
        assert_eq!(
            sanitize_name("../My Report (final).pdf"),
            "My_Report__final_.pdf"
        );
        assert_eq!(
            sanitize_name("../../.ssh/authorized_keys"),
            "authorized_keys"
        );
        assert_eq!(
            sanitize_name("C:\\Users\\me\\Bericht ä.txt"),
            "Bericht_ä.txt"
        );
        assert_eq!(sanitize_name(".bashrc"), "bashrc");
        assert_eq!(sanitize_name(".."), "file");
        assert_eq!(sanitize_name(""), "file");
        assert_eq!(sanitize_name("dir/"), "file");
        assert_eq!(sanitize_name("a\u{202e}b\n.sh"), "a_b_.sh");
        let long = format!("{}.tar.gz", "ö".repeat(100));
        let kept = sanitize_name(&long);
        assert!(kept.len() <= MAX_NAME_BYTES, "{}", kept.len());
        assert!(kept.ends_with(".gz"), "{kept}");
        assert!(kept.starts_with("ööö"), "{kept}");
        let no_extension = "x".repeat(300);
        assert_eq!(sanitize_name(&no_extension).len(), MAX_NAME_BYTES);
    }

    /// An upload stored in a directory of its own: the name cannot reach
    /// another directory, and an existing file of that name is not touched.
    #[test]
    fn an_upload_name_never_selects_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("paste");
        let mut upload = Upload::begin(&store, "../../outside.txt", 1).unwrap();
        upload.append(0, b"x").unwrap();
        let path = upload.finish();
        assert_eq!(path.file_name().unwrap(), "outside.txt");
        assert_eq!(path.parent().unwrap().parent(), Some(store.as_path()));
        assert!(!tmp.path().join("outside.txt").exists());
    }

    /// terminal-sessions, Chunked File Uploads, Chunk out of order; more
    /// bytes than the size; an abandoned upload is removed.
    #[test]
    fn a_failed_or_abandoned_upload_leaves_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("paste");
        let mut upload = Upload::begin(&store, "a.bin", 10).unwrap();
        upload.append(0, &[0; 4]).unwrap();
        assert!(matches!(
            upload.append(0, &[0; 4]),
            Err(UploadError::OutOfOrder(4))
        ));
        assert!(matches!(
            upload.append(4, &[0; 7]),
            Err(UploadError::Overrun)
        ));
        assert_eq!(uploads(&store).len(), 1);
        drop(upload);
        assert!(uploads(&store).is_empty());

        assert!(matches!(
            Upload::begin(&store, "huge", MAX_FILE_BYTES + 1),
            Err(UploadError::TooLarge)
        ));
        assert!(uploads(&store).is_empty());
    }

    #[test]
    fn an_empty_file_is_complete_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let upload = Upload::begin(tmp.path(), "empty.txt", 0).unwrap();
        assert!(upload.is_complete());
        let path = upload.finish();
        assert_eq!(fs::read(path).unwrap(), b"");
    }

    #[test]
    fn chunks_are_bounded_before_and_after_decoding() {
        use base64::engine::general_purpose::STANDARD;
        assert_eq!(
            decode_chunk(STANDARD.encode([5u8; 10]).as_bytes()).unwrap(),
            [5; 10]
        );
        assert!(matches!(
            decode_chunk(b"%%%"),
            Err(UploadError::InvalidBase64)
        ));
        let too_long = STANDARD.encode(vec![0u8; MAX_CHUNK_BYTES + 3]);
        assert!(matches!(
            decode_chunk(too_long.as_bytes()),
            Err(UploadError::ChunkTooLarge)
        ));
        assert!(decode_chunk(STANDARD.encode(vec![0u8; MAX_CHUNK_BYTES]).as_bytes()).is_ok());
    }

    /// Old uploads go before a new one starts: older than a day, then beyond
    /// the newest hundred. Links and other entries stay.
    #[test]
    fn old_uploads_are_removed_before_a_new_one() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("paste");
        paste::prepare_dir(&store).unwrap();
        let old = store.join("upload-20000101-000000-00000000");
        fs::create_dir(&old).unwrap();
        fs::write(old.join("x"), b"x").unwrap();
        let dir = fs::File::open(&old).unwrap();
        dir.set_modified(SystemTime::now() - MAX_AGE - Duration::from_secs(60))
            .unwrap();
        fs::write(store.join("upload-not-a-dir"), b"x").unwrap();
        fs::write(store.join("paste-image.png"), b"x").unwrap();
        for i in 0..=MAX_KEPT {
            fs::create_dir(store.join(format!("upload-20990101-000000-{i:08}"))).unwrap();
        }

        let upload = Upload::begin(&store, "new.txt", 0).unwrap();
        let path = upload.finish();

        assert!(!old.exists());
        assert!(store.join("upload-not-a-dir").exists());
        assert!(store.join("paste-image.png").exists());
        assert!(path.exists());
        // Of the 101 young directories the newest 100 remain, then the new
        // one; the file named like one counts in `uploads` too.
        assert_eq!(uploads(&store).len(), MAX_KEPT + 2);
    }
}
