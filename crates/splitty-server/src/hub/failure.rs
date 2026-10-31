//! Why a hub socket failed before its relay, and what the client reads about
//! it: the `remote.error` code, a message and, where the user can act, a hint.
//!
//! The codes tell the client whether retrying can help (`unreachable`,
//! `daemon`, `bridge`, `internal`) or not (the others), so the mapping is
//! kept in one pure function.

use std::io;

use serde::Serialize;
use splitty_remote::handshake::{HandshakeError, Screen};
use splitty_remote::install::InstallError;
use splitty_remote::ssh::{self, SshError, TargetError};
use splitty_remote::tunnel::{PortTunnelError, ReverseTunnelError};

/// Why a socket failed before the relay.
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error(transparent)]
    Target(#[from] TargetError),

    #[error(transparent)]
    Install(#[from] InstallError),

    #[error(transparent)]
    Handshake(#[from] HandshakeError),

    /// Nothing runs the server on the host: none is installed, none is in its
    /// `PATH`, and this client carries none for its architecture.
    #[error(
        "Splitty is not installed on {host}, and this Splitty carries no server for its \
         architecture ({arch})."
    )]
    NoServer { host: String, arch: String },

    #[error("No Splitty daemon is running on {host}.")]
    NoDaemon { host: String },

    #[error(
        "{host} runs Splitty {version} (protocol {protocol}); this Splitty needs protocol {}.",
        supported(*ours)
    )]
    Incompatible {
        host: String,
        version: String,
        protocol: u32,
        /// Whether its sessions outlive it, so an update ends none.
        keeps_sessions: bool,
        ours: u32,
        /// The daemon's screens, so the client can say what an update ends.
        screens: Vec<Screen>,
    },

    /// `update=1`, but the host has no server of this version to start
    /// instead: none is installed, and this client carries none for it.
    #[error(
        "{host} has no Splitty {} to update its daemon (Splitty {version}) to; the daemon keeps \
         running.",
        super::VERSION
    )]
    NoUpdate {
        host: String,
        version: String,
        arch: String,
    },

    /// `daemon stop` failed on the host.
    #[error(transparent)]
    Stop(SshError),

    /// After `daemon stop`, the host still runs the daemon of the other
    /// version: it has not ended yet.
    #[error("{host} still runs Splitty {version}: its daemon did not stop in time.")]
    NotStopped { host: String, version: String },

    #[error("No local port is free for the daemon's port hint ({0}).")]
    Port(io::Error),

    /// The bridge could not be started, or ended before the daemon's first
    /// message.
    #[error(transparent)]
    Bridge(SshError),

    #[error("The bridge to {host} delivered nothing within {seconds} seconds.")]
    BridgeSilent { host: String, seconds: u64 },

    /// A forward's `ssh` failed, or its port did not answer.
    #[error(transparent)]
    Forward(PortTunnelError),

    #[error("No local port is free for the forward ({0}).")]
    ForwardPort(io::Error),

    /// The host cannot tell which user a connection comes from.
    #[error("Sharing a port needs a Linux host; {host} runs {kernel}.")]
    ReverseUnsupported { host: String, kernel: String },

    /// `remote-relay prepare` failed on the host.
    #[error(transparent)]
    RelayPrepare(SshError),

    /// `remote-relay prepare` answered something unusable.
    #[error("The relay on {host} answered: {detail}")]
    RelayAnswer { host: String, detail: String },

    /// A reverse forward's `ssh` or the host's relay failed.
    #[error(transparent)]
    Reverse(ReverseTunnelError),
}

/// The `code` of a `remote.error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    InvalidTarget,
    HostKey,
    Auth,
    Unreachable,
    UnsupportedHost,
    Install,
    Daemon,
    Incompatible,
    Bridge,
    /// Only in the answers of `/api/remote/forwards`: the forward's `ssh`
    /// ran, but the forward failed.
    Forward,
    Internal,
}

impl Code {
    /// The code as it goes on the wire, also the close reason after the error.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::InvalidTarget => "invalid_target",
            Code::HostKey => "host_key",
            Code::Auth => "auth",
            Code::Unreachable => "unreachable",
            Code::UnsupportedHost => "unsupported_host",
            Code::Install => "install",
            Code::Daemon => "daemon",
            Code::Incompatible => "incompatible",
            Code::Bridge => "bridge",
            Code::Forward => "forward",
            Code::Internal => "internal",
        }
    }
}

