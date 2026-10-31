//! `ports.changed` (mux-protocol, Listening Ports): the TCP ports processes of
//! this server's sessions listen on, sent to a client right after its
//! `mux.welcome` and to every client whenever the list changes.
//!
//! The scan (`splitty_core::pty::ports`) runs every [`INTERVAL`] while at least
//! one client is connected, on a blocking thread since it reads `/proc`. A
//! client that connects gets the list the others hold, so all clients agree;
//! only when no scan runs yet is one made for it. The list is sent even when
//! it is empty, so a client knows the server reports ports. On systems
//! without the scan nothing is sent.

use std::sync::Arc;
use std::time::Duration;

use splitty_core::protocol::ServerEvent;
use splitty_core::pty::ports::{self, ListeningPort};
use splitty_core::pty::Backend;
use tokio::sync::Mutex;

use super::sender::WsSender;
use super::WsState;

/// How often the ports are checked while clients are connected.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// How the ports are found: the scan over the sessions of the server's PTY
/// backend, or a list a test sets. Blocking.
pub type Scan = Arc<dyn Fn() -> Vec<ListeningPort> + Send + Sync>;

/// The listening ports of one server's sessions, as its clients know them.
pub struct PortWatch {
    /// Whether ports are reported at all.
    enabled: bool,
    scan: Scan,
    state: Mutex<Watching>,
}

#[derive(Default)]
struct Watching {
    /// Whether the task that checks every [`INTERVAL`] runs.
    running: bool,
    /// The list every client holds.
    sent: Vec<ListeningPort>,
}

impl PortWatch {
    /// The ports of `backend`'s sessions, on systems with the scan.
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self::with_scan(
            ports::SUPPORTED,
            Arc::new(move || ports::listening(backend.as_ref())),
        )
    }

    pub fn with_scan(enabled: bool, scan: Scan) -> Self {
        Self {
            enabled,
            scan,
            state: Mutex::default(),
        }
    }

    async fn scan(&self) -> Vec<ListeningPort> {
        let scan = self.scan.clone();
        match tokio::task::spawn_blocking(move || scan()).await {
            Ok(ports) => ports,
            Err(error) => {
                tracing::warn!("the port scan failed: {error}");
                Vec::new()
            }
        }
    }
}

/// Send `sender`, a client that just got its `mux.welcome`, the current list,
/// and keep checking while clients are connected. The client must be
/// registered already, so the check that stops without clients sees it.
pub(super) async fn greet(ws_state: &Arc<WsState>, sender: &WsSender) {
    let watch = &ws_state.ports;
    if !watch.enabled {
        return;
    }
    let mut state = watch.state.lock().await;
    if !state.running {
        state.sent = watch.scan().await;
        state.running = true;
        tokio::spawn(check(ws_state.clone()));
    }
    let _ = sender
        .send_event(&ServerEvent::ports_changed(state.sent.clone()))
        .await;
}

/// Check every [`INTERVAL`] until no client is connected, and send every
/// client the list when it changed.
async fn check(ws_state: Arc<WsState>) {
    let watch = &ws_state.ports;
    loop {
        tokio::time::sleep(INTERVAL).await;
        // Under the watch's lock: a client that connects meanwhile finds this
        // task running, or starts the next one.
        let mut state = watch.state.lock().await;
        if ws_state.clients.lock().await.is_empty() {
            state.running = false;
            return;
        }
        let ports = watch.scan().await;
        if ports == state.sent {
            continue;
        }
        state.sent = ports;
        let text = match serde_json::to_string(&ServerEvent::ports_changed(state.sent.clone())) {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!("cannot encode ports.changed: {error}");
                continue;
            }
        };
        // In order per client: a sender only queues the message.
        for entry in ws_state.clients.lock().await.values() {
            let _ = entry.sender.send_text(&text).await;
        }
    }
}
