//! Descriptor-relative access to an owned private directory. Group-writable
//! ancestors are namespace/liveness concerns, not authority to its contents.
use nix::{
    fcntl::{openat, renameat, AtFlags, OFlag},
    sys::stat::{fchmod, fchmodat, fstat, fstatat, mkdirat, FchmodatFlags, FileStat, Mode},
    unistd::{unlinkat, UnlinkatFlags},
};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    path::{Component, Path, PathBuf},
};

fn denied(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message.into())
}

fn directory_flags() -> OFlag {
    #[cfg(target_os = "linux")]
    let access = nix::libc::O_PATH;
    #[cfg(target_os = "macos")]
    let access = nix::libc::O_SEARCH;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let access = nix::libc::O_RDONLY;
    OFlag::from_bits_retain(access) | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC
}

/// Retaining this handle prevents an ancestor rename from redirecting file
/// creation, publication or cleanup. It does not prevent namespace disruption.
#[derive(Debug)]
pub struct PrivateDir {
    file: File,
    path: PathBuf,
}

impl PrivateDir {
    pub fn open(path: &Path, create: bool) -> io::Result<Self> {
        Self::open_directory(path, create, false)
    }

    /// Authorized config writers may tighten a directory left readable by an
    /// older version. Never repair endpoints, shared ancestors or a directory
    /// whose entries another user could replace.
    pub fn prepare_config(path: &Path) -> io::Result<Self> {
        if path.file_name().is_none() {
            return Err(denied("configuration must name a directory below the root"));
        }
        let directory = Self::open_directory(path, true, true)?;
        directory.verify()?;
        Ok(directory)
    }