/// What a `remote.error` carries besides its `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub code: Code,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// For `incompatible`: the host daemon's version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// For `incompatible`: the host daemon's protocol version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    /// For `incompatible`: whether the host daemon's sessions outlive it, so
    /// an update ends none.
    #[serde(rename = "keepsSessions", skip_serializing_if = "Option::is_none")]
    pub keeps_sessions: Option<bool>,
    /// For `incompatible`: the daemon's screens, as in `remote.ready`, which
    /// an update would end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screens: Option<Vec<Screen>>,
}

impl Failure {
    fn new(code: Code, message: String, hint: Option<String>) -> Self {
        Self {
            code,
            message,
            hint,
            version: None,
            protocol: None,
            keeps_sessions: None,
            screens: None,
        }
    }
}

/// Whether a client of protocol `ours` can talk to a daemon of `theirs`: the
/// same version or the one before (`remote-hosts`, Protocol Version).
pub fn compatible(theirs: u32, ours: u32) -> bool {
    theirs == ours || theirs.checked_add(1) == Some(ours)
}

/// The protocol versions a client of `ours` supports, for a message.
fn supported(ours: u32) -> String {
    match ours.checked_sub(1).filter(|before| *before > 0) {
        Some(before) => format!("{before} or {ours}"),
        None => ours.to_string(),
    }
}

/// What `ssh` failing means when it is about reaching `host` rather than
/// about the command it ran: `ssh` not starting, or its own status 255.
fn connection_failure(host: &str, error: &SshError) -> Option<Failure> {
    match error {
        SshError::Spawn(_) => Some(Failure::new(
            Code::Internal,
            error.to_string(),
            Some("Splitty reaches hosts with the system's OpenSSH client; install it.".to_owned()),
        )),
        SshError::Failed {
            status: Some(255),
            stderr,
            ..
        } => {
            let message = format!("Failed to reach {host}: {error}");
            Some(if ssh::host_key_failed(stderr) {
                Failure::new(Code::HostKey, message, error.hint().map(str::to_owned))
            } else if ssh::authentication_failed(stderr) {
                Failure::new(
                    Code::Auth,
                    message,
                    Some(format!(
                        "Splitty logs in without prompts. Check that `ssh {host}` logs in with a \
                         key (an agent or an IdentityFile) without asking for a password."
                    )),
                )
            } else {
                Failure::new(Code::Unreachable, message, None)
            })
        }
        SshError::Failed { .. } | SshError::NotFound { .. } => None,
    }
}

/// A `splitty` on the host's `PATH` older than `remote-handshake`: clap
/// rejects the subcommand or the flag.
/// The host's `splitty` does not know `remote-relay`.
fn relay_unknown(error: &SshError) -> bool {
    matches!(error, SshError::Failed { status, stderr, .. }
        if *status != Some(255) && stderr.contains("'remote-relay'"))
}

/// The host's `sshd` refused the remote forward.
fn remote_forward_refused(error: &SshError) -> bool {
    matches!(error, SshError::Failed { stderr, .. }
        if stderr.contains("remote port forwarding failed"))
}

fn too_old(error: &SshError) -> bool {
    matches!(error, SshError::Failed { status, stderr, .. }
        if *status != Some(255)
            && (stderr.contains("'remote-handshake'") || stderr.contains("'--json'")))
}

