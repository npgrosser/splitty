//! Native connections to a host's daemon, shared by desktop helpers and the
//! explicit browser gateway. Installation, capability checks and reconnection
//! use SSH exec pipes; deliberate port forwards remain separate owned workers.

pub mod exposure;
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
    backoff, connect, migration_offer, older, supervise, update_plan, Connected, Event, Progress,
    Remote, Step, UpdatePlan, GIVE_UP_AFTER, PREPARE_TIMEOUT,
};

use std::io;
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::Duration;

use serde::Serialize;
use splitty_transport::worker::Worker;

use crate::bridge::NativeBridge;
use crate::handshake::{self, HandshakeError, RunningDaemon};
use crate::install::{self, InstallError, Payload, Server};
use crate::ssh::{run_checked, Ssh, SshError};
use splitty_transport::{
    admission,
    budget::{Charged, ConnectionBudget, Lease},
    negotiation, wire,
};

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
    SplittyMissing {
        host: String,
        allocation: Option<Box<Lease>>,
    },

    #[error("the splitty on {host} is too old for this client; update it there")]
    SplittyTooOld {
        host: String,
        allocation: Option<Box<Lease>>,
    },

    #[error(transparent)]
    Ssh(SshError),

    #[error("native connection failed: {0}")]
    Native(io::Error),

    #[error("native backend preparation exceeded its deadline")]
    PreparationTimeout,

    #[error("daemon instance changed during preparation; refresh discovery")]
    DiscoveryChanged,

    #[error(transparent)]
    Install(#[from] InstallError),

    #[error("No Splitty daemon is running on {host}.")]
    NoDaemon {
        host: String,
        server: String,
        allocation: Option<Box<Lease>>,
    },

    #[error("{host} gave an unexpected answer: {detail}")]
    InvalidAnswer {
        host: String,
        detail: String,
        allocation: Option<Box<Lease>>,
    },

    #[error("{host} runs legacy Splitty {}; update to {} to connect while preserving its {} kept session(s).", offer.from_version, offer.to_version, offer.sessions)]
    MigrationRequired {
        host: String,
        offer: MigrationOffer,
        allocation: Option<Box<Lease>>,
    },

    /// A requested migration is a mutation. An ambiguous result must not be
    /// replayed by discovery or connection retries.
    #[error("The requested server migration could not be confirmed: {0}. Reconnect to inspect the host before requesting migration again.")]
    MigrationFailed(Box<RemoteError>),

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

/// Safe renderer-facing migration offer. The host's revalidation token stays
/// in native handshake state and is never serialized into a UI diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationOffer {
    pub available: bool,
    pub from_version: String,
    pub to_version: String,
    pub sessions: u32,
}

/// What the user reads about a failed connect: what went wrong and, where
/// there is one, what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration: Option<MigrationOffer>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Failure {
    pub fn from_error_budgeted(
        error: &impl std::fmt::Display,
        budget: &ConnectionBudget,
    ) -> io::Result<Charged<Self>> {
        budget.display(error, false).map(|message| {
            message.map(|message| Self {
                migration: None,
                message,
                hint: None,
            })
        })
    }
}

