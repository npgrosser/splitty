//! Offer a file of this pane to the windows that show it (`splitty download`);
//! the user saves it there. A folder goes as a zip archive that the command
//! packs into a directory of its own below `splitty/download/` of the cache
//! and removes when it ends.
use super::{request_within, CliError, ServerInfo};
use serde_json::{json, Value};
use splitty_core::protocol::{paste::civil_from_days, upload::MAX_FILE_BYTES};
use std::fs;
use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zip::{result::ZipError, write::SimpleFileOptions, ZipWriter};

/// The start of every archive directory's name in the store, and of only
/// those.
const ARCHIVE_PREFIX: &str = "download-";

/// An archive directory older than this is left over from a killed command;
/// the next folder download removes it.
const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const MIB: u64 = 1024 * 1024;

/// What `splitty download` was given, links followed.
#[derive(Debug, PartialEq)]
pub enum Target {
    /// The absolute path of a regular file.
    File(String),
    /// The canonical path of a folder.
    Folder(PathBuf),
}

/// What `path` is: a regular file or a folder.
pub fn resolve(path: &Path) -> Result<Target, CliError> {
    let path = path
        .canonicalize()
        .map_err(|e| CliError::Usage(format!("Cannot resolve {path:?}: {e}")))?;
    let metadata = path
        .metadata()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    if metadata.is_dir() {
        return Ok(Target::Folder(path));
    }
    if !metadata.is_file() {
        return Err(CliError::Usage(
            "Only regular files and folders can be downloaded".into(),
        ));
    }
    utf8(path).map(Target::File)
}

fn utf8(path: PathBuf) -> Result<String, CliError> {
    path.into_os_string()
        .into_string()
        .map_err(|_| CliError::Usage("The path must be valid UTF-8".into()))
}

/// What the server's answer means for the user: `Ok` only once the file was
/// saved.
pub fn outcome(status: u16, body: &Value) -> Result<(), CliError> {
    if (200..300).contains(&status) && body["outcome"] == "saved" {
        return Ok(());
    }
    let message = body["error"].as_str().unwrap_or("unknown error");
    match (status, body["code"].as_str()) {
        (404, Some("session_not_found")) => Err(CliError::Usage(
            "This terminal session is gone; run the command in a Splitty terminal".into(),
        )),
        (404, _) => Err(CliError::Usage(
            "This Splitty server cannot offer files yet. Update it (for a host: Update… in the Hosts panel) and reopen its terminal.".into(),
        )),
        (409, Some("declined")) => Err(CliError::Failed("Declined in Splitty.".into())),
        (408, Some("expired")) => Err(CliError::Failed(
            "Nobody chose Save in Splitty within 60 seconds.".into(),
        )),
        (409, Some("no_client")) => Err(CliError::Failed(format!(
            "{message}. Open this terminal in a Splitty window and run the command again."
        ))),
        (400 | 413, _) => Err(CliError::Usage(message.into())),
        _ => Err(CliError::Failed(message.into())),
    }
}

pub async fn download(server: &ServerInfo, path: &Path, session: &str) -> Result<(), CliError> {
    download_in(server, path, session, default_store(), interrupted()).await
}

/// [`download`] with the archive store and the end of a folder download
/// given: `interrupted` resolving removes the archive and fails the command.
pub async fn download_in(
    server: &ServerInfo,
    path: &Path,
    session: &str,
    store: Option<PathBuf>,
    interrupted: impl Future<Output = ()>,
) -> Result<(), CliError> {
    if session.is_empty() || session.len() > 256 {
        return Err(CliError::Usage(
            "Run this command in a Splitty terminal, or supply --session <id>".into(),
        ));
    }
    match resolve(path)? {
        Target::File(path) => offer(server, &path, session).await,
        Target::Folder(folder) => {
            let store = store.ok_or_else(|| {
                CliError::Failed("There is no cache directory to pack the folder in".into())
            })?;
            download_folder(server, folder, session, &store, interrupted).await
        }
    }
}

