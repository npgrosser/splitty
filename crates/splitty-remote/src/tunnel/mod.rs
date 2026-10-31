//! A tunnel to a host's Splitty daemon, as `splitty connect` and the desktop
//! apps' remote windows open it.
//!
//! [`open`] makes sure the host has this client's server, asks it for its
//! daemon with the handshake (starting one), forwards the daemon's loopback
//! port to the same port on this machine, and mints a fresh bootstrap code on
//! the host, so the local browser can establish a session without any
//! long-lived credential crossing the link. The same `ssh` forwards the ports
//! of [`open_forwarding`] (`splitty connect --forward`). [`supervise`] rebuilds
//! the tunnel when it drops. A [`PortTunnel`] forwards one port of a host by
//! an `ssh` of its own, and a [`ReverseTunnel`] shares one port of this
//! machine with a host the same way.

mod forward;
mod reverse;
mod supervisor;

pub use forward::{
    check_forward_address, parse_forwards, reserve, reserve_exact, Forward, ForwardError,
    PortForward, PortTunnel, PortTunnelError,
};
pub use reverse::{
    parse_prepared, prepare_command, socket_path, ReverseTunnel, ReverseTunnelError, HEARTBEAT,
    MAX_SOCKET_PATH, RELAY_FAILED, RELAY_READY,
};
pub use supervisor::{
    backoff, connect, supervise, Connected, Event, Progress, Remote, Step, Tunnel, GIVE_UP_AFTER,
};

use std::io;
use std::net::TcpListener;
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Child;

use crate::handshake::{self, HandshakeError, RunningDaemon};
use crate::install::{self, InstallError, Payload, Server};
use crate::ssh::{run_checked, Ssh, SshError};

/// The forwards of a tunnel: the remote daemon's `port`, and `forwards` to
/// `localhost` on the host.
///
/// Every local end names `127.0.0.1` explicitly. Without a bind address, OpenSSH
/// applies `GatewayPorts` from the user's configuration, which can open the
/// forward on every interface and hand the remote server to the LAN.
///
/// The daemon's forward comes last. OpenSSH binds the local ends in this order
/// and, with `ExitOnForwardFailure`, exits right after the last one when any
/// failed, so once the daemon's port answers, the others are bound as well.
fn forward_args(port: u16, forwards: &[Forward]) -> Vec<String> {
    let mut args = Vec::with_capacity(2 * forwards.len() + 3);
    for forward in forwards {
        args.push("-L".to_owned());
        args.push(format!(
            "127.0.0.1:{}:localhost:{}",
            forward.local, forward.remote
        ));
    }
    args.push("-L".to_owned());
    args.push(format!("127.0.0.1:{port}:127.0.0.1:{port}"));
    args.push("-N".to_owned());
    args
}

/// Why talking to the host failed.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("ssh could not be started: {0}")]
    Spawn(io::Error),

    #[error(
        "splitty is not installed on {host}, or not in the PATH of non-interactive SSH \
         sessions there (often set in ~/.profile but not in ~/.bashrc). A client that \
         carries a server for the host's architecture installs one itself; see \
         SPLITTY_REMOTE_PAYLOAD in the README"
    )]
    SplittyMissing { host: String },

    #[error("the splitty on {host} is too old for this client; update it there")]
    SplittyTooOld { host: String },

    #[error("{message}")]
    Ssh {
        message: String,
        hint: Option<String>,
    },

    #[error(transparent)]
    Install(#[from] InstallError),

    #[error("No Splitty daemon is running on {host}.")]
    NoDaemon { host: String, server: String },

    #[error("{host} gave an unexpected answer: {detail}")]
    InvalidAnswer { host: String, detail: String },

    #[error("local port {port} is not available ({source})")]
    PortUnavailable { port: u16, source: io::Error },

    /// A forward's local port, taken by another process while the tunnel was
    /// down. Forwards keep their ports, so the rebuild waits for it.
    #[error("local port {local} of the forward of port {remote} is not available ({source})")]
    ForwardPortUnavailable {
        local: u16,
        remote: u16,
        source: io::Error,
    },

    #[error("{0}")]
    Forward(String),

    #[error("Failed to obtain a session from the remote host: {0}")]
    Bootstrap(Box<RemoteError>),

    #[error("giving up after {} without a tunnel to {host}: {last}", describe(.after))]
    GaveUp {
        host: String,
        after: Duration,
        last: Box<RemoteError>,
    },
}

