//! Common client/session implementation, independent of a transport carrier.
//!
//! Manages connected clients via `ClientState`, which holds a map of `ClientEntry`
//! instances (each wrapping a `ClientSender` and optional `MuxHandler`). Provides
//! broadcasting of commands and screen-specific nudges to connected clients.
//!
//! Sub-modules:
//! - [`codec`] — binary and JSON wire format encoding/decoding.
//! - [`editor`] and [`files`] — editor requests and file offers from pane
//!   processes to the windows attached to their session.
//! - [`handler`] — per-connection message loop (upgrade, dispatch, cleanup).
//! - [`mux_handler`] — handles `mux.*` protocol messages (op, resync).
//! - [`ports`] — `ports.changed`, the ports processes of the sessions listen on.
//! - [`reviews`] — waits for a review of a viewer pane and the windows'
//!   answers to them.
//! - [`sender`] — ordered client sender.
//! - [`viewer`] — `viewer.read`, the documents of viewer panes.

pub mod codec;
pub mod editor;
pub mod files;
mod handler;
mod mux_handler;
pub mod ports;
pub mod reviews;
pub mod sender;
pub mod viewer;

pub use connection::{connect, connect_admitted, connect_with, Connection, Input, Message};
mod connection;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

use splitty_core::protocol::{notify::NotificationBus, ServerEvent};
use std::sync::Mutex;

use splitty_core::ids::*;
use splitty_core::mux::effect_runner::{EffectRunner, EnvContext};
use splitty_core::mux::Store;
use splitty_core::protocol::dispatcher::Dispatcher;

use mux_handler::MuxHandler;
use sender::ClientSender;

/// Shared session state, used by service operations and every client adapter.
pub struct ClientState {
    pub hub: Option<Arc<crate::hub::Hub>>,
    pub admission: Arc<splitty_transport::admission::Backend>,
    pub editor_requests: Arc<editor::EditorRequests>,
    pub file_offers: Arc<files::FileOffers>,
    /// The processes waiting for a review of a viewer pane.
    pub reviews: Arc<reviews::Reviews>,
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
    pub notify_tx: NotificationBus,
    /// Server mode: "embedded" or "daemon".
    pub server_mode: String,
    /// The listening ports of the sessions, as the clients know them.
    pub ports: ports::PortWatch,
}

/// An independently connected service client.
pub struct ClientEntry {
    pub sender: Arc<ClientSender>,
    pub notifications: Arc<ClientSender>,
    pub mux_handler: Option<Arc<MuxHandler>>,
}

impl ClientState {
    pub fn new(
        mux_store: Arc<Store>,
        dispatcher: Arc<Dispatcher>,
        shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
        effect_runner: Arc<EffectRunner>,
        env: EnvContext,
        notify_tx: NotificationBus,
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
            hub: None,
            admission: splitty_transport::admission::Backend::new(),
            editor_requests: Arc::new(editor::EditorRequests::default()),
            file_offers: Arc::new(files::FileOffers::default()),
            reviews: Arc::new(reviews::Reviews::default()),
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

    pub fn with_hub(self, hub: Arc<crate::hub::Hub>) -> Self {
        Self {
            hub: Some(hub),
            ..self
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
        self.broadcast_events(&[event], None)
    }

    /// Send a nudge to all connected clients for the given screen.
    pub async fn nudge_screen_clients(&self, reason: &str, screen_id: &ScreenId) {
        let event = splitty_core::protocol::ServerEvent::mux_nudge(
            reason.to_string(),
            Some(screen_id.to_string()),
        );
        self.broadcast_events(&[event], None);
    }

    /// Deliver a batch of typed events to every connected client,
    /// optionally skipping one sender (the originating client, which has
    /// usually already been served directly).
    ///
    /// Ordering matters here: `mux.pane_spawned` must reach a client before the
    /// `mux.nudge` for the same op, otherwise the nudge triggers a snapshot
    /// rebuild that tears down the pane still waiting to be told its session id.
    /// Enqueue the batch synchronously under the registry lock, in order.
    /// No per-event tasks or extra sender queues are created.
    ///
    /// Returns the number of clients the batch was dispatched to.
    pub fn broadcast_events(
        &self,
        events: &[ServerEvent],
        except: Option<&Arc<ClientSender>>,
    ) -> usize {
        if events.is_empty() {
            return 0;
        }

        let clients = self.clients.lock().expect("client registry");
        let mut sent = 0;
        for entry in clients.values() {
            if except.is_some_and(|s| Arc::ptr_eq(s, &entry.sender)) {
                continue;
            }
            if events
                .iter()
                .all(|event| entry.notifications.try_send_json(event).is_ok())
            {
                sent += 1;
            }
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn foreign_broadcasts_wait_for_welcome_and_preserve_batch_order_and_origin_skip() {
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let notifications = NotificationBus::default();
        let state = ClientState::new(
            Arc::new(Store::new()),
            Arc::new(Dispatcher::new(backend.clone())),
            Arc::new(RwLock::new(Default::default())),
            Arc::new(EffectRunner::new(backend)),
            EnvContext::default(),
            notifications.clone(),
            "embedded".into(),
        );
        let context = splitty_transport::queue::Context::default();
        let (sender, mut output) = ClientSender::with_context(context.clone());
        let sender = Arc::new(sender);
        let (pending, mut input) = ClientSender::with_context(context.clone());
        let pending = Arc::new(pending);
        let _subscription = notifications.subscribe(pending.clone());
        state.clients.lock().unwrap().insert(
            "starting".into(),
            ClientEntry {
                sender: sender.clone(),
                notifications: pending,
                mux_handler: None,
            },
        );
        let batch = [
            ServerEvent::mux_pane_spawned("pane", "session"),
            ServerEvent::mux_nudge("mux_op".into(), None),
            ServerEvent::settings_changed(serde_json::json!({"drawer":{"sidebarWidth":280}})),
        ];
        assert_eq!(state.broadcast_events(&batch, Some(&sender)), 0);
        assert!(input.try_recv().is_err());
        assert_eq!(state.broadcast_events(&batch, None), 1);
        notifications.send(&ServerEvent::mux_nudge("job".into(), None));
        assert!(
            output.try_recv().is_err(),
            "registered clients cannot receive foreign events before welcome"
        );
        sender
            .try_send_json(&serde_json::json!({"type":"mux.welcome"}))
            .unwrap();
        while let Ok(message) = input.try_recv() {
            sender.forward(message).unwrap();
        }
        let mut types = Vec::new();
        while let Ok(message) = output.try_recv() {
            let Message::Text(text) = &message.value else {
                panic!("text event");
            };
            types.push(
                serde_json::from_str::<serde_json::Value>(text).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        assert_eq!(
            types,
            [
                "mux.welcome",
                "mux.pane_spawned",
                "mux.nudge",
                "settings.changed",
                "mux.nudge"
            ]
        );
        assert_eq!(context.budget.used(), 0);
    }
}
