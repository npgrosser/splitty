//! Handles the `mux.*` sub-protocol for a single WebSocket connection.
//!
//! Processes `mux.op` and `mux.resync` messages. Sends a `mux.welcome` on connect
//! with the screen list. No per-screen attach/detach — persistence is a server-mode
//! property, not per-screen.

use std::sync::Arc;
use std::sync::RwLock;

use serde::Deserialize;
use serde_json::Value;

use splitty_core::ids::*;
use splitty_core::mux::effect_runner::{EffectRunner, EnvContext};
use splitty_core::mux::{Effect, Op, ScreenOp, Store};
use splitty_core::protocol::ServerEvent;

use super::codec;
use super::sender::WsSender;

/// Delivers an ordered batch of serialized events to every connected client
/// except the originating one (passed as the second argument).
///
/// Fire-and-forget: the implementation spawns the actual delivery so callers in
/// async context never block on the client map.
pub type BroadcastFn = Arc<dyn Fn(Vec<String>, Arc<WsSender>) + Send + Sync>;

/// Handles the mux.* protocol for a single WebSocket connection.
pub struct MuxHandler {
    store: Arc<Store>,
    sender: Arc<WsSender>,
    client_id: String,
    server_mode: String,
    effect_runner: Arc<EffectRunner>,
    shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
    env: EnvContext,
    broadcast: BroadcastFn,
    jobs: Arc<crate::jobs::Jobs>,
}

// ── Mux wire message types (client -> server) ───────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MuxOpMsg {
    screen_id: String,
    op: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MuxResyncMsg {
    screen_id: String,
}