/// A span of time as a message names it.
fn describe(duration: &Duration) -> String {
    match duration.as_secs() {
        60 => "1 minute".to_owned(),
        seconds if seconds % 60 == 0 => format!("{} minutes", seconds / 60),
        1 => "1 second".to_owned(),
        seconds => format!("{seconds} seconds"),
    }
}

/// What the user reads about a failed connect: what went wrong and, where
/// there is one, what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl RemoteError {
    /// The message and hint of this error, for every way a client shows it.
    pub fn failure(&self) -> Failure {
        let message = match self {
            RemoteError::NoDaemon { .. }
            | RemoteError::Bootstrap(_)
            | RemoteError::GaveUp { .. } => self.to_string(),
            // The probe is the first command on the host, so its ssh failing
            // (255) means the host was not reached, not that installing failed.
            RemoteError::Install(InstallError::Ssh(
                SshError::Spawn(_)
                | SshError::Failed {
                    status: Some(255), ..
                },
            )) => format!("Failed to reach the remote host: {self}"),
            RemoteError::Install(_) => format!("Failed to install Splitty on the host: {self}"),
            RemoteError::PortUnavailable { port: 0, source } => {
                format!("No local port is available ({source}).")
            }
            RemoteError::PortUnavailable { port, source } => {
                format!("Local port {port} is not available ({source}).")
            }
            RemoteError::ForwardPortUnavailable { .. } | RemoteError::Forward(_) => {
                format!("Failed to open SSH tunnel: {self}")
            }
            _ => format!("Failed to reach the remote daemon: {self}"),
        };
        Failure {
            message,
            hint: self.hint(),
        }
    }

    fn hint(&self) -> Option<String> {
        match self {
            RemoteError::Ssh { hint, .. } => hint.clone(),
            RemoteError::Install(InstallError::Ssh(error)) => error.hint().map(str::to_owned),
            RemoteError::NoDaemon { server, .. } => {
                Some(format!("Start one there with: {server} daemon start"))
            }
            RemoteError::PortUnavailable { port, .. } if *port != 0 => Some(format!(
                "The tunnel has to use the same port as the remote daemon, because the remote \
                 server only accepts requests from its own origin. Stop whatever listens on \
                 127.0.0.1:{port} here (a local Splitty daemon, for example), or run the remote \
                 daemon on another port: splitty daemon start --addr 127.0.0.1:<port>"
            )),
            RemoteError::ForwardPortUnavailable { local, .. } => Some(format!(
                "A forward keeps its local port when the tunnel is rebuilt, so that open pages \
                 keep working. Stop whatever listens on 127.0.0.1:{local} here."
            )),
            RemoteError::Bootstrap(inner) => inner.hint(),
            RemoteError::GaveUp { last, .. } => last.hint(),
            _ => None,
        }
    }
}

/// What a failed `splitty` command on the host means for a connect.
fn from_ssh(host: &str, error: SshError) -> RemoteError {
    match error {
        SshError::Spawn(error) => RemoteError::Spawn(error),
        SshError::NotFound { .. } => RemoteError::SplittyMissing {
            host: host.to_owned(),
        },
        // clap rejects the subcommand or flag a server older than
        // `remote-handshake` does not know; ssh itself fails with 255.
        SshError::Failed { status, stderr, .. }
            if status != Some(255)
                && (stderr.contains("'remote-handshake'") || stderr.contains("'--json'")) =>
        {
            RemoteError::SplittyTooOld {
                host: host.to_owned(),
            }
        }
        SshError::Failed { message, hint, .. } => RemoteError::Ssh { message, hint },
    }
}

/// The code from `splitty issue-bootstrap`'s output, if it is well-formed.
fn parse_bootstrap(stdout: &[u8]) -> Result<String, String> {
    let code = String::from_utf8_lossy(stdout)
        .lines()
        .find_map(|line| line.strip_prefix("SPLITTY_BOOTSTRAP=").map(str::trim))
        .map(str::to_string)
        .ok_or("the remote host did not return a bootstrap code")?;
    if code.len() != 32 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the remote host returned a malformed bootstrap code".to_owned());
    }
    Ok(code)
}

