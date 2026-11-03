//! Private native endpoint, separate from owner control and session keepers.
//! Filesystem checks and peer credentials are required in both directions.
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::{AsFd, AsRawFd},
        unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
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

/// Validate every ancestor without following a link. Root/owner ancestors may
/// be traversable; writable shared ancestors require the sticky-bit protection
/// (for example /tmp), while the final endpoint directory must be owner-only.
fn directory(dir: &Path, create: bool) -> io::Result<()> {
    if !dir.is_absolute() || !splitty_core::config::socket_dir_supported(dir) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native endpoint needs an absolute path on a local Unix filesystem",
        ));
    }
    let own = nix::unistd::geteuid().as_raw();
    let mut current = PathBuf::new();
    for component in dir.components() {
        match component {
            Component::RootDir => current.push(component),
            Component::Normal(name) => current.push(name),
            _ => {
                return Err(unsafe_path(
                    "native endpoint path contains a dot or prefix component",
                ))
            }
        }
        let meta = match fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                match fs::DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                fs::symlink_metadata(&current)?
            }
            Err(error) => return Err(error),
        };
        if !meta.is_dir() || meta.file_type().is_symlink() || (meta.uid() != own && meta.uid() != 0)
        {
            return Err(unsafe_path(format!(
                "unsafe native endpoint ancestor: {}",
                current.display()
            )));
        }
        if meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0 {
            return Err(unsafe_path(format!(
                "writable native endpoint ancestor: {}",
                current.display()
            )));
        }
        if current == dir && (meta.uid() != own || meta.mode() & 0o777 != 0o700) {
            return Err(unsafe_path(
                "native endpoint directory must belong to this user with mode 0700",
            ));
        }
    }
    filesystem(dir)
}

fn filesystem(dir: &Path) -> io::Result<()> {
    let fs = nix::sys::statfs::statfs(dir)?;
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

fn path_checked(path: &Path, create: bool) -> io::Result<()> {
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

fn socket_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket()
        || meta.uid() != nix::unistd::geteuid().as_raw()
        || meta.mode() & 0o777 != 0o600
    {
        return Err(unsafe_path(
            "native endpoint must be an owned 0600 socket, without symlinks",
        ));
    }
    Ok(meta)
}

/// Closing the file releases the lock only when the last inherited descriptor
/// closes. There is deliberately no explicit unlock in Drop: a launcher and its
/// daemon can temporarily share the open file description across exec.
pub struct InstanceLock {
    file: File,
}
impl InstanceLock {
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        directory(dir, true)?;
        let path = dir.join(LOCK_NAME);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
            .open(&path)?;
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
        Ok(Self { file })
    }
    pub fn file(&self) -> &File {
        &self.file
    }
}

