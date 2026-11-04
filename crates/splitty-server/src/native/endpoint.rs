//! Private native endpoint, separate from owner control and session keepers.
//! Filesystem checks and peer credentials are required in both directions.
use nix::{
    fcntl::OFlag,
    sys::stat::{FileStat, Mode},
};
use splitty_core::config::PrivateDir;
use std::ffi::OsStr;
use std::{
    fs::File,
    io,
    os::{
        fd::{AsFd, AsRawFd},
        unix::fs::MetadataExt,
    },
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::net::{UnixListener, UnixStream};

const MAX_PATH: usize = 103; // portable to macOS, including room for the NUL
pub const SOCKET_NAME: &str = "client.sock";
pub const LOCK_NAME: &str = "daemon.lock";

fn unsafe_path(reason: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason.into())
}

pub fn socket_path() -> PathBuf {
    splitty_core::config::run_dir().join(SOCKET_NAME)
}

/// Pin the private directory before any file or socket operation. Shared
/// group-writable ancestors may affect availability, never endpoint authority.
fn directory(dir: &Path, create: bool) -> io::Result<PrivateDir> {
    if !dir.is_absolute() || !splitty_core::config::socket_dir_supported(dir) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native endpoint needs an absolute path on a local Unix filesystem",
        ));
    }
    let pinned = PrivateDir::open(dir, create)?;
    filesystem(&pinned)?;
    Ok(pinned)
}

fn filesystem(dir: &PrivateDir) -> io::Result<()> {
    let fs = nix::sys::statfs::fstatfs(dir.file())?;
    #[cfg(target_os = "linux")]
    {
        use nix::sys::statfs::{FsType, MSDOS_SUPER_MAGIC, NFS_SUPER_MAGIC, SMB_SUPER_MAGIC};
        // 9p includes DrvFs; the remaining types cannot guarantee local Unix
        // socket/mode semantics. Do not rely on /mnt path spelling alone.
        let unsupported = [
            MSDOS_SUPER_MAGIC,
            NFS_SUPER_MAGIC,
            SMB_SUPER_MAGIC,
            FsType(0x01021997),
            FsType(0xff534d42),
            FsType(0x2011bab0),
            FsType(0x5346544e),
        ];
        if unsupported.contains(&fs.filesystem_type()) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native endpoint requires a local filesystem with Unix sockets and private modes",
            ));
        }
    }
    #[cfg(target_os = "macos")]
    if matches!(
        fs.filesystem_type_name(),
        "nfs" | "smbfs" | "msdos" | "exfat" | "ntfs"
    ) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native endpoint requires a local Unix filesystem",
        ));
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = fs;
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native peer verification is supported on Linux and macOS",
        ));
    }
    Ok(())
}

fn path_checked(path: &Path, create: bool) -> io::Result<PrivateDir> {
    if path.file_name().is_none() || path.as_os_str().len() > MAX_PATH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native socket path is missing or exceeds 103 bytes",
        ));
    }
    directory(
        path.parent()
            .ok_or_else(|| unsafe_path("native socket has no parent"))?,
        create,
    )
}

fn socket_metadata(dir: &PrivateDir, name: &OsStr) -> io::Result<FileStat> {
    let meta = dir.metadata(name)?;
    if meta.st_mode & nix::libc::S_IFMT != nix::libc::S_IFSOCK
        || meta.st_uid != nix::unistd::geteuid().as_raw()
        || meta.st_mode & 0o777 != 0o600
    {
        return Err(unsafe_path(
            "native endpoint must be an owned 0600 socket, without symlinks",
        ));
    }
    Ok(meta)
}

/// macOS has no bindat/connectat or /proc directory aliases. A dedicated
/// thread's cwd pins name resolution without changing the process cwd or
/// leaving a cwd override on an executor thread. The OS discards it on exit.
#[cfg(target_os = "macos")]
fn in_directory<T: Send>(
    dir: &PrivateDir,
    operation: impl FnOnce() -> io::Result<T> + Send,
) -> io::Result<T> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("splitty-unix-socket".into())
            .spawn_scoped(scope, || {
                // libSystem SPI, available since macOS 10.12. Unlike fchdir this
                // changes only this dedicated thread's cwd (libpthread/private.h).
                unsafe extern "C" {
                    fn pthread_fchdir_np(fd: nix::libc::c_int) -> nix::libc::c_int;
                }
                // SAFETY: dir holds a live directory descriptor for this thread's
                // entire lifetime. No global cwd or borrowed Rust memory changes.
                if unsafe { pthread_fchdir_np(dir.file().as_raw_fd()) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                operation()
            })?
            .join()
            .map_err(|_| io::Error::other("native socket thread panicked"))?
    })
}

