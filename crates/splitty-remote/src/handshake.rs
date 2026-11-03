//! The handshake: what a host tells a client in one SSH call, the host's
//! server, its daemon (started on request) and its screens.
//!
//! The host side is `splitty remote-handshake --json` in the `splitty`
//! binary; [`handshake`] runs it and reads the answer.

use serde::{Deserialize, Serialize};
use splitty_transport::{
    budget::{Charged, ConnectionBudget, Lease},
    codec,
};

use crate::install::Server;
use crate::ssh::{run_checked, Executor, SshError};

/// The format version of [`Handshake`].
pub const FORMAT: u32 = 2;

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
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub version: String,
    pub protocol: u32,
    pub path: String,
    pub transport_versions: Vec<u16>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_version: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub legacy: bool,
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
    Invalid {
        host: String,
        detail: String,
        allocation: Option<Box<Lease>>,
    },
}

/// Parse the handshake's output.
#[cfg(test)]
fn parse(host: &str, stdout: &[u8]) -> Result<Handshake, HandshakeError> {
    parse_budgeted(host, stdout, &ConnectionBudget::default()).map(|value| value.into_parts().0)
}
pub fn parse_budgeted(
    host: &str,
    stdout: &[u8],
    budget: &ConnectionBudget,
) -> Result<Charged<Handshake>, HandshakeError> {
    let bytes = codec::json_typed_allocation_bound(stdout)
        .map_err(|error| HandshakeError::Ssh(SshError::Spawn(error)))?
        .checked_add(4 * host.len())
        .ok_or_else(|| {
            HandshakeError::Ssh(SshError::Spawn(std::io::Error::other(
                "handshake capacity overflow",
            )))
        })?;
    let mut lease = Some(budget.reserve(bytes, false).map_err(|error| {
        HandshakeError::Ssh(SshError::Spawn(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            error,
        )))
    })?);
    let mut invalid = |detail: String| HandshakeError::Invalid {
        host: host.to_owned(),
        detail,
        allocation: lease.take().map(Box::new),
    };
    let handshake: Handshake = serde_json::from_slice(stdout)
        .map_err(|e| invalid(format!("not the expected JSON ({e})")))?;
    if handshake.handshake != FORMAT {
        return Err(invalid(format!(
            "format {} instead of {FORMAT}",
            handshake.handshake
        )));
    }
    if !handshake.server.path.starts_with('/')
        || handshake.server.path.contains('\0')
        || handshake.server.transport_versions.is_empty()
        || handshake.server.transport_versions.len() > 16
        || handshake.server.transport_versions.contains(&0)
    {
        return Err(invalid(
            "invalid answering server identity or transport capabilities".into(),
        ));
    }
    match &handshake.daemon {
        DaemonState {
            running: true,
            daemon: Some(daemon),
        } => {
            if daemon.pid == 0 || i32::try_from(daemon.pid).is_err() {
                return Err(invalid("invalid daemon PID".into()));
            }
            if daemon.legacy {
                if daemon.instance.is_some() || daemon.transport_version.is_some() || daemon.started
                {
                    return Err(invalid(
                        "legacy daemon claims native service identity".into(),
                    ));
                }
            } else if !daemon.instance.as_deref().is_some_and(valid_instance)
                || !daemon.transport_version.is_some_and(|version| version > 0)
            {
                return Err(invalid(
                    "native daemon lacks a valid instance or transport version".into(),
                ));
            }
        }
        DaemonState {
            running: false,
            daemon: None,
        } if handshake.screens.is_empty() => {}
        _ => return Err(invalid("inconsistent daemon state or screens".into())),
    }
    Ok(lease
        .take()
        .expect("handshake reservation")
        .charge(handshake))
}