impl HubError {
    /// The `remote.error` for this failure on `host` (the target as the client
    /// sent it).
    pub fn failure(&self, host: &str) -> Failure {
        let ssh_error = match self {
            HubError::Install(InstallError::Ssh(error))
            | HubError::Handshake(HandshakeError::Ssh(error))
            | HubError::Stop(error)
            | HubError::Bridge(error)
            | HubError::Forward(PortTunnelError::Ssh(error))
            | HubError::RelayPrepare(error) => Some(error),
            // `ssh` exits with 255 when the host refuses the forward too,
            // which is no failure to reach the host.
            HubError::Reverse(ReverseTunnelError::Ssh(error)) if !remote_forward_refused(error) => {
                Some(error)
            }
            _ => None,
        };
        if let Some(failure) = ssh_error.and_then(|error| connection_failure(host, error)) {
            return failure;
        }

        match self {
            HubError::Target(error) => Failure::new(Code::InvalidTarget, error.to_string(), None),
            HubError::Install(InstallError::UnsupportedKernel { .. }) => {
                Failure::new(Code::UnsupportedHost, self.to_string(), None)
            }
            // The probe runs `sh`; a host without one is not a POSIX host.
            HubError::Install(InstallError::Ssh(SshError::NotFound { .. })) => Failure::new(
                Code::UnsupportedHost,
                format!("{host} has no POSIX shell to probe it with."),
                None,
            ),
            HubError::Install(error) => Failure::new(
                Code::Install,
                format!("Failed to install Splitty on {host}: {error}"),
                None,
            ),
            HubError::NoServer { arch, .. } => Failure::new(
                Code::UnsupportedHost,
                self.to_string(),
                Some(format!(
                    "Install splitty on {host}, in the PATH of non-interactive SSH sessions, or \
                     give this Splitty a server for {arch} in SPLITTY_REMOTE_PAYLOAD."
                )),
            ),
            HubError::Handshake(HandshakeError::Ssh(error)) if too_old(error) => Failure::new(
                Code::UnsupportedHost,
                format!("The splitty on {host} is too old for this Splitty."),
                Some(format!("Update splitty on {host}.")),
            ),
            HubError::Handshake(error) => Failure::new(
                Code::Daemon,
                format!("Failed to start the Splitty daemon on {host}: {error}"),
                None,
            ),
            HubError::NoDaemon { .. } => Failure::new(Code::Daemon, self.to_string(), None),
            HubError::Incompatible {
                version,
                protocol,
                keeps_sessions,
                ours,
                screens,
                ..
            } => {
                let hint = if protocol > ours {
                    format!("Update this Splitty to use {host}. Its daemon keeps running.")
                } else {
                    format!(
                        "Update Splitty on {host} to use it here. Its daemon and sessions keep \
                         running until then."
                    )
                };
                Failure {
                    version: Some(version.clone()),
                    protocol: Some(*protocol),
                    keeps_sessions: Some(*keeps_sessions),
                    screens: Some(screens.clone()),
                    ..Failure::new(Code::Incompatible, self.to_string(), Some(hint))
                }
            }
            HubError::NoUpdate { arch, .. } => Failure::new(
                Code::UnsupportedHost,
                self.to_string(),
                Some(format!(
                    "Install splitty {} on {host}, in the PATH of non-interactive SSH sessions, \
                     or give this Splitty a server for {arch} in SPLITTY_REMOTE_PAYLOAD.",
                    super::VERSION
                )),
            ),
            HubError::Stop(error) => Failure::new(
                Code::Daemon,
                format!("Failed to stop the Splitty daemon on {host}: {error}"),
                None,
            ),
            HubError::NotStopped { .. } => Failure::new(Code::Daemon, self.to_string(), None),
            HubError::Port(_) => Failure::new(Code::Internal, self.to_string(), None),
            HubError::Bridge(error) => Failure::new(
                Code::Bridge,
                format!("The bridge to {host} ended before its daemon answered: {error}"),
                None,
            ),
            HubError::BridgeSilent { .. } => Failure::new(Code::Bridge, self.to_string(), None),
            HubError::Forward(PortTunnelError::ConfiguredForwards { .. }) => Failure::new(
                Code::Forward,
                format!("Splitty does not forward ports of {host}: {self}."),
                Some(format!(
                    "Move the forwards to a Host entry of their own for the same machine in the \
                     SSH configuration, and keep {host} without forwards."
                )),
            ),
            HubError::Forward(error) => Failure::new(
                Code::Forward,
                format!("Failed to forward a port of {host}: {error}"),
                None,
            ),
            HubError::ForwardPort(_) => Failure::new(Code::Internal, self.to_string(), None),
            HubError::ReverseUnsupported { .. } => {
                Failure::new(Code::UnsupportedHost, self.to_string(), None)
            }
            HubError::RelayPrepare(error) if relay_unknown(error) => Failure::new(
                Code::UnsupportedHost,
                format!("The splitty on {host} is too old to share ports with."),
                Some(format!("Update splitty on {host}.")),
            ),
            HubError::RelayPrepare(error) => Failure::new(
                Code::Forward,
                format!("Failed to prepare {host} for a shared port: {error}"),
                None,
            ),
            HubError::RelayAnswer { .. } => Failure::new(Code::Forward, self.to_string(), None),
            HubError::Reverse(ReverseTunnelError::ConfiguredForwards { .. }) => Failure::new(
                Code::Forward,
                format!("Splitty does not share ports with {host}: {self}."),
                Some(format!(
                    "Move the forwards to a Host entry of their own for the same machine in the \
                     SSH configuration, and keep {host} without forwards."
                )),
            ),
            HubError::Reverse(ReverseTunnelError::Relay { code, .. }) if code == "unsupported" => {
                Failure::new(Code::UnsupportedHost, self.to_string(), None)
            }
            HubError::Reverse(ReverseTunnelError::Ssh(error)) if remote_forward_refused(error) => {
                Failure::new(
                    Code::Forward,
                    format!("{host} refused the forward of a shared port: {error}"),
                    Some(format!(
                        "The SSH server of {host} must allow forwarding to Unix sockets \
                         (AllowStreamLocalForwarding)."
                    )),
                )
            }
            HubError::Reverse(error) => Failure::new(
                Code::Forward,
                format!("Failed to share a port with {host}: {error}"),
                None,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(status: i32, stderr: &str) -> SshError {
        SshError::Failed {
            message: format!("ssh to vm failed: {stderr}"),
            status: Some(status),
            stderr: stderr.to_owned(),
            hint: None,
        }
    }

    /// `ssh::classify` as the real calls produce their errors, with its hint.
    #[cfg(unix)]
    fn classified(status: i32, stderr: &str) -> SshError {
        use std::os::unix::process::ExitStatusExt;
        ssh::classify(
            "vm",
            std::process::ExitStatus::from_raw(status << 8),
            stderr,
        )
    }

    fn code(error: HubError) -> Code {
        error.failure("vm").code
    }

    #[test]
    fn the_protocol_window_is_the_same_version_and_the_one_before() {
        assert!(compatible(1, 1));
        assert!(compatible(1, 2));
        assert!(compatible(2, 2));
        assert!(!compatible(3, 2), "a newer daemon");
        assert!(!compatible(1, 3), "two versions older");
        assert!(!compatible(0, 2));
        assert!(!compatible(u32::MAX, 1));
        assert_eq!(supported(1), "1");
        assert_eq!(supported(3), "2 or 3");
    }

    #[test]
    fn an_invalid_target_is_named() {
        let failure = HubError::from(TargetError::LooksLikeOption("-oProxyCommand=x".into()))
            .failure("-oProxyCommand=x");
        assert_eq!(failure.code, Code::InvalidTarget);
        assert!(
            failure.message.contains("starts with a dash"),
            "{failure:?}"
        );
        assert_eq!(failure.hint, None);
    }

    /// ssh's own failures (255), wherever they happen, say whether the host
    /// key, the login or the network failed.
    #[cfg(unix)]
    #[test]
    fn ssh_failures_tell_host_key_login_and_network_apart() {
        let host_key = HubError::Install(InstallError::Ssh(classified(
            255,
            "Host key verification failed.",
        )))
        .failure("vm");
        assert_eq!(host_key.code, Code::HostKey);
        let hint = host_key.hint.expect("a hint");
        assert!(hint.contains("Run `ssh vm` once"), "{hint}");
        assert!(hint.contains("fingerprint"), "{hint}");
        assert!(host_key.message.starts_with("Failed to reach vm: "));

        let auth = HubError::Handshake(HandshakeError::Ssh(classified(
            255,
            "user@vm: Permission denied (publickey).",
        )))
        .failure("vm");
        assert_eq!(auth.code, Code::Auth);
        assert!(auth.hint.expect("a hint").contains("`ssh vm`"));

        for stderr in [
            "ssh: connect to host vm port 22: Connection refused",
            "ssh: Could not resolve hostname vm: Name or service not known",
            "kex_exchange_identification: Connection closed by remote host",
        ] {
            let unreachable = HubError::Bridge(classified(255, stderr)).failure("vm");
            assert_eq!(unreachable.code, Code::Unreachable, "{stderr}");
            assert_eq!(unreachable.hint, None);
        }

        let spawn = HubError::Install(InstallError::Ssh(SshError::Spawn(io::Error::from(
            io::ErrorKind::NotFound,
        ))))
        .failure("vm");
        assert_eq!(spawn.code, Code::Internal);
        assert!(spawn.hint.expect("a hint").contains("OpenSSH"));
    }

    #[test]
    fn hosts_splitty_cannot_serve_are_unsupported() {
        assert_eq!(
            code(HubError::Install(InstallError::UnsupportedKernel {
                host: "vm".into(),
                kernel: "Darwin".into()
            })),
            Code::UnsupportedHost
        );
        assert_eq!(
            code(HubError::Install(InstallError::Ssh(SshError::NotFound {
                host: "vm".into()
            }))),
            Code::UnsupportedHost
        );

        let no_server = HubError::NoServer {
            host: "vm".into(),
            arch: "ppc64le".into(),
        }
        .failure("vm");
        assert_eq!(no_server.code, Code::UnsupportedHost);
        assert!(no_server.message.contains("ppc64le"), "{no_server:?}");
        let hint = no_server.hint.expect("a hint");
        assert!(hint.contains("SPLITTY_REMOTE_PAYLOAD"), "{hint}");

        let old = HubError::Handshake(HandshakeError::Ssh(failed(
            2,
            "error: unrecognized subcommand 'remote-handshake'",
        )))
        .failure("vm");
        assert_eq!(old.code, Code::UnsupportedHost);
        assert!(old.message.contains("too old"), "{old:?}");
    }

    #[test]
    fn failed_installs_and_daemons_are_named() {
        for error in [
            InstallError::Checksum { host: "vm".into() },
            InstallError::DoesNotRun {
                host: "vm".into(),
                arch: "x86_64".into(),
            },
            InstallError::InvalidProbe {
                host: "vm".into(),
                detail: "x".into(),
            },
            // The install script failed on the host: no space, `$HOME` not
            // writable.
            InstallError::Ssh(failed(1, "mkdir: cannot create directory: No space left")),
        ] {
            let failure = HubError::Install(error).failure("vm");
            assert_eq!(failure.code, Code::Install, "{failure:?}");
            assert!(
                failure
                    .message
                    .starts_with("Failed to install Splitty on vm: "),
                "{failure:?}"
            );
        }

        let daemon = HubError::Handshake(HandshakeError::Ssh(failed(
            1,
            "cannot run daemon start: Address already in use",
        )))
        .failure("vm");
        assert_eq!(daemon.code, Code::Daemon);
        assert!(
            daemon.message.contains("Address already in use"),
            "{daemon:?}"
        );
        assert_eq!(
            code(HubError::Handshake(HandshakeError::Invalid {
                host: "vm".into(),
                detail: "x".into()
            })),
            Code::Daemon
        );
        assert_eq!(code(HubError::NoDaemon { host: "vm".into() }), Code::Daemon);
    }

    #[test]
    fn an_incompatible_daemon_names_its_version_and_protocol() {
        let older = HubError::Incompatible {
            host: "vm".into(),
            version: "0.3.9".into(),
            protocol: 1,
            keeps_sessions: false,
            ours: 3,
            screens: vec![Screen {
                id: "s1".into(),
                name: Some("api".into()),
                panes: 3,
            }],
        }
        .failure("vm");
        assert_eq!(older.code, Code::Incompatible);
        assert_eq!(
            older.message,
            "vm runs Splitty 0.3.9 (protocol 1); this Splitty needs protocol 2 or 3."
        );
        assert_eq!(
            (older.version.as_deref(), older.protocol),
            (Some("0.3.9"), Some(1))
        );
        assert_eq!(older.keeps_sessions, Some(false));
        // What an update would end, for the client's warning.
        assert_eq!(
            older.screens.as_deref().map(|screens| screens[0].panes),
            Some(3)
        );
        assert!(older
            .hint
            .expect("hint")
            .starts_with("Update Splitty on vm"));

        let newer = HubError::Incompatible {
            host: "vm".into(),
            version: "9.0.0".into(),
            protocol: 2,
            keeps_sessions: true,
            ours: 1,
            screens: Vec::new(),
        }
        .failure("vm");
        assert!(newer.hint.expect("hint").starts_with("Update this Splitty"));
    }

    /// `update=1`: a host without this version keeps its daemon and needs the
    /// user; a stop that failed or has not taken effect yet can be retried.
    #[cfg(unix)]
    #[test]
    fn updates_that_cannot_finish_are_named() {
        let no_update = HubError::NoUpdate {
            host: "vm".into(),
            version: "0.0.9".into(),
            arch: "riscv64".into(),
        }
        .failure("vm");
        assert_eq!(no_update.code, Code::UnsupportedHost);
        assert!(
            no_update.message.contains("Splitty 0.0.9")
                && no_update.message.contains("keeps running"),
            "{no_update:?}"
        );
        let hint = no_update.hint.expect("a hint");
        assert!(hint.contains("riscv64"), "{hint}");

        let stop = HubError::Stop(failed(1, "Permission denied")).failure("vm");
        assert_eq!(stop.code, Code::Daemon);
        assert!(
            stop.message
                .starts_with("Failed to stop the Splitty daemon on vm: "),
            "{stop:?}"
        );
        assert_eq!(
            HubError::Stop(classified(255, "Connection refused"))
                .failure("vm")
                .code,
            Code::Unreachable
        );

        let not_stopped = HubError::NotStopped {
            host: "vm".into(),
            version: "0.0.9".into(),
        }
        .failure("vm");
        assert_eq!(not_stopped.code, Code::Daemon);
        assert!(
            not_stopped.message.contains("did not stop"),
            "{not_stopped:?}"
        );
    }

    #[test]
    fn bridges_that_end_early_and_local_problems_are_named() {
        let ended =
            HubError::Bridge(failed(1, "No Splitty daemon is running on this host.")).failure("vm");
        assert_eq!(ended.code, Code::Bridge);
        assert!(ended.message.contains("No Splitty daemon"), "{ended:?}");
        assert_eq!(
            code(HubError::BridgeSilent {
                host: "vm".into(),
                seconds: 30
            }),
            Code::Bridge
        );
        assert_eq!(
            code(HubError::Port(io::Error::from(io::ErrorKind::AddrInUse))),
            Code::Internal
        );
    }

    /// `/api/remote/forwards`: ssh's own failures as everywhere, a host whose
    /// configuration forwards ports with what to do about it, and a forward
    /// that did not answer.
    #[cfg(unix)]
    #[test]
    fn failed_forwards_are_named() {
        let configured = HubError::Forward(PortTunnelError::ConfiguredForwards {
            host: "vm".into(),
            forwards: vec!["remoteforward 9999 [localhost]:9999".into()],
        })
        .failure("vm");
        assert_eq!(configured.code, Code::Forward);
        assert!(
            configured
                .message
                .starts_with("Splitty does not forward ports of vm: ")
                && configured
                    .message
                    .contains("(remoteforward 9999 [localhost]:9999)"),
            "{configured:?}"
        );
        assert!(
            configured.hint.expect("a hint").contains("Host entry"),
            "the hint says what to do"
        );

        let auth = HubError::Forward(PortTunnelError::Ssh(classified(
            255,
            "user@vm: Permission denied (publickey).",
        )))
        .failure("vm");
        assert_eq!(auth.code, Code::Auth);

        let silent = HubError::Forward(PortTunnelError::NotEstablished {
            host: "vm".into(),
            remote: 3000,
            detail: "ssh forward was not established in time".into(),
        })
        .failure("vm");
        assert_eq!(silent.code, Code::Forward);
        assert_eq!(
            silent.message,
            "Failed to forward a port of vm: the forward of port 3000 on vm was not \
             established: ssh forward was not established in time"
        );
        assert_eq!(
            code(HubError::ForwardPort(io::Error::from(
                io::ErrorKind::AddrInUse
            ))),
            Code::Internal
        );
    }

    #[test]
    fn a_failure_encodes_only_the_fields_it_has() {
        let failure = Failure::new(Code::Unreachable, "gone".into(), None);
        assert_eq!(
            serde_json::to_value(&failure).expect("json"),
            serde_json::json!({"code": "unreachable", "message": "gone"})
        );
        for code in [
            Code::InvalidTarget,
            Code::HostKey,
            Code::Auth,
            Code::Unreachable,
            Code::UnsupportedHost,
            Code::Install,
            Code::Daemon,
            Code::Incompatible,
            Code::Bridge,
            Code::Forward,
            Code::Internal,
        ] {
            assert_eq!(
                serde_json::to_value(code).expect("json"),
                serde_json::json!(code.as_str())
            );
        }
    }
}