async fn download_folder(
    server: &ServerInfo,
    folder: PathBuf,
    session: &str,
    store: &Path,
    interrupted: impl Future<Output = ()>,
) -> Result<(), CliError> {
    let name = archive_name(&folder)?;
    let store = prepare_store(store)?;
    remove_old(&store, SystemTime::now());
    // Removed when this function returns, whatever the outcome.
    let dir = archive_dir(&store)?;
    let archive = dir.path().join(format!("{name}.zip"));
    eprintln!("Packing {name}…");
    tokio::pin!(interrupted);
    let packing = tokio::task::spawn_blocking({
        let (name, archive) = (name.clone(), archive.clone());
        move || pack(&folder, &name, &archive, &store, MAX_FILE_BYTES)
    });
    let packed = tokio::select! {
        packed = packing => packed.map_err(|e| CliError::Failed(e.to_string()))??,
        () = &mut interrupted => return Err(interrupted_error()),
    };
    eprintln!("{}", packed.report(&name));
    let archive = utf8(archive)?;
    tokio::select! {
        result = offer(server, &archive, session) => result,
        () = &mut interrupted => Err(interrupted_error()),
    }
}

fn interrupted_error() -> CliError {
    CliError::Failed("Interrupted; the archive was removed.".into())
}

/// Ctrl+C, a closed terminal (SIGHUP) or `kill` (SIGTERM). Never resolves
/// when the handlers cannot be installed.
async fn interrupted() {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut interrupt), Ok(mut hangup), Ok(mut terminate)) = (
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
        signal(SignalKind::terminate()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = hangup.recv() => {}
        _ = terminate.recv() => {}
    }
}

async fn offer(server: &ServerInfo, path: &str, session: &str) -> Result<(), CliError> {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    eprintln!("Waiting for Save in Splitty (expires in 60 seconds)…");
    // The server bounds the wait: a minute for Save, a minute per chunk.
    let (status, body) = request_within(
        server,
        "POST",
        "/api/file/offer",
        Some(json!({"sessionId":session,"path":path})),
        None,
    )
    .await?;
    outcome(status, &body)?;
    println!("Saved {name} in Splitty.");
    Ok(())
}

/// `splitty/download/` of the user's cache directory (`$XDG_CACHE_HOME`, else
/// `~/.cache`; `~/Library/Caches` on macOS), or `None` without one.
pub fn default_store() -> Option<PathBuf> {
    dirs::cache_dir().map(|cache| cache.join("splitty").join("download"))
}

/// The folder's name, which names the archive and its top-level folder;
/// `folder` for `/`.
fn archive_name(folder: &Path) -> Result<String, CliError> {
    match folder.file_name() {
        None => Ok("folder".into()),
        Some(name) => name
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| CliError::Usage("The folder's name must be valid UTF-8".into())),
    }
}

/// Create `store` with mode 0700, refusing a symbolic link, and return its
/// canonical path.
fn prepare_store(store: &Path) -> Result<PathBuf, CliError> {
    let failed = |e: io::Error| CliError::Failed(format!("Cannot use {}: {e}", store.display()));
    match fs::symlink_metadata(store) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(CliError::Failed(format!(
                "{} is a symbolic link; remove it",
                store.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(error)),
    }
    splitty_core::config::ensure_private_dir(store).map_err(failed)?;
    store.canonicalize().map_err(failed)
}

/// A new directory of one archive in `store`, mode 0700.
fn archive_dir(store: &Path) -> Result<tempfile::TempDir, CliError> {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .prefix(ARCHIVE_PREFIX)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(store)
        .map_err(|e| CliError::Failed(format!("Cannot create the archive: {e}")))
}

/// Remove the archive directories in `store` older than [`MAX_AGE`]. Only
/// directories (not links to them) whose names start with
/// [`ARCHIVE_PREFIX`] count; a failure leaves the entry.
fn remove_old(store: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(store) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(ARCHIVE_PREFIX.as_bytes())
        {
            continue;
        }
        // Does not follow a link, so a link is not a directory here.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let old = meta
            .modified()
            .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() > MAX_AGE);
        if meta.is_dir() && old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// What [`pack`] put into an archive.
#[derive(Debug, PartialEq)]
pub struct Packed {
    pub files: u64,
    /// Symbolic links and special files left out.
    pub skipped: u64,
    /// The archive's size in bytes.
    pub size: u64,
}

impl Packed {
    fn report(&self, name: &str) -> String {
        let mut line = format!(
            "Packed {} into {name}.zip ({})",
            count(self.files, "file", "files"),
            size(self.size)
        );
        if self.skipped > 0 {
            line += &format!(
                "; left out {}",
                count(
                    self.skipped,
                    "symbolic link or special file",
                    "symbolic links or special files"
                )
            );
        }
        line + "."
    }
}

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn size(bytes: u64) -> String {
    if bytes < MIB {
        format!("{} KiB", bytes.div_ceil(1024))
    } else if bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{}.{} MiB", bytes / MIB, bytes % MIB * 10 / MIB)
    }
}