impl RemoteError {
    /// Retrying cannot fix credentials, host identity, unsupported capabilities
    /// or bad installation/protocol data. Such errors require explicit action.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Ssh(error) | Self::Install(InstallError::Ssh(error)) => ssh_retryable(error),
            Self::Spawn(error) | Self::Native(error) => {
                crate::bridge::stale_instance(error)
                    || matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                    )
            }
            Self::PreparationTimeout | Self::DiscoveryChanged | Self::NoDaemon { .. } => true,
            _ => false,
        }
    }
    /// Keep native failure copies charged from construction through delivery.
    pub fn failure_budgeted(&self, budget: &ConnectionBudget) -> io::Result<Charged<Failure>> {
        let bytes = splitty_transport::budget::display_size(self)?
            .checked_add(self.hint_storage())
            .and_then(|bytes| bytes.checked_mul(8))
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or_else(|| io::Error::other("failure exceeds capacity"))?;
        budget
            .charge(bytes, false, || self.failure())
            .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))
    }
    fn hint_storage(&self) -> usize {
        match self {
            Self::NoDaemon { server, .. } => server.len(),
            Self::Ssh(error) | Self::Install(InstallError::Ssh(error)) => {
                error.hint().map_or(0, str::len)
            }
            Self::Bootstrap(error) | Self::GaveUp { last: error, .. } => error.hint_storage(),
            _ => 256,
        }
    }
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
            migration: match self {
                Self::MigrationRequired { offer, .. } => Some(offer.clone()),
                _ => None,
            },
            message,
            hint: self.hint(),
        }
    }

    fn hint(&self) -> Option<String> {
        match self {
            RemoteError::Ssh(error) => error.hint().map(str::to_owned),
            RemoteError::Install(InstallError::Ssh(error)) => error.hint().map(str::to_owned),
            RemoteError::NoDaemon { server, .. } => {
                Some(format!("Start one there with: {server} daemon start"))
            }
            RemoteError::PortUnavailable { port, .. } if *port != 0 => Some(format!(
                "Choose a different local browser gateway port, or stop the listener on 127.0.0.1:{port}."
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
#[cfg(test)]
fn from_ssh(host: &str, error: SshError) -> RemoteError {
    from_ssh_budgeted(host, error, &ConnectionBudget::default())
}

fn from_ssh_budgeted(host: &str, error: SshError, budget: &ConnectionBudget) -> RemoteError {
    match error {
        SshError::Spawn(error) => RemoteError::Spawn(error),
        SshError::NotFound { host, allocation } => RemoteError::SplittyMissing { host, allocation },
        SshError::Failed {
            status: Some(75), ..
        } => RemoteError::DiscoveryChanged,
        // clap rejects the subcommand or flag a server older than
        // `remote-handshake` does not know; ssh itself fails with 255.
        SshError::Failed { status, stderr, .. }
            if status != Some(255)
                && (stderr.contains("'remote-handshake'") || stderr.contains("'--json'")) =>
        {
            match budget.display(&host, false) {
                Ok(host) => {
                    let (host, allocation) = host.into_parts();
                    RemoteError::SplittyTooOld {
                        host,
                        allocation: Some(Box::new(allocation)),
                    }
                }
                Err(error) => RemoteError::Native(error),
            }
        }
        error @ SshError::Failed { .. } => RemoteError::Ssh(error),
    }
}

fn ssh_retryable(error: &SshError) -> bool {
    match error {
        SshError::Failed {
            status: Some(255),
            stderr,
            ..
        } => !crate::ssh::host_key_failed(stderr) && !crate::ssh::authentication_failed(stderr),
        SshError::Spawn(error) => matches!(
            error.kind(),
            io::ErrorKind::ConnectionRefused
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::UnexpectedEof
                | io::ErrorKind::TimedOut
                | io::ErrorKind::WouldBlock
        ),
        _ => false,
    }
}

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

    /// Transfer the already-bound listener to an in-process gateway. Unlike an
    /// external SSH forward, it can keep ownership continuously across startup.
    pub fn into_listener(mut self) -> io::Result<TcpListener> {
        self.listener.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "port reservation was already handed off",
            )
        })
    }
}

/// The local ports of a tunnel, each held until its `ssh` takes it over: the
/// daemon's, and one per forward.
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
    answered: Mutex<Option<std::sync::Arc<Charged<String>>>>,
    protocol: u32,
    assignment: admission::Assignment,
    update: AtomicBool,
}

impl SshRemote {
    pub fn new(
        ssh: Ssh,
        version: &'static str,
        protocol: u32,
        payload_for: PayloadFor,
        assignment: admission::Assignment,
    ) -> Self {
        Self {
            ssh: ssh.into_stdio_only(),
            version,
            payload_for,
            server: Mutex::new(Server::Path),
            answered: Mutex::new(None),
            protocol,
            assignment,
            update: AtomicBool::new(false),
        }
    }

