//! Where keepers are found: the `keepers` directory below the daemon's run
//! directory, private to the user, with a socket per keeper. Both sides check
//! that the other end of a connection is the same user.

use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// A socket path may be at most 103 bytes on macOS (`sun_path` holds 104
/// with its terminating NUL); Linux allows 107.
const MAX_SOCKET_PATH: usize = 103;

/// Letters and digits of the session id a socket name keeps.
const NAME_LENGTH: usize = 16;

/// The keepers of this configuration directory.
pub fn keepers_dir() -> PathBuf {
    crate::config::run_dir().join("keepers")
}

/// Create `dir` private to the user, where Unix sockets work.
pub fn prepare(dir: &Path) -> io::Result<()> {
    if !crate::config::socket_dir_supported(dir) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "refusing to place keeper sockets in {}: Unix sockets do not work there",
                dir.display()
            ),
        ));
    }
    crate::config::ensure_private_dir(dir)
}

/// A free socket path in `dir` for `session_id`: the first letters and digits
/// of the id, then `-2`, `-3` … while a live keeper holds the name. A socket
/// nobody answers on is a dead keeper's and is removed. The name only has to
/// be unique; a keeper's identity is its `hello`.
pub fn socket_path(dir: &Path, session_id: &str) -> io::Result<PathBuf> {
    let base: String = session_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(NAME_LENGTH)
        .collect();
    let base = if base.is_empty() {
        "session".to_owned()
    } else {
        base
    };
    for attempt in 1..=1000u32 {
        let name = if attempt == 1 {
            format!("{base}.sock")
        } else {
            format!("{base}-{attempt}.sock")
        };
        let path = dir.join(name);
        if path.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "the keeper socket path {} is longer than {MAX_SOCKET_PATH} bytes",
                    path.display()
                ),
            ));
        }
        if !path.exists() || remove_if_dead(&path) {
            return Ok(path);
        }
    }
    Err(io::Error::other("no free keeper socket name"))
}

/// The socket files in `dir`.
pub fn sockets(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sock"))
        .collect();
    paths.sort();
    paths
}

/// Remove a socket nobody answers on. True when it is gone. Any answer other
/// than a refusal keeps the socket: a live keeper's must never be removed.
pub fn remove_if_dead(path: &Path) -> bool {
    for _ in 0..5 {
        match UnixStream::connect(path) {
            Ok(_) => return false,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e)
                if e.kind() == io::ErrorKind::ConnectionRefused
                    || e.kind() == io::ErrorKind::NotFound =>
            {
                return std::fs::remove_file(path).is_ok() || !path.exists();
            }
            Err(_) => return false,
        }
    }
    false
}

/// Connect to a keeper, checking that it runs as this user.
pub fn connect(path: &Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(path)?;
    if !peer_is_owner(&stream) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} belongs to another user", path.display()),
        ));
    }
    Ok(stream)
}

/// Is the other end of `stream` the same user as this process? Unreadable
/// credentials are not a reason to trust.
pub fn peer_is_owner(stream: &impl AsFd) -> bool {
    same_user(peer_uid(stream), nix::unistd::geteuid().as_raw())
}

fn same_user(peer: Option<u32>, own: u32) -> bool {
    peer == Some(own)
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &impl AsFd) -> Option<u32> {
    nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
        .ok()
        .map(|credentials| credentials.uid())
}

/// macOS has no SO_PEERCRED; `getpeereid` names the peer's effective user.
#[cfg(target_os = "macos")]
fn peer_uid(stream: &impl AsFd) -> Option<u32> {
    nix::unistd::getpeereid(stream)
        .ok()
        .map(|(uid, _)| uid.as_raw())
}

/// Without a peer credential call, only the directory's and the socket's
/// modes keep other users out.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn peer_uid(_stream: &impl AsFd) -> Option<u32> {
    Some(nix::unistd::geteuid().as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    /// A socket file nobody listens on, as a keeper that died with the
    /// machine leaves. Bound but never listening: a listener this test
    /// dropped could still answer for a moment from a process a neighbouring
    /// test forks meanwhile, which holds a copy until it executes.
    fn dead_socket(path: &Path) {
        use nix::sys::socket::{bind, socket, AddressFamily, SockFlag, SockType, UnixAddr};
        use std::os::fd::AsRawFd;
        let socket = socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::empty(),
            None,
        )
        .expect("socket");
        bind(socket.as_raw_fd(), &UnixAddr::new(path).expect("address")).expect("dead keeper");
    }

    #[test]
    fn names_come_from_the_session_id_and_skip_live_keepers() {
        let dir = tempfile::tempdir().expect("dir");
        let first = socket_path(dir.path(), "3f9a1111-0000-4000-8000-000000000001").expect("path");
        assert_eq!(first, dir.path().join("3f9a111100004000.sock"));

        let _live = UnixListener::bind(&first).expect("live keeper");
        let second = socket_path(dir.path(), "3f9a1111-0000-4000-8000-000000000001").expect("path");
        assert_eq!(second, dir.path().join("3f9a111100004000-2.sock"));

        // A keeper that died with the machine left its socket behind.
        dead_socket(&second);
        assert!(second.exists());
        assert_eq!(
            socket_path(dir.path(), "3f9a1111-0000-4000-8000-000000000001").expect("path"),
            second
        );
        assert!(!second.exists(), "the dead socket is removed");

        assert_eq!(
            socket_path(dir.path(), "../--").expect("path"),
            dir.path().join("session.sock")
        );
        assert_eq!(sockets(dir.path()), vec![first]);
    }

    #[test]
    fn long_paths_and_unsupported_locations_are_refused() {
        let long = PathBuf::from(format!("/{}", "d".repeat(100)));
        assert!(socket_path(&long, "abcd").is_err());
        assert!(prepare(Path::new("/mnt/c/Users/me/splitty/run/keepers")).is_err());
        assert!(prepare(Path::new("/media/usb/keepers")).is_err());
    }

    #[test]
    fn the_directory_is_private() {
        let dir = tempfile::tempdir().expect("dir");
        let keepers = dir.path().join("run/keepers");
        prepare(&keepers).expect("prepare");
        let mode = std::fs::metadata(&keepers)
            .expect("meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn peers_of_another_or_an_unknown_user_are_refused() {
        assert!(same_user(Some(501), 501));
        assert!(!same_user(Some(0), 501));
        assert!(!same_user(None, 501));
        let (left, _right) = UnixStream::pair().expect("pair");
        assert!(
            peer_is_owner(&left),
            "a socket of this process is this user's"
        );
    }
}
