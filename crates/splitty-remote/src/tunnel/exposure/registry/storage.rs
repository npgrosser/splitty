//! A stable separate lock protects read/check/commit, including directory sync.
//! Neither a corrupt/missing state nor an unsafe path becomes an empty registry.
use super::{denied, Records, MAX_STATE_BYTES};
use nix::{
    fcntl::{Flock, FlockArg},
    libc,
    sys::statfs,
    unistd::geteuid,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
#[error("forwarding registry is busy")]
struct Busy;
pub(super) fn is_busy(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|source| source.is::<Busy>())
}

pub(super) struct Locked {
    directory: PathBuf,
    directory_fd: File,
    file: Flock<File>,
}
fn private_file(meta: &fs::Metadata) -> io::Result<()> {
    if !meta.is_file()
        || meta.uid() != geteuid().as_raw()
        || meta.mode() & 0o777 != 0o600
        || meta.nlink() != 1
    {
        return Err(denied(
            "forwarding registry needs private owned regular files, without hard links",
        ));
    }
    Ok(())
}
fn private_opened_file(file: &File) -> io::Result<()> {
    private_file(&file.metadata()?)?;
    #[cfg(target_os = "macos")]
    super::acl::check_file(file, true)?;
    Ok(())
}
fn directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(denied("forwarding registry path must be absolute"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => current.push(component),
            _ => {
                return Err(denied(
                    "forwarding registry path contains an unsafe component",
                ))
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                // Commit creation of the durable directory chain too.
                if let Some(parent) = current.parent() {
                    File::open(parent)?.sync_all()?;
                }
            }
            Err(error) => return Err(error),
        }
        let meta = fs::symlink_metadata(&current)?;
        if !meta.is_dir()
            || (meta.uid() != geteuid().as_raw() && meta.uid() != 0)
            || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0)
        {
            return Err(denied("unsafe forwarding registry ancestor"));
        }
        if current == path && (meta.uid() != geteuid().as_raw() || meta.mode() & 0o777 != 0o700) {
            return Err(denied(
                "forwarding registry directory must be owned with mode 0700",
            ));
        }
        #[cfg(target_os = "macos")]
        {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&current)?;
            super::acl::check_file(&file, current == path)?;
        }
    }
    let filesystem = statfs::statfs(path)?;
    #[cfg(target_os = "linux")]
    if ![0xef53, 0x58465342, 0x9123683e, 0x2fc12fc1].contains(&filesystem.filesystem_type().0) {
        // ext2/3/4, XFS, Btrfs and ZFS have the required local durable Unix
        // lock/rename/sync semantics. Unknown/stacked/network/temporary FS do
        // not become authority by omission from a denylist.
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "forwarding registry requires a supported persistent local Unix filesystem",
        ));
    }
    #[cfg(target_os = "macos")]
    if !matches!(filesystem.filesystem_type_name(), "apfs" | "hfs") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "forwarding registry requires local APFS or HFS+ storage",
        ));
    }
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}
impl Locked {
    pub fn acquire(path: &Path) -> io::Result<Self> {
        let directory_fd = directory(path)?;
        let name = path.join("registry.lock");
        let options = || {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
            options
        };
        let file = match options().create_new(true).open(&name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => options().open(&name)?,
            Err(error) => return Err(error),
        };
        private_opened_file(&file)?;
        // Never create a waiter/task around a contested registry. The caller
        // gets an explicit busy error and cannot expose a port in the meantime.
        let file =
            Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_file, error)| {
                if error == nix::errno::Errno::EWOULDBLOCK {
                    io::Error::new(io::ErrorKind::WouldBlock, Busy)
                } else {
                    io::Error::from(error)
                }
            })?;
        let locked = Self {
            directory: path.to_owned(),
            directory_fd,
            file,
        };
        locked.verify()?;
        locked.initialize()?;
        Ok(locked)
    }
    fn initialize(&self) -> io::Result<()> {
        const READY: &[u8] = b"SPLITTY_EXPOSURE_V1\n";
        match self.file.metadata()?.len() {
            0 => {
                // Any winner of the first flock may finish initialization.
                // A zero marker proves that no claim could yet be admitted.
                if self.directory.join("registry.json").try_exists()? {
                    let records = self.read()?;
                    if records.format != 1 || !records.entries.is_empty() {
                        return Err(denied(
                            "uninitialized registry contains claims; admission is closed",
                        ));
                    }
                } else {
                    self.write(&Records {
                        format: 1,
                        entries: Vec::new(),
                    })?;
                }
                self.file.write_all_at(READY, 0)?;
                self.file.sync_all()?;
                self.directory_fd.sync_all()?;
            }
            size if size == READY.len() as u64 => {
                let mut marker = [0; READY.len()];
                self.file.read_exact_at(&mut marker, 0)?;
                if marker != READY {
                    return Err(denied("invalid forwarding registry lock marker"));
                }
            }
            _ => {
                return Err(denied(
                    "incomplete forwarding registry lock marker; admission is closed",
                ))
            }
        }
        Ok(())
    }
    fn verify(&self) -> io::Result<()> {
        let current = fs::symlink_metadata(self.directory.join("registry.lock"))?;
        private_file(&current)?;
        private_opened_file(&self.file)?;
        let directory = fs::symlink_metadata(&self.directory)?;
        let opened = self.directory_fd.metadata()?;
        let held = self.file.metadata()?;
        if current.dev() != held.dev()
            || current.ino() != held.ino()
            || directory.dev() != opened.dev()
            || directory.ino() != opened.ino()
            || directory.file_type().is_symlink()
        {
            return Err(denied("forwarding registry identity changed while locked"));
        }
        Ok(())
    }
    pub fn identity(&self) -> io::Result<(u64, u64)> {
        let metadata = self.directory_fd.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }
    pub fn read(&self) -> io::Result<Records> {
        self.verify()?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(self.directory.join("registry.json"))?;
        private_opened_file(&file)?;
        if file.metadata()?.len() > MAX_STATE_BYTES as u64 {
            return Err(denied("forwarding registry exceeds its bound"));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_STATE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(denied("forwarding registry exceeds its bound"));
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            denied(format!(
                "invalid forwarding registry; admission is closed: {error}"
            ))
        })
    }
    pub fn write(&self, records: &Records) -> io::Result<()> {
        self.verify()?;
        let size = splitty_transport::codec::json_size(records, MAX_STATE_BYTES)?;
        let mut bytes = Vec::with_capacity(size);
        serde_json::to_writer(&mut bytes, records)?;
        let temporary = self
            .directory
            .join(format!(".registry-{}.tmp", super::nonce()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        let result = (|| {
            private_opened_file(&file)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            self.verify()?;
            fs::rename(&temporary, self.directory.join("registry.json"))?;
            self.directory_fd.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}