/// Percent-encode everything but RFC 3986's unreserved characters.
fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The URL that opens a session on the tunnel's local end. `host` is only shown
/// by the UI (the title bar's remote indicator); it never directs traffic.
pub fn session_url(port: u16, code: &str, host: &str) -> String {
    format!(
        "http://127.0.0.1:{port}/?bootstrap={code}&host={}",
        percent_encode(host)
    )
}

/// A local port held open until the tunnel takes it over.
///
/// The reservation is kept as long as possible on purpose (SR-13). Picking a
/// port, closing the socket and only then starting `ssh` leaves a window in
/// which another local process can take it — and the browser would then send its
/// session cookie to whatever is listening. The listener is dropped immediately
/// before spawning `ssh`, which is the shortest window achievable without socket
/// hand-off; while a dropped tunnel is rebuilt, the supervisor holds one again.
pub struct PortReservation {
    listener: Option<TcpListener>,
    port: u16,
}

impl PortReservation {
    /// Reserve any free loopback port.
    pub fn acquire() -> io::Result<Self> {
        Self::bind(0)
    }

    /// Reserve a specific loopback port. Fails if anything listens on it.
    pub fn acquire_port(port: u16) -> io::Result<Self> {
        Self::bind(port)
    }

    fn bind(port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener: Some(listener),
            port,
        })
    }

    /// Recheck IPv6 while the IPv4 reservation stays held (including rebuilds).
    pub fn check_forward_address(&self) -> io::Result<()> {
        forward::check_forward_address(self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    fn release(&mut self) {
        self.listener.take();
    }
}

/// The local ports of a tunnel, each held until its `ssh` takes it over: the
/// daemon's, and one per forward.
pub struct Reservations {
    pub daemon: PortReservation,
    pub forwards: Vec<(Forward, PortReservation)>,
}

impl Reservations {
    /// Let go of every port, right before `ssh` binds them; returns the
    /// forwards.
    fn release(mut self) -> Vec<Forward> {
        self.daemon.release();
        self.forwards
            .iter_mut()
            .map(|(forward, reservation)| {
                reservation.release();
                *forward
            })
            .collect()
    }
}

/// Where the server for a host's kernel (`uname -s`) and architecture
/// (`uname -m`) comes from, such as [`install::payload_for`].
pub type PayloadFor = fn(&str, &str) -> Result<Option<Payload>, InstallError>;

/// The host as reached with the system `ssh`, and the server to run there.
pub struct SshRemote {
    ssh: Ssh,
    /// This client's version, the server the host gets.
    version: &'static str,
    payload_for: PayloadFor,
    /// `splitty` from the host's `PATH` until [`Remote::prepare`] found or
    /// installed this version.
    server: Mutex<Server>,
    /// The version of the server that answered the last handshake.
    answered: Mutex<Option<String>>,
    /// The host's ports forwarded next to the daemon's.
    forwards: Vec<PortForward>,
}

impl SshRemote {
    pub fn new(ssh: Ssh, version: &'static str, payload_for: PayloadFor) -> Self {
        Self {
            ssh,
            version,
            payload_for,
            server: Mutex::new(Server::Path),
            answered: Mutex::new(None),
            forwards: Vec::new(),
        }
    }

    fn server(&self) -> Server {
        self.server
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn install(&self, on_install: impl FnOnce(&str) + Send) -> Result<(), RemoteError> {
        let prepared = install::prepare(&self.ssh, self.version, self.payload_for, |probe| {
            on_install(&probe.arch)
        })
        .await?;
        *self
            .server
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = prepared.server;
        Ok(())
    }

    /// Run the host's server with `args` and return its output, or the error.
    async fn run_server(&self, args: &[&'static str]) -> Result<Output, RemoteError> {
        let line = args
            .iter()
            .fold(self.server().command(), |line, arg| line.word(arg));
        run_checked(&self.ssh, &line, None)
            .await
            .map_err(|error| from_ssh(self.host(), error))
    }

    async fn handshake(&self, port_hint: u16) -> Result<handshake::Handshake, RemoteError> {
        handshake::handshake(&self.ssh, &self.server(), Some(port_hint))
            .await
            .map_err(|error| match error {
                HandshakeError::Ssh(error) => from_ssh(self.host(), error),
                HandshakeError::Invalid { host, detail } => {
                    RemoteError::InvalidAnswer { host, detail }
                }
            })
    }
}

impl Remote for SshRemote {
    type Tunnel = SshTunnel;

    fn host(&self) -> &str {
        self.ssh.target().as_str()
    }

    fn version(&self) -> &str {
        self.version
    }

    fn forwards(&self) -> &[PortForward] {
        &self.forwards
    }

    async fn prepare(&self, on_install: impl FnOnce(&str) + Send) -> Result<(), RemoteError> {
        self.install(on_install).await
    }

    /// The handshake, which starts a missing daemon on `port_hint`. When the
    /// installed server is gone (a rebuilt container), it is installed again
    /// once.
    async fn daemon(&self, port_hint: u16) -> Result<RunningDaemon, RemoteError> {
        let answer = match self.handshake(port_hint).await {
            Err(RemoteError::SplittyMissing { .. })
                if matches!(self.server(), Server::Installed { .. }) =>
            {
                self.install(|_| {}).await?;
                self.handshake(port_hint).await?
            }
            answer => answer?,
        };
        *self
            .answered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(answer.server.version.clone());
        match answer.daemon.daemon {
            Some(daemon) if answer.daemon.running => Ok(daemon),
            _ => Err(RemoteError::NoDaemon {
                host: self.host().to_owned(),
                server: answer.server.path,
            }),
        }
    }

    fn server_version(&self) -> Option<String> {
        self.answered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// `daemon stop` of any version stops the daemon `daemon.json` names.
    async fn stop_daemon(&self) -> Result<(), RemoteError> {
        self.run_server(&["daemon", "stop"]).await.map(|_| ())
    }

    /// Returns only once the daemon's local port answers *and* the ssh process
    /// is still alive. A listening socket alone is not proof — it could be the
    /// unrelated process that won the race for the port. The forwards' ports are
    /// not probed: a connection there would reach the host's server, or make
    /// `ssh` report a failed channel when none listens yet.
    async fn open_tunnel(&self, ports: Reservations, port: u16) -> Result<SshTunnel, RemoteError> {
        let forwards = ports.release();
        let mut child = self
            .ssh
            .command()
            .args(forward_args(port, &forwards))
            .arg(self.host())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(RemoteError::Spawn)?;
        match wait_for_forward(&mut child, port).await {
            Ok(()) => Ok(SshTunnel { child }),
            Err(error) => {
                let _ = child.kill().await;
                Err(RemoteError::Forward(error.to_string()))
            }
        }
    }

    /// A single-use code minted on the remote, rather than a long-lived token
    /// read out of its daemon.json (SR-13/SR-21). What crosses the link is
    /// valid for exactly one session exchange.
    async fn issue_bootstrap(&self) -> Result<String, RemoteError> {
        let output = self
            .run_server(&["issue-bootstrap"])
            .await
            .map_err(|e| RemoteError::Bootstrap(Box::new(e)))?;
        parse_bootstrap(&output.stdout).map_err(|detail| {
            RemoteError::Bootstrap(Box::new(RemoteError::InvalidAnswer {
                host: self.host().to_owned(),
                detail,
            }))
        })
    }
}

/// The `ssh` process that holds a forward. Dropping it kills the process, so
/// a task that is dropped (a closed window) leaves no tunnel behind.
pub struct SshTunnel {
    child: Child,
}

impl Tunnel for SshTunnel {
    async fn closed(&mut self) -> String {
        match self.child.wait().await {
            Ok(status) => format!("ssh exited with {status}"),
            Err(error) => format!("ssh could not be watched: {error}"),
        }
    }

    async fn close(&mut self) {
        let _ = self.child.kill().await;
    }
}

async fn wait_for_forward(child: &mut Child, local_port: u16) -> io::Result<()> {
    use tokio::net::TcpStream;
    use tokio::time::{sleep, timeout, Instant};

    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline {
        // If ssh exited, ExitOnForwardFailure did its job — report that rather
        // than waiting out the timeout.
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "ssh exited before the forward was established ({status})"
            )));
        }

        let connect = TcpStream::connect(("127.0.0.1", local_port));
        if let Ok(Ok(_)) = timeout(Duration::from_millis(250), connect).await {
            // Re-check liveness: something else could be listening on that port.
            // With ExitOnForwardFailure set, a live ssh means the forward bound.
            if child.try_wait()?.is_none() {
                return Ok(());
            }
            return Err(io::Error::other(
                "the local port is in use by another process, not by the ssh forward",
            ));
        }

        sleep(Duration::from_millis(100)).await;
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "ssh forward was not established in time",
    ))
}