pub struct EndpointGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
    cleanup: bool,
    pub lock: InstanceLock,
}
impl EndpointGuard {
    /// The launched daemon now owns endpoint cleanup and inherited descriptors.
    pub fn preserve_endpoint(&mut self) {
        self.cleanup = false;
    }
}
impl Drop for EndpointGuard {
    fn drop(&mut self) {
        if self.cleanup
            && fs::symlink_metadata(&self.path).is_ok_and(|meta| {
                meta.dev() == self.device
                    && meta.ino() == self.inode
                    && meta.file_type().is_socket()
            })
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
pub struct Bound {
    pub listener: UnixListener,
    pub guard: EndpointGuard,
}

/// Take the two descriptors deliberately inherited from `daemon start`.
/// Validate their types and exact configured paths before taking ownership;
/// passing arbitrary descriptors to the internal command must fail closed.
pub fn adopt(listen_fd: i32, lock_fd: i32, path: &Path) -> io::Result<Bound> {
    use nix::{
        fcntl::{fcntl, FcntlArg, FdFlag},
        sys::socket::{getsockopt, sockopt},
    };
    use std::os::fd::{BorrowedFd, FromRawFd};
    if listen_fd < 3 || lock_fd < 3 || listen_fd == lock_fd {
        return Err(unsafe_path("invalid inherited native descriptors"));
    }
    path_checked(path, false)?;
    let meta = socket_metadata(path)?;
    fcntl(listen_fd, FcntlArg::F_GETFD)?;
    fcntl(lock_fd, FcntlArg::F_GETFD)?;
    // SAFETY: F_GETFD verified both descriptors are open. These borrows end
    // before ownership is transferred below.
    let socket = unsafe { BorrowedFd::borrow_raw(listen_fd) };
    if getsockopt(&socket, sockopt::SockType)? != nix::sys::socket::SockType::Stream {
        return Err(unsafe_path("native listener is not a stream socket"));
    }
    let listening = match getsockopt(&socket, sockopt::AcceptConn) {
        Ok(value) => value,
        Err(nix::errno::Errno::ENOPROTOOPT) => {
            nix::sys::socket::listen(&socket, nix::sys::socket::Backlog::MAXCONN).is_ok()
        }
        Err(error) => return Err(error.into()),
    };
    let address: nix::sys::socket::UnixAddr = nix::sys::socket::getsockname(listen_fd)?;
    if !listening || address.path() != Some(path) {
        return Err(unsafe_path(
            "inherited listener does not name the configured native endpoint",
        ));
    }
    let lock_meta = nix::sys::stat::fstat(lock_fd)?;
    let lock_path = path.parent().expect("checked parent").join(LOCK_NAME);
    let on_disk = fs::symlink_metadata(lock_path)?;
    if !on_disk.is_file()
        || on_disk.file_type().is_symlink()
        || on_disk.uid() != nix::unistd::geteuid().as_raw()
        || on_disk.mode() & 0o777 != 0o600
        || on_disk.nlink() != 1
        || lock_meta.st_dev as u64 != on_disk.dev()
        || lock_meta.st_ino as u64 != on_disk.ino()
    {
        return Err(unsafe_path(
            "inherited instance lock does not name the configured lock",
        ));
    }
    // SAFETY: lock_fd is an open descriptor verified above. A launcher passes
    // the same open file description, so reacquiring its lock cannot unlock it.
    if unsafe { nix::libc::flock(lock_fd, nix::libc::LOCK_EX | nix::libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    fcntl(listen_fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
    fcntl(lock_fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
    // SAFETY: validated descriptors were transferred to this internal command
    // by its caller; neither is owned elsewhere in this process.
    let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(listen_fd) };
    let file = unsafe { File::from_raw_fd(lock_fd) };
    listener.set_nonblocking(true)?;
    Ok(Bound {
        listener: UnixListener::from_std(listener)?,
        guard: EndpointGuard {
            path: path.to_owned(),
            device: meta.dev(),
            inode: meta.ino(),
            cleanup: true,
            lock: InstanceLock { file },
        },
    })
}

/// Hold the instance lock before probing/removing a stale socket. A timeout,
/// permission error or live endpoint is never treated as a stale socket.
pub async fn bind(path: &Path) -> io::Result<Bound> {
    path_checked(path, true)?;
    let lock = InstanceLock::acquire(path.parent().expect("checked parent"))?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let old = socket_metadata(path)?;
            match tokio::time::timeout(Duration::from_secs(1), UnixStream::connect(path)).await {
                Ok(Err(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) =>
                {
                    let current = socket_metadata(path)?;
                    if current.dev() != old.dev() || current.ino() != old.ino() {
                        return Err(unsafe_path("native endpoint changed during stale check"));
                    }
                    fs::remove_file(path)?;
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
    let listener = UnixListener::bind(path)?;
    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    let meta = socket_metadata(path)?;
    Ok(Bound {
        listener,
        guard: EndpointGuard {
            path: path.to_owned(),
            device: meta.dev(),
            inode: meta.ino(),
            cleanup: true,
            lock,
        },
    })
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
    path_checked(path, false)?;
    let before = socket_metadata(path)?;
    let stream = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(path))
        .await
        .map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "native endpoint connect timed out")
        })??;
    verify_peer(&stream)?;
    let after = socket_metadata(path)?;
    if before.dev() != after.dev() || before.ino() != after.ino() {
        return Err(unsafe_path("native endpoint changed during connect"));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn inherited_descriptors_keep_the_instance_locked_after_the_launcher_exits() {
        use nix::fcntl::{fcntl, FcntlArg, FdFlag};
        let dir = dir();
        let path = dir.path().canonicalize().unwrap().join(SOCKET_NAME);
        let mut parent = bind(&path).await.unwrap();
        // Atomic CLOEXEC avoids leaking the fixture lock into children spawned
        // by parallel tests between duplication and adoption.
        let listen_fd = fcntl(parent.listener.as_raw_fd(), FcntlArg::F_DUPFD_CLOEXEC(3)).unwrap();
        let lock_fd = fcntl(
            parent.guard.lock.file().as_raw_fd(),
            FcntlArg::F_DUPFD_CLOEXEC(3),
        )
        .unwrap();
        let child = adopt(listen_fd, lock_fd, &path).unwrap();
        for fd in [listen_fd, lock_fd] {
            assert!(
                FdFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFD).unwrap())
                    .contains(FdFlag::FD_CLOEXEC)
            );
        }
        parent.guard.preserve_endpoint();
        drop(parent);
        assert!(InstanceLock::acquire(path.parent().unwrap()).is_err());
        let client = connect(&path).await.unwrap();
        let (server, _) = child.listener.accept().await.unwrap();
        verify_peer(&server).unwrap();
        drop(client);
        drop(server);
        drop(child);
        assert!(!path.exists());
        bind(&path).await.unwrap();
    }

    #[tokio::test]
    async fn adoption_refuses_wrong_lock_path_and_non_native_or_non_listening_descriptors() {
        let dir = dir();
        let path = dir.path().canonicalize().unwrap().join(SOCKET_NAME);
        let bound = bind(&path).await.unwrap();
        let listener = bound.listener.as_raw_fd();
        let lock = bound.guard.lock.file().as_raw_fd();
        assert!(adopt(0, lock, &path).is_err());
        assert!(adopt(listener, listener, &path).is_err());
        let wrong = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.path().join("wrong.lock"))
            .unwrap();
        assert!(adopt(listener, wrong.as_raw_fd(), &path).is_err());
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(adopt(tcp.as_raw_fd(), lock, &path).is_err());
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(adopt(a.as_raw_fd(), lock, &path).is_err());
        assert!(adopt(listener, lock, &dir.path().join("different.sock")).is_err());
        // Rejections do not unlink the live endpoint or take these FDs.
        let client = connect(&path).await.unwrap();
        let (server, _) = bound.listener.accept().await.unwrap();
        verify_peer(&server).unwrap();
        drop(client);
    }
}