/// Pack `folder` into a new zip archive at `archive` (mode 0600), as one
/// top-level folder `name` with every file and folder below it, sorted, with
/// their Unix permissions and modification times in UTC. Symbolic links and
/// special files below are left out and counted, `store` is left out
/// entirely, and an unreadable entry, a name that is not UTF-8 or an archive
/// beyond `limit` bytes fails. Blocking.
pub fn pack(
    folder: &Path,
    name: &str,
    archive: &Path,
    store: &Path,
    limit: u64,
) -> Result<Packed, CliError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(archive)
        .map_err(|e| CliError::Failed(format!("Cannot create the archive: {e}")))?;
    let mut zip = ZipWriter::new(Bounded::new(file, limit));
    let mut packed = Packed {
        files: 0,
        skipped: 0,
        size: 0,
    };
    let root = fs::metadata(folder).map_err(|e| unreadable(folder, e))?;
    let mut folders = vec![(folder.to_path_buf(), name.to_owned(), root)];
    while let Some((dir, entry_name, meta)) = folders.pop() {
        zip.add_directory(entry_name.as_str(), entry_options(&meta))
            .map_err(|e| write_failed(e, limit))?;
        let mut entries = fs::read_dir(&dir)
            .and_then(Iterator::collect::<io::Result<Vec<_>>>)
            .map_err(|e| unreadable(&dir, e))?;
        entries.sort_by_key(fs::DirEntry::file_name);
        let mut below = Vec::new();
        for entry in entries {
            let path = entry.path();
            if path == store {
                continue;
            }
            let Ok(child) = entry.file_name().into_string() else {
                return Err(CliError::Usage(format!(
                    "{} has a name that is not valid UTF-8",
                    path.display()
                )));
            };
            let child = format!("{entry_name}/{child}");
            // Does not follow a link.
            let meta = entry.metadata().map_err(|e| unreadable(&path, e))?;
            if meta.is_dir() {
                below.push((path, child, meta));
            } else if meta.is_file() && add_file(&mut zip, &path, &child, limit)? {
                packed.files += 1;
            } else {
                packed.skipped += 1;
            }
        }
        // Popped in order, so subfolders follow the files, sorted.
        folders.extend(below.into_iter().rev());
    }
    let mut bounded = zip.finish().map_err(|e| write_failed(e, limit))?;
    bounded.flush().map_err(|e| write_failed(e.into(), limit))?;
    packed.size = bounded.position;
    Ok(packed)
}

/// Add the file at `path` as `name`; `false` when it is no longer a regular
/// file.
fn add_file(
    zip: &mut ZipWriter<Bounded>,
    path: &Path,
    name: &str,
    limit: u64,
) -> Result<bool, CliError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Swapped for a FIFO since it was listed, it must not hang here.
        options.custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOCTTY);
    }
    let mut file = options.open(path).map_err(|e| unreadable(path, e))?;
    let meta = file.metadata().map_err(|e| unreadable(path, e))?;
    if !meta.is_file() {
        return Ok(false);
    }
    zip.start_file(name, entry_options(&meta))
        .map_err(|e| write_failed(e, limit))?;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(unreadable(path, error)),
        };
        zip.write_all(&buffer[..read])
            .map_err(|e| write_failed(e.into(), limit))?;
    }
}

