//! What the `splitty` binary does on a remote host for a client that reaches
//! it over SSH: the handshake (`remote-handshake`), the bridge
//! (`remote-bridge`), explicit legacy migration (`remote-migrate`), and the
//! relay of a reverse forward (`remote-relay`). They run as the SSH user and
//! reach only that user's verified private daemon endpoints, keeper greetings
//! and explicitly selected process generation, or the relay's own socket.

pub mod bridge;
pub mod handshake;
pub mod migration;
pub mod relay;

use splitty_core::daemon::{self, DaemonInfo};

/// Read-only discovery. A stale observation must never remove a replacement's
/// record; only the instance-locked daemon lifecycle may rewrite discovery.
fn running_daemon() -> Result<Option<DaemonInfo>, String> {
    let info = daemon::read_info_checked()
        .map_err(|error| format!("cannot verify daemon discovery: {error}"))?;
    match info {
        Some(info) if daemon::is_running(info.pid) => Ok(Some(info)),
        Some(_) => Ok(None),
        None => Ok(None),
    }
}