impl MuxHandler {
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per collaborator the handler holds; built once per connection in handler.rs"
    )]
    pub fn new(
        store: Arc<Store>,
        sender: Arc<WsSender>,
        client_id: String,
        server_mode: String,
        effect_runner: Arc<EffectRunner>,
        shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
        env: EnvContext,
        broadcast: BroadcastFn,
        jobs: Arc<crate::jobs::Jobs>,
    ) -> Self {
        Self {
            store,
            sender,
            client_id,
            server_mode,
            effect_runner,
            shell_profiles,
            env,
            broadcast,
            jobs,
        }
    }

    /// Send the welcome message with screen list on connect.
    pub async fn send_welcome(&self) {
        let screens = self.store.list_screen_infos();

        let mut event = ServerEvent::mux_welcome(
            self.client_id.clone(),
            screens,
            Some(self.server_mode.clone()),
        );

        if let ServerEvent::MuxWelcome { capabilities, .. } = &mut event {
            capabilities.push(super::editor::CAPABILITY.into());
            capabilities.push(super::files::CAPABILITY.into());
            capabilities.push(splitty_core::protocol::sizing::CAPABILITY.into());
        }
        self.send_event(&event).await;
    }

    /// Try to handle a message as a mux.* message.
    /// Returns true if the message was handled (starts with "mux.").
    pub async fn try_handle(&self, text: &str) -> bool {
        // Peek at the type field
        let peek: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return false,
        };

        let msg_type = match peek.get("type").and_then(|t| t.as_str()) {
            Some(t) if t.starts_with("mux.") => t,
            _ => return false,
        };

        match msg_type {
            codec::MSG_MUX_OP => {
                if let Ok(msg) = serde_json::from_str::<MuxOpMsg>(text) {
                    self.handle_op(msg).await;
                } else {
                    tracing::warn!("invalid mux.op message");
                }
            }
            codec::MSG_MUX_RESYNC => {
                if let Ok(msg) = serde_json::from_str::<MuxResyncMsg>(text) {
                    self.handle_resync(msg).await;
                } else {
                    tracing::warn!("invalid mux.resync message");
                }
            }
            _ => {
                tracing::warn!("unknown mux message type: {}", msg_type);
            }
        }

        true
    }

    async fn handle_op(&self, msg: MuxOpMsg) {
        let screen_id = ScreenId::from(msg.screen_id.as_str());

        let op_type = msg
            .op
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        // Pane the op would create — reported back if the op is rejected so the
        // client can stop waiting for a session that will never be spawned.
        let new_pane_id = msg
            .op
            .get("newPaneId")
            .and_then(|v| v.as_str())
            .map(String::from);

        // The target of a pane moved to another screen, nudged with its source.
        let mut moved_to = None;
        let result = match op_type.as_str() {
            "move_pane_to_screen" => {
                let field = |name: &str| msg.op.get(name).and_then(|v| v.as_str());
                let (Some(pane_id), Some(target)) = (field("paneId"), field("targetScreenId"))
                else {
                    tracing::warn!("mux.op: move_pane_to_screen needs paneId and targetScreenId");
                    return;
                };
                let target = ScreenId::from(target);
                moved_to = Some(target.clone());
                self.store.dispatch(Op::MovePaneToScreen {
                    screen_id: screen_id.clone(),
                    pane_id: PaneId::from(pane_id),
                    target_screen_id: target,
                    create_screen: msg.op.get("createScreen").and_then(|v| v.as_bool())
                        == Some(true),
                    screen_name: field("screenName").map(String::from),
                })
            }
            "create_screen" => {
                let name = msg
                    .op
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let sid = msg
                    .op
                    .get("screenId")
                    .and_then(|v| v.as_str())
                    .map(ScreenId::from);
                let pid = msg
                    .op
                    .get("newPaneId")
                    .and_then(|v| v.as_str())
                    .map(PaneId::from);
                let shell_profile_id = msg
                    .op
                    .get("shellProfileId")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| {
                        self.shell_profiles
                            .read()
                            .unwrap()
                            .default_profile()
                            .map(|p| p.id.clone())
                    });

                self.store.dispatch(Op::CreateScreen {
                    name,
                    screen_id: sid,
                    new_pane_id: pid,
                    shell_profile_id,
                })
            }
            "close_screen" => self.store.dispatch(Op::CloseScreen {
                screen_id: screen_id.clone(),
            }),
            "rename_screen" => {
                let name = msg
                    .op
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.store.dispatch(Op::RenameScreen {
                    screen_id: screen_id.clone(),
                    name,
                })
            }
            "set_active_screen" => self.store.dispatch(Op::SetActiveScreen {
                screen_id: screen_id.clone(),
            }),
            "reorder_screens" => {
                let order: Vec<ScreenId> = msg
                    .op
                    .get("screenOrder")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(ScreenId::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.store.dispatch(Op::ReorderScreens {
                    screen_order: order,
                })
            }
            _ => {
                // Screen-level op — parse as ScreenOp
                tracing::debug!(
                    "mux.op screen-level: type={}, screen={}, op={}",
                    op_type,
                    msg.screen_id,
                    msg.op
                );
                let screen_op: ScreenOp = match serde_json::from_value(msg.op) {
                    Ok(op) => op,
                    Err(e) => {
                        tracing::warn!("mux.op: invalid op: {}", e);
                        return;
                    }
                };

                self.store.dispatch_screen(&screen_id, screen_op)
            }
        };

        match result {
            Ok(effects) => {
                if let Some(target) = moved_to {
                    // Starts and ends no session, yet both screens changed.
                    self.nudge_others(&[&screen_id, &target]);
                }
                if op_type == "set_scratchpad" || op_type == "arrange_panes" {
                    // Starts and ends no session either, yet the other
                    // windows show the text (`session-scratchpad`) or the
                    // arrangement (`arrange-panes`).
                    self.nudge_others(&[&screen_id]);
                }
                self.run_effects(effects, &screen_id, &op_type).await;
            }
            Err(e) => {
                // Windows before `layout-arrange-v1` send `move_pane` without a
                // target for every drag; it was always rejected, and harmless.
                if op_type == "move_pane" {
                    tracing::debug!(
                        "mux.op {} ignored: screen={}, err={}",
                        op_type,
                        msg.screen_id,
                        e
                    );
                } else {
                    // List what the screen does hold — a rejected op means the
                    // client's layout and the mux model have drifted apart, and
                    // the known panes say where.
                    let known = match self.store.get_screen(&screen_id) {
                        Some(s) => s
                            .panes
                            .keys()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                        None => "<screen unknown>".to_string(),
                    };
                    tracing::warn!(
                        "mux.op {} failed: screen={}, err={}, known panes=[{}]",
                        op_type,
                        msg.screen_id,
                        e,
                        known
                    );
                }

                // A rejected arrangement makes the window resync its screen
                // (mux-protocol, Window Layout Changes Reach the Server).
                if new_pane_id.is_some() || op_type == "arrange_panes" {
                    self.send_event(&ServerEvent::mux_op_failed(
                        op_type,
                        Some(msg.screen_id.clone()),
                        new_pane_id,
                        e.to_string(),
                    ))
                    .await;
                }
            }
        }
    }

    /// Execute effects produced by the reducer and nudge clients if any were run.
    async fn run_effects(&self, effects: Vec<Effect>, screen_id: &ScreenId, op_type: &str) {
        if effects.is_empty() {
            return;
        }

        if let Err(error) = self.jobs.stop_effects(&effects).await {
            self.send_event(&ServerEvent::mux_op_failed(
                op_type,
                Some(screen_id.to_string()),
                None,
                error.error,
            ))
            .await;
            return;
        }

        let errors = self.effect_runner.execute(effects, &self.env).await;
        let actual_screen = errors
            .iter()
            .find_map(|outcome| match &outcome.effect {
                Effect::SpawnSession { screen_id, .. } => Some(screen_id.clone()),
                Effect::SpawnJob { job } => Some(job.screen_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| screen_id.clone());
        let spawn_info: Vec<_> = errors
            .iter()
            .filter_map(|o| o.spawned())
            .map(|(_, p, s)| (p.to_string(), s.to_string()))
            .collect();
        for outcome in &errors {
            if let Err(error) = &outcome.result {
                if let Effect::SpawnSession {
                    pane_id, screen_id, ..
                } = &outcome.effect
                {
                    self.send_event(&ServerEvent::mux_op_failed(
                        op_type,
                        Some(screen_id.to_string()),
                        Some(pane_id.to_string()),
                        error.to_string(),
                    ))
                    .await;
                }
            }
        }
        for e in errors
            .iter()
            .filter_map(|outcome| outcome.result.as_ref().err())
        {
            tracing::warn!("mux.op {} effect error: {e}", op_type);
        }

        // The requesting client already applied this op to its own layout — it
        // only needs the session ids for the panes it just created. Nudging it
        // would make it resync and rebuild the layout from the snapshot, tearing
        // down panes whose terminal is still attaching (blank pane, lost stream).
        // It is served directly and awaited, so the events are enqueued on its
        // socket before this op is considered handled.
        for (pane_id, session_id) in &spawn_info {
            self.send_event(&ServerEvent::mux_pane_spawned(pane_id, session_id))
                .await;
        }

        // Everyone else gets pane_spawned first, then the nudge that makes them
        // resync. A client that sees the nudge first rebuilds from the snapshot
        // and tears down the pane still waiting to learn its session id, so this
        // order is load-bearing on every delivery path.
        let mut batch = Vec::with_capacity(spawn_info.len() + 1);
        for (pane_id, session_id) in &spawn_info {
            batch.push(ServerEvent::mux_pane_spawned(pane_id, session_id));
        }
        batch.push(ServerEvent::mux_nudge(
            "mux_op".to_string(),
            Some(actual_screen.to_string()),
        ));

        let texts: Vec<String> = batch
            .iter()
            .filter_map(|e| match serde_json::to_string(e) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!("mux broadcast serialization failed: {e}");
                    None
                }
            })
            .collect();
        (self.broadcast)(texts, self.sender.clone());
    }

    /// Send every other client a `mux_op` nudge for each of `screens`.
    fn nudge_others(&self, screens: &[&ScreenId]) {
        let texts: Vec<String> = screens
            .iter()
            .filter_map(|screen| {
                serde_json::to_string(&ServerEvent::mux_nudge(
                    "mux_op".to_string(),
                    Some(screen.to_string()),
                ))
                .ok()
            })
            .collect();
        (self.broadcast)(texts, self.sender.clone());
    }

    async fn handle_resync(&self, msg: MuxResyncMsg) {
        let screen_id = ScreenId::from(msg.screen_id.as_str());

        match self.store.get_screen(&screen_id) {
            Some(screen) => {
                let event = ServerEvent::mux_screen_snapshot(screen);
                self.send_event(&event).await;
            }
            None => {
                tracing::warn!("mux.resync: screen not found: {}", msg.screen_id);
            }
        }
    }

    /// Send a nudge to this client for a specific screen.
    pub async fn send_nudge(&self, reason: &str, screen_id: &str) {
        let screen_id_opt = if screen_id.is_empty() {
            None
        } else {
            Some(screen_id.to_string())
        };
        let event = ServerEvent::mux_nudge(reason.to_string(), screen_id_opt);
        self.send_event(&event).await;
    }

    async fn send_event(&self, event: &ServerEvent) {
        let text = match serde_json::to_string(event) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("mux send_event serialization failed: {}", e);
                return;
            }
        };
        if let Err(e) = self.sender.send_text(&text).await {
            tracing::warn!("mux send failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::Message;

    /// A reloading client's `create_screen` for a screen the server already
    /// holds is refused, and that client is told with `mux.op_failed`
    /// (workspace-model, Screen Creation; mux-protocol, Rejected Op Reporting).
    #[tokio::test]
    async fn create_screen_over_a_screen_with_panes_is_reported_as_failed() {
        let store = Arc::new(Store::new());
        store
            .dispatch(Op::CreateScreen {
                name: Some("Work".to_string()),
                screen_id: Some(ScreenId::from("screen-1")),
                new_pane_id: Some(PaneId::from("terminal-1")),
                shell_profile_id: None,
            })
            .unwrap();
        let (sender, mut sent) = WsSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            tokio::sync::broadcast::channel(16).0,
        );
        let shell_profiles = splitty_core::shell_profiles::state_from_settings(&Default::default());
        let handler = MuxHandler::new(
            store.clone(),
            Arc::new(sender),
            "client".to_string(),
            "daemon".to_string(),
            runner,
            Arc::new(RwLock::new(shell_profiles)),
            EnvContext::default(),
            Arc::new(|_, _| {}),
            jobs,
        );

        let op = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"create_screen","screenId":"screen-1","name":"Default","newPaneId":"terminal-9"}}"#;
        assert!(handler.try_handle(op).await);

        let reply = match sent.recv().await {
            Some(Message::Text(text)) => serde_json::from_str::<Value>(text.as_str()).unwrap(),
            other => panic!("expected a text reply, got {other:?}"),
        };
        assert_eq!(reply["type"], "mux.op_failed");
        assert_eq!(reply["paneId"], "terminal-9");
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or_default()
                .contains("already exists"),
            "reply: {reply}"
        );
        let screen = store.get_screen(&ScreenId::from("screen-1")).unwrap();
        assert!(screen.panes.contains_key(&PaneId::from("terminal-1")));
        assert_eq!(screen.panes.len(), 1);
    }

    /// mux-protocol, Change Propagation to Other Clients (Pane moved by a
    /// client): the others are nudged for both screens, the mover is not.
    #[tokio::test]
    async fn a_moved_pane_nudges_the_other_clients_for_both_screens() {
        let store = Arc::new(Store::new());
        for (screen, pane) in [("screen-1", "terminal-1"), ("screen-2", "terminal-2")] {
            store
                .dispatch(Op::CreateScreen {
                    name: None,
                    screen_id: Some(ScreenId::from(screen)),
                    new_pane_id: Some(PaneId::from(pane)),
                    shell_profile_id: None,
                })
                .unwrap();
        }
        let (sender, mut sent) = WsSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            tokio::sync::broadcast::channel(16).0,
        );
        let broadcasts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorded = broadcasts.clone();
        let shell_profiles = splitty_core::shell_profiles::state_from_settings(&Default::default());
        let handler = MuxHandler::new(
            store.clone(),
            Arc::new(sender),
            "client".to_string(),
            "daemon".to_string(),
            runner,
            Arc::new(RwLock::new(shell_profiles)),
            EnvContext::default(),
            Arc::new(move |texts, _| recorded.lock().unwrap().extend(texts)),
            jobs,
        );

        let op = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"move_pane_to_screen","paneId":"terminal-1","targetScreenId":"screen-2"}}"#;
        assert!(handler.try_handle(op).await);

        let nudged: Vec<Value> = broadcasts
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str(text).unwrap())
            .collect();
        assert_eq!(nudged.len(), 2, "{nudged:?}");
        for (event, screen) in nudged.iter().zip(["screen-1", "screen-2"]) {
            assert_eq!(event["type"], "mux.nudge");
            assert_eq!(event["reason"], "mux_op");
            assert_eq!(event["screenId"], screen);
        }
        assert!(sent.try_recv().is_err(), "the mover gets no nudge");
        assert_eq!(
            store.screen_of_movable_pane(&PaneId::from("terminal-1")),
            Some(ScreenId::from("screen-2"))
        );
        assert!(store.get_screen(&ScreenId::from("screen-1")).is_none());
    }

    /// mux-protocol, Change Propagation to Other Clients (Panes arranged by a
    /// client) and Rejected Op Reporting (Rejected arrangement): an applied
    /// arrangement nudges the others and not the window that sent it; a
    /// rejected one answers only that window with `mux.op_failed`. A selected
    /// tab (Selected Tabs Reach the Server) does neither.
    #[tokio::test]
    async fn an_arrangement_nudges_the_others_and_its_rejection_answers_the_window() {
        let store = Arc::new(Store::new());
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("screen-1")),
                new_pane_id: Some(PaneId::from("terminal-1")),
                shell_profile_id: None,
            })
            .unwrap();
        store
            .dispatch_screen(
                &ScreenId::from("screen-1"),
                ScreenOp::SplitPane {
                    pane_id: PaneId::from("terminal-1"),
                    axis: splitty_core::mux::Axis::Horizontal,
                    new_pane_index: None,
                    new_pane_id: Some(PaneId::from("terminal-2")),
                    resulting_sizes: None,
                    cwd: None,
                    shell_profile_id: None,
                },
            )
            .unwrap();
        let (sender, mut sent) = WsSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            tokio::sync::broadcast::channel(16).0,
        );
        let broadcasts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorded = broadcasts.clone();
        let shell_profiles = splitty_core::shell_profiles::state_from_settings(&Default::default());
        let handler = MuxHandler::new(
            store.clone(),
            Arc::new(sender),
            "client".to_string(),
            "daemon".to_string(),
            runner,
            Arc::new(RwLock::new(shell_profiles)),
            EnvContext::default(),
            Arc::new(move |texts, _| recorded.lock().unwrap().extend(texts)),
            jobs,
        );

        let tabs = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"arrange_panes","root":{"type":"slot","paneIds":["terminal-1","terminal-2"],"activePaneId":"terminal-2"}}}"#;
        assert!(handler.try_handle(tabs).await);
        let nudged: Vec<Value> = broadcasts
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str(text).unwrap())
            .collect();
        assert_eq!(nudged.len(), 1, "{nudged:?}");
        assert_eq!(nudged[0]["type"], "mux.nudge");
        assert_eq!(nudged[0]["reason"], "mux_op");
        assert_eq!(nudged[0]["screenId"], "screen-1");
        assert!(sent.try_recv().is_err(), "the window gets no nudge");
        let screen = store.get_screen(&ScreenId::from("screen-1")).unwrap();
        assert_eq!(
            screen.root.collect_pane_ids(),
            [PaneId::from("terminal-1"), PaneId::from("terminal-2")]
        );
        assert!(matches!(
            screen.root,
            splitty_core::mux::LayoutNode::Slot { .. }
        ));

        let missing = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"arrange_panes","root":{"type":"slot","paneIds":["terminal-1"]}}}"#;
        assert!(handler.try_handle(missing).await);
        let reply = match sent.recv().await {
            Some(Message::Text(text)) => serde_json::from_str::<Value>(text.as_str()).unwrap(),
            other => panic!("expected a text reply, got {other:?}"),
        };
        assert_eq!(reply["type"], "mux.op_failed");
        assert_eq!(reply["op"], "arrange_panes");
        assert_eq!(reply["screenId"], "screen-1");
        assert!(reply.get("paneId").is_none(), "{reply}");
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or_default()
                .contains("misses"),
            "{reply}"
        );
        assert_eq!(
            broadcasts.lock().unwrap().len(),
            1,
            "no nudge for a rejection"
        );
        assert_eq!(
            store
                .get_screen(&ScreenId::from("screen-1"))
                .unwrap()
                .revision,
            screen.revision
        );

        // A tab choice nudges no one, and its rejection answers no one.
        let select = |pane: &str| {
            format!(
                r#"{{"type":"mux.op","screenId":"screen-1","op":{{"type":"select_tab","paneId":"{pane}"}}}}"#
            )
        };
        assert!(handler.try_handle(&select("terminal-1")).await);
        assert!(matches!(
            store.get_screen(&ScreenId::from("screen-1")).unwrap().root,
            splitty_core::mux::LayoutNode::Slot { active_pane_id: Some(ref active), .. }
                if active == &PaneId::from("terminal-1")
        ));
        assert!(handler.try_handle(&select("terminal-9")).await);
        assert_eq!(broadcasts.lock().unwrap().len(), 1, "no nudge for a tab");
        assert!(sent.try_recv().is_err(), "no reply for a tab");
    }

    /// session-scratchpad, Saving Through The Window and Scratchpad Capability,
    /// and mux-protocol, Change Propagation to Other Clients (Scratchpad saved
    /// by a client): the server announces scratchpads, stores a save in the
    /// session record, nudges the others for its screen and not the saver, and
    /// the snapshot of the next resync carries the text.
    #[tokio::test]
    async fn a_saved_scratchpad_nudges_the_other_clients_and_reaches_their_snapshot() {
        let welcome = serde_json::to_value(ServerEvent::mux_welcome(
            "client".to_string(),
            Vec::new(),
            None,
        ))
        .unwrap();
        assert!(
            welcome["capabilities"]
                .as_array()
                .is_some_and(|all| all.iter().any(|c| c == "scratchpad-v1")),
            "{welcome}"
        );
        let store = Arc::new(Store::new());
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("screen-1")),
                new_pane_id: Some(PaneId::from("terminal-1")),
                shell_profile_id: None,
            })
            .unwrap();
        let session = store
            .session_id_for_pane(&PaneId::from("terminal-1"))
            .unwrap();
        let (sender, mut sent) = WsSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            tokio::sync::broadcast::channel(16).0,
        );
        let broadcasts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorded = broadcasts.clone();
        let shell_profiles = splitty_core::shell_profiles::state_from_settings(&Default::default());
        let handler = MuxHandler::new(
            store.clone(),
            Arc::new(sender),
            "client".to_string(),
            "daemon".to_string(),
            runner,
            Arc::new(RwLock::new(shell_profiles)),
            EnvContext::default(),
            Arc::new(move |texts, _| recorded.lock().unwrap().extend(texts)),
            jobs,
        );

        let op = format!(
            r#"{{"type":"mux.op","screenId":"screen-1","op":{{"type":"set_scratchpad","sessionId":"{session}","text":"ask why the test is skipped"}}}}"#
        );
        assert!(handler.try_handle(&op).await);

        let nudged: Vec<Value> = broadcasts
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str(text).unwrap())
            .collect();
        assert_eq!(nudged.len(), 1, "{nudged:?}");
        assert_eq!(nudged[0]["type"], "mux.nudge");
        assert_eq!(nudged[0]["reason"], "mux_op");
        assert_eq!(nudged[0]["screenId"], "screen-1");
        assert!(sent.try_recv().is_err(), "the saver gets nothing");

        assert!(
            handler
                .try_handle(r#"{"type":"mux.resync","screenId":"screen-1"}"#)
                .await
        );
        let snapshot = match sent.recv().await {
            Some(Message::Text(text)) => serde_json::from_str::<Value>(text.as_str()).unwrap(),
            other => panic!("expected a snapshot, got {other:?}"),
        };
        assert_eq!(snapshot["type"], "mux.screen_snapshot");
        assert_eq!(
            snapshot["screen"]["sessions"][session.as_str()]["scratchpad"],
            "ask why the test is skipped"
        );
    }
}