fn bind_in(dir: &PrivateDir, name: &OsStr) -> io::Result<UnixListener> {
    #[cfg(target_os = "linux")]
    let listener = std::os::unix::net::UnixListener::bind(dir.descriptor_path(name)?)?;
    #[cfg(target_os = "macos")]
    let listener = in_directory(dir, || {
        std::os::unix::net::UnixListener::bind(Path::new(name))
    })?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let listener: std::os::unix::net::UnixListener = {
        let _ = (dir, name);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported native socket platform",
        ));
    };
    listener.set_nonblocking(true)?;
    UnixListener::from_std(listener)
}

async fn connect_in(dir: &PrivateDir, name: &OsStr) -> io::Result<UnixStream> {
    #[cfg(target_os = "linux")]
    {
        UnixStream::connect(dir.descriptor_path(name)?).await
    }
    #[cfg(target_os = "macos")]
    {
        use nix::{
            fcntl::{fcntl, FcntlArg},
            sys::socket::{connect, socket, AddressFamily, SockFlag, SockType, UnixAddr},
        };
        use std::os::fd::{FromRawFd, IntoRawFd};
        let stream = in_directory(dir, || {
            let fd = socket(
                AddressFamily::Unix,
                SockType::Stream,
                SockFlag::empty(),
                None,
            )?;
            fcntl(
                fd.as_raw_fd(),
                FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
            )?;
            fcntl(fd.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
            match connect(fd.as_raw_fd(), &UnixAddr::new(Path::new(name))?) {
                Ok(()) | Err(nix::errno::Errno::EINPROGRESS) => {}
                Err(error) => return Err(error.into()),
            }
            // SAFETY: socket() created this owned nonblocking stream; transfer
            // ownership exactly once to the standard library stream.
            Ok(unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd.into_raw_fd()) })
        })?;
        let stream = UnixStream::from_std(stream)?;
        stream.writable().await?;
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
        Ok(stream)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (dir, name);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported native socket platform",
        ))
    }
}

/// Closing the file releases the lock only when the last inherited descriptor
/// closes. There is deliberately no explicit unlock in Drop: a launcher and its
/// daemon can temporarily share the open file description across exec.
pub struct InstanceLock {
    file: File,
    directory: PrivateDir,
}
impl InstanceLock {
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        Self::in_directory(directory(dir, true)?)
    }
    fn in_directory(directory: PrivateDir) -> io::Result<Self> {
        let file = directory.open_file(
            OsStr::new(LOCK_NAME),
            OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NONBLOCK,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?;
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.uid() != nix::unistd::geteuid().as_raw()
            || meta.mode() & 0o777 != 0o600
            || meta.nlink() != 1
        {
            return Err(unsafe_path("unsafe daemon instance lock"));
        }
        // SAFETY: file owns a live descriptor; flock neither takes ownership nor
        // accesses Rust memory. Closing the final descriptor releases this lock.
        if unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) }
            != 0
        {
            return Err(io::Error::last_os_error());
        }
        directory.verify()?;
        Ok(Self { file, directory })
    }
    fn check_path(&self, path: &Path) -> io::Result<()> {
        if path.parent() != Some(self.directory.path())
            || path.file_name().is_none()
            || path.as_os_str().len() > MAX_PATH
        {
            return Err(unsafe_path(
                "socket does not belong to the held instance directory",
            ));
        }
        self.directory.verify()
    }
    /// Called only after an owned child has exited, while this launcher still
    /// holds the instance lock. A live/unverifiable socket is never removed.
    pub async fn cleanup_socket(&self, path: &Path) -> io::Result<()> {
        self.check_path(path)?;
        clear_stale(&self.directory, path.file_name().expect("checked name")).await
    }
    pub fn file(&self) -> &File {
        &self.file
    }
}

