//! WebSocket subsystem for real-time client communication.
//!
//! Manages connected clients via `WsState`, which holds a map of `ClientEntry`
//! instances (each wrapping a `WsSender` and optional `MuxHandler`). Provides
//! broadcasting of commands and screen-specific nudges to connected clients.
//!
//! Sub-modules:
//! - [`codec`] — binary and JSON wire format encoding/decoding.
//! - [`editor`] and [`files`] — editor requests and file offers from pane
//!   processes to the windows attached to their session.
//! - [`handler`] — per-connection message loop (upgrade, dispatch, cleanup).
//! - [`mux_handler`] — handles `mux.*` protocol messages (op, resync).
//! - [`ports`] — `ports.changed`, the ports processes of the sessions listen on.
//! - [`sender`] — thread-safe WebSocket writer with channel-backed async drain.

pub mod codec;
pub mod editor;
pub mod files;
mod handler;
mod mux_handler;
pub mod ports;
mod sender;

pub use handler::handle_socket;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use tokio::sync::{broadcast, Mutex};

use splitty_core::ids::*;
use splitty_core::mux::effect_runner::{EffectRunner, EnvContext};
use splitty_core::mux::Store;
use splitty_core::protocol::dispatcher::Dispatcher;

use mux_handler::MuxHandler;
use sender::WsSender;

/// Shared WebSocket state, accessible from HTTP handlers for broadcasting.
pub struct WsState {
    pub editor_requests: Arc<editor::EditorRequests>,
    pub file_offers: Arc<files::FileOffers>,
    pub jobs: Arc<crate::jobs::Jobs>,
    pub clients: Mutex<HashMap<String, ClientEntry>>,
    pub mux_store: Arc<Store>,
    pub dispatcher: Arc<Dispatcher>,
    pub shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
    /// Executes side effects (PTY spawn/kill) produced by the mux reducer.
    pub effect_runner: Arc<EffectRunner>,
    /// Environment context injected into spawned sessions.
    pub env: EnvContext,
    /// Broadcast channel for server-generated notifications (serialized JSON).
    pub notify_tx: broadcast::Sender<String>,
    /// Server mode: "embedded" or "daemon".
    pub server_mode: String,
    /// The listening ports of the sessions, as the clients know them.
    pub ports: ports::PortWatch,
}

/// A connected WebSocket client.
pub struct ClientEntry {
    pub sender: Arc<WsSender>,
    pub mux_handler: Option<Arc<MuxHandler>>,
}

impl WsState {
    pub fn new(
        mux_store: Arc<Store>,
        dispatcher: Arc<Dispatcher>,
        shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
        effect_runner: Arc<EffectRunner>,
        env: EnvContext,
        notify_tx: broadcast::Sender<String>,
        server_mode: String,
    ) -> Self {
        let job_store = Arc::clone(&mux_store);
        dispatcher.set_job_resolver(Arc::new(move |pane_id| {
            job_store
                .job_for_pane(pane_id)
                .map(|(id, state)| (id.to_string(), state))
        }));
        let ports = ports::PortWatch::new(dispatcher.backend().clone());
        let jobs = crate::jobs::Jobs::new(
            mux_store.clone(),
            dispatcher.backend().clone(),
            effect_runner.clone(),
            env.clone(),
            notify_tx.clone(),
        );
        Self {
            editor_requests: Arc::new(editor::EditorRequests::default()),
            file_offers: Arc::new(files::FileOffers::default()),
            jobs,
            clients: Mutex::new(HashMap::new()),
            mux_store,
            dispatcher,
            shell_profiles,
            effect_runner,
            env,
            notify_tx,
            server_mode,
            ports,
        }
    }

    /// This state with the listening ports found by `ports` instead.
    pub fn with_ports(self, ports: ports::PortWatch) -> Self {
        Self { ports, ..self }
    }

    /// Broadcast a client command to all connected clients.
    /// Returns the number of clients the command was sent to.
    pub async fn broadcast_command(&self, command: splitty_core::protocol::ClientCommand) -> usize {
        let event = splitty_core::protocol::ServerEvent::command(command);
        let text = serde_json::to_string(&event).unwrap_or_default();

        self.broadcast_events(vec![text], None).await
    }

    /// Send a nudge to all connected clients for the given screen.
    pub async fn nudge_screen_clients(&self, reason: &str, screen_id: &ScreenId) {
        let event = splitty_core::protocol::ServerEvent::mux_nudge(
            reason.to_string(),
            Some(screen_id.to_string()),
        );
        let text = serde_json::to_string(&event).unwrap_or_default();

        self.broadcast_events(vec![text], None).await;
    }

    /// Deliver a batch of pre-serialized events to every connected client,
    /// optionally skipping one sender (the originating client, which has
    /// usually already been served directly).
    ///
    /// Ordering matters here: `mux.pane_spawned` must reach a client before the
    /// `mux.nudge` for the same op, otherwise the nudge triggers a snapshot
    /// rebuild that tears down the pane still waiting to be told its session id.
    /// Each client therefore gets a single task that awaits the sends in
    /// sequence, rather than one task per message.
    ///
    /// Returns the number of clients the batch was dispatched to.
    pub async fn broadcast_events(
        &self,
        texts: Vec<String>,
        except: Option<&Arc<WsSender>>,
    ) -> usize {
        if texts.is_empty() {
            return 0;
        }

        let clients = self.clients.lock().await;
        let mut sent = 0;
        for entry in clients.values() {
            if except.is_some_and(|s| Arc::ptr_eq(s, &entry.sender)) {
                continue;
            }
            let sender = entry.sender.clone();
            let texts = texts.clone();
            tokio::spawn(async move {
                for text in &texts {
                    if sender.send_text(text).await.is_err() {
                        break;
                    }
                }
            });
            sent += 1;
        }
        sent
    }
}