/// Install the server when the host lacks it, ask the host for its daemon
/// (starting one), forward the daemon's port and mint the first session URL,
/// reporting each step to `on_progress`. Returns what [`supervise`] needs.
///
/// The future is `Send`, so a desktop app can run it as a task; dropping it
/// ends every `ssh` it started.
pub async fn open(
    ssh: Ssh,
    version: &'static str,
    payload_for: PayloadFor,
    on_progress: impl FnMut(Progress) + Send,
) -> Result<(SshRemote, Connected<SshTunnel>), RemoteError> {
    open_forwarding(ssh, version, payload_for, Vec::new(), on_progress).await
}

/// [`open`], forwarding `forwards` in the same `ssh` as the daemon's port
/// (`splitty connect --forward`). [`Connected::forwards`] says where each
/// ended up; [`supervise`] keeps them on those ports.
pub async fn open_forwarding(
    ssh: Ssh,
    version: &'static str,
    payload_for: PayloadFor,
    forwards: Vec<PortForward>,
    on_progress: impl FnMut(Progress) + Send,
) -> Result<(SshRemote, Connected<SshTunnel>), RemoteError> {
    let remote = SshRemote {
        forwards,
        ..SshRemote::new(ssh, version, payload_for)
    };
    let connected = connect(&remote, on_progress).await?;
    Ok((remote, connected))
}