pub struct EndpointGuard {
    name: std::ffi::OsString,
    device: nix::libc::dev_t,
    inode: nix::libc::ino_t,
    pub lock: InstanceLock,
}
impl Drop for EndpointGuard {
    fn drop(&mut self) {
        if self.lock.directory.metadata(&self.name).is_ok_and(|meta| {
            meta.st_dev == self.device
                && meta.st_ino == self.inode
                && meta.st_mode & nix::libc::S_IFMT == nix::libc::S_IFSOCK
        }) {
            let _ = self.lock.directory.remove_file(&self.name);
        }
    }
}
pub struct Bound {
    pub listener: UnixListener,
    pub guard: EndpointGuard,
}

/// Take only the lock deliberately inherited from `daemon start`. The daemon
/// binds its own listener after matching this descriptor to the configured
/// lock inode; no socket-address spelling or connecting proof is required.
pub fn adopt_lock(lock_fd: i32, dir: &Path) -> io::Result<InstanceLock> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    use std::os::fd::FromRawFd;
    if lock_fd < 3 {
        return Err(unsafe_path("invalid inherited lock descriptor"));
    }
    let directory = directory(dir, false)?;
    fcntl(lock_fd, FcntlArg::F_GETFD)?;
    let held = nix::sys::stat::fstat(lock_fd)?;
    let disk = directory.metadata(OsStr::new(LOCK_NAME))?;
    if disk.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG
        || disk.st_uid != nix::unistd::geteuid().as_raw()
        || disk.st_mode & 0o777 != 0o600
        || disk.st_nlink != 1
        || held.st_dev != disk.st_dev
        || held.st_ino != disk.st_ino
    {
        return Err(unsafe_path(
            "inherited instance lock does not name the configured lock",
        ));
    }
    // SAFETY: the verified descriptor is live; flock does not take ownership.
    // Reacquiring the same inherited open file description keeps its lock.
    if unsafe { nix::libc::flock(lock_fd, nix::libc::LOCK_EX | nix::libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    directory.verify()?;
    fcntl(lock_fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
    // SAFETY: the internal caller transferred this verified descriptor; it
    // remains caller-owned on every failure above.
    Ok(InstanceLock {
        file: unsafe { File::from_raw_fd(lock_fd) },
        directory,
    })
}

/// Hold the instance lock before probing/removing a stale socket. A timeout,
/// permission error or live endpoint is never treated as a stale socket.
pub async fn bind(path: &Path) -> io::Result<Bound> {
    let lock = InstanceLock::in_directory(path_checked(path, true)?)?;
    bind_locked(path, lock).await
}

pub async fn bind_locked(path: &Path, lock: InstanceLock) -> io::Result<Bound> {
    lock.check_path(path)?;
    let dir = &lock.directory;
    let name = path.file_name().expect("checked name");
    clear_stale(dir, name).await?;
    dir.verify()?;
    let listener = bind_in(dir, name)?;
    if let Err(error) = dir.private_socket(name) {
        let _ = dir.remove_file(name);
        return Err(error);
    }
    let meta = socket_metadata(dir, name)?;
    let bound = Bound {
        listener,
        guard: EndpointGuard {
            name: name.to_owned(),
            device: meta.st_dev,
            inode: meta.st_ino,
            lock,
        },
    };
    bound.guard.lock.directory.verify()?;
    Ok(bound)
}

async fn clear_stale(dir: &PrivateDir, name: &OsStr) -> io::Result<()> {
    match dir.metadata(name) {
        Ok(_) => {
            let old = socket_metadata(dir, name)?;
            match tokio::time::timeout(Duration::from_secs(1), connect_in(dir, name)).await {
                Ok(Err(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) =>
                {
                    let current = socket_metadata(dir, name)?;
                    if current.st_dev != old.st_dev || current.st_ino != old.st_ino {
                        return Err(unsafe_path("native endpoint changed during stale check"));
                    }
                    dir.remove_file(name)?;
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "a live or unverifiable daemon owns the native endpoint",
                    ))
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

fn peer_uid(stream: &impl AsFd) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
            .ok()
            .map(|creds| creds.uid())
    }
    #[cfg(target_os = "macos")]
    {
        nix::unistd::getpeereid(stream)
            .ok()
            .map(|(uid, _)| uid.as_raw())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = stream;
        None
    }
}
fn same_user(uid: Option<u32>) -> bool {
    uid == Some(nix::unistd::geteuid().as_raw())
}
pub fn verify_peer(stream: &impl AsFd) -> io::Result<()> {
    if same_user(peer_uid(stream)) {
        Ok(())
    } else {
        Err(unsafe_path("native peer user cannot be verified"))
    }
}

pub async fn connect(path: &Path) -> io::Result<UnixStream> {
    let dir = path_checked(path, false)?;
    let name = path.file_name().expect("checked name");
    let before = socket_metadata(&dir, name)?;
    let stream = tokio::time::timeout(Duration::from_secs(5), connect_in(&dir, name))
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "native endpoint connect timed out")
        })??;
    verify_peer(&stream)?;
    dir.verify()?;
    let after = socket_metadata(&dir, name)?;
    if before.st_dev != after.st_dev || before.st_ino != after.st_ino {
        return Err(unsafe_path("native endpoint changed during connect"));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    fn dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }
    #[tokio::test]
    async fn private_modes_both_peer_checks_and_instance_lock_are_mandatory() {
        let dir = dir();
        let path = dir.path().canonicalize().unwrap().join(SOCKET_NAME);
        let bound = bind(&path).await.unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(InstanceLock::acquire(path.parent().unwrap()).is_err());
        let client = connect(&path).await.unwrap();
        let (server, _) = bound.listener.accept().await.unwrap();
        verify_peer(&client).unwrap();
        verify_peer(&server).unwrap();
        assert!(
            verify_peer(bound.guard.lock.file()).is_err(),
            "unavailable socket credentials fail closed"
        );
        assert!(!same_user(None));
        assert!(!same_user(Some(
            nix::unistd::geteuid().as_raw().wrapping_add(1)
        )));
        assert!(bind(&path).await.is_err());
        assert!(path.exists());
        drop(client);
        drop(server);
        drop(bound);
        assert!(!path.exists());
        bind(&path).await.unwrap();
    }
    #[tokio::test]
    async fn group_writable_ancestors_work_without_chmod_or_cwd_changes() {
        let temp = dir();
        let parent = temp.path().canonicalize().unwrap().join("shared");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o775)).unwrap();
        let path = parent.join("private").join(SOCKET_NAME);
        let cwd = std::env::current_dir().unwrap();
        let bound = bind(&path).await.unwrap();
        let client = connect(&path).await.unwrap();
        let (server, _) = bound.listener.accept().await.unwrap();
        verify_peer(&server).unwrap();
        verify_peer(&client).unwrap();
        assert_eq!(std::env::current_dir().unwrap(), cwd);
        assert_eq!(fs::metadata(&parent).unwrap().mode() & 0o777, 0o775);
    }

    #[tokio::test]
    async fn pinned_bind_lock_and_cleanup_never_follow_a_replaced_directory() {
        let temp = dir();
        let root = temp.path().canonicalize().unwrap();
        let original = root.join("original");
        let moved = root.join("moved");
        let lock = InstanceLock::acquire(&original).unwrap();
        fs::rename(&original, &moved).unwrap();
        let replacement = bind(&original.join(SOCKET_NAME)).await.unwrap();
        assert!(lock.directory.verify().is_err());
        // Direct platform operation exercises the race *after* the last path
        // verification: it still binds inside the originally pinned inode.
        let listener = bind_in(&lock.directory, OsStr::new(SOCKET_NAME)).unwrap();
        lock.directory
            .private_socket(OsStr::new(SOCKET_NAME))
            .unwrap();
        let meta = socket_metadata(&lock.directory, OsStr::new(SOCKET_NAME)).unwrap();
        let bound = Bound {
            listener,
            guard: EndpointGuard {
                name: SOCKET_NAME.into(),
                device: meta.st_dev,
                inode: meta.st_ino,
                lock,
            },
        };
        assert!(InstanceLock::acquire(&moved).is_err());
        let client = connect(&moved.join(SOCKET_NAME)).await.unwrap();
        let (server, _) = bound.listener.accept().await.unwrap();
        verify_peer(&client).unwrap();
        verify_peer(&server).unwrap();
        drop(bound);
        assert!(!moved.join(SOCKET_NAME).exists());
        let client = connect(&original.join(SOCKET_NAME)).await.unwrap();
        let (server, _) = replacement.listener.accept().await.unwrap();
        verify_peer(&client).unwrap();
        verify_peer(&server).unwrap();
    }

    #[tokio::test]
    async fn symlinks_files_unsafe_modes_mounts_and_long_paths_fail_closed() {
        let dir = dir();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join(SOCKET_NAME);
        fs::write(&path, "do not unlink").unwrap();
        assert!(bind(&path).await.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "do not unlink");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), &path).unwrap();
        assert!(bind(&path).await.is_err());
        fs::remove_file(&path).unwrap();
        let linked = root.join("linked");
        std::os::unix::fs::symlink(&root, &linked).unwrap();
        assert!(bind(&linked.join(SOCKET_NAME)).await.is_err());
        let public = root.join("public");
        fs::create_dir(&public).unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(bind(&public.join(SOCKET_NAME)).await.is_err());
        assert!(bind(Path::new("/mnt/c/splitty/client.sock")).await.is_err());
        assert!(bind(&root.join("a".repeat(104))).await.is_err());
    }
    #[tokio::test]
    async fn only_a_refused_owned_socket_is_removed_and_cleanup_checks_its_inode() {
        use nix::sys::socket::{socket, AddressFamily, SockFlag, SockType, UnixAddr};
        let dir = dir();
        let path = dir.path().canonicalize().unwrap().join(SOCKET_NAME);
        let stale = socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::SOCK_CLOEXEC,
            None,
        )
        .unwrap();
        nix::sys::socket::bind(stale.as_raw_fd(), &UnixAddr::new(&path).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        drop(stale);
        let bound = bind(&path).await.unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, "replacement").unwrap();
        drop(bound);
        assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    }

    #[tokio::test]
    async fn inherited_lock_keeps_exclusivity_and_child_binds_after_launcher_exits() {
        use nix::fcntl::{fcntl, FcntlArg, FdFlag};
        let temp = dir();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join(SOCKET_NAME);
        let parent = InstanceLock::acquire(&root).unwrap();
        assert!(!path.exists(), "launcher never binds a socket");
        let fd = fcntl(parent.file().as_raw_fd(), FcntlArg::F_DUPFD_CLOEXEC(3)).unwrap();
        let child_lock = adopt_lock(fd, &root).unwrap();
        assert!(
            FdFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFD).unwrap())
                .contains(FdFlag::FD_CLOEXEC)
        );
        drop(parent);
        assert!(InstanceLock::acquire(&root).is_err());
        let child = bind_locked(&path, child_lock).await.unwrap();
        let client = connect(&path).await.unwrap();
        let (server, _) = child.listener.accept().await.unwrap();
        verify_peer(&server).unwrap();
        drop(client);
        drop(server);
        drop(child);
        assert!(!path.exists());
        // Parallel PTY tests may briefly inherit CLOEXEC descriptors between
        // fork and exec. The lock correctly lasts until the final inherited
        // descriptor closes, not just until this test drops its own handles.
        let replacement = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match InstanceLock::acquire(&root) {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    result => return result,
                }
            }
        })
        .await
        .expect("the final inherited lock descriptor closes")
        .unwrap();
        bind_locked(&path, replacement).await.unwrap();
    }

    #[tokio::test]
    async fn adoption_refuses_wrong_lock_inode_path_and_descriptor_type_without_consuming_them() {
        let temp = dir();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join(SOCKET_NAME);
        let bound = bind(&path).await.unwrap();
        assert!(adopt_lock(0, &root).is_err());
        assert!(adopt_lock(bound.listener.as_raw_fd(), &root).is_err());
        let other = root.join("other");
        let other_lock = InstanceLock::acquire(&other).unwrap();
        assert!(adopt_lock(other_lock.file().as_raw_fd(), &root).is_err());
        assert!(adopt_lock(bound.guard.lock.file().as_raw_fd(), &other).is_err());
        let wrong = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("wrong.lock"))
            .unwrap();
        assert!(adopt_lock(wrong.as_raw_fd(), &root).is_err());
        let client = connect(&path).await.unwrap();
        let (server, _) = bound.listener.accept().await.unwrap();
        verify_peer(&server).unwrap();
        drop(client);
        assert!(InstanceLock::acquire(&root).is_err());
    }
}
