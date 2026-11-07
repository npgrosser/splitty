use super::*;
use serde_json::json;

fn model(ids: &[&str]) -> LocalWorkspace {
    let mut model = LocalWorkspace::new(Uuid::new_v4());
    model.set_catalog_supported(true);
    catalog(&mut model, ids, ids.first().copied());
    model
}
fn catalog(model: &mut LocalWorkspace, ids: &[&str], active: Option<&str>) -> WorkspaceDelta {
    model.catalog_in_flight = Some(model.mutation);
    model
        .apply_catalog(model.connection_id, &catalog_event(ids, active))
        .unwrap()
}
fn catalog_event(ids: &[&str], active: Option<&str>) -> Value {
    json!({"type":"mux.workspace_snapshot", "screens":ids.iter().map(|id| json!({"id":id,"name":id})).collect::<Vec<_>>(), "activeScreenId":active})
}
fn snapshot(id: &str, panes: &[&str]) -> Value {
    json!({"type":"mux.screen_snapshot", "screen":{"id":id,"revision":1,
        "root":{"type":"slot","id":format!("slot-{id}"),"paneIds":panes,"activePaneId":panes.first()},
        "panes":panes.iter().map(|pane| (pane.to_string(),json!({"id":pane}))).collect::<serde_json::Map<_,_>>(),
        "sessions":{}}})
}
fn reserve(model: &mut LocalWorkspace, screen: &str, pane: &str, mutation: u64) {
    let screen_model = ScreenModel::new(
        ScreenIds {
            screen_id: screen.into(),
            pane_id: pane.into(),
        },
        model.connection_id,
    );
    model.models.insert(screen.into(), screen_model);
    model
        .pending_screens
        .insert(screen.into(), PendingScreen { mutation });
    model.mutation = mutation;
    model.activate_created = Some(screen.into());
    model.mark_catalog_dirty();
}

fn terminal_snapshot(screen: &str, pane: &str, session: &str, busy: bool) -> Value {
    let mut event = snapshot(screen, &[pane]);
    event["screen"]["panes"][pane]["sessionId"] = json!(session);
    event["screen"]["sessions"][session] = json!({
        "id":session, "status":"running", "busy":busy
    });
    event
}

#[test]
fn close_plan_counts_unique_sessions_across_hidden_screens_and_rejects_identity_changes() {
    let mut workspace = model(&["a", "b"]);
    let connection = workspace.connection_id;
    workspace
        .apply_snapshot(connection, &terminal_snapshot("a", "pa", "sa", false))
        .unwrap();
    workspace
        .apply_snapshot(connection, &terminal_snapshot("b", "pb", "sb", true))
        .unwrap();
    let plan = workspace.assess_close(7, CloseTarget::Window).unwrap();
    assert_eq!(plan.busy_count, 1, "hidden busy session is included once");
    assert_eq!(plan.unresolved_count, 0);
    assert!(workspace.validate_close(&plan, true).is_ok());
    let mut titled = terminal_snapshot("b", "pb", "sb", true);
    titled["screen"]["panes"]["pb"]["title"] = json!("new title");
    workspace.apply_snapshot(connection, &titled).unwrap();
    assert!(
        workspace.validate_close(&plan, true).is_ok(),
        "title alone is harmless"
    );
    workspace
        .apply_snapshot(
            connection,
            &terminal_snapshot("b", "pb", "replacement", true),
        )
        .unwrap();
    assert!(
        workspace.validate_close(&plan, true).is_err(),
        "same count cannot hide session replacement"
    );
}