/// One line of `splitty connect --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum ConnectEvent {
    /// A step of [`connect`].
    Progress { step: Step, message: String },
    /// Something the user should know, such as a daemon of another version.
    Notice { message: String },
    /// The tunnel is verified and the first session URL obtained.
    Ready { url: String, port: u16 },
    /// A port of the host forwarded to `local` here, one per forward after
    /// `ready`.
    Forward {
        remote: u16,
        local: u16,
        url: String,
        /// The local port asked for, which the text output names when `local`
        /// differs.
        #[serde(skip)]
        asked: u16,
    },
    /// The tunnel dropped; it is being rebuilt.
    Lost { message: String },
    /// An attempt to rebuild it failed; the next follows in `retry_in` seconds.
    Retry {
        message: String,
        #[serde(rename = "retryIn")]
        retry_in: u64,
    },
    /// The same daemon is reachable again.
    Restored,
    /// A restarted or moved daemon needs a new session.
    Url { url: String },
    /// The command ends with status 1.
    Failed {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    /// The command ends with status 0.
    Closed,
}

impl From<Progress> for ConnectEvent {
    fn from(progress: Progress) -> Self {
        match progress {
            Progress::Step { step, message } => ConnectEvent::Progress { step, message },
            Progress::Notice { message } => ConnectEvent::Notice { message },
        }
    }
}

impl From<Forward> for ConnectEvent {
    fn from(forward: Forward) -> Self {
        ConnectEvent::Forward {
            remote: forward.remote,
            local: forward.local,
            url: forward.url(),
            asked: forward.asked,
        }
    }
}

impl From<Failure> for ConnectEvent {
    fn from(Failure { message, hint }: Failure) -> Self {
        ConnectEvent::Failed { message, hint }
    }
}

