//! What the `splitty` binary does on a remote host for a client that reaches
//! it over SSH: the handshake (`remote-handshake`) and the bridge
//! (`remote-bridge`). Both run as the SSH user and touch only that user's
//! daemon, over loopback or its owner-only control socket.

pub mod bridge;
pub mod handshake;

use splitty_core::daemon::{self, DaemonInfo};

/// The running daemon, after removing a stale info file.
fn running_daemon() -> Option<DaemonInfo> {
    let info = daemon::read_info()?;
    if daemon::is_running(info.pid) {
        Some(info)
    } else {
        daemon::remove_info();
        None
    }
}

/// How the local CLI reaches this daemon: its control socket, or loopback TCP
/// with the token file when it runs without one.
fn reach(info: &DaemonInfo) -> Result<crate::cli::ServerInfo, String> {
    use crate::cli::{ServerInfo, Transport};

    let transport = if info.socket.is_empty() {
        let (token, _) = splitty_core::auth::resolve("").map_err(|e| e.to_string())?;
        Transport::Tcp {
            addr: format!("127.0.0.1:{}", info.port),
            token,
        }
    } else {
        Transport::LocalSocket(info.socket.clone())
    };
    Ok(ServerInfo {
        transport,
        source: "daemon",
    })
}