    pub fn with_update(self, update: bool) -> Self {
        self.update.store(update, Ordering::Release);
        self
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

    async fn handshake(&self) -> Result<Charged<handshake::Handshake>, RemoteError> {
        handshake::handshake(&self.ssh, &self.server(), true)
            .await
            .map_err(|error| match error {
                HandshakeError::Ssh(error) => {
                    from_ssh_budgeted(self.host(), error, &self.ssh.context().budget)
                }
                HandshakeError::Invalid {
                    host,
                    detail,
                    allocation,
                } => RemoteError::InvalidAnswer {
                    host,
                    detail,
                    allocation,
                },
            })
    }
}

impl Remote for SshRemote {
    type Client = NativeBridge;
    type Pending = (admission::Carrier, admission::Opening);

    fn host(&self) -> &str {
        self.ssh.target().as_str()
    }

    fn version(&self) -> &str {
        self.version
    }

    fn protocol(&self) -> u32 {
        self.protocol
    }
    fn context(&self) -> splitty_transport::queue::Context {
        self.ssh.context()
    }

    fn admit(&self) -> Result<Self::Pending, RemoteError> {
        let admission = admission::Admission::global();
        let carrier = admission.carrier().map_err(|error| {
            RemoteError::Spawn(io::Error::new(io::ErrorKind::WouldBlock, error))
        })?;
        let opening = admission.begin(self.assignment.clone()).map_err(|error| {
            RemoteError::Spawn(io::Error::new(io::ErrorKind::WouldBlock, error))
        })?;
        Ok((carrier, opening))
    }

    async fn prepare(&self, on_install: impl FnOnce(&str) + Send) -> Result<(), RemoteError> {
        self.install(on_install).await
    }

    /// The handshake, which starts a missing private daemon. When the
    /// installed server is gone (a rebuilt container), it is installed again
    /// once.
    async fn daemon(&self) -> Result<Charged<RunningDaemon>, RemoteError> {
        let answer = match self.handshake().await {
            Err(RemoteError::SplittyMissing { .. })
                if matches!(self.server(), Server::Installed { .. }) =>
            {
                self.install(|_| {}).await?;
                self.handshake().await?
            }
            answer => answer?,
        };
        *self
            .answered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(std::sync::Arc::new(
            self.ssh
                .context()
                .budget
                .charge(answer.server.version.len() + 256, false, || {
                    answer.server.version.clone()
                })
                .map_err(|error| {
                    RemoteError::Spawn(io::Error::new(io::ErrorKind::WouldBlock, error))
                })?,
        ));
        if !answer.server.transport_versions.contains(&wire::VERSION) {
            return Err(RemoteError::InvalidAnswer {
                allocation: None,
                host: self.host().into(),
                detail: "the answering server does not support native transport v1".into(),
            });
        }
        match &answer.daemon.daemon {
            Some(_) if answer.daemon.running => {
                Ok(answer.map(|answer| answer.daemon.daemon.expect("validated daemon")))
            }
            _ => {
                let (answer, lease) = answer.into_parts();
                Err(RemoteError::NoDaemon {
                    host: self.host().to_owned(),
                    server: answer.server.path,
                    allocation: Some(Box::new(lease)),
                })
            }
        }
    }

