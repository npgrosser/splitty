//! Server discovery: find a running Splitty server to talk to.
//!
//! Resolution order:
//! 1. A nonempty `SPLITTY_AGENT_SOCK`, exclusively.
//! 2. `SPLITTY_SOCK` (set inside a Splitty-spawned shell).
//! 3. The `socket` recorded in the daemon info file of a running daemon.
//!
//! The first socket named is the one used: an unreachable socket never
//! falls back to the next source.

use splitty_core::daemon;

/// Connection info for a running Splitty server.
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub socket: String,
    pub source: &'static str,
}

/// Discover a running Splitty server.
pub fn discover() -> Result<ServerInfo, super::CliError> {
    let agent = match std::env::var("SPLITTY_AGENT_SOCK") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(super::CliError::Usage(
                "agent socket path is not valid UTF-8; owner discovery is forbidden".into(),
            ))
        }
    };
    discover_with(agent, std::env::var("SPLITTY_SOCK").ok(), || {
        daemon::read_info()
            .filter(|info| daemon::is_running(info.pid) && !info.socket.is_empty())
            .map(|info| info.socket)
    })
}

fn discover_with(
    agent: Option<String>,
    owner: Option<String>,
    daemon: impl FnOnce() -> Option<String>,
) -> Result<ServerInfo, super::CliError> {
    if let Some(socket) = agent {
        if !socket.is_empty() {
            return Ok(ServerInfo {
                socket,
                source: "agent",
            });
        }
    }
    // 2. Control socket, set when running inside a Splitty-spawned shell.
    if let Some(socket) = owner {
        if !socket.is_empty() {
            return Ok(ServerInfo {
                socket,
                source: "env",
            });
        }
    }

    // 3. Daemon info file.
    if let Some(socket) = daemon() {
        return Ok(ServerInfo {
            socket,
            source: "daemon",
        });
    }

    Err(super::CliError::NoServer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_precedence_is_fail_closed_without_reading_daemon_files_early() {
        let no_file = || panic!("must not consult daemon info");
        let scoped = discover_with(
            Some("unreachable-agent".into()),
            Some("owner".into()),
            no_file,
        )
        .expect("scoped route");
        assert_eq!(scoped.socket, "unreachable-agent");
        assert_eq!(scoped.source, "agent");
        assert_eq!(
            discover_with(Some("".into()), Some("owner".into()), no_file)
                .expect("owner")
                .source,
            "env"
        );
        assert_eq!(
            discover_with(None, Some("".into()), || Some("daemon".into()))
                .expect("daemon")
                .source,
            "daemon"
        );
        assert!(matches!(
            discover_with(None, None, || None),
            Err(super::super::CliError::NoServer)
        ));
    }
}
