//! Host update policy and deliberately forwarded ports over owned SSH workers.

pub mod exposure;
mod forward;
mod reverse;
mod supervisor;

pub use forward::{
    check_forward_address, reserve, reserve_exact, reserve_free_at, Forward, PortForward,
    PortTunnel, PortTunnelError,
};
pub use reverse::{
    parse_prepared, prepare_command, socket_path, ReverseTunnel, ReverseTunnelError, HEARTBEAT,
    MAX_SOCKET_PATH, RELAY_FAILED, RELAY_READY,
};
pub use supervisor::{backoff, migration_offer, older, update_plan, UpdatePlan, PREPARE_TIMEOUT};

use std::io;
use std::net::TcpListener;
use std::time::Duration;

use serde::Serialize;
use splitty_transport::worker::Worker;

use crate::install::InstallError;
use crate::ssh::SshError;
use splitty_transport::budget::{Charged, ConnectionBudget, Lease};

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

#[cfg(test)]
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

/// A local port held open until the tunnel takes it over.
///
/// The reservation is kept as long as possible on purpose (SR-13). Picking a
/// port, closing the socket and only then starting `ssh` leaves a window in
/// which another local process can take it — and the browser would then send its
/// session cookie to whatever is listening. The listener is dropped immediately
/// before spawning `ssh`, which is the shortest window achievable without socket
/// hand-off; while a dropped tunnel is rebuilt, the supervisor holds one again.
pub struct PortReservation {
    bind: std::net::IpAddr,
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
        Self::acquire_at(std::net::Ipv4Addr::LOCALHOST.into(), port)
    }

    pub fn acquire_at(bind: std::net::IpAddr, port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind((bind, port))?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener: Some(listener),
            bind,
            port,
        })
    }

    /// Recheck IPv6 while the IPv4 reservation stays held (including rebuilds).
    pub fn check_forward_address(&self) -> io::Result<()> {
        Self::check_bind(self.bind, self.port)
    }

    pub fn bind_address(&self) -> std::net::IpAddr {
        self.bind
    }
    pub fn check_bind(bind: std::net::IpAddr, port: u16) -> io::Result<()> {
        if bind.is_ipv4()
            && (bind.is_unspecified()
                || bind == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
        {
            forward::check_forward_address(port)
        } else {
            Ok(())
        }
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

const EXIT_GRACE: Duration = Duration::from_secs(2);

async fn wait_for_forward_at(
    child: &mut Worker,
    local_port: u16,
    bind: std::net::IpAddr,
) -> io::Result<()> {
    let bind = if bind.is_unspecified() {
        if bind.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        }
    } else {
        bind
    };
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

        let connect = TcpStream::connect((bind, local_port));
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

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::wire;

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
            after: Duration::from_secs(600),
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

    #[test]
    fn give_up_times_read_as_minutes_or_seconds() {
        assert_eq!(describe(&Duration::from_secs(600)), "10 minutes");
        assert_eq!(describe(&Duration::from_secs(60)), "1 minute");
        assert_eq!(describe(&Duration::from_secs(90)), "90 seconds");
    }
}
