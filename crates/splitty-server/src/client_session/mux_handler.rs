//! Handles the `mux.*` sub-protocol for a single WebSocket connection.
//!
//! Processes `mux.op` and `mux.resync` messages, and `viewer.review_answer`,
//! a window's answer to a process waiting for a review. Sends a `mux.welcome`
//! on connect with the screen list. No per-screen attach/detach — persistence
//! is a server-mode property, not per-screen.

use std::sync::Arc;
use std::sync::RwLock;

use serde::Deserialize;
use serde_json::Value;

use splitty_core::ids::*;
use splitty_core::mux::effect_runner::{EffectRunner, EnvContext};
use splitty_core::mux::{Effect, Op, ScreenOp, Store};
use splitty_core::protocol::ServerEvent;

use super::codec;
use super::sender::ClientSender;

/// Enqueues an ordered batch of typed events for every other client.
/// The implementation reserves each recipient's bytes before serialization;
/// it does not spawn per-event sends or allocate shared JSON copies.
pub type BroadcastFn = Arc<dyn Fn(&[ServerEvent], Arc<ClientSender>) + Send + Sync>;

/// Handles the mux.* protocol for a single WebSocket connection.
pub struct MuxHandler {
    store: Arc<Store>,
    sender: Arc<ClientSender>,
    client_id: String,
    server_mode: String,
    effect_runner: Arc<EffectRunner>,
    shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
    env: EnvContext,
    broadcast: BroadcastFn,
    jobs: Arc<crate::jobs::Jobs>,
    reviews: Arc<super::reviews::Reviews>,
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
        sender: Arc<ClientSender>,
        client_id: String,
        server_mode: String,
        effect_runner: Arc<EffectRunner>,
        shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
        env: EnvContext,
        broadcast: BroadcastFn,
        jobs: Arc<crate::jobs::Jobs>,
        reviews: Arc<super::reviews::Reviews>,
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
            reviews,
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
            capabilities.push(splitty_core::protocol::upload::CAPABILITY.into());
        }
        self.send_event(&event).await;
    }

    /// Consume the already admitted session DOM; do not parse or clone its
    /// potentially large operation a second time. The caller retains its lease.
    pub(super) async fn handle_value(&self, value: Value) {
        match value.get("type").and_then(Value::as_str) {
            Some(codec::MSG_MUX_OP) => {
                if let Ok(msg) = serde_json::from_value::<MuxOpMsg>(value) {
                    self.handle_op(msg).await;
                } else {
                    tracing::warn!("invalid mux.op message");
                }
            }
            Some(codec::MSG_MUX_RESYNC) => {
                if let Ok(msg) = serde_json::from_value::<MuxResyncMsg>(value) {
                    self.handle_resync(msg).await;
                } else {
                    tracing::warn!("invalid mux.resync message");
                }
            }
            Some(codec::MSG_VIEWER_REVIEW_ANSWER) => {
                // Settled once; every answer is acknowledged to its sender
                // only (`mux-protocol`, Review Answers). The waiting request
                // ends the wait in the record and nudges the windows.
                let answered = self.reviews.answer(&value);
                if let Err(error) = self.sender.try_send_json(&answered) {
                    tracing::debug!(%error, "a review acknowledgement was not queued");
                }
            }
            _ => tracing::warn!("unknown mux message type"),
        }
    }

    #[cfg(test)]
    async fn try_handle(&self, text: &str) -> bool {
        let Ok(_decoded) = splitty_transport::codec::reserve_json_typed(
            text.as_bytes(),
            &self.sender.context().budget,
            false,
        ) else {
            return false;
        };
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return false;
        };
        if !value["type"]
            .as_str()
            .is_some_and(|kind| kind.starts_with("mux.") || kind == codec::MSG_VIEWER_REVIEW_ANSWER)
        {
            return false;
        }
        self.handle_value(value).await;
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
        // The viewer pane an annotation op names, reported back if the op is
        // rejected (`viewer-annotations`).
        let annotation_op = matches!(
            op_type.as_str(),
            "put_annotations"
                | "remove_annotations"
                | "put_replies"
                | "remove_replies"
                | "mark_sent"
        );
        let annotated_pane = msg
            .op
            .get("paneId")
            .and_then(|v| v.as_str())
            .filter(|_| annotation_op)
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
                        // A window waits for the answer to an annotation op
                        // before it shows its own state again.
                        if annotation_op {
                            self.send_event(&ServerEvent::mux_op_failed(
                                op_type,
                                Some(msg.screen_id.clone()),
                                annotated_pane,
                                format!("invalid op: {e}"),
                            ))
                            .await;
                        }
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
                let viewer_change = matches!(
                    op_type.as_str(),
                    "open_viewer"
                        | "put_annotations"
                        | "remove_annotations"
                        | "put_replies"
                        | "remove_replies"
                        | "mark_sent"
                ) || (op_type == "close_pane" && effects.is_empty());
                if op_type == "set_scratchpad" || op_type == "arrange_panes" || viewer_change {
                    // Starts and ends no session either, yet the other
                    // windows show the text (`session-scratchpad`), the
                    // arrangement (`arrange-panes`), the viewer pane that
                    // came or went (`viewer-panes`) or its annotations
                    // (`viewer-annotations`).
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

                // A rejected arrangement or annotation op makes the window
                // show the server's state again (mux-protocol, Rejected Op
                // Reporting).
                if new_pane_id.is_some() || op_type == "arrange_panes" || annotation_op {
                    self.send_event(&ServerEvent::mux_op_failed(
                        op_type,
                        Some(msg.screen_id.clone()),
                        annotated_pane.or(new_pane_id),
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

        (self.broadcast)(&batch, self.sender.clone());
    }

    /// Send every other client a `mux_op` nudge for each of `screens`.
    fn nudge_others(&self, screens: &[&ScreenId]) {
        let events: Vec<_> = screens
            .iter()
            .map(|screen| ServerEvent::mux_nudge("mux_op".to_string(), Some(screen.to_string())))
            .collect();
        (self.broadcast)(&events, self.sender.clone());
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
        if let Err(e) = self.sender.send_event(event).await {
            tracing::warn!("mux send failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_session::Message;

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
        let (sender, mut sent) = ClientSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            Default::default(),
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
            Default::default(),
        );

        let op = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"create_screen","screenId":"screen-1","name":"Default","newPaneId":"terminal-9"}}"#;
        assert!(handler.try_handle(op).await);

        let reply = match sent.recv().await.map(|m| m.value) {
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
        let (sender, mut sent) = ClientSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            Default::default(),
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
            Arc::new(move |texts, _| {
                recorded.lock().unwrap().extend(
                    texts
                        .iter()
                        .map(|event| serde_json::to_string(event).unwrap()),
                )
            }),
            jobs,
            Default::default(),
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
        let (sender, mut sent) = ClientSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            Default::default(),
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
            Arc::new(move |texts, _| {
                recorded.lock().unwrap().extend(
                    texts
                        .iter()
                        .map(|event| serde_json::to_string(event).unwrap()),
                )
            }),
            jobs,
            Default::default(),
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
        let reply = match sent.recv().await.map(|m| m.value) {
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
        let (sender, mut sent) = ClientSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            Default::default(),
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
            Arc::new(move |texts, _| {
                recorded.lock().unwrap().extend(
                    texts
                        .iter()
                        .map(|event| serde_json::to_string(event).unwrap()),
                )
            }),
            jobs,
            Default::default(),
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
        let snapshot = match sent.recv().await.map(|m| m.value) {
            Some(Message::Text(text)) => serde_json::from_str::<Value>(text.as_str()).unwrap(),
            other => panic!("expected a snapshot, got {other:?}"),
        };
        assert_eq!(snapshot["type"], "mux.screen_snapshot");
        assert_eq!(
            snapshot["screen"]["sessions"][session.as_str()]["scratchpad"],
            "ask why the test is skipped"
        );
    }

    /// viewer-panes, Viewer Capability and Document Text Outside Snapshots
    /// (Snapshot of a screen with a text document), and mux-protocol, Change
    /// Propagation to Other Clients (Viewer opened and closed by a client):
    /// the welcome announces viewers, opening and closing one nudges the other
    /// clients and not the sender, and a resync's snapshot holds the record
    /// without the text.
    #[tokio::test]
    async fn a_viewer_opened_and_closed_nudges_the_others_and_its_text_stays_out_of_snapshots() {
        let store = Arc::new(Store::new());
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("screen-1")),
                new_pane_id: Some(PaneId::from("terminal-1")),
                shell_profile_id: None,
            })
            .unwrap();
        let (sender, mut sent) = ClientSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            Default::default(),
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
            Arc::new(move |texts, _| {
                recorded.lock().unwrap().extend(
                    texts
                        .iter()
                        .map(|event| serde_json::to_string(event).unwrap()),
                )
            }),
            jobs,
            Default::default(),
        );
        let next = |sent: &mut splitty_transport::queue::Receiver<Message>| match sent
            .try_recv()
            .map(|m| m.value)
        {
            Ok(Message::Text(text)) => serde_json::from_str::<Value>(text.as_str()).unwrap(),
            other => panic!("expected a text message, got {other:?}"),
        };
        let nudges = || -> Vec<Value> {
            broadcasts
                .lock()
                .unwrap()
                .iter()
                .map(|text| serde_json::from_str(text).unwrap())
                .collect()
        };

        handler.send_welcome().await;
        let welcome = next(&mut sent);
        assert_eq!(welcome["type"], "mux.welcome");
        assert!(
            welcome["capabilities"]
                .as_array()
                .is_some_and(|all| all.iter().any(|c| c == "viewer-v1")),
            "{welcome}"
        );

        let open = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"open_viewer","paneId":"terminal-1","newPaneId":"viewer-0123456789abcdef","cwd":"/home/me/app","source":{"kind":"text","title":"Selection from claude","text":"the agent's answer"}}}"#;
        assert!(handler.try_handle(open).await);
        let nudged = nudges();
        assert_eq!(nudged.len(), 1, "{nudged:?}");
        assert_eq!(nudged[0]["type"], "mux.nudge");
        assert_eq!(nudged[0]["reason"], "mux_op");
        assert_eq!(nudged[0]["screenId"], "screen-1");
        assert!(sent.try_recv().is_err(), "the opener gets nothing");
        assert_eq!(
            store
                .get_screen(&ScreenId::from("screen-1"))
                .unwrap()
                .sessions
                .len(),
            1
        );

        assert!(
            handler
                .try_handle(r#"{"type":"mux.resync","screenId":"screen-1"}"#)
                .await
        );
        let snapshot = next(&mut sent);
        assert_eq!(snapshot["type"], "mux.screen_snapshot");
        let pane = &snapshot["screen"]["panes"]["viewer-0123456789abcdef"];
        assert_eq!(pane["role"], "viewer", "{pane}");
        assert_eq!(pane["title"], "Selection from claude");
        assert_eq!(
            pane["viewer"],
            serde_json::json!({
                "source": {"kind": "text", "title": "Selection from claude"},
                "originPaneId": "terminal-1",
                "revision": 1,
            })
        );
        assert!(!snapshot.to_string().contains("the agent's answer"));

        let close = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"close_pane","paneId":"viewer-0123456789abcdef"}}"#;
        assert!(handler.try_handle(close).await);
        let nudged = nudges();
        assert_eq!(nudged.len(), 2, "{nudged:?}");
        assert_eq!(nudged[1]["type"], "mux.nudge");
        assert_eq!(nudged[1]["screenId"], "screen-1");
        assert!(sent.try_recv().is_err(), "the closer gets nothing");
        let screen = store.get_screen(&ScreenId::from("screen-1")).unwrap();
        assert!(!screen
            .panes
            .contains_key(&PaneId::from("viewer-0123456789abcdef")));
        assert_eq!(screen.sessions.len(), 1, "no session ended");
    }

    /// A window's handler on `store`, as `client_id`, whose broadcasts to the
    /// other clients land in `others`.
    fn window(
        store: &Arc<Store>,
        client_id: &str,
        others: &Arc<std::sync::Mutex<Vec<Value>>>,
    ) -> (MuxHandler, splitty_transport::queue::Receiver<Message>) {
        window_with(store, client_id, others, &Default::default())
    }

    /// [`window`], answering the waits of `reviews`.
    fn window_with(
        store: &Arc<Store>,
        client_id: &str,
        others: &Arc<std::sync::Mutex<Vec<Value>>>,
        reviews: &Arc<super::super::reviews::Reviews>,
    ) -> (MuxHandler, splitty_transport::queue::Receiver<Message>) {
        let (sender, sent) = ClientSender::channel();
        let backend = Arc::new(splitty_core::pty::manager::Manager::new());
        let runner = Arc::new(EffectRunner::new(backend.clone()));
        let jobs = crate::jobs::Jobs::new(
            store.clone(),
            backend,
            runner.clone(),
            EnvContext::default(),
            Default::default(),
        );
        let shell_profiles = splitty_core::shell_profiles::state_from_settings(&Default::default());
        let recorded = others.clone();
        let handler = MuxHandler::new(
            store.clone(),
            Arc::new(sender),
            client_id.to_string(),
            "daemon".to_string(),
            runner,
            Arc::new(RwLock::new(shell_profiles)),
            EnvContext::default(),
            Arc::new(move |events, _| {
                recorded.lock().expect("recorder").extend(
                    events
                        .iter()
                        .map(|event| serde_json::to_value(event).expect("event")),
                )
            }),
            jobs,
            reviews.clone(),
        );
        (handler, sent)
    }

    fn next_text(sent: &mut splitty_transport::queue::Receiver<Message>) -> Value {
        match sent.try_recv().map(|message| message.value) {
            Ok(Message::Text(text)) => serde_json::from_str(text.as_str()).expect("JSON"),
            other => panic!("expected a text message, got {other:?}"),
        }
    }

    /// viewer-annotations, Annotations Capability and Saving Annotations
    /// Through The Window (A second window sees the annotation), and
    /// mux-protocol, Change Propagation to Other Clients (Annotation added by
    /// a client): the welcome announces annotations; a put and a remove nudge
    /// the other clients and not the sender, and the second window's resync
    /// shows the annotation, then none.
    #[tokio::test]
    async fn an_annotation_reaches_the_other_windows_and_not_its_sender() {
        let store = Arc::new(Store::new());
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("screen-1")),
                new_pane_id: Some(PaneId::from("terminal-1")),
                shell_profile_id: None,
            })
            .expect("screen");
        let open: ScreenOp = serde_json::from_value(serde_json::json!({
            "type": "open_viewer", "paneId": "terminal-1", "newPaneId": "viewer-0123456789abcdef",
            "source": {"kind": "file", "path": "/home/me/app/PLAN.md"},
        }))
        .expect("wire op");
        store
            .dispatch_screen(&ScreenId::from("screen-1"), open)
            .expect("viewer");
        let others = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (a, mut sent_a) = window(&store, "window-a", &others);
        let (b, mut sent_b) = window(&store, "window-b", &Default::default());
        let nudges = || others.lock().expect("recorder").clone();
        let resync = r#"{"type":"mux.resync","screenId":"screen-1"}"#;

        a.send_welcome().await;
        let welcome = next_text(&mut sent_a);
        assert!(
            welcome["capabilities"]
                .as_array()
                .is_some_and(|all| all.iter().any(|c| c == "viewer-annotations-v1")),
            "{welcome}"
        );

        let mut annotation = serde_json::json!({
            "id": "ann-0123456789abcdef", "kind": "comment", "body": "Why not five times?",
            "target": {
                "view": "source", "quote": "retry three times", "prefix": "On failure, ",
                "suffix": ".", "startLine": 30, "endLine": 30,
            },
        });
        let put = serde_json::json!({
            "type": "mux.op", "screenId": "screen-1",
            "op": {"type": "put_annotations", "paneId": "viewer-0123456789abcdef", "annotations": [annotation]},
        });
        assert!(a.try_handle(&put.to_string()).await);
        let nudged = nudges();
        assert_eq!(nudged.len(), 1, "{nudged:?}");
        assert_eq!(nudged[0]["type"], "mux.nudge");
        assert_eq!(nudged[0]["reason"], "mux_op");
        assert_eq!(nudged[0]["screenId"], "screen-1");
        assert!(sent_a.try_recv().is_err(), "the sender gets nothing");

        assert!(b.try_handle(resync).await);
        let snapshot = next_text(&mut sent_b);
        assert_eq!(snapshot["type"], "mux.screen_snapshot");
        annotation["number"] = serde_json::json!(1);
        assert_eq!(
            snapshot["screen"]["panes"]["viewer-0123456789abcdef"]["viewer"]["annotations"],
            serde_json::json!([annotation])
        );

        let remove = r#"{"type":"mux.op","screenId":"screen-1","op":{"type":"remove_annotations","paneId":"viewer-0123456789abcdef","ids":["ann-0123456789abcdef"]}}"#;
        assert!(a.try_handle(remove).await);
        let nudged = nudges();
        assert_eq!(nudged.len(), 2, "{nudged:?}");
        assert_eq!(nudged[1]["type"], "mux.nudge");
        assert_eq!(nudged[1]["screenId"], "screen-1");
        assert!(sent_a.try_recv().is_err(), "the sender gets nothing");
        assert!(b.try_handle(resync).await);
        let snapshot = next_text(&mut sent_b);
        let viewer = &snapshot["screen"]["panes"]["viewer-0123456789abcdef"]["viewer"];
        assert!(viewer.get("annotations").is_none(), "{viewer}");
    }

    /// mux-protocol, Rejected Op Reporting (Rejected annotation): a put that
    /// would give viewer pane `v1` its 101st annotation, and a remove with a
    /// malformed ID, are reported to their sender with the pane; nobody else
    /// hears of them, and the pane is unchanged.
    #[tokio::test]
    async fn a_rejected_annotation_is_reported_to_its_sender_only() {
        let store = Arc::new(Store::new());
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("s1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .expect("screen");
        let comment = |n: u32| serde_json::json!({"id": format!("ann-{n:016x}"), "kind": "comment", "body": "x"});
        for op in [
            serde_json::json!({
                "type": "open_viewer", "paneId": "p1", "newPaneId": "v1",
                "source": {"kind": "file", "path": "/home/me/app/PLAN.md"},
            }),
            serde_json::json!({
                "type": "put_annotations", "paneId": "v1",
                "annotations": (0..100).map(comment).collect::<Vec<_>>(),
            }),
        ] {
            let op: ScreenOp = serde_json::from_value(op).expect("wire op");
            store
                .dispatch_screen(&ScreenId::from("s1"), op)
                .expect("applied");
        }
        let before = store.get_screen(&ScreenId::from("s1")).expect("screen");
        let others = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (a, mut sent_a) = window(&store, "window-a", &others);

        for (op, kind) in [
            (
                serde_json::json!({"type": "put_annotations", "paneId": "v1", "annotations": [comment(100)]}),
                "put_annotations",
            ),
            (
                serde_json::json!({"type": "remove_annotations", "paneId": "v1", "ids": ["ann-1"]}),
                "remove_annotations",
            ),
        ] {
            let message = serde_json::json!({"type": "mux.op", "screenId": "s1", "op": op});
            assert!(a.try_handle(&message.to_string()).await);
            let failed = next_text(&mut sent_a);
            assert_eq!(failed["type"], "mux.op_failed", "{failed}");
            assert_eq!(failed["op"], kind);
            assert_eq!(failed["screenId"], "s1");
            assert_eq!(failed["paneId"], "v1");
            assert!(
                failed["error"]
                    .as_str()
                    .is_some_and(|error| !error.is_empty()),
                "{failed}"
            );
            assert!(sent_a.try_recv().is_err(), "nothing else for the sender");
        }
        assert!(
            others.lock().expect("recorder").is_empty(),
            "no other client hears of it"
        );
        let after = store.get_screen(&ScreenId::from("s1")).expect("screen");
        assert_eq!(after.revision, before.revision);
        assert_eq!(
            after.panes[&PaneId::from("v1")].viewer,
            before.panes[&PaneId::from("v1")].viewer
        );
    }

    /// Screen `s1` with the terminal `p1` and the viewer `v1`, whose
    /// annotation #1 the user handed over and an agent replied to.
    fn handed_over() -> Arc<Store> {
        let store = Arc::new(Store::new());
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("s1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .expect("screen");
        for op in [
            serde_json::json!({
                "type": "open_viewer", "paneId": "p1", "newPaneId": "v1",
                "source": {"kind": "file", "path": "/home/me/app/PLAN.md"},
            }),
            serde_json::json!({
                "type": "put_annotations", "paneId": "v1",
                "annotations": [{"id": "ann-0000000000000001", "kind": "comment", "body": "Why?"}],
            }),
            serde_json::json!({"type": "mark_sent", "paneId": "v1", "entries": [
                {"id": "ann-0000000000000001", "kind": "comment", "body": "Why?"},
            ]}),
        ] {
            let op: ScreenOp = serde_json::from_value(op).expect("wire op");
            store
                .dispatch_screen(&ScreenId::from("s1"), op)
                .expect("applied");
        }
        store
            .dispatch(Op::AgentReply {
                screen_id: ScreenId::from("s1"),
                pane_id: PaneId::from("v1"),
                number: 1,
                reply: splitty_core::mux::AnnotationReply {
                    id: "rep-00000000000000a1".into(),
                    author: splitty_core::mux::ReplyAuthor::Agent,
                    body: "Because.".into(),
                    sent: false,
                    from: Some(splitty_core::mux::ReplyFrom {
                        pane_id: PaneId::from("p1"),
                        title: "claude".into(),
                    }),
                },
            })
            .expect("agent's reply");
        store
    }

    /// mux-protocol, Change Propagation to Other Clients and Rejected Op
    /// Reporting (Rejected reply): the user's reply, its removal and a
    /// hand-over nudge the other windows and not the sender; a reply under
    /// the ID of an agent's reply, a malformed reply and a malformed
    /// hand-over are reported to the sender only, with the viewer pane, and
    /// change nothing. workspace-model, Layout Operations (No client writes
    /// an agent's reply): an agent's reply, a wait's start or its end sent
    /// by a window has no wire form and changes nothing.
    #[tokio::test]
    async fn replies_and_hand_overs_nudge_the_others_and_a_rejected_reply_answers_the_sender() {
        let store = handed_over();
        let others = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (a, mut sent_a) = window(&store, "window-a", &others);
        let message = |op: Value| serde_json::json!({"type": "mux.op", "screenId": "s1", "op": op});

        for (count, op) in [
            serde_json::json!({
                "type": "put_replies", "paneId": "v1", "annotationId": "ann-0000000000000001",
                "replies": [{"id": "rep-00000000000000b1", "author": "user", "body": "And the timeout?"}],
            }),
            serde_json::json!({"type": "mark_sent", "paneId": "v1", "entries": [
                {"id": "rep-00000000000000b1", "body": "And the timeout?"},
            ]}),
            serde_json::json!({
                "type": "remove_replies", "paneId": "v1", "annotationId": "ann-0000000000000001",
                "ids": ["rep-00000000000000b1"],
            }),
        ]
        .into_iter()
        .enumerate()
        {
            let kind = op["type"].clone();
            assert!(a.try_handle(&message(op).to_string()).await);
            let nudged = others.lock().expect("recorder").clone();
            assert_eq!(nudged.len(), count + 1, "{kind}: {nudged:?}");
            assert_eq!(nudged[count]["type"], "mux.nudge");
            assert_eq!(nudged[count]["reason"], "mux_op");
            assert_eq!(nudged[count]["screenId"], "s1");
            assert!(sent_a.try_recv().is_err(), "{kind}: the sender gets nothing");
        }

        let before = store.get_screen(&ScreenId::from("s1")).expect("screen");
        for (op, kind) in [
            (
                serde_json::json!({
                    "type": "put_replies", "paneId": "v1", "annotationId": "ann-0000000000000001",
                    "replies": [{"id": "rep-00000000000000a1", "author": "user", "body": "Rewritten."}],
                }),
                "put_replies",
            ),
            (
                serde_json::json!({
                    "type": "put_replies", "paneId": "v1", "annotationId": "ann-0000000000000001",
                    "replies": [{"id": "rep-00000000000000b2", "author": "system", "body": "x"}],
                }),
                "put_replies",
            ),
            (
                serde_json::json!({"type": "mark_sent", "paneId": "v1", "entries": []}),
                "mark_sent",
            ),
            (
                serde_json::json!({
                    "type": "remove_replies", "paneId": "v1", "annotationId": "ann-0000000000000001",
                    "ids": ["rep-1"],
                }),
                "remove_replies",
            ),
        ] {
            assert!(a.try_handle(&message(op).to_string()).await);
            let failed = next_text(&mut sent_a);
            assert_eq!(failed["type"], "mux.op_failed", "{failed}");
            assert_eq!(failed["op"], kind);
            assert_eq!(failed["screenId"], "s1");
            assert_eq!(failed["paneId"], "v1");
            assert!(
                failed["error"]
                    .as_str()
                    .is_some_and(|error| !error.is_empty()),
                "{failed}"
            );
            assert!(sent_a.try_recv().is_err(), "nothing else for the sender");
        }
        for op in [
            serde_json::json!({
                "type": "agent_reply", "paneId": "v1", "number": 1,
                "reply": {"id": "rep-00000000000000c1", "author": "agent", "body": "x",
                          "from": {"paneId": "p1", "title": "claude"}},
            }),
            serde_json::json!({
                "type": "start_review_wait", "paneId": "v1",
                "wait": {"id": "wait-00000000000000c1", "paneId": "p1", "title": "claude"},
            }),
            serde_json::json!({
                "type": "end_review_wait", "paneId": "v1", "waitId": "wait-00000000000000c1",
            }),
        ] {
            assert!(a.try_handle(&message(op).to_string()).await);
            assert!(
                sent_a.try_recv().is_err(),
                "a trusted operation is not an op"
            );
        }
        assert!(store.review_wait(&PaneId::from("v1")).is_none());
        assert_eq!(
            others.lock().expect("recorder").len(),
            3,
            "no other client hears of a rejection"
        );
        let after = store.get_screen(&ScreenId::from("s1")).expect("screen");
        assert_eq!(after.revision, before.revision);
        let thread = &after.panes[&PaneId::from("v1")]
            .viewer
            .as_ref()
            .expect("viewer")
            .annotations[0]
            .replies;
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].body, "Because.", "the agent's reply as written");
    }

    /// mux-protocol, Review Answers (The first answer, A stale answer) and
    /// viewer-annotations, Waiting For A Review (Two windows answer): the
    /// first answer naming the wait settles it and reaches the waiting
    /// process; every answer is acknowledged to its sender only, `accepted`
    /// only for the first; an answer to a wait whose process stopped waiting,
    /// or for another pane, changes nothing.
    #[tokio::test]
    async fn the_first_answer_settles_a_wait_and_every_answer_is_acknowledged_to_its_sender() {
        use super::super::reviews::{Outcome, Reviews};
        let store = handed_over();
        let reviews = Arc::new(Reviews::default());
        let others = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (a, mut sent_a) = window_with(&store, "window-a", &others, &reviews);
        let (b, mut sent_b) = window_with(&store, "window-b", &others, &reviews);
        let mut waiting = reviews.register(PaneId::from("v1")).expect("wait");
        let wait = waiting.wait_id().to_owned();
        let answer = |pane: &str, outcome: Value| {
            let mut answer = serde_json::json!({
                "type": "viewer.review_answer", "screenId": "s1", "paneId": pane, "waitId": wait,
            });
            if let (Some(answer), Some(outcome)) = (answer.as_object_mut(), outcome.as_object()) {
                answer.extend(outcome.clone());
            }
            answer.to_string()
        };
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");

        assert!(
            a.try_handle(&answer(
                "v1",
                serde_json::json!({"outcome": "changes_requested", "review": "# Review of PLAN.md"})
            ))
            .await
        );
        assert!(
            b.try_handle(&answer("v1", serde_json::json!({"outcome": "approved"})))
                .await
        );
        assert_eq!(
            next_text(&mut sent_a),
            serde_json::json!({"type": "viewer.review_answered", "paneId": "v1", "waitId": wait, "accepted": true})
        );
        assert_eq!(
            next_text(&mut sent_b),
            serde_json::json!({"type": "viewer.review_answered", "paneId": "v1", "waitId": wait, "accepted": false})
        );
        assert!(sent_a.try_recv().is_err() && sent_b.try_recv().is_err());
        assert_eq!(
            waiting.answer().await,
            Some(Outcome::ChangesRequested("# Review of PLAN.md".into())),
            "the waiting process receives A's review"
        );
        drop(waiting);

        // A stale answer: the process stopped waiting.
        let stopped = reviews.register(PaneId::from("v1")).expect("wait");
        let stale = stopped.wait_id().to_owned();
        drop(stopped);
        let message = serde_json::json!({
            "type": "viewer.review_answer", "screenId": "s1", "paneId": "v1", "waitId": stale,
            "outcome": "approved",
        });
        assert!(a.try_handle(&message.to_string()).await);
        assert_eq!(next_text(&mut sent_a)["accepted"], false);
        // Another pane's answer does not settle the wait of `v1`.
        let mut held = reviews.register(PaneId::from("v1")).expect("wait");
        let mut other_pane = message.clone();
        other_pane["waitId"] = serde_json::json!(held.wait_id());
        other_pane["paneId"] = serde_json::json!("v2");
        assert!(a.try_handle(&other_pane.to_string()).await);
        assert_eq!(next_text(&mut sent_a)["accepted"], false);
        assert_eq!(reviews.held(), 1, "the wait of v1 still waits");
        let answered =
            tokio::time::timeout(std::time::Duration::from_millis(20), held.answer()).await;
        assert!(answered.is_err(), "no answer reached it");

        assert!(
            others.lock().expect("recorder").is_empty(),
            "answers nudge no one; the waiting request does"
        );
        assert_eq!(
            serde_json::to_value(store.get_workspace_state()).expect("workspace"),
            before,
            "answers change no record"
        );
    }
}