fn valid_instance(instance: &str) -> bool {
    instance.len() == 32
        && instance
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl RunningDaemon {
    pub fn supports_native(&self) -> bool {
        !self.legacy
            && self.transport_version == Some(u32::from(splitty_transport::wire::VERSION))
            && self.instance.as_deref().is_some_and(valid_instance)
    }
    pub fn supports_mux(&self, current: u32) -> bool {
        self.protocol == current || current.checked_sub(1) == Some(self.protocol)
    }
}

/// Request format 2. Ensuring a daemon never asks it to bind a TCP port or
/// replace a running process; update policy belongs to the connector.
pub async fn handshake<E: Executor>(
    executor: &E,
    server: &Server,
    ensure_daemon: bool,
) -> Result<Charged<Handshake>, HandshakeError> {
    let mut line = server.command().word("remote-handshake").word("--json");
    if ensure_daemon {
        line = line.word("--ensure-daemon");
    }
    let output = run_checked(executor, &line, None).await?;
    parse_budgeted(executor.host(), &output.stdout, &executor.budget())
}

#[cfg(test)]
mod tests {
    use super::*;
    const EXAMPLE: &str = r#"{"handshake":2,"server":{"version":"0.1.30","protocol":1,"path":"/home/me/.local/share/splitty/remote/0.1.30/splitty","transportVersions":[1]},"daemon":{"running":true,"pid":4711,"instance":"0123456789abcdef0123456789abcdef","transportVersion":1,"version":"0.1.29","protocol":1,"startedAt":"1760","keepsSessions":true,"started":false,"legacy":false},"screens":[{"id":"s1","name":"main","panes":3},{"id":"s2","panes":1}],"later":"ignored"}"#;

    #[tokio::test]
    async fn typed_metadata_and_invalid_diagnostics_keep_their_owner_leases() {
        let budget = ConnectionBudget::default();
        let answer = parse_budgeted("host", EXAMPLE.as_bytes(), &budget).unwrap();
        assert_eq!(answer.screens.len(), 2);
        assert!(budget.used() > 0);
        let (started, waiting) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _answer = answer;
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        waiting.await.unwrap();
        assert!(
            budget.used() > 0,
            "metadata remains owned across suspension"
        );
        task.abort();
        let _ = task.await;
        assert_eq!(budget.used(), 0);
        let error = parse_budgeted("host", br#"{"handshake":"wrong type"}"#, &budget).unwrap_err();
        assert!(matches!(
            &error,
            HandshakeError::Invalid {
                allocation: Some(_),
                ..
            }
        ));
        assert!(
            budget.used() > 0,
            "serde-derived error keeps parser/formatting capacity"
        );
        drop(error);
        assert_eq!(budget.used(), 0);
        let held = budget
            .reserve(
                splitty_transport::wire::CONNECTION_BYTES
                    - splitty_transport::wire::CONNECTION_CONTROL_BYTES,
                false,
            )
            .unwrap();
        assert!(
            matches!(parse_budgeted("host", EXAMPLE.as_bytes(), &budget), Err(HandshakeError::Ssh(SshError::Spawn(error))) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        drop(held);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn format_two_separates_the_answering_binary_and_the_running_service() {
        let answer = parse("host", EXAMPLE.as_bytes()).unwrap();
        assert_eq!(answer.server.version, "0.1.30");
        assert_eq!(answer.server.transport_versions, [1]);
        let daemon = answer.daemon.daemon.as_ref().unwrap();
        assert_eq!(daemon.version, "0.1.29");
        assert!(daemon.keeps_sessions);
        assert!(daemon.supports_native());
        assert!(daemon.supports_mux(1));
        assert_eq!(answer.screens.len(), 2);
        let json = serde_json::to_value(&answer).unwrap();
        assert!(json["daemon"].get("port").is_none());
        assert!(json["daemon"].get("clientSocket").is_none());
    }

    #[test]
    fn absent_and_legacy_daemons_do_not_acquire_native_capabilities() {
        let mut value: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
        value["daemon"] = serde_json::json!({"running":false});
        value["screens"] = serde_json::json!([]);
        let absent = parse("host", &serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(absent.daemon.daemon.is_none());
        value["daemon"] = serde_json::json!({"running":true,"pid":4711,"version":"0.1.0","protocol":1,"startedAt":"1","legacy":true,"port":9811});
        let legacy = parse("host", &serde_json::to_vec(&value).unwrap())
            .unwrap()
            .daemon
            .daemon
            .unwrap();
        assert!(legacy.legacy);
        assert!(!legacy.keeps_sessions);
        assert!(!legacy.supports_native());
        assert!(
            legacy.supports_mux(1),
            "mux compatibility never authorizes a TCP fallback"
        );
    }

    #[test]
    fn mux_and_transport_support_are_independent() {
        let mut daemon = parse("host", EXAMPLE.as_bytes())
            .unwrap()
            .daemon
            .daemon
            .unwrap();
        daemon.protocol = 2;
        assert!(daemon.supports_native());
        assert!(!daemon.supports_mux(1));
        daemon.protocol = 0;
        assert!(daemon.supports_mux(1));
        daemon.transport_version = Some(999);
        assert!(!daemon.supports_native());
        assert!(daemon.supports_mux(1));
    }

    #[test]
    fn inconsistent_or_legacy_format_answers_are_refused() {
        let base: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
        for (pointer, replacement) in [
            ("/handshake", serde_json::json!(1)),
            ("/daemon/instance", serde_json::json!("bad")),
            ("/daemon/pid", serde_json::json!(0)),
            ("/daemon/transportVersion", serde_json::json!(0)),
            ("/daemon/legacy", serde_json::json!(true)),
            ("/daemon/running", serde_json::json!(false)),
            ("/server/path", serde_json::json!("relative")),
            ("/server/transportVersions", serde_json::json!([])),
        ] {
            let mut value = base.clone();
            *value.pointer_mut(pointer).unwrap() = replacement;
            assert!(
                parse("host", &serde_json::to_vec(&value).unwrap()).is_err(),
                "{pointer}"
            );
        }
        assert!(parse("host", b"Daemon running:").is_err());
    }
}