#[test]
fn close_plan_requires_consent_for_new_busy_or_unresolved_work() {
    let mut workspace = model(&["a"]);
    let connection = workspace.connection_id;
    workspace
        .apply_snapshot(connection, &terminal_snapshot("a", "pa", "sa", false))
        .unwrap();
    let idle = workspace
        .assess_close(
            1,
            CloseTarget::Pane {
                screen: "a".into(),
                pane: "pa".into(),
            },
        )
        .unwrap();
    assert!(!idle.needs_confirmation());
    workspace
        .apply_snapshot(connection, &terminal_snapshot("a", "pa", "sa", true))
        .unwrap();
    assert!(workspace.validate_close(&idle, false).is_err());
    let busy = workspace
        .assess_close(2, CloseTarget::Screen { screen: "a".into() })
        .unwrap();
    assert!(busy.needs_confirmation());
    assert!(workspace.validate_close(&busy, false).is_err());
    assert!(workspace.validate_close(&busy, true).is_ok());
    workspace
        .models
        .get_mut("a")
        .unwrap()
        .pending_panes
        .insert("new".into());
    assert!(
        workspace.validate_close(&busy, true).is_err(),
        "pending work broadens the close"
    );
}

#[test]
fn pending_screen_and_connection_replacement_invalidate_window_close() {
    let mut workspace = model(&["a"]);
    workspace
        .apply_snapshot(
            workspace.connection_id,
            &terminal_snapshot("a", "pa", "sa", false),
        )
        .unwrap();
    let idle = workspace.assess_close(3, CloseTarget::Window).unwrap();
    assert!(!idle.needs_confirmation());
    reserve(&mut workspace, "pending", "new-pane", 1);
    assert!(
        workspace.validate_close(&idle, false).is_err(),
        "pending Screen adds unresolved work"
    );
    let pending = workspace.assess_close(4, CloseTarget::Window).unwrap();
    assert!(pending.needs_confirmation());
    assert!(pending.unresolved_count > 0);
    workspace.reset_connection(Uuid::new_v4());
    assert!(
        workspace.validate_close(&pending, true).is_err(),
        "old connection approval is never reusable"
    );
}

#[test]
fn catalog_preserves_screen_local_selection_and_ignores_later_host_activation() {
    let mut model = model(&["a", "b"]);
    for (id, panes) in [("a", ["a1", "a2"]), ("b", ["b1", "b2"])] {
        model
            .apply_snapshot(model.connection_id, &snapshot(id, &panes))
            .unwrap();
    }
    model.select_tab("a2");
    model.shown = Some("b".into());
    model.select_tab("b2");
    catalog(&mut model, &["b", "a"], Some("a"));
    assert_eq!(model.shown_screen_id(), Some("b"));
    assert_eq!(model.screen("a").unwrap().focused_pane(), Some("a2"));
    assert_eq!(model.screen("b").unwrap().focused_pane(), Some("b2"));
    model.shown = Some("a".into());
    assert_eq!(
        model.selected_pane(&model.snapshot().unwrap().root),
        Some("a2")
    );
}

#[test]
fn older_catalog_does_not_cancel_new_creation_and_refresh_is_coalesced() {
    let mut model = model(&["a"]);
    model.mark_catalog_dirty();
    model.flush_catalog_request(|| Ok(())).unwrap(); // observes mutation 0
    reserve(&mut model, "new", "new-pane", 1);
    model.observe_spawn(model.connection_id, "new-pane", "new-session");
    model
        .flush_catalog_request(|| panic!("one request is already in flight"))
        .unwrap();
    model
        .apply_catalog(model.connection_id, &catalog_event(&["a"], Some("a")))
        .unwrap();
    assert_eq!(model.session_for("new-pane"), Some("new-session"));
    assert!(model.has_pending_create());
    assert_eq!(model.shown_screen_id(), Some("a"));
    model.flush_catalog_request(|| Ok(())).unwrap();
    assert_eq!(model.catalog_in_flight, Some(1));
    model
        .apply_catalog(
            model.connection_id,
            &catalog_event(&["a", "new"], Some("a")),
        )
        .unwrap();
    assert!(!model.has_pending_create());
    assert_eq!(model.shown_screen_id(), Some("new"));
    assert_eq!(
        model.admission_count(),
        1,
        "route and pending pane count once"
    );
}

