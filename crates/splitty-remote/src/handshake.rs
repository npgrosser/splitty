//! The handshake: what a host tells a client in one SSH call, the host's
//! server, its daemon (started on request) and its screens.
//!
//! The host side is `splitty remote-handshake --json` in the `splitty`
//! binary; [`handshake`] runs it and reads the answer.

use serde::{Deserialize, Serialize};

use crate::install::Server;
use crate::ssh::{run_checked, Executor, SshError};

/// The format version of [`Handshake`].
pub const FORMAT: u32 = 1;

/// The answer of `splitty remote-handshake --json`. Unknown fields are
/// ignored, so a newer host can add some.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    pub handshake: u32,
    pub server: ServerInfo,
    pub daemon: DaemonState,
    #[serde(default)]
    pub screens: Vec<Screen>,
}

/// The server that answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    pub protocol: u32,
    pub path: String,
}

/// Whether a daemon runs, and which.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonState {
    pub running: bool,
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<RunningDaemon>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningDaemon {
    pub pid: u32,
    pub port: u16,
    pub version: String,
    pub protocol: u32,
    pub started_at: String,
    /// Whether its sessions outlive it in session keepers, so that a restart
    /// or an update ends none. False for a host that does not say.
    #[serde(default)]
    pub keeps_sessions: bool,
    /// Whether this handshake started it.
    #[serde(default)]
    pub started: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub panes: usize,
}

/// Why the handshake failed.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error(transparent)]
    Ssh(#[from] SshError),

    #[error("{host} gave an unexpected handshake: {detail}")]
    Invalid { host: String, detail: String },
}

/// Parse the handshake's output.
pub fn parse(host: &str, stdout: &[u8]) -> Result<Handshake, HandshakeError> {
    let invalid = |detail: String| HandshakeError::Invalid {
        host: host.to_owned(),
        detail,
    };
    let handshake: Handshake = serde_json::from_slice(stdout)
        .map_err(|e| invalid(format!("not the expected JSON ({e})")))?;
    if handshake.handshake != FORMAT {
        return Err(invalid(format!(
            "format {} instead of {FORMAT}",
            handshake.handshake
        )));
    }
    match &handshake.daemon {
        DaemonState {
            running: true,
            daemon: None,
        } => return Err(invalid("a running daemon without its details".to_owned())),
        DaemonState {
            running: true,
            daemon: Some(daemon),
        } if daemon.port == 0 => return Err(invalid("the daemon reported port 0".to_owned())),
        _ => {}
    }
    Ok(handshake)
}

/// Run the handshake on the host with `server`; with `ensure_port`, a missing
/// daemon is started on `127.0.0.1:<port>` first.
pub async fn handshake<E: Executor>(
    executor: &E,
    server: &Server,
    ensure_port: Option<u16>,
) -> Result<Handshake, HandshakeError> {
    let mut line = server.command().word("remote-handshake").word("--json");
    if let Some(port) = ensure_port {
        line = line
            .word("--ensure-daemon")
            .word("--port")
            .value(&port.to_string());
    }
    let output = run_checked(executor, &line, None).await?;
    parse(executor.host(), &output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"{"handshake":1,
     "server":{"version":"0.1.0","protocol":1,"path":"/home/u/.local/share/splitty/remote/0.1.0/splitty"},
     "daemon":{"running":true,"pid":4711,"port":9811,"version":"0.1.0","protocol":1,"startedAt":"1760","started":true},
     "screens":[{"id":"s1","name":"main","panes":3},{"id":"s2","panes":1}],
     "later":"ignored"}"#;

    #[test]
    fn the_example_of_format_1_parses() {
        let handshake = parse("vm", EXAMPLE.as_bytes()).expect("handshake");
        let daemon = handshake.daemon.daemon.expect("running daemon");
        assert_eq!(
            (daemon.pid, daemon.port, daemon.started),
            (4711, 9811, true)
        );
        assert_eq!(daemon.started_at, "1760");
        assert!(
            !daemon.keeps_sessions,
            "a host that does not say keeps none"
        );
        assert_eq!(handshake.server.protocol, 1);
        assert_eq!(handshake.screens.len(), 2);
        assert_eq!(handshake.screens[1].name, None);
    }

    #[test]
    fn a_daemon_that_keeps_its_sessions_says_so() {
        let text = EXAMPLE.replace(
            r#""started":true"#,
            r#""keepsSessions":true,"started":true"#,
        );
        let handshake = parse("vm", text.as_bytes()).expect("handshake");
        let daemon = handshake.daemon.daemon.expect("running daemon");
        assert!(daemon.keeps_sessions);
        let round_trip = serde_json::to_value(&daemon).expect("serialize");
        assert_eq!(round_trip["keepsSessions"], true);
    }

    #[test]
    fn a_host_without_a_daemon_parses() {
        let text = r#"{"handshake":1,"server":{"version":"0.1.0","protocol":1,"path":"/p"},"daemon":{"running":false},"screens":[]}"#;
        let handshake = parse("vm", text.as_bytes()).expect("handshake");
        assert!(!handshake.daemon.running);
        assert!(handshake.daemon.daemon.is_none());
        let round_trip = serde_json::to_string(&handshake).expect("serialize");
        assert_eq!(round_trip, text);
    }

    #[test]
    fn inconsistent_answers_are_rejected() {
        for text in [
            r#"{"handshake":2,"server":{"version":"x","protocol":1,"path":"/p"},"daemon":{"running":false}}"#,
            r#"{"handshake":1,"server":{"version":"x","protocol":1,"path":"/p"},"daemon":{"running":true}}"#,
            r#"{"handshake":1,"server":{"version":"x","protocol":1,"path":"/p"},"daemon":{"running":true,"pid":1,"port":0,"version":"x","protocol":1,"startedAt":"1"}}"#,
            "Daemon running:",
        ] {
            assert!(
                matches!(
                    parse("vm", text.as_bytes()),
                    Err(HandshakeError::Invalid { .. })
                ),
                "{text}"
            );
        }
    }
}
