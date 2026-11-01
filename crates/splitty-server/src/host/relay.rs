//! `splitty remote-relay`: the host side of a reverse forward (`remote-hosts`,
//! Remote Relay; `access-control`, Hub Reverse Forwards).
//!
//! The client's `ssh -R` makes `sshd` listen on a Unix socket in the private
//! relay directory and carry each connection to it to `localhost:<local>` on
//! the client's machine. `serve` puts a TCP port on the host's loopback in
//! front of that socket and admits only connections from the user it runs
//! as: loopback is shared by every user of the host, the socket (owner-only,
//! in an owner-only directory) is not. The owner of a connection is read
//! from `/proc/net/tcp`, so the relay runs on Linux only.
//!
//! `prepare` makes the directory and prints it; `serve` reports one line on
//! standard output, [`READY`] or [`FAILED`] with a code, and then lives as
//! long as its standard input delivers something at least every
//! [`HEARTBEAT_TIMEOUT`].

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The line `serve` prints once it listens.
pub const READY: &str = "SPLITTY_RELAY_READY_V1";

/// The start of the line `serve` prints when it cannot listen, followed by
/// a code and a message.
pub const FAILED: &str = "SPLITTY_RELAY_FAILED_V1";

/// The relay directory's name in the run directory.
pub const DIR_NAME: &str = "relay";

/// How long `serve` ends after its input last delivered something.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);

/// How long `serve` waits for `sshd` to create the socket.
const SOCKET_WAIT: Duration = Duration::from_secs(10);

/// Socket files at least this old that refuse connections are left over
/// from a relay that could not remove its own, and `prepare` removes them.
/// Younger ones may belong to a forward `sshd` is just setting up.
const STALE_AFTER: Duration = Duration::from_secs(60);

/// At most this many connections are relayed at once; more are closed.
const MAX_CONNECTIONS: usize = 256;

/// The relay directory of this host's server.
pub fn relay_dir() -> PathBuf {
    splitty_core::config::run_dir().join(DIR_NAME)
}

/// Why `serve` does not listen: the code of its [`FAILED`] line and a
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The line that reports it, on one line whatever the message holds.
    pub fn line(&self) -> String {
        let message: String = self
            .message
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        format!("{FAILED} {} {message}", self.code)
    }
}