    fn server_version(&self) -> Option<std::sync::Arc<Charged<String>>> {
        self.answered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    async fn stop_daemon(&self, daemon: &RunningDaemon) -> Result<(), RemoteError> {
        let instance = daemon
            .instance
            .as_deref()
            .ok_or_else(|| RemoteError::InvalidAnswer {
                allocation: None,
                host: self.host().into(),
                detail: "legacy daemon has no verified instance; explicitly migrate it on the host"
                    .into(),
            })?;
        let command = self
            .server()
            .command()
            .word("daemon")
            .word("stop")
            .word("--if-instance")
            .value(instance);
        run_checked(&self.ssh, &command, None)
            .await
            .map(|_| ())
            .map_err(|error| from_ssh_budgeted(self.host(), error, &self.ssh.context().budget))
    }

    fn take_update_intent(&self) -> bool {
        self.update.swap(false, Ordering::AcqRel)
    }

    async fn migrate_legacy(&self, daemon: &RunningDaemon) -> Result<(), RemoteError> {
        let Some(handshake::LegacyMigration::Ready { token, .. }) = &daemon.legacy_migration else {
            return Err(RemoteError::InvalidAnswer {
                allocation: None,
                host: self.host().into(),
                detail: "legacy migration is unavailable".into(),
            });
        };
        let command = self
            .server()
            .command()
            .word("remote-migrate")
            .word("--expected")
            .value(token);
        run_checked(&self.ssh, &command, None)
            .await
            .map(|_| ())
            .map_err(|error| from_ssh_budgeted(self.host(), error, &self.ssh.context().budget))
    }

    async fn open_bridge(
        &self,
        daemon: &RunningDaemon,
        pending: Self::Pending,
    ) -> Result<NativeBridge, RemoteError> {
        NativeBridge::open_with_ssh(
            &self.ssh,
            &self.server(),
            pending,
            negotiation::Expected {
                instance: daemon.instance.as_deref(),
                purpose: &wire::Purpose::Direct,
                mux_protocol: daemon.protocol,
                preparation: wire::HANDSHAKE_TIMEOUT,
            },
        )
        .await
        .map_err(
            |error| match crate::bridge::ssh_failure(self.host(), &error) {
                Some(failure) => {
                    from_ssh_budgeted(self.host(), failure, &self.ssh.context().budget)
                }
                None => RemoteError::Native(error),
            },
        )
    }
}

async fn wait_for_forward(child: &mut Worker, local_port: u16) -> io::Result<()> {
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

/// Prepare and open a native connection. The caller owns any browser gateway
/// or deliberate forwarding separately; no browser credential crosses SSH.
pub async fn open(
    ssh: Ssh,
    version: &'static str,
    protocol: u32,
    payload_for: PayloadFor,
    assignment: admission::Assignment,
    on_progress: impl FnMut(Charged<Progress>) + Send,
) -> Result<(SshRemote, Connected<NativeBridge>), RemoteError> {
    open_with_update(
        ssh,
        version,
        protocol,
        payload_for,
        assignment,
        false,
        on_progress,
    )
    .await
}

/// Only a native launcher or an explicitly confirmed host update supplies
/// `update`. The intent belongs to this preparation, never later reconnects.
pub async fn open_with_update(
    ssh: Ssh,
    version: &'static str,
    protocol: u32,
    payload_for: PayloadFor,
    assignment: admission::Assignment,
    update: bool,
    on_progress: impl FnMut(Charged<Progress>) + Send,
) -> Result<(SshRemote, Connected<NativeBridge>), RemoteError> {
    let owner = ssh.owner_context();
    let remote =
        SshRemote::new(ssh, version, protocol, payload_for, assignment).with_update(update);
    let connected = match owner {
        None => connect(&remote, on_progress).await?,
        Some(owner) => {
            let attempt = supervisor::attempts(&remote, on_progress);
            tokio::pin!(attempt);
            match tokio::time::timeout(PREPARE_TIMEOUT, &mut attempt).await {
                Ok(result) => result?,
                Err(_) => {
                    owner.close();
                    if let Ok(connected) = attempt.await {
                        let _ = connected.client.finish().await;
                    }
                    return Err(RemoteError::PreparationTimeout);
                }
            }
        }
    };
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
    fn from(Failure { message, hint, .. }: Failure) -> Self {
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
            Event::Replaced { .. } => ConnectEvent::Restored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn native_setup_rechecks_missing_and_replaced_hosts_and_stops_only_the_checked_instance()
    {
        use std::os::unix::fs::PermissionsExt;
        for mode in [
            "missing",
            "stale",
            "stale_handshake",
            "update",
            "auth_bridge",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let program = directory.path().join("ssh");
            std::fs::write(&program, include_str!("../../tests/native_ssh.py")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::write(
                directory.path().join("native_bridge.py"),
                include_str!("../../tests/native_bridge.py"),
            )
            .unwrap();
            std::fs::write(directory.path().join("mode"), mode).unwrap();
            let ssh = Ssh::new(crate::ssh::SshTarget::parse("user@host").unwrap())
                .with_test_program(program);
            let result = open(
                ssh,
                "0.1.30",
                1,
                |_, _| Ok(None),
                admission::Assignment::new(admission::Backend::new(), admission::Domain::new()),
                |_| {},
            )
            .await;
            match result {
                Ok((_remote, mut connected)) => {
                    assert_ne!(mode, "auth_bridge");
                    let welcome = connected.client.recv().await.unwrap();
                    assert_eq!(
                        welcome.frame.application().unwrap().1,
                        br#"{"type":"mux.welcome"}"#
                    );
                    drop(welcome);
                    let (result, ended) = connected.client.finish().await;
                    result.unwrap();
                    assert!(ended.status.unwrap().success(), "{}", ended.stderr);
                }
                Err(error) => {
                    assert_eq!(mode, "auth_bridge", "{error}");
                    assert!(!error.retryable());
                    assert!(matches!(error, RemoteError::Ssh(_)));
                }
            }
            let calls: Vec<Vec<String>> = std::fs::read_to_string(directory.path().join("calls"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            for args in &calls {
                for flag in [
                    "-T",
                    "-a",
                    "-x",
                    "ClearAllForwardings=yes",
                    "ControlMaster=no",
                    "ControlPath=none",
                    "ControlPersist=no",
                    "ForkAfterAuthentication=no",
                    "StdinNull=no",
                ] {
                    assert!(
                        args.iter().any(|arg| arg == flag),
                        "{mode}: missing {flag}: {args:?}"
                    );
                }
                let command = args.last().unwrap();
                if !command.contains("splitty-probe") {
                    assert!(
                        command.starts_with(
                            r#"'/home/it'\''s me/.local/share/splitty/remote/0.1.30/splitty' "#
                        ),
                        "{command}"
                    );
                }
                assert!(!command.contains("--port"));
            }
            let commands: Vec<_> = calls.iter().map(|args| args.last().unwrap()).collect();
            assert_eq!(
                commands
                    .iter()
                    .filter(|command| command.contains("daemon stop"))
                    .count(),
                usize::from(mode == "update")
            );
            assert_eq!(
                commands
                    .iter()
                    .filter(|command| command.contains("remote-handshake"))
                    .count(),
                if ["missing", "stale", "stale_handshake", "update"].contains(&mode) {
                    2
                } else {
                    1
                }
            );
        }
    }

    fn failed(status: i32, stderr: &str) -> SshError {
        SshError::Failed {
            allocation: None,
            message: format!("ssh to vm failed: {stderr}"),
            status: Some(status),
            stderr: stderr.to_owned(),
            hint: None,
        }
    }

    #[test]
    fn failure_hint_copies_are_admitted_and_retained_beyond_the_source_error() {
        let budget = ConnectionBudget::default();
        let error = RemoteError::NoDaemon {
            host: "vm".into(),
            server: format!("/{}", "p".repeat(64 * 1024)),
            allocation: None,
        };
        let failure = error.failure_budgeted(&budget).unwrap();
        drop(error);
        let payload = failure.message.capacity() + failure.hint.as_ref().unwrap().capacity();
        assert!(budget.used() >= payload);
        assert!(failure
            .hint
            .as_deref()
            .unwrap()
            .starts_with("Start one there with:"));
        drop(failure);
        assert_eq!(budget.used(), 0);
        let held = budget
            .reserve(
                wire::CONNECTION_BYTES - wire::CONNECTION_CONTROL_BYTES,
                false,
            )
            .unwrap();
        assert_eq!(
            RemoteError::NoDaemon {
                host: "vm".into(),
                server: "/bin/splitty".into(),
                allocation: None
            }
            .failure_budgeted(&budget)
            .unwrap_err()
            .kind(),
            io::ErrorKind::WouldBlock
        );
        drop(held);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_missing_or_old_splitty_is_named() {
        let missing = from_ssh(
            "vm",
            SshError::NotFound {
                host: "vm".into(),
                allocation: None,
            },
        );
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
        assert!(matches!(refused, RemoteError::Ssh(_)), "{refused:?}");
    }

    #[cfg(unix)]
    #[test]
    fn missing_splitty_conversion_keeps_the_original_host_and_reservation() {
        use std::os::unix::process::ExitStatusExt;
        let budget = ConnectionBudget::default();
        let source = crate::ssh::classify_budgeted(
            "vm",
            std::process::ExitStatus::from_raw(127 << 8),
            "not found",
            &budget,
        );
        let pointer = match &source {
            SshError::NotFound { host, .. } => host.as_ptr(),
            other => panic!("unexpected error: {other}"),
        };
        let used = budget.used();
        assert!(used > 0);
        let converted = from_ssh("vm", source);
        assert!(
            matches!(&converted, RemoteError::SplittyMissing { host, allocation: Some(_) } if host.as_ptr() == pointer)
        );
        assert_eq!(budget.used(), used);
        drop(converted);
        assert_eq!(budget.used(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn old_cli_conversion_is_charged_until_drop_and_refuses_exhaustion() {
        use std::os::unix::process::ExitStatusExt;
        let budget = ConnectionBudget::default();
        let make_source = || {
            crate::ssh::classify_budgeted(
                "vm",
                std::process::ExitStatus::from_raw(2 << 8),
                "unexpected argument '--json'",
                &budget,
            )
        };
        let converted = from_ssh_budgeted("vm", make_source(), &budget);
        assert!(matches!(
            &converted,
            RemoteError::SplittyTooOld {
                allocation: Some(_),
                ..
            }
        ));
        assert!(budget.used() >= 2);
        drop(converted);
        assert_eq!(budget.used(), 0);
        let source = make_source();
        let held = budget
            .reserve(
                splitty_transport::wire::CONNECTION_BYTES
                    - splitty_transport::wire::CONNECTION_CONTROL_BYTES
                    - budget.used(),
                false,
            )
            .unwrap();
        assert!(
            matches!(from_ssh_budgeted("vm", source, &budget), RemoteError::Native(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        drop(held);
        assert_eq!(budget.used(), 0);
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
        RemoteError::Ssh(SshError::Failed {
            allocation: None,
            message: "ssh to vm failed: Host key verification failed.".into(),
            status: Some(255),
            stderr: "Host key verification failed.".into(),
            hint: Some("Run `ssh vm` once in a terminal and check the fingerprint.".into()),
        })
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
        assert!(hint.contains("local browser gateway port"), "{hint}");

        let no_daemon = RemoteError::NoDaemon {
            allocation: None,
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

        let corrupted = RemoteError::from(InstallError::Checksum {
            host: "vm".into(),
            allocation: None,
        })
        .failure();
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

    /// The machine-readable lines a launcher parses (`cli`, Remote Connect
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
                Event::Replaced {
                    instance: "0123456789abcdef0123456789abcdef".into()
                }
            )),
            r#"{"event":"restored"}"#
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
    #[test]
    fn the_native_flow_can_run_as_a_task() {
        let target = crate::ssh::SshTarget::parse("host").unwrap();
        let assignment =
            admission::Assignment::new(admission::Backend::new(), admission::Domain::new());
        let flow = open(
            Ssh::stdio_only(target),
            "0.1.30",
            1,
            install::payload_for,
            assignment,
            |_| {},
        );
        assert_send(&flow);
    }
}