fn entry_options(meta: &fs::Metadata) -> SimpleFileOptions {
    use std::os::unix::fs::PermissionsExt;
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(meta.permissions().mode())
        .last_modified_time(modified(meta))
        .large_file(meta.len() >= u64::from(u32::MAX))
}

/// The entry's modification time in UTC, or zip's earliest time
/// (1980-01-01) when it has none a zip can hold.
fn modified(meta: &fs::Metadata) -> zip::DateTime {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|since| dos_time(since.as_secs()))
        .unwrap_or_default()
}

/// `secs` seconds after the Unix epoch as a zip time, between 1980 and 2107.
fn dos_time(secs: u64) -> Option<zip::DateTime> {
    let (year, month, day) = civil_from_days(secs / 86_400);
    let time = secs % 86_400;
    zip::DateTime::from_date_and_time(
        u16::try_from(year).ok()?,
        u8::try_from(month).ok()?,
        u8::try_from(day).ok()?,
        u8::try_from(time / 3_600).ok()?,
        u8::try_from(time % 3_600 / 60).ok()?,
        u8::try_from(time % 60).ok()?,
    )
    .ok()
}

fn unreadable(path: &Path, error: io::Error) -> CliError {
    CliError::Failed(format!("Cannot read {}: {error}", path.display()))
}

fn write_failed(error: ZipError, limit: u64) -> CliError {
    match error {
        ZipError::Io(error) if error.kind() == io::ErrorKind::FileTooLarge => {
            CliError::Usage(format!(
                "The folder packs to more than {}, the most a download takes; download a smaller folder",
                size(limit)
            ))
        }
        error => CliError::Failed(format!("Cannot write the archive: {error}")),
    }
}

/// The archive file, refusing to grow beyond `limit` bytes
/// ([`io::ErrorKind::FileTooLarge`]). After its first failure it discards
/// what follows, so the zip writer's drop, which finalizes the archive about
/// to be removed, neither writes nor complains on stderr.
struct Bounded {
    file: io::BufWriter<fs::File>,
    position: u64,
    limit: u64,
    failed: bool,
}

impl Bounded {
    fn new(file: fs::File, limit: u64) -> Self {
        Self {
            file: io::BufWriter::new(file),
            position: 0,
            limit,
            failed: false,
        }
    }

    fn check<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        self.failed |= result.is_err();
        result
    }
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        if self.failed {
            self.position = self.position.saturating_add(len);
            return Ok(buf.len());
        }
        if self.position.saturating_add(len) > self.limit {
            return self.check(Err(io::ErrorKind::FileTooLarge.into()));
        }
        let written = self.file.write(buf);
        let written = self.check(written)?;
        self.position += u64::try_from(written).unwrap_or(u64::MAX);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        let flushed = self.file.flush();
        self.check(flushed)
    }
}

