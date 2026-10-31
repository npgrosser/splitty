//! `splitty remote-handshake --json [--ensure-daemon --port <port>]`: this
//! host's server, its daemon and its screens, for a client, in one call.

use splitty_remote::handshake::{
    DaemonState, Handshake, RunningDaemon, Screen, ServerInfo, FORMAT,
};

/// The handshake, starting a missing daemon on `127.0.0.1:<ensure_port>`
/// first when asked. The error is the reason to print.
pub async fn run(ensure_port: Option<u16>) -> Result<Handshake, String> {
    let executable =
        std::env::current_exe().map_err(|e| format!("cannot locate the splitty binary: {e}"))?;
    let mut info = super::running_daemon();
    let mut started = false;
    if let (None, Some(port)) = (&info, ensure_port) {
        start_daemon(&executable, port).await?;
        info = super::running_daemon();
        if info.is_none() {
            return Err("the daemon reported ready but is not running".to_owned());
        }
        started = true;
    }

    let screens = match &info {
        Some(info) => screens(info).await?,
        None => Vec::new(),
    };
    Ok(Handshake {
        handshake: FORMAT,
        server: ServerInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: splitty_core::protocol::PROTOCOL_VERSION,
            path: executable.display().to_string(),
        },
        daemon: DaemonState {
            running: info.is_some(),
            daemon: info.map(|info| RunningDaemon {
                pid: info.pid,
                port: info.port,
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
async fn start_daemon(executable: &std::path::Path, port: u16) -> Result<(), String> {
    let output = tokio::process::Command::new(executable)
        .args(["daemon", "start", "--addr", &format!("127.0.0.1:{port}")])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("cannot run daemon start: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reason = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    Err(reason.trim().to_owned())
}

async fn screens(info: &splitty_core::daemon::DaemonInfo) -> Result<Vec<Screen>, String> {
    let server = super::reach(info)?;
    let value = crate::cli::api_get(&server, "/api/mux/screens")
        .await
        .map_err(|e| format!("the daemon did not list its screens: {e}"))?;
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