#[test]
fn failed_refresh_keeps_accepted_reservation_and_can_retry() {
    let mut model = model(&[]);
    reserve(&mut model, "new", "pane", 1);
    let error = model
        .flush_catalog_request(|| Err(capacity("queue full")))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(model.catalog_dirty);
    assert_eq!(model.catalog_in_flight, None);
    assert_eq!(model.admission_count(), 1);
    model.flush_catalog_request(|| Ok(())).unwrap();
    model
        .apply_catalog(model.connection_id, &catalog_event(&["new"], Some("new")))
        .unwrap();
    assert_eq!(model.shown_screen_id(), Some("new"));
    assert_eq!(model.admission_count(), 1);
}

#[test]
fn admission_is_global_and_failed_spawn_and_closed_screen_release_it() {
    let mut model = model(&["a", "b"]);
    for i in 0..MAX_ATTACHMENTS {
        let id = if i % 2 == 0 { "a" } else { "b" };
        model
            .models
            .get_mut(id)
            .unwrap()
            .pending_panes
            .insert(format!("pane-{i}"));
    }
    assert!(model.admit_new_pane().is_err());
    model.observe_spawn(model.connection_id, "pane-0", "session-0");
    assert_eq!(model.admission_count(), MAX_ATTACHMENTS);
    model.observe_spawn_failure(model.connection_id, "pane-1");
    assert!(model.admit_new_pane().is_ok());
    let delta = catalog(&mut model, &["b"], Some("b"));
    assert_eq!(delta.removed.len(), 1);
    assert_eq!(model.admission_count(), MAX_ATTACHMENTS / 2 - 1);
}

#[test]
fn rejected_catalog_preserves_routes_and_selection_atomically() {
    let mut model = model(&["a"]);
    model
        .models
        .get_mut("a")
        .unwrap()
        .pending_panes
        .insert("pane".into());
    model.observe_spawn(model.connection_id, "pane", "session");
    model.catalog_in_flight = Some(0);
    let oversized: Vec<_> = (0..=MAX_LOCAL_SCREENS)
        .map(|i| format!("screen-{i}"))
        .collect();
    let ids: Vec<_> = oversized.iter().map(String::as_str).collect();
    assert!(model
        .apply_catalog(model.connection_id, &catalog_event(&ids, None))
        .is_err());
    assert_eq!(model.catalog_in_flight, None);
    model.mark_catalog_dirty();
    model.flush_catalog_request(|| Ok(())).unwrap();
    assert!(model
        .apply_catalog(model.connection_id, &catalog_event(&["a", "a"], None))
        .is_err());
    assert_eq!(model.shown_screen_id(), Some("a"));
    assert_eq!(model.session_for("pane"), Some("session"));
    assert_eq!(model.screens().len(), 1);
    model.mark_catalog_dirty();
    model.flush_catalog_request(|| Ok(())).unwrap();
    model
        .apply_catalog(model.connection_id, &catalog_event(&["a", "b"], Some("b")))
        .unwrap();
    assert_eq!(model.screens().len(), 2);
    assert_eq!(model.shown_screen_id(), Some("a"));
}

#[test]
fn removed_screen_and_old_generation_cannot_reenter_routes() {
    let mut model = model(&["a"]);
    let old = model.connection_id;
    model
        .apply_snapshot(old, &snapshot("a", &["pane"]))
        .unwrap();
    model.observe_spawn(old, "pane", "session");
    catalog(&mut model, &[], None);
    assert!(model.shown_screen_id().is_none());
    assert!(model.observe_spawn(old, "pane", "late").added.is_empty());
    assert!(
        !model
            .apply_snapshot(old, &snapshot("a", &["pane"]))
            .unwrap()
            .snapshot_changed
    );
    assert!(model.routes().is_empty());
    model.reset_connection(Uuid::new_v4());
    model.set_catalog_supported(true);
    model.catalog_in_flight = Some(0);
    model
        .apply_catalog(old, &catalog_event(&["a"], Some("a")))
        .unwrap();
    assert!(model.screens().is_empty());
    assert_eq!(
        model.catalog_in_flight,
        Some(0),
        "stale response does not consume current query"
    );
}