impl Seek for Bounded {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.failed {
            self.position = match pos {
                SeekFrom::Start(position) => position,
                SeekFrom::Current(delta) => self.position.saturating_add_signed(delta),
                SeekFrom::End(_) => self.position,
            };
            return Ok(self.position);
        }
        let position = self.file.seek(pos);
        self.position = self.check(position)?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn resolves_regular_files_folders_and_links() {
        let dir = tempfile::tempdir().expect("fixture");
        let file = dir.path().join("report #? 界.pdf");
        std::fs::write(&file, "pdf").expect("file");
        let canonical = file.canonicalize().expect("canonical");
        assert_eq!(
            resolve(&dir.path().join("./report #? 界.pdf")).expect("file"),
            Target::File(canonical.to_str().expect("UTF-8").into())
        );
        assert_eq!(
            resolve(dir.path()).expect("folder"),
            Target::Folder(dir.path().canonicalize().expect("canonical"))
        );
        assert!(resolve(&dir.path().join("missing")).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).expect("link");
        assert_eq!(
            resolve(&link).expect("link"),
            Target::File(canonical.to_str().expect("UTF-8").into())
        );
        assert!(matches!(
            resolve(Path::new("/dev/null")),
            Err(CliError::Usage(m)) if m.contains("folders")
        ));
        let fifo = dir.path().join("pipe");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU).expect("fifo");
        assert!(matches!(resolve(&fifo), Err(CliError::Usage(_))));
    }

    /// cli, Download To The Window, Saved and Declined; an older server.
    #[test]
    fn only_a_saved_file_succeeds() {
        assert!(outcome(200, &json!({"outcome":"saved"})).is_ok());
        assert!(matches!(
            outcome(409, &json!({"code":"declined","error":"x"})),
            Err(CliError::Failed(m)) if m.contains("Declined")
        ));
        assert!(matches!(
            outcome(408, &json!({"code":"expired","error":"x"})),
            Err(CliError::Failed(_))
        ));
        assert!(matches!(
            outcome(404, &json!({"error":"Not Found"})),
            Err(CliError::Usage(m)) if m.contains("Update")
        ));
        assert!(matches!(
            outcome(413, &json!({"code":"too_large","error":"Files over 512 MiB cannot be offered"})),
            Err(CliError::Usage(m)) if m.contains("512")
        ));
        assert!(outcome(200, &json!({"outcome":"other"})).is_err());
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    /// cli, What A Folder Archive Holds, Links left out: one top-level
    /// folder, files, empty folders and modes, no links or FIFOs, nothing of
    /// the store.
    #[test]
    fn a_folder_packs_its_tree_without_links() {
        let fixture = tempfile::tempdir().expect("fixture");
        let folder = fixture.path().join("build");
        fs::create_dir_all(folder.join("bin")).expect("bin");
        fs::create_dir(folder.join("empty")).expect("empty");
        fs::write(folder.join("a.txt"), "alpha").expect("a");
        fs::write(folder.join(".hidden"), "").expect("hidden");
        let script = folder.join("bin").join("run.sh");
        fs::write(&script, "#!/bin/sh\n").expect("script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("mode");
        // 2024-05-17 12:34:56 UTC.
        let time = UNIX_EPOCH + Duration::from_secs(1_715_949_296);
        fs::File::options()
            .write(true)
            .open(folder.join("a.txt"))
            .and_then(|file| file.set_modified(time))
            .expect("time");
        std::os::unix::fs::symlink("a.txt", folder.join("link")).expect("link");
        nix::unistd::mkfifo(&folder.join("pipe"), nix::sys::stat::Mode::S_IRWXU).expect("fifo");
        let store = folder.join("cache");
        fs::create_dir(&store).expect("store");
        fs::write(store.join("other.zip"), "x").expect("other");
        let store = prepare_store(&store).expect("store");
        let dir = archive_dir(&store).expect("archive dir");
        let archive = dir.path().join("build.zip");

        let folder = folder.canonicalize().expect("canonical");
        let packed = pack(&folder, "build", &archive, &store, MAX_FILE_BYTES).expect("pack");
        assert_eq!(
            packed,
            Packed {
                files: 3,
                skipped: 2,
                size: fs::metadata(&archive).expect("archive").len(),
            }
        );
        assert_eq!(
            packed.report("build"),
            "Packed 3 files into build.zip (1 KiB); left out 2 symbolic links or special files."
        );
        assert_eq!(mode(&archive), 0o600);
        assert_eq!(mode(dir.path()), 0o700);
        assert_eq!(mode(&store), 0o700);

        let mut zip = zip::ZipArchive::new(fs::File::open(&archive).expect("open")).expect("zip");
        let names: Vec<String> = zip
            .file_names()
            .map(|name| name.expect("name").into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "build/",
                "build/.hidden",
                "build/a.txt",
                "build/bin/",
                "build/bin/run.sh",
                "build/empty/"
            ]
        );
        let mut text = String::new();
        let mut entry = zip.by_name("build/a.txt").expect("a");
        entry.read_to_string(&mut text).expect("read");
        assert_eq!(text, "alpha");
        assert_eq!(
            entry.last_modified(),
            zip::DateTime::from_date_and_time(2024, 5, 17, 12, 34, 56).ok()
        );
        drop(entry);
        let entry = zip.by_name("build/bin/run.sh").expect("script");
        assert_eq!(entry.unix_mode().map(|m| m & 0o777), Some(0o755));
        drop(entry);
        assert!(zip.by_name("build/empty/").expect("empty").is_dir());

        drop(dir);
        assert_eq!(fs::read_dir(&store).expect("store").count(), 1);
    }

    /// cli, What A Folder Archive Holds, Too large: packing stops at the
    /// limit, and the archive's directory goes with it.
    #[test]
    fn packing_stops_beyond_the_limit() {
        let fixture = tempfile::tempdir().expect("fixture");
        let folder = fixture.path().join("logs");
        fs::create_dir(&folder).expect("folder");
        let mut noise = vec![0u8; 64 * 1024];
        rand::Rng::fill(&mut rand::thread_rng(), &mut noise[..]);
        fs::write(folder.join("noise.bin"), noise).expect("noise");
        let store = prepare_store(&fixture.path().join("store")).expect("store");
        let dir = archive_dir(&store).expect("archive dir");
        let archive = dir.path().join("logs.zip");

        let error = pack(&folder, "logs", &archive, &store, 16 * 1024).expect_err("too large");
        assert!(
            matches!(&error, CliError::Usage(m) if m.contains("more than 16 KiB")),
            "{error:?}"
        );
        assert!(write_failed(
            ZipError::Io(io::ErrorKind::FileTooLarge.into()),
            MAX_FILE_BYTES
        )
        .to_string()
        .contains("more than 512 MiB"));
        drop(dir);
        assert_eq!(fs::read_dir(&store).expect("store").count(), 0);
    }

    #[test]
    fn the_store_is_private_and_never_a_link() {
        let fixture = tempfile::tempdir().expect("fixture");
        let store = fixture.path().join("splitty").join("download");
        prepare_store(&store).expect("store");
        assert_eq!(mode(&store), 0o700);
        let target = fixture.path().join("elsewhere");
        fs::create_dir(&target).expect("target");
        let link = fixture.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        assert!(prepare_store(&link).is_err());
    }

    /// cli, Download Folders As Zip: archive directories older than 24 hours
    /// go, younger ones, links and other entries stay.
    #[test]
    fn old_archives_are_removed_before_a_new_one() {
        let fixture = tempfile::tempdir().expect("fixture");
        let store = fixture.path();
        for name in ["download-old", "download-young", "other-old"] {
            fs::create_dir(store.join(name)).expect("dir");
            fs::write(store.join(name).join("a.zip"), "x").expect("archive");
        }
        std::os::unix::fs::symlink(store.join("other-old"), store.join("download-link"))
            .expect("link");
        let now = SystemTime::now();
        for name in ["download-old", "other-old"] {
            fs::File::open(store.join(name))
                .and_then(|dir| dir.set_modified(now - MAX_AGE - Duration::from_secs(60)))
                .expect("age");
        }

        remove_old(store, now);
        let mut left: Vec<String> = fs::read_dir(store)
            .expect("store")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect();
        left.sort();
        assert_eq!(left, ["download-link", "download-young", "other-old"]);
    }

    #[test]
    fn sizes_and_times_for_the_report_and_the_entries() {
        assert_eq!(size(0), "0 KiB");
        assert_eq!(size(1500), "2 KiB");
        assert_eq!(size(MAX_FILE_BYTES), "512 MiB");
        assert_eq!(size(5 * MIB + MIB / 2), "5.5 MiB");
        assert_eq!(
            Packed {
                files: 1,
                skipped: 0,
                size: 10
            }
            .report("src"),
            "Packed 1 file into src.zip (1 KiB)."
        );
        assert_eq!(dos_time(0), None);
        assert_eq!(
            dos_time(1_715_949_296),
            zip::DateTime::from_date_and_time(2024, 5, 17, 12, 34, 56).ok()
        );
        assert_eq!(archive_name(Path::new("/")).expect("root"), "folder");
        assert_eq!(
            archive_name(Path::new("/srv/build")).expect("name"),
            "build"
        );
    }
}