/// Whether `name` is the name of a relay socket: 16 lowercase hex characters
/// and `.sock`.
pub fn is_socket_name(name: &str) -> bool {
    name.strip_suffix(".sock").is_some_and(|stem| {
        stem.len() == 16
            && stem
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `socket` if it names a relay socket directly inside `dir`.
fn check_socket_path(dir: &Path, socket: &Path) -> Result<(), Failure> {
    let inside = socket.parent() == Some(dir);
    let named = socket
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_socket_name);
    if inside && named {
        Ok(())
    } else {
        Err(Failure::new(
            "socket",
            format!(
                "{} is not a relay socket in {}",
                socket.display(),
                dir.display()
            ),
        ))
    }
}

#[cfg(target_os = "linux")]
pub use linux::{prepare, serve};

/// Off Linux nothing tells the user a connection comes from.
#[cfg(not(target_os = "linux"))]
pub fn prepare() -> Result<String, String> {
    Err(
        "Sharing a port needs a Linux host: this host cannot tell which user a \
         connection comes from."
            .into(),
    )
}

#[cfg(not(target_os = "linux"))]
pub async fn serve(port: u16, socket: &Path) -> i32 {
    let _ = (port, socket);
    println!(
        "{}",
        Failure::new("unsupported", "sharing a port needs a Linux host").line()
    );
    1
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream, UnixStream};
    use tokio::sync::Semaphore;

    use super::{
        check_socket_path, relay_dir, Failure, HEARTBEAT_TIMEOUT, MAX_CONNECTIONS, READY,
        SOCKET_WAIT, STALE_AFTER,
    };

    /// `remote-relay prepare`: make the relay directory owner-only, check
    /// that connections can be told apart by user, remove stale sockets,
    /// and return the JSON line to print.
    pub fn prepare() -> Result<String, String> {
        let dir = prepare_in(&relay_dir(), Path::new("/proc/net/tcp"), STALE_AFTER)?;
        let dir = dir
            .to_str()
            .ok_or_else(|| format!("the relay directory {} is not UTF-8", dir.display()))?;
        Ok(serde_json::json!({"version": 1, "dir": dir}).to_string())
    }

    pub(super) fn prepare_in(
        dir: &Path,
        tcp: &Path,
        stale_after: Duration,
    ) -> Result<PathBuf, String> {
        if !dir.is_absolute() {
            return Err(format!(
                "the relay directory {} is not absolute",
                dir.display()
            ));
        }
        std::fs::File::open(tcp).map_err(|error| {
            format!(
                "Sharing a port needs {}, to tell which user a connection comes from: {error}",
                tcp.display()
            )
        })?;
        // A link would let the directory be somewhere else entirely.
        if let Ok(meta) = std::fs::symlink_metadata(dir) {
            if !meta.is_dir() {
                return Err(format!("{} is not a directory", dir.display()));
            }
        }
        splitty_core::config::ensure_private_dir(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let meta = std::fs::symlink_metadata(dir)
            .map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
        if !meta.is_dir() || meta.uid() != own_uid() || meta.mode() & 0o077 != 0 {
            return Err(format!(
                "{} is not a directory only this user can use",
                dir.display()
            ));
        }
        remove_stale(dir, stale_after);
        Ok(dir.to_owned())
    }

    /// Remove the sockets no relay serves any more: old enough, and refusing
    /// connections, which means nothing listens on them.
    fn remove_stale(dir: &Path, stale_after: Duration) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let old = meta
                .modified()
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age >= stale_after);
            if !meta.file_type().is_socket() || !old {
                continue;
            }
            if let Err(error) = std::os::unix::net::UnixStream::connect(&path) {
                if error.kind() == io::ErrorKind::ConnectionRefused {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    fn own_uid() -> u32 {
        nix::unistd::getuid().as_raw()
    }

    /// `remote-relay serve --port <port> --socket <path>`: the exit status.
    pub async fn serve(port: u16, socket: &Path) -> i32 {
        let options = Options {
            dir: relay_dir(),
            port,
            socket: socket.to_owned(),
            uid: own_uid(),
            heartbeat: HEARTBEAT_TIMEOUT,
            socket_wait: SOCKET_WAIT,
        };
        let owner = |peer, local| splitty_core::pty::ports::connection_owner(peer, local);
        match run(options, tokio::io::stdin(), tokio::io::stdout(), owner).await {
            Ok(()) => 0,
            Err(_) => 1,
        }
    }

    pub(super) struct Options {
        pub dir: PathBuf,
        pub port: u16,
        pub socket: PathBuf,
        pub uid: u32,
        pub heartbeat: Duration,
        pub socket_wait: Duration,
    }

    /// Who opened a connection, from its peer and local address.
    pub(super) type Owner = fn(SocketAddr, SocketAddr) -> Option<u32>;

    /// Report on `output`, then relay until `input` ends or falls silent.
    pub(super) async fn run(
        options: Options,
        mut input: impl AsyncRead + Unpin,
        mut output: impl AsyncWrite + Unpin,
        owner: Owner,
    ) -> Result<(), Failure> {
        let listeners = match listen(&options).await {
            Ok(listeners) => listeners,
            Err(failure) => {
                let _ = output
                    .write_all(format!("{}\n", failure.line()).as_bytes())
                    .await;
                let _ = output.flush().await;
                return Err(failure);
            }
        };
        output
            .write_all(format!("{READY}\n").as_bytes())
            .await
            .and(output.flush().await)
            .map_err(|error| Failure::new("socket", format!("cannot report: {error}")))?;

        let socket = Arc::new(options.socket.clone());
        let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let mut heartbeat = [0u8; 64];
        let Listening { v4, v6, _remove } = listeners;
        // Only input counts as a heartbeat, not connections: they keep
        // arriving from the host after the SSH connection is gone.
        let mut deadline = tokio::time::Instant::now() + options.heartbeat;
        loop {
            tokio::select! {
                // `read` is cancel-safe: a heartbeat is never half consumed.
                read = input.read(&mut heartbeat) => match read {
                    // Input ended: the SSH session is over.
                    Ok(0) | Err(_) => return Ok(()),
                    Ok(_) => deadline = tokio::time::Instant::now() + options.heartbeat,
                },
                () = tokio::time::sleep_until(deadline) => {
                    eprintln!(
                        "remote-relay: no heartbeat for {} s; ending",
                        options.heartbeat.as_secs()
                    );
                    return Ok(());
                }
                accepted = v4.accept() => admit(accepted, &socket, &slots, options.uid, owner),
                accepted = accept(v6.as_ref()) => admit(accepted, &socket, &slots, options.uid, owner),
            }
        }
    }

    /// The listeners of a relay, and the removal of its socket when they
    /// close.
    struct Listening {
        v4: TcpListener,
        v6: Option<TcpListener>,
        _remove: Remove,
    }

    async fn accept(listener: Option<&TcpListener>) -> io::Result<(TcpStream, SocketAddr)> {
        match listener {
            Some(listener) => listener.accept().await,
            None => std::future::pending().await,
        }
    }

    /// Relay one accepted connection when its owner is the relay's user.
    fn admit(
        accepted: io::Result<(TcpStream, SocketAddr)>,
        socket: &Arc<PathBuf>,
        slots: &Arc<Semaphore>,
        uid: u32,
        owner: Owner,
    ) {
        let Ok((stream, peer)) = accepted else {
            return;
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            return;
        };
        let socket = socket.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let Ok(local) = stream.local_addr() else {
                return;
            };
            let found = tokio::task::spawn_blocking(move || owner(peer, local))
                .await
                .ok()
                .flatten();
            if found != Some(uid) {
                match found {
                    Some(other) => eprintln!("remote-relay: refused a connection of uid {other}"),
                    None => eprintln!("remote-relay: refused a connection of an unknown user"),
                }
                return;
            }
            let Ok(mut upstream) = UnixStream::connect(socket.as_path()).await else {
                return;
            };
            let mut stream = stream;
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
        });
    }

    /// Check the socket and bind both loopback addresses. Once the path is
    /// known to be a relay socket, it is removed again however this ends:
    /// `sshd` leaves it behind.
    async fn listen(options: &Options) -> Result<Listening, Failure> {
        check_socket_path(&options.dir, &options.socket)?;
        let remove = Remove(options.socket.clone());
        if options.port == 0 {
            return Err(Failure::new("listen", "port 0 is not a port to share"));
        }
        check_socket(&options.socket, options.uid, options.socket_wait).await?;
        let port = options.port;
        let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| bind_failure(port, "127.0.0.1", &error))?;
        let v6 = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
            Ok(listener) => Some(listener),
            Err(error) if no_ipv6(&error) => None,
            Err(error) => return Err(bind_failure(port, "[::1]", &error)),
        };
        Ok(Listening {
            v4,
            v6,
            _remove: remove,
        })
    }

    fn bind_failure(port: u16, address: &str, error: &io::Error) -> Failure {
        if error.kind() == io::ErrorKind::AddrInUse {
            Failure::new(
                "host_port_conflict",
                format!("port {port} is already in use on this host ({address})"),
            )
        } else {
            Failure::new(
                "listen",
                format!("cannot listen on {address}:{port} on this host: {error}"),
            )
        }
    }

    /// The host has no IPv6 loopback.
    fn no_ipv6(error: &io::Error) -> bool {
        const FAMILY_UNSUPPORTED: i32 = 97; // EAFNOSUPPORT
        error.kind() == io::ErrorKind::AddrNotAvailable
            || error.kind() == io::ErrorKind::Unsupported
            || error.raw_os_error() == Some(FAMILY_UNSUPPORTED)
    }

    /// Wait for `sshd` to create the socket, then make sure only this user
    /// can use it.
    async fn check_socket(socket: &Path, uid: u32, wait: Duration) -> Result<(), Failure> {
        let deadline = tokio::time::Instant::now() + wait;
        let meta = loop {
            match std::fs::symlink_metadata(socket) {
                Ok(meta) => break meta,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(Failure::new(
                            "socket",
                            format!(
                                "{} did not appear; was the forward refused?",
                                socket.display()
                            ),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => {
                    return Err(Failure::new(
                        "socket",
                        format!("cannot read {}: {error}", socket.display()),
                    ))
                }
            }
        };
        if !meta.file_type().is_socket() || meta.uid() != uid {
            return Err(Failure::new(
                "socket",
                format!("{} is not a socket of this user", socket.display()),
            ));
        }
        if meta.mode() & 0o077 != 0 {
            std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).map_err(
                |error| {
                    Failure::new(
                        "socket",
                        format!("cannot make {} owner-only: {error}", socket.display()),
                    )
                },
            )?;
        }
        Ok(())
    }

    /// Removes the relay's socket when the relay ends: `sshd` leaves it
    /// behind.
    struct Remove(PathBuf);

    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_names_are_sixteen_hex_characters() {
        assert!(is_socket_name("0123456789abcdef.sock"));
        for name in [
            "0123456789ABCDEF.sock",
            "0123456789abcde.sock",
            "0123456789abcdef0.sock",
            "0123456789abcdef",
            "../456789abcdef.sock",
            "0123456789abcdeg.sock",
        ] {
            assert!(!is_socket_name(name), "{name}");
        }
    }

    #[test]
    fn a_socket_path_must_be_directly_inside_the_directory() {
        let dir = Path::new("/home/me/.local/share/splitty/run/relay");
        assert!(check_socket_path(dir, &dir.join("0123456789abcdef.sock")).is_ok());
        for socket in [
            PathBuf::from("/tmp/0123456789abcdef.sock"),
            dir.join("nested").join("0123456789abcdef.sock"),
            dir.join("other.sock"),
            PathBuf::from("0123456789abcdef.sock"),
        ] {
            assert_eq!(
                check_socket_path(dir, &socket).map_err(|f| f.code),
                Err("socket"),
                "{}",
                socket.display()
            );
        }
    }

    #[test]
    fn a_failure_is_one_line() {
        let failure = Failure::new("host_port_conflict", "port 1 is\nin use");
        assert_eq!(
            failure.line(),
            "SPLITTY_RELAY_FAILED_V1 host_port_conflict port 1 is in use"
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdListener};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
    use tokio::net::{TcpStream, UnixListener};
    use tokio::task::JoinHandle;

    use super::linux::{prepare_in, run, Options, Owner};
    use super::{Failure, READY};

    fn uid() -> u32 {
        nix::unistd::getuid().as_raw()
    }

    fn free_port() -> u16 {
        StdListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|listener| listener.local_addr())
            .expect("free port")
            .port()
    }

    fn private_dir(root: &Path) -> PathBuf {
        let dir = root.join("relay");
        splitty_core::config::ensure_private_dir(&dir).expect("dir");
        dir
    }

    /// A relay of `socket` in `dir` on `port`, with its input, the first
    /// line it printed, and its end.
    struct Running {
        input: DuplexStream,
        line: String,
        ended: JoinHandle<Result<(), Failure>>,
    }

    async fn start(
        dir: &Path,
        socket: &Path,
        port: u16,
        heartbeat: Duration,
        owner: Owner,
    ) -> Running {
        let (input, relay_input) = tokio::io::duplex(64);
        let (relay_output, output) = tokio::io::duplex(1024);
        let options = Options {
            dir: dir.to_owned(),
            port,
            socket: socket.to_owned(),
            uid: uid(),
            heartbeat,
            socket_wait: Duration::from_millis(200),
        };
        let ended = tokio::spawn(run(options, relay_input, relay_output, owner));
        let mut line = String::new();
        BufReader::new(output)
            .read_line(&mut line)
            .await
            .expect("line");
        Running {
            input,
            line: line.trim_end().to_owned(),
            ended,
        }
    }

    fn mine(_: SocketAddr, _: SocketAddr) -> Option<u32> {
        Some(nix::unistd::getuid().as_raw())
    }

    fn unknown(_: SocketAddr, _: SocketAddr) -> Option<u32> {
        None
    }

    fn real(peer: SocketAddr, local: SocketAddr) -> Option<u32> {
        splitty_core::pty::ports::connection_owner(peer, local)
    }

    /// `sshd`'s end: a Unix socket that echoes what it receives, and how
    /// many connections it got.
    fn echo(socket: &Path) -> Arc<AtomicUsize> {
        let listener = UnixListener::bind(socket).expect("socket");
        let served = Arc::new(AtomicUsize::new(0));
        let counted = served.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buffer = [0u8; 64];
                    while let Ok(n) = stream.read(&mut buffer).await {
                        if n == 0 || stream.write_all(&buffer[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        served
    }

    async fn round_trip(port: u16) -> Vec<u8> {
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect");
        client.write_all(b"ping").await.expect("write");
        let mut answer = vec![0u8; 4];
        match tokio::time::timeout(Duration::from_secs(5), client.read(&mut answer)).await {
            Ok(Ok(n)) => answer.truncate(n),
            _ => answer.clear(),
        }
        answer
    }

    #[test]
    fn prepare_makes_a_private_directory_and_removes_stale_sockets() {
        let root = tempfile::tempdir().expect("dir");
        let tcp = root.path().join("tcp");
        std::fs::write(&tcp, "").expect("tcp");
        let dir = root.path().join("run").join("relay");

        let prepared = prepare_in(&dir, &tcp, Duration::ZERO).expect("prepared");
        assert_eq!(prepared, dir);
        let meta = std::fs::metadata(&dir).expect("meta");
        assert_eq!(meta.mode() & 0o777, 0o700);
        assert_eq!(meta.uid(), uid());

        // A socket nothing listens on any more, and one still served.
        let stale = dir.join("0000000000000000.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).expect("stale"));
        let live = dir.join("1111111111111111.sock");
        let _served = std::os::unix::net::UnixListener::bind(&live).expect("live");
        prepare_in(&dir, &tcp, Duration::ZERO).expect("prepared again");
        assert!(!stale.exists(), "the stale socket is removed");
        assert!(live.exists(), "the served socket stays");

        // Young sockets stay whatever they answer.
        let young = dir.join("2222222222222222.sock");
        drop(std::os::unix::net::UnixListener::bind(&young).expect("young"));
        prepare_in(&dir, &tcp, Duration::from_secs(3600)).expect("prepared");
        assert!(young.exists());
    }

    #[test]
    fn prepare_refuses_a_link_and_a_host_without_the_tcp_table() {
        let root = tempfile::tempdir().expect("dir");
        let tcp = root.path().join("tcp");
        std::fs::write(&tcp, "").expect("tcp");
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).expect("dir");
        let link = root.path().join("relay");
        std::os::unix::fs::symlink(&elsewhere, &link).expect("link");
        assert!(prepare_in(&link, &tcp, Duration::ZERO).is_err());

        let missing = root.path().join("no-tcp");
        let error =
            prepare_in(&root.path().join("ok"), &missing, Duration::ZERO).expect_err("no table");
        assert!(error.contains("Sharing a port needs"), "{error}");
    }

    #[tokio::test]
    async fn a_connection_of_this_user_reaches_the_socket() {
        let root = tempfile::tempdir().expect("dir");
        let dir = private_dir(root.path());
        let socket = dir.join("0123456789abcdef.sock");
        let _sshd = echo(&socket);
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).expect("mode");
        let port = free_port();

        let running = start(&dir, &socket, port, Duration::from_secs(30), real).await;
        assert_eq!(running.line, READY);
        let mode = std::fs::metadata(&socket).expect("socket").mode();
        assert_eq!(mode & 0o077, 0, "the socket was made owner-only");
        assert_eq!(round_trip(port).await, b"ping");
        drop(running.input);
        assert!(running.ended.await.expect("relay").is_ok());
        assert!(
            !socket.exists(),
            "the relay removes its socket when it ends"
        );
    }

    #[tokio::test]
    async fn a_connection_of_an_unknown_user_is_closed() {
        let root = tempfile::tempdir().expect("dir");
        let dir = private_dir(root.path());
        let socket = dir.join("0123456789abcdef.sock");
        let sshd = echo(&socket);
        let port = free_port();

        let running = start(&dir, &socket, port, Duration::from_secs(30), unknown).await;
        assert_eq!(running.line, READY);
        assert!(
            round_trip(port).await.is_empty(),
            "closed without an answer"
        );
        assert_eq!(sshd.load(Ordering::SeqCst), 0, "nothing reached the socket");
        drop(running.input);
        running.ended.await.expect("relay").expect("ended");
    }

    #[tokio::test]
    async fn a_port_in_use_on_the_host_is_a_conflict() {
        let root = tempfile::tempdir().expect("dir");
        let dir = private_dir(root.path());
        let socket = dir.join("0123456789abcdef.sock");
        let _sshd = echo(&socket);
        let taken = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("taken");
        let port = taken.local_addr().expect("address").port();

        let running = start(&dir, &socket, port, Duration::from_secs(30), mine).await;
        assert!(
            running
                .line
                .starts_with("SPLITTY_RELAY_FAILED_V1 host_port_conflict "),
            "{}",
            running.line
        );
        let failure = running.ended.await.expect("relay").expect_err("failed");
        assert_eq!(failure.code, "host_port_conflict");
    }

    #[tokio::test]
    async fn a_socket_outside_the_directory_is_refused_before_listening() {
        let root = tempfile::tempdir().expect("dir");
        let dir = private_dir(root.path());
        let outside = root.path().join("0123456789abcdef.sock");
        let _sshd = echo(&outside);
        let port = free_port();

        let running = start(&dir, &outside, port, Duration::from_secs(30), mine).await;
        assert!(
            running.line.starts_with("SPLITTY_RELAY_FAILED_V1 socket "),
            "{}",
            running.line
        );
        assert!(
            StdListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok(),
            "nothing listens on the port"
        );
    }

    #[tokio::test]
    async fn a_missing_socket_is_refused() {
        let root = tempfile::tempdir().expect("dir");
        let dir = private_dir(root.path());
        let socket = dir.join("0123456789abcdef.sock");
        let running = start(&dir, &socket, free_port(), Duration::from_secs(30), mine).await;
        assert!(
            running.line.starts_with("SPLITTY_RELAY_FAILED_V1 socket "),
            "{}",
            running.line
        );
    }

    #[tokio::test]
    async fn the_relay_ends_when_the_heartbeat_stops_and_lives_while_it_comes() {
        let root = tempfile::tempdir().expect("dir");
        let dir = private_dir(root.path());
        let socket = dir.join("0123456789abcdef.sock");
        let _sshd = echo(&socket);
        let port = free_port();

        let mut running = start(&dir, &socket, port, Duration::from_millis(300), mine).await;
        assert_eq!(running.line, READY);
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            running.input.write_all(b"\n").await.expect("heartbeat");
        }
        assert!(!running.ended.is_finished(), "alive while beats arrive");
        let ended = tokio::time::timeout(Duration::from_secs(5), running.ended)
            .await
            .expect("ended after silence");
        assert!(ended.expect("relay").is_ok());
        assert!(!socket.exists());
        assert!(
            StdListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok(),
            "the port is free again"
        );
    }
}