#[test]
fn unknown_active_hint_uses_first_catalog_entry() {
    let mut model = LocalWorkspace::new(Uuid::new_v4());
    model.set_catalog_supported(true);
    catalog(&mut model, &["a", "b"], Some("unlisted"));
    assert_eq!(model.shown_screen_id(), Some("a"));
}

#[test]
fn snapshot_cannot_claim_another_screens_pane_or_session() {
    let mut model = model(&["a", "b"]);
    model
        .apply_snapshot(model.connection_id, &snapshot("a", &["pane"]))
        .unwrap();
    model.observe_spawn(model.connection_id, "pane", "session");
    let before = model.screen("b").unwrap().snapshot().cloned();
    assert!(model
        .apply_snapshot(model.connection_id, &snapshot("b", &["pane"]))
        .is_err());
    assert_eq!(
        model.screen("b").unwrap().snapshot().is_some(),
        before.is_some()
    );
    model
        .apply_snapshot(model.connection_id, &snapshot("b", &["other"]))
        .unwrap();
    assert!(model
        .observe_spawn(model.connection_id, "other", "session")
        .added
        .is_empty());
    assert_eq!(model.session_for("pane"), Some("session"));
    assert!(model.session_for("other").is_none());
}

#[cfg(not(target_os = "windows"))]
mod service {
    use super::*;
    use crate::{EmbeddedService, Event};
    use std::time::Duration;

    struct Window {
        client: Client,
        model: LocalWorkspace,
        output: HashMap<String, Vec<u8>>,
    }
    impl Window {
        async fn next(&mut self) {
            self.model.flush_catalog(&self.client).unwrap();
            let event: Event =
                tokio::time::timeout(Duration::from_secs(10), self.client.next_event())
                    .await
                    .expect("native event timeout")
                    .expect("native event")
                    .expect("open connection");
            if let Some(output) = event.output() {
                self.output
                    .entry(output.session_id.into())
                    .or_default()
                    .extend_from_slice(output.bytes);
            }
            let Some(value) = event.json() else {
                return;
            };
            let delta = match value["type"].as_str() {
                Some("mux.workspace_snapshot") => self
                    .model
                    .apply_catalog(event.connection_id, value)
                    .unwrap(),
                Some("mux.screen_snapshot") => self
                    .model
                    .apply_snapshot(event.connection_id, value)
                    .unwrap(),
                Some("mux.pane_spawned") => self.model.observe_spawn(
                    event.connection_id,
                    value["paneId"].as_str().unwrap(),
                    value["sessionId"].as_str().unwrap(),
                ),
                Some("attached") => {
                    self.model.observe_attached(
                        event.connection_id,
                        value["sessionId"].as_str().unwrap(),
                    );
                    WorkspaceDelta::default()
                }
                Some("mux.op_failed") => panic!("unexpected rejected operation: {value}"),
                _ => WorkspaceDelta::default(),
            };
            for route in delta.removed {
                self.client.detach(&route.session_id).unwrap();
            }
            for route in delta.added {
                self.client.attach(&route.session_id).unwrap();
            }
        }
        async fn until(&mut self, done: impl Fn(&Self) -> bool) {
            tokio::time::timeout(Duration::from_secs(15), async {
                while !done(self) {
                    self.next().await;
                }
            })
            .await
            .expect("workspace condition before timeout");
        }
        fn has_output(&self, session: &str, needle: &[u8]) -> bool {
            self.output
                .get(session)
                .is_some_and(|output| output.windows(needle.len()).any(|part| part == needle))
        }
        fn ready(&self, ids: &ScreenIds) -> bool {
            self.model
                .screen(&ids.screen_id)
                .is_some_and(|screen| screen.snapshot().is_some())
                && self
                    .model
                    .session_for(&ids.pane_id)
                    .is_some_and(|session| self.model.session_ready(session))
        }
    }

