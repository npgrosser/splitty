//! `splitty remote-handshake --json [--ensure-daemon]`: this
//! host's server, its daemon and its screens, for a client, in one call.

use splitty_remote::handshake::{
    DaemonState, Handshake, RunningDaemon, Screen, ServerInfo, FORMAT,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("daemon changed or disconnected during discovery; refresh discovery")]
    DiscoveryChanged,
    #[error("{0}")]
    Failed(String),
}
impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}
impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Failed(message.into())
    }
}
impl Error {
    pub fn exit_code(&self) -> i32 {
        if matches!(self, Self::DiscoveryChanged) {
            75
        } else {
            1
        }
    }
    fn native(error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        if splitty_transport::negotiation::stale_instance(&error)
            || matches!(
                error.kind(),
                ErrorKind::NotFound
                    | ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::BrokenPipe
                    | ErrorKind::TimedOut
            )
        {
            Self::DiscoveryChanged
        } else {
            Self::Failed(error.to_string())
        }
    }
}

/// Report the answering binary and daemon separately. Ensuring a missing
/// daemon creates only its private endpoint; a live daemon is never replaced.
pub async fn run(ensure_daemon: bool) -> Result<Handshake, Error> {
    let executable =
        std::env::current_exe().map_err(|e| format!("cannot locate the splitty binary: {e}"))?;
    let mut info = super::running_daemon()?;
    let mut started = false;
    if info.is_none() && ensure_daemon {
        start_daemon(&executable).await?;
        info = super::running_daemon()?;
        if info.is_none() {
            return Err(Error::DiscoveryChanged);
        }
        started = true;
    }

    let screens = match &info {
        Some(info)
            if !info.legacy()
                && info.transport_version == Some(u32::from(splitty_transport::wire::VERSION))
                && (info.protocol == splitty_core::protocol::PROTOCOL_VERSION
                    || splitty_core::protocol::PROTOCOL_VERSION.checked_sub(1)
                        == Some(info.protocol)) =>
        {
            screens(info).await?
        }
        Some(_) => Vec::new(),
        None => Vec::new(),
    };
    Ok(Handshake {
        handshake: FORMAT,
        server: ServerInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: splitty_core::protocol::PROTOCOL_VERSION,
            path: executable.display().to_string(),
            transport_versions: vec![splitty_transport::wire::VERSION],
        },
        daemon: DaemonState {
            running: info.is_some(),
            daemon: info.map(|info| RunningDaemon {
                legacy: info.legacy(),
                pid: info.pid,
                instance: info.instance,
                transport_version: info.transport_version,
                version: info.version,
                protocol: info.protocol,
                started_at: info.started_at,
                keeps_sessions: info.keeps_sessions,
                started,
            }),
        },
        screens,
    })
}

/// `daemon start` of this very binary, so its readiness wait and its errors
/// apply unchanged; its output stays off the handshake's.
async fn start_daemon(executable: &std::path::Path) -> Result<(), Error> {
    let mut command = tokio::process::Command::new(executable);
    command.args(["daemon", "start"]);
    let output = splitty_remote::ssh::run_with_input(command, None)
        .await
        .map_err(|e| format!("cannot run daemon start: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    if output.status.code() == Some(75) {
        return Err(Error::DiscoveryChanged);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reason = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    Err(reason.trim().to_owned().into())
}

#[cfg(unix)]
async fn screens(info: &splitty_core::daemon::DaemonInfo) -> Result<Vec<Screen>, Error> {
    let (value, _decoded): (serde_json::Value, _) = {
        let connector = crate::native::client::Connector::default();
        let mut client = connector.local(info).await.map_err(Error::native)?;
        let result = client
            .request(
                crate::service::catalog::Operation::MuxScreens,
                serde_json::Map::new(),
            )
            .await;
        let budget = client.sender().context().budget;
        client.shutdown().await;
        let reply = result.map_err(Error::native)?;
        if reply.status != 200 {
            return Err(format!("the daemon could not list screens: {}", reply.body).into());
        }
        let lease = splitty_transport::codec::reserve_json(reply.body.as_bytes(), &budget, false)
            .map_err(Error::native)?;
        let value =
            serde_json::from_str(&reply.body).map_err(|e| format!("invalid screen list: {e}"))?;
        (value, lease)
    };
    let list = value
        .as_array()
        .ok_or("the daemon's screen list is not a list")?;
    Ok(list
        .iter()
        .filter_map(|screen| {
            Some(Screen {
                id: screen["id"].as_str()?.to_owned(),
                name: screen["name"].as_str().map(str::to_owned),
                panes: screen["paneCount"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0),
            })
        })
        .collect())
}

#[cfg(not(unix))]
async fn screens(_info: &splitty_core::daemon::DaemonInfo) -> Result<Vec<Screen>, Error> {
    Err("native daemon discovery requires Unix".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_races_remain_retryable_but_invalid_ownership_and_protocol_do_not() {
        use std::io::{Error as IoError, ErrorKind};
        assert_eq!(
            Error::native(IoError::new(
                ErrorKind::InvalidData,
                splitty_transport::negotiation::StaleInstance
            ))
            .exit_code(),
            75
        );
        for kind in [
            ErrorKind::NotFound,
            ErrorKind::ConnectionRefused,
            ErrorKind::ConnectionAborted,
            ErrorKind::UnexpectedEof,
        ] {
            assert_eq!(Error::native(IoError::from(kind)).exit_code(), 75);
        }
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidData,
            ErrorKind::Unsupported,
        ] {
            assert_eq!(Error::native(IoError::from(kind)).exit_code(), 1);
        }
    }
}