impl ConnectEvent {
    /// The supervisor's `event` for the tunnel to `host`.
    pub fn from_event(host: &str, event: Event) -> Self {
        match event {
            Event::Lost { reason } => ConnectEvent::Lost {
                message: format!("SSH tunnel lost ({reason}). Reconnecting to {host}..."),
            },
            Event::RetryFailed { error, retry_in } => {
                let mut message = format!("Reconnect failed: {error}");
                if let Some(hint) = error.hint() {
                    message.push('\n');
                    message.push_str(&hint);
                }
                ConnectEvent::Retry {
                    message,
                    retry_in: retry_in.as_secs(),
                }
            }
            Event::Restored => ConnectEvent::Restored,
            Event::NewUrl { url } => ConnectEvent::Url { url },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The forward names its local bind address, whatever `GatewayPorts` says.
    #[test]
    fn the_forward_is_bound_to_loopback_explicitly() {
        assert_eq!(
            forward_args(9800, &[]),
            ["-L", "127.0.0.1:9800:127.0.0.1:9800", "-N"]
        );
    }

    /// `cli`, Remote Connect Port Forwards, and `access-control`, Forwarded
    /// Ports Bound To Loopback: one `ssh`, every local end on `127.0.0.1`, the
    /// host's ends on `localhost`, the daemon's forward last.
    #[test]
    fn forwards_go_to_localhost_on_the_host_before_the_daemons_forward() {
        let forwards = [
            Forward {
                remote: 3000,
                local: 3000,
                asked: 3000,
            },
            Forward {
                remote: 5173,
                local: 41234,
                asked: 5173,
            },
        ];
        assert_eq!(
            forward_args(9800, &forwards),
            [
                "-L",
                "127.0.0.1:3000:localhost:3000",
                "-L",
                "127.0.0.1:41234:localhost:5173",
                "-L",
                "127.0.0.1:9800:127.0.0.1:9800",
                "-N"
            ]
        );
    }

    fn failed(status: i32, stderr: &str) -> SshError {
        SshError::Failed {
            message: format!("ssh to vm failed: {stderr}"),
            status: Some(status),
            stderr: stderr.to_owned(),
            hint: None,
        }
    }

    #[test]
    fn a_missing_or_old_splitty_is_named() {
        let missing = from_ssh("vm", SshError::NotFound { host: "vm".into() });
        assert!(
            matches!(missing, RemoteError::SplittyMissing { .. }),
            "{missing:?}"
        );
        let message = missing.to_string();
        assert!(message.contains("PATH of non-interactive SSH"), "{message}");
        assert!(message.contains("SPLITTY_REMOTE_PAYLOAD"), "{message}");

        let old = from_ssh("vm", failed(2, "error: unexpected argument '--json' found"));
        assert!(matches!(old, RemoteError::SplittyTooOld { .. }), "{old:?}");
        let old = from_ssh(
            "vm",
            failed(2, "error: unrecognized subcommand 'remote-handshake'"),
        );
        assert!(matches!(old, RemoteError::SplittyTooOld { .. }), "{old:?}");

        let refused = from_ssh("vm", failed(255, "Connection refused '--json'"));
        assert!(matches!(refused, RemoteError::Ssh { .. }), "{refused:?}");
    }

    #[test]
    fn only_a_well_formed_bootstrap_code_is_accepted() {
        let code = "0123456789abcdef0123456789ABCDEF";
        assert_eq!(
            parse_bootstrap(format!("SPLITTY_BOOTSTRAP={code}\n").as_bytes()),
            Ok(code.to_owned())
        );
        assert!(parse_bootstrap(b"SPLITTY_BOOTSTRAP=short").is_err());
        assert!(parse_bootstrap(b"SPLITTY_BOOTSTRAP=0123456789abcdef0123456789abcdeg").is_err());
        assert!(parse_bootstrap(b"nothing").is_err());
    }

    /// `host` is shown by the title bar; it must arrive as one opaque value.
    #[test]
    fn the_session_url_carries_the_encoded_target() {
        assert_eq!(
            session_url(9800, "abc", "user@host"),
            "http://127.0.0.1:9800/?bootstrap=abc&host=user%40host"
        );
        assert_eq!(
            session_url(9800, "abc", "user@[2001:db8::1]:2222"),
            "http://127.0.0.1:9800/?bootstrap=abc&host=user%40%5B2001%3Adb8%3A%3A1%5D%3A2222"
        );
    }

    #[test]
    fn a_reservation_holds_its_port_until_released() {
        let reservation = PortReservation::acquire().expect("acquire");
        let port = reservation.port();

        // While reserved, a competing bind must fail.
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_err(),
            "the reservation must actually hold the port",
        );

        drop(reservation);
        assert!(
            TcpListener::bind(("127.0.0.1", port)).is_ok(),
            "the port must be free once the reservation is dropped",
        );
    }

    /// `splitty connect` mirrors the remote daemon's port locally, so a port that
    /// is already taken here must be reported, not silently replaced.
    #[test]
    fn a_specific_port_is_reserved_or_refused() {
        let first = PortReservation::acquire().expect("acquire");
        let port = first.port();

        assert!(
            PortReservation::acquire_port(port).is_err(),
            "a port that is in use must not be reserved twice",
        );

        drop(first);
        let second = PortReservation::acquire_port(port).expect("free port");
        assert_eq!(second.port(), port);
    }

    fn host_key_failed() -> RemoteError {
        RemoteError::Ssh {
            message: "ssh to vm failed: Host key verification failed.".into(),
            hint: Some("Run `ssh vm` once in a terminal and check the fingerprint.".into()),
        }
    }

    /// Text and JSON output show the same message and the same instruction.
    #[test]
    fn a_failure_separates_what_happened_from_what_to_do() {
        let failure = host_key_failed().failure();
        assert_eq!(
            failure.message,
            "Failed to reach the remote daemon: ssh to vm failed: Host key verification failed."
        );
        assert!(failure.hint.expect("hint").contains("`ssh vm` once"));

        let busy = RemoteError::PortUnavailable {
            port: 9800,
            source: io::Error::from(io::ErrorKind::AddrInUse),
        }
        .failure();
        assert!(
            busy.message
                .starts_with("Local port 9800 is not available ("),
            "{}",
            busy.message
        );
        let hint = busy.hint.expect("hint");
        assert!(hint.contains("same port as the remote daemon"), "{hint}");
        assert!(
            hint.contains("splitty daemon start --addr 127.0.0.1:<port>"),
            "{hint}"
        );

        let no_daemon = RemoteError::NoDaemon {
            host: "vm".into(),
            server: "/home/u/splitty".into(),
        }
        .failure();
        assert_eq!(no_daemon.message, "No Splitty daemon is running on vm.");
        assert_eq!(
            no_daemon.hint.as_deref(),
            Some("Start one there with: /home/u/splitty daemon start")
        );

        let gave_up = RemoteError::GaveUp {
            host: "vm".into(),
            after: GIVE_UP_AFTER,
            last: Box::new(host_key_failed()),
        }
        .failure();
        assert!(
            gave_up
                .message
                .starts_with("giving up after 10 minutes without a tunnel to vm: "),
            "{}",
            gave_up.message
        );
        assert!(gave_up.hint.is_some(), "the cause's hint is kept");
    }

    /// Not the daemon's hint: a forward's port need not match the host's.
    #[test]
    fn a_forward_port_taken_meanwhile_names_the_forward() {
        let taken = RemoteError::ForwardPortUnavailable {
            local: 41234,
            remote: 3000,
            source: io::Error::from(io::ErrorKind::AddrInUse),
        };
        assert!(
            taken
                .to_string()
                .starts_with("local port 41234 of the forward of port 3000 is not available ("),
            "{taken}"
        );
        let failure = taken.failure();
        assert!(
            failure
                .message
                .starts_with("Failed to open SSH tunnel: local port 41234 of the forward"),
            "{}",
            failure.message
        );
        let hint = failure.hint.expect("hint");
        assert!(hint.contains("127.0.0.1:41234"), "{hint}");
        assert!(!hint.contains("remote daemon"), "{hint}");
    }

    /// The probe is the first ssh call: a host it cannot reach is not an
    /// install failure.
    #[test]
    fn an_unreachable_host_is_not_reported_as_a_failed_install() {
        let unreachable = RemoteError::from(InstallError::Ssh(failed(
            255,
            "ssh: connect to host vm port 22: Connection refused",
        )))
        .failure();
        assert!(
            unreachable
                .message
                .starts_with("Failed to reach the remote host: ssh to vm failed: "),
            "{}",
            unreachable.message
        );
        assert_eq!(unreachable.hint, None);

        let corrupted = RemoteError::from(InstallError::Checksum { host: "vm".into() }).failure();
        assert!(
            corrupted
                .message
                .starts_with("Failed to install Splitty on the host: "),
            "{}",
            corrupted.message
        );
    }

    fn json(event: ConnectEvent) -> String {
        serde_json::to_string(&event).expect("serialize")
    }

    /// The lines the Electron shell parses (`cli`, Remote Connect
    /// Machine-Readable Output).
    #[test]
    fn connect_events_encode_as_one_object_with_an_event_field() {
        assert_eq!(
            json(
                Progress::Step {
                    step: Step::Started,
                    message: "Started".into()
                }
                .into()
            ),
            r#"{"event":"progress","step":"started","message":"Started"}"#
        );
        for (step, name) in [
            (Step::Install, "install"),
            (Step::Daemon, "daemon"),
            (Step::Started, "started"),
            (Step::Tunnel, "tunnel"),
        ] {
            assert_eq!(
                serde_json::to_value(step).expect("step"),
                serde_json::json!(name)
            );
        }
        assert_eq!(
            json(
                Progress::Notice {
                    message: "n".into()
                }
                .into()
            ),
            r#"{"event":"notice","message":"n"}"#
        );
        assert_eq!(
            json(ConnectEvent::Ready {
                url: "http://127.0.0.1:9811/?bootstrap=x&host=vm".into(),
                port: 9811
            }),
            r#"{"event":"ready","url":"http://127.0.0.1:9811/?bootstrap=x&host=vm","port":9811}"#
        );
        assert_eq!(
            json(
                Forward {
                    remote: 3000,
                    local: 41234,
                    asked: 3000
                }
                .into()
            ),
            r#"{"event":"forward","remote":3000,"local":41234,"url":"http://localhost:41234"}"#
        );
        assert_eq!(
            json(ConnectEvent::from_event(
                "vm",
                Event::Lost {
                    reason: "ssh exited with 255".into()
                }
            )),
            r#"{"event":"lost","message":"SSH tunnel lost (ssh exited with 255). Reconnecting to vm..."}"#
        );
        assert_eq!(
            json(ConnectEvent::from_event(
                "vm",
                Event::RetryFailed {
                    error: RemoteError::Forward("no".into()),
                    retry_in: Duration::from_secs(4)
                }
            )),
            r#"{"event":"retry","message":"Reconnect failed: no","retryIn":4}"#
        );
        assert_eq!(
            json(ConnectEvent::from_event("vm", Event::Restored)),
            r#"{"event":"restored"}"#
        );
        assert_eq!(
            json(ConnectEvent::from_event(
                "vm",
                Event::NewUrl { url: "u".into() }
            )),
            r#"{"event":"url","url":"u"}"#
        );
        assert_eq!(
            json(host_key_failed().failure().into()),
            r#"{"event":"failed","message":"Failed to reach the remote daemon: ssh to vm failed: Host key verification failed.","hint":"Run `ssh vm` once in a terminal and check the fingerprint."}"#
        );
        assert_eq!(
            json(RemoteError::Forward("x".into()).failure().into()),
            r#"{"event":"failed","message":"Failed to open SSH tunnel: x"}"#
        );
        assert_eq!(json(ConnectEvent::Closed), r#"{"event":"closed"}"#);
    }

    /// A failed reconnect keeps the instruction of its cause.
    #[test]
    fn a_retry_keeps_the_hint_of_its_failure() {
        let ConnectEvent::Retry { message, .. } = ConnectEvent::from_event(
            "vm",
            Event::RetryFailed {
                error: host_key_failed(),
                retry_in: Duration::from_secs(1),
            },
        ) else {
            panic!("a retry");
        };
        assert!(message.starts_with("Reconnect failed: ssh to vm failed"));
        assert!(message.ends_with("check the fingerprint."), "{message}");
    }

    #[test]
    fn give_up_times_read_as_minutes_or_seconds() {
        assert_eq!(describe(&GIVE_UP_AFTER), "10 minutes");
        assert_eq!(describe(&Duration::from_secs(60)), "1 minute");
        assert_eq!(describe(&Duration::from_secs(90)), "90 seconds");
    }

    fn assert_send<T: Send>(_: &T) {}

    /// A desktop app spawns the flow and the supervisor as tasks.
    #[test]
    fn the_flow_and_the_supervisor_can_run_as_tasks() {
        let target = crate::ssh::SshTarget::parse("vm").expect("target");
        let flow = open(Ssh::new(target), "0.1.0", install::payload_for, |_| {});
        assert_send(&flow);

        fn supervised(
            remote: &SshRemote,
            connected: Connected<SshTunnel>,
        ) -> impl std::future::Future<Output = Result<(), RemoteError>> + Send + '_ {
            supervise(
                remote,
                connected,
                GIVE_UP_AFTER,
                std::future::pending(),
                |_| {},
            )
        }
        let _ = supervised;
    }
}