    #[tokio::test]
    async fn local_screens_keep_live_sessions_through_switch_rename_close_and_reopen() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let service = EmbeddedService::start();
        let mut client = service.connect().await.unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.welcome")
            {
                break;
            }
        }
        let mut model = LocalWorkspace::new(client.connection_id());
        model.set_catalog_supported(true);
        model.set_tab_selection_supported(true);
        model.mark_catalog_dirty();
        let mut window = Window {
            client,
            model,
            output: HashMap::new(),
        };
        window
            .until(|window| window.model.catalog_initialized())
            .await;
        let a = window.model.create_screen(&window.client, "First").unwrap();
        window.until(|window| window.ready(&a)).await;
        let session_a = window.model.session_for(&a.pane_id).unwrap().to_owned();
        window.client.activity(true).unwrap();
        window.client.resize(&session_a, 91, 29, true).unwrap();
        window
            .client
            .input(&session_a, "printf '\\nNATIVE_A_%s\\n' \"$$\"\n")
            .unwrap();
        window
            .until(|window| window.has_output(&session_a, b"NATIVE_A_"))
            .await;
        let b = window
            .model
            .create_screen(&window.client, "Second")
            .unwrap();
        window
            .until(|window| window.ready(&b) && window.model.screens().len() == 2)
            .await;
        let session_b = window.model.session_for(&b.pane_id).unwrap().to_owned();
        assert_ne!(session_a, session_b);
        assert_eq!(window.model.shown_screen_id(), Some(b.screen_id.as_str()));
        window.client.hide(&session_a).unwrap();
        window.client.resize(&session_b, 80, 24, true).unwrap();
        // Output and a real terminal-status reply continue on the hidden route.
        window.client.input(&session_a,"bash -c 'printf \"\\033[6n\"; IFS= read -r -s -d R reply; printf \"\\nHIDDEN_REPLY_OK\\n\"'\n").unwrap();
        window
            .until(|window| window.has_output(&session_a, b"\x1b[6n"))
            .await;
        window.client.input(&session_a, "\x1b[1;1R").unwrap();
        window
            .until(|window| window.has_output(&session_a, b"\r\nHIDDEN_REPLY_OK\r\n"))
            .await;
        window
            .model
            .rename_screen(&window.client, &a.screen_id, "Renamed")
            .unwrap();
        window
            .until(|window| {
                window.model.screens().iter().any(|screen| {
                    screen.id.as_str() == a.screen_id && screen.name.as_deref() == Some("Renamed")
                })
            })
            .await;
        window
            .model
            .switch_screen(&window.client, &a.screen_id)
            .unwrap();
        assert_eq!(
            window.model.session_for(&a.pane_id),
            Some(session_a.as_str())
        );
        assert!(window.model.session_ready(&session_a));
        window.client.hide(&session_b).unwrap();
        window.client.resize(&session_a, 91, 29, true).unwrap();
        window
            .model
            .close_screen(&window.client, &b.screen_id)
            .unwrap();
        window
            .until(|window| window.model.screens().len() == 1)
            .await;
        assert_eq!(
            window.model.session_for(&a.pane_id),
            Some(session_a.as_str())
        );
        assert!(window.model.session_for(&b.pane_id).is_none());
        assert!(window
            .model
            .observe_spawn(window.model.connection_id(), &b.pane_id, &session_b)
            .added
            .is_empty());
        window
            .model
            .close_screen(&window.client, &a.screen_id)
            .unwrap();
        window
            .until(|window| window.model.screens().is_empty())
            .await;
        assert!(window.model.routes().is_empty());
        assert!(window.model.shown_screen_id().is_none());
        let c = window
            .model
            .create_screen(&window.client, "New screen")
            .unwrap();
        window.until(|window| window.ready(&c)).await;
        assert_ne!(c.screen_id, a.screen_id);
        assert_ne!(
            window.model.session_for(&c.pane_id),
            Some(session_a.as_str())
        );
        window.client.finish().await.unwrap();
        service.close().await.unwrap();
    }
}
