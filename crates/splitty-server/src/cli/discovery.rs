//! Server discovery: find a running Splitty server to talk to.
//!
//! Resolution order:
//! 0. A nonempty `SPLITTY_AGENT_SOCK`, exclusively (TCP overrides are refused).
//! 1. Explicit `--addr` / `--token` CLI flags (TCP, for reaching a server
//!    deliberately, e.g. across an SSH tunnel)
//! 2. `SPLITTY_SOCK` (set inside a Splitty-spawned shell)
//! 3. The `socket` recorded in `~/.config/splitty/daemon.json`
//!
//! The `SPLITTY_SERVER` + `SPLITTY_TOKEN` pair is gone from this list
//! (security review SR-05): the credential it carried was
//! exfiltratable and authorised the whole API. Local callers now go through the
//! control socket, which is authorised by filesystem permissions and exposes
//! only multiplexer operations.

use splitty_core::daemon;

/// How to reach a running Splitty server.
#[derive(Clone, Debug)]
pub enum Transport {
    /// A Unix socket — local, authorised by filesystem permissions.
    LocalSocket(String),
    /// A TCP address with a bearer token, used only for explicitly targeted
    /// servers (`--addr` + `--token`).
    Tcp { addr: String, token: String },
}

/// Connection info for a running Splitty server.
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub transport: Transport,
    pub source: &'static str,
}

/// Discover a running Splitty server.
pub fn discover(
    flag_addr: Option<&str>,
    flag_token: Option<&str>,
) -> Result<ServerInfo, super::CliError> {
    let agent = match std::env::var("SPLITTY_AGENT_SOCK") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(super::CliError::Usage(
                "agent socket path is not valid UTF-8; owner discovery is forbidden".into(),
            ))
        }
    };
    discover_with(
        flag_addr,
        flag_token,
        agent,
        std::env::var("SPLITTY_SOCK").ok(),
        || {
            daemon::read_info()
                .filter(|info| daemon::is_running(info.pid) && !info.socket.is_empty())
                .map(|info| info.socket)
        },
    )
}

fn discover_with(
    flag_addr: Option<&str>,
    flag_token: Option<&str>,
    agent: Option<String>,
    owner: Option<String>,
    daemon: impl FnOnce() -> Option<String>,
) -> Result<ServerInfo, super::CliError> {
    if let Some(socket) = agent {
        if !socket.is_empty() {
            if flag_addr.is_some() || flag_token.is_some() {
                return Err(super::CliError::Usage(
                    "TCP overrides are forbidden under an agent grant".into(),
                ));
            }
            return Ok(ServerInfo {
                transport: Transport::LocalSocket(socket),
                source: "agent",
            });
        }
    }
    if (flag_addr.is_some() || flag_token.is_some())
        && !(flag_addr.is_some_and(|a| !a.is_empty()) && flag_token.is_some_and(|t| !t.is_empty()))
    {
        return Err(super::CliError::Usage(
            "--addr and --token must both be non-empty".into(),
        ));
    }
    // 1. Explicit flags. Kept for reaching a specific server on purpose — for
    //    instance through an SSH tunnel, where there is no shared filesystem to
    //    authorise against.
    if let (Some(addr), Some(token)) = (flag_addr, flag_token) {
        if !addr.is_empty() && !token.is_empty() {
            return Ok(ServerInfo {
                transport: Transport::Tcp {
                    addr: addr.to_string(),
                    token: token.to_string(),
                },
                source: "flags",
            });
        }
    }

    // 2. Control socket, set when running inside a Splitty-spawned shell.
    if let Some(socket) = owner {
        if !socket.is_empty() {
            return Ok(ServerInfo {
                transport: Transport::LocalSocket(socket),
                source: "env",
            });
        }
    }

    // 3. Daemon info file.
    if let Some(socket) = daemon() {
        return Ok(ServerInfo {
            transport: Transport::LocalSocket(socket),
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
        for (addr, token) in [
            (Some("host:1"), None),
            (None, Some("token")),
            (Some(""), Some("token")),
            (Some("host:1"), Some("")),
        ] {
            assert!(matches!(
                discover_with(addr, token, None, Some("owner".into()), no_file),
                Err(super::super::CliError::Usage(_))
            ));
        }
        assert!(discover_with(
            Some("host:1"),
            Some("token"),
            Some("agent".into()),
            Some("owner".into()),
            no_file
        )
        .is_err());
        let scoped = discover_with(
            None,
            None,
            Some("unreachable-agent".into()),
            Some("owner".into()),
            no_file,
        )
        .expect("scoped route");
        assert!(
            matches!(scoped.transport,Transport::LocalSocket(path) if path=="unreachable-agent")
        );
        assert_eq!(scoped.source, "agent");
        let explicit = discover_with(
            Some("host:1"),
            Some("token"),
            None,
            Some("owner".into()),
            no_file,
        )
        .expect("flags");
        assert!(
            matches!(explicit.transport,Transport::Tcp{addr,token} if addr=="host:1" && token=="token")
        );
        assert_eq!(
            discover_with(None, None, Some("".into()), Some("owner".into()), no_file)
                .expect("owner")
                .source,
            "env"
        );
        assert_eq!(
            discover_with(None, None, None, Some("".into()), || Some("daemon".into()))
                .expect("daemon")
                .source,
            "daemon"
        );
        assert!(matches!(
            discover_with(None, None, None, None, || None),
            Err(super::super::CliError::NoServer)
        ));
    }
}