    fn open_directory(path: &Path, create: bool, repair_config: bool) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(denied("private directory must be absolute"));
        }
        // Reject non-normal spellings before creating anything.
        if path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(denied("unsafe private directory component"));
        }
        let mut file: Option<File> = None;
        let mut current = PathBuf::new();
        let own = nix::unistd::geteuid().as_raw();
        for part in path.components() {
            current.push(part);
            let parent = file.as_ref().map(AsRawFd::as_raw_fd);
            let name = part.as_os_str();
            // O_PATH/O_SEARCH remain sufficient for ancestors. The final
            // config descriptor must support fchmod without reopening a path.
            let flags = if repair_config && current == path {
                OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC
            } else {
                directory_flags()
            };
            let descriptor = match openat(parent, name, flags, Mode::empty()) {
                Ok(descriptor) => descriptor,
                Err(nix::errno::Errno::ENOENT) if create => {
                    match mkdirat(parent, name, Mode::S_IRWXU) {
                        Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
                        Err(error) => return Err(error.into()),
                    }
                    openat(parent, name, flags, Mode::empty())?
                }
                Err(error) => return Err(error.into()),
            };
            // SAFETY: openat returned a new descriptor owned by this call.
            let opened = unsafe { File::from_raw_fd(descriptor) };
            let mut meta = fstat(opened.as_raw_fd())?;
            if meta.st_mode & nix::libc::S_IFMT != nix::libc::S_IFDIR
                || (meta.st_uid != own && meta.st_uid != 0)
                || (meta.st_mode & 0o002 != 0 && meta.st_mode & 0o1000 == 0)
            {
                return Err(denied(format!(
                    "unsafe private directory ancestor: {}",
                    current.display()
                )));
            }
            if current == path {
                if repair_config
                    && meta.st_uid == own
                    && meta.st_mode & 0o022 == 0
                    && meta.st_mode & 0o777 != 0o700
                {
                    fchmod(opened.as_raw_fd(), Mode::S_IRWXU)?;
                    meta = fstat(opened.as_raw_fd())?;
                }
                if meta.st_uid != own || meta.st_mode & 0o777 != 0o700 {
                    return Err(denied(
                        "private directory must belong to this user with mode 0700",
                    ));
                }
            }
            file = Some(opened);
        }
        Ok(Self {
            file: file.ok_or_else(|| denied("missing private directory"))?,
            path: path.to_owned(),
        })
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path replacement is an error, never permission to use its new object.
    pub fn verify(&self) -> io::Result<()> {
        let current = Self::open(&self.path, false)?;
        let before = fstat(self.file.as_raw_fd())?;
        let after = fstat(current.file.as_raw_fd())?;
        if before.st_dev != after.st_dev || before.st_ino != after.st_ino {
            return Err(denied("private directory changed during operation"));
        }
        Ok(())
    }

    fn name(name: &OsStr) -> io::Result<&Path> {
        use std::os::unix::ffi::OsStrExt;
        let bytes = name.as_bytes();
        // Path::components normalizes x/ and x/., which can make the kernel
        // follow a final symlink despite O_NOFOLLOW. Validate actual bytes.
        if bytes.is_empty()
            || bytes == b"."
            || bytes == b".."
            || bytes.contains(&b'/')
            || bytes.contains(&0)
        {
            return Err(denied("private file name must be one normal component"));
        }
        Ok(Path::new(name))
    }

    pub fn metadata(&self, name: &OsStr) -> io::Result<FileStat> {
        Ok(fstatat(
            Some(self.file.as_raw_fd()),
            Self::name(name)?,
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )?)
    }

    pub fn open_file(&self, name: &OsStr, flags: OFlag, mode: Mode) -> io::Result<File> {
        let fd = openat(
            Some(self.file.as_raw_fd()),
            Self::name(name)?,
            flags | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            mode,
        )?;
        // SAFETY: openat transferred this newly opened descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        Ok(renameat(
            Some(self.file.as_raw_fd()),
            Self::name(from)?,
            Some(self.file.as_raw_fd()),
            Self::name(to)?,
        )?)
    }

    pub fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        Ok(unlinkat(
            Some(self.file.as_raw_fd()),
            Self::name(name)?,
            UnlinkatFlags::NoRemoveDir,
        )?)
    }

    /// Only a freshly bound owned socket in this private directory is changed.
    pub fn private_socket(&self, name: &OsStr) -> io::Result<()> {
        let meta = self.metadata(name)?;
        if meta.st_mode & nix::libc::S_IFMT != nix::libc::S_IFSOCK
            || meta.st_uid != nix::unistd::geteuid().as_raw()
        {
            return Err(denied("private endpoint is not an owned socket"));
        }
        // Other users cannot replace entries inside this retained 0700
        // directory. The no-follow metadata check rejects an existing symlink.
        Ok(fchmodat(
            Some(self.file.as_raw_fd()),
            Self::name(name)?,
            Mode::S_IRUSR | Mode::S_IWUSR,
            FchmodatFlags::FollowSymlink,
        )?)
    }

    #[cfg(target_os = "linux")]
    pub fn descriptor_path(&self, name: &OsStr) -> io::Result<PathBuf> {
        let alias = PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()));
        use std::os::unix::fs::MetadataExt;
        let held = fstat(self.file.as_raw_fd())?;
        let resolved = std::fs::metadata(&alias).map_err(|_| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "native sockets require /proc/self/fd directory access",
            )
        })?;
        if resolved.dev() != held.st_dev || resolved.ino() != held.st_ino {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native socket directory alias mismatch",
            ));
        }
        Ok(alias.join(Self::name(name)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::unix::fs::PermissionsExt};

    #[test]
    fn config_repair_tightens_only_the_owned_final_directory() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().canonicalize().unwrap().join("shared");
        let config = parent.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o775)).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(config.join("config.toml"), b"# preserved\n").unwrap();
        assert!(PrivateDir::open(&config, false).is_err());
        assert!(PrivateDir::open(&config, true).is_err());
        let before = fs::metadata(&config).unwrap();
        let pinned = PrivateDir::prepare_config(&config).unwrap();
        use std::os::unix::fs::MetadataExt;
        let after = pinned.file().metadata().unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        assert_eq!(after.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            fs::read(config.join("config.toml")).unwrap(),
            b"# preserved\n"
        );
        assert_eq!(
            fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o775
        );
        pinned.verify().unwrap();
        PrivateDir::prepare_config(&config).unwrap();

        // A later name replacement never redirects operations through the
        // retained descriptor, including a handle returned by config repair.
        let moved = parent.join("original");
        fs::rename(&config, &moved).unwrap();
        fs::create_dir(&config).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(config.join("config.toml"), b"replacement").unwrap();
        assert!(pinned.verify().is_err());
        pinned.remove_file(OsStr::new("config.toml")).unwrap();
        assert!(!moved.join("config.toml").exists());
        assert_eq!(
            fs::read(config.join("config.toml")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn config_repair_refuses_writable_contents_and_symlinks_without_chmod() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let config = root.join("config");
        fs::create_dir(&config).unwrap();
        for mode in [0o770, 0o775, 0o777, 0o1707] {
            fs::set_permissions(&config, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                PrivateDir::prepare_config(&config).is_err(),
                "mode {mode:o}"
            );
            assert_eq!(
                fs::metadata(&config).unwrap().permissions().mode() & 0o7777,
                mode
            );
        }
        fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&config, root.join("alias")).unwrap();
        assert!(PrivateDir::prepare_config(&root.join("alias")).is_err());
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(PrivateDir::prepare_config(&root.join("alias/child")).is_err());
        assert!(!config.join("child").exists());
    }

    #[test]
    fn group_writable_parents_are_preserved_and_operations_keep_the_original_directory() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().canonicalize().unwrap().join("shared");
        let private = parent.join("private");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o775)).unwrap();
        let pinned = PrivateDir::open(&private, true).unwrap();
        let flags = OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_WRONLY;
        pinned
            .open_file(OsStr::new("original"), flags, Mode::S_IRUSR | Mode::S_IWUSR)
            .unwrap()
            .write_all(b"owned")
            .unwrap();
        assert_eq!(
            fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o775
        );
        let moved = parent.join("moved");
        fs::rename(&private, &moved).unwrap();
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(private.join("original"), b"replacement").unwrap();
        assert!(pinned.verify().is_err());
        pinned
            .rename(OsStr::new("original"), OsStr::new("published"))
            .unwrap();
        assert_eq!(fs::read(moved.join("published")).unwrap(), b"owned");
        pinned.remove_file(OsStr::new("published")).unwrap();
        assert_eq!(fs::read(private.join("original")).unwrap(), b"replacement");
    }

    #[test]
    fn symlinks_world_writable_ancestors_and_non_private_final_directories_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let dir = root.join("private");
        let pinned = PrivateDir::open(&dir, true).unwrap();
        std::os::unix::fs::symlink(&dir, root.join("alias")).unwrap();
        assert!(PrivateDir::open(&root.join("alias"), false).is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(PrivateDir::open(&dir, false).is_err());
        assert!(pinned.verify().is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(PrivateDir::open(&dir.join("child"), true).is_err());
        assert!(!dir.join("child").exists());
        for name in [
            "",
            ".",
            "..",
            "../escaped",
            "child/file",
            "/absolute",
            "child/",
            "child/.",
            "child//",
        ] {
            assert!(pinned
                .open_file(
                    OsStr::new(name),
                    OFlag::O_CREAT | OFlag::O_WRONLY,
                    Mode::S_IRWXU
                )
                .is_err());
        }
    }
}
