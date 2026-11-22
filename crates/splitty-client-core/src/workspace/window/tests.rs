use super::*;
use serde_json::json;

fn catalog(window: &mut WindowWorkspace, id: SourceId, ids: &[&str]) {
    let mut source = window.source_mut(id).unwrap();
    source.set_catalog_supported(true);
    source.mark_catalog_dirty();
    source.flush_catalog_request(|| Ok(())).unwrap();
    let connection = source.connection_id();
    source.apply_catalog(connection, &json!({"type":"mux.workspace_snapshot", "screens":ids.iter().map(|id| json!({"id":id,"name":id})).collect::<Vec<_>>(), "activeScreenId":ids.last()})).unwrap();
    drop(source);
    if let Some(action) = window.host(id).map(|host| host.action_target().clone()) {
        window.host_mut(&action).unwrap().catalog = CatalogState::Ready;
    }
}
fn remote(window: &mut WindowWorkspace, target: &str) -> SourceId {
    let id = window
        .admit_remote(window.lifecycle_token(), target)
        .unwrap();
    window
        .bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())
        .unwrap();
    assert_eq!(window.execution_token(id), None);
    let owner = window.source(id).unwrap().source_token();
    window.confirm_ready(owner).unwrap();
    id
}
fn open(window: &mut WindowWorkspace, id: SourceId, screen: &str) -> ScreenKey {
    let source = window.source(id).unwrap();
    let key = source.screen_key(screen);
    window.open_existing(source.source_token(), &key).unwrap();
    key
}
fn snapshot(
    window: &mut WindowWorkspace,
    id: SourceId,
    screen: &str,
    count: usize,
) -> io::Result<WorkspaceDelta> {
    let panes: Vec<_> = (0..count).map(|index| format!("p{index}")).collect();
    let event = json!({"type":"mux.screen_snapshot","screen":{"id":screen,"revision":1,
        "root":{"type":"slot","id":"slot","paneIds":panes,"activePaneId":panes.first()},
        "panes":panes.iter().map(|pane| (pane.clone(),json!({"id":pane,"sessionId":format!("s-{pane}")}))).collect::<serde_json::Map<_,_>>(),
        "sessions":panes.iter().map(|pane| (format!("s-{pane}"),json!({"id":format!("s-{pane}"),"status":"running"}))).collect::<serde_json::Map<_,_>>()}});
    let mut source = window.source_mut(id).unwrap();
    let connection = source.connection_id();
    source.apply_snapshot(connection, &event)
}

#[test]
fn remote_catalog_is_metadata_and_only_explicit_refs_survive_missing_and_rebind() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let id = remote(&mut window, "alice@example.test");
    catalog(&mut window, id, &["a", "b"]);
    assert!(window.opened().is_empty());
    assert_eq!(window.source(id).unwrap().live_catalog().unwrap().len(), 2);
    assert!(window.source(id).unwrap().screen("a").is_none());
    snapshot(&mut window, id, "a", 1).unwrap();
    assert!(window.routes().is_empty());
    let key = open(&mut window, id, "a");
    snapshot(&mut window, id, "a", 1).unwrap();
    catalog(&mut window, id, &["b"]);
    assert_eq!(window.shown_key(), Some(key.clone()));
    assert_eq!(window.opened()[0].state(), OpenedScreenState::Missing);
    assert!(window.shown().is_none());
    assert!(window.source(id).unwrap().screen("b").is_none());
    window.disconnect(id).unwrap();
    assert!(!window.host(id).unwrap().can_retry());
    window.finish_attempt(id).unwrap();
    window.retry(id).unwrap();
    window
        .bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())
        .unwrap();
    let owner = window.source(id).unwrap().source_token();
    window.confirm_ready(owner).unwrap();
    catalog(&mut window, id, &["a", "b"]);
    assert_eq!(window.opened().len(), 1);
    assert_eq!(window.shown_key(), Some(key.clone()));
    window.detach(window.lifecycle_token(), &key).unwrap();
    snapshot(&mut window, id, "a", 1).unwrap();
    catalog(&mut window, id, &["a", "b"]);
    assert!(window.opened().is_empty());
    assert!(window.routes().is_empty());
}

#[test]
fn colliding_ids_route_hidden_replay_and_retain_offline_bindings_without_execution() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let local = window.local_source().source_id();
    let first = remote(&mut window, "one.test");
    let second = remote(&mut window, "two.test");
    for id in [local, first, second] {
        catalog(&mut window, id, &["same"]);
        if id != local {
            open(&mut window, id, "same");
        }
        snapshot(&mut window, id, "same", 1).unwrap();
    }
    assert_eq!(window.routes().len(), 3);
    let tokens: Vec<_> = [local, first, second]
        .map(|id| window.source(id).unwrap().terminal_token("p0").unwrap())
        .into();
    assert_ne!(tokens[0], tokens[1]);
    assert_ne!(tokens[1], tokens[2]);
    for token in &tokens {
        assert!(window.contains_terminal_binding(token));
        assert!(!window.accepts_terminal(token));
        let route = window
            .route_for_session(token.source_token(), "s-p0")
            .unwrap();
        assert_eq!(route.terminal(), token);
    }
    let mut source = window.source_mut(first).unwrap();
    let connection = source.connection_id();
    source.observe_attached(connection, "s-p0");
    drop(source);
    assert!(window.accepts_terminal(&tokens[1]));
    window.disconnect(first).unwrap();
    assert!(window.contains_terminal_binding(&tokens[1]));
    assert!(!window.accepts_terminal(&tokens[1]));
    assert!(window
        .route_for_session(tokens[1].source_token(), "s-p0")
        .is_none());
    assert_eq!(window.routes().len(), 3);
    window.finish_attempt(first).unwrap();
    window.retry(first).unwrap();
    window
        .bind_remote_parts(first, Uuid::new_v4(), ConnectionBudget::default())
        .unwrap();
    assert!(!window.contains_terminal_binding(&tokens[1]));
    assert!(window.confirm_ready(tokens[1].source_token()).is_err());
    assert_eq!(window.execution_token(first), None);
    let current = window.source(first).unwrap().source_token();
    window.confirm_ready(current).unwrap();
    catalog(&mut window, first, &["same"]);
    snapshot(&mut window, first, "same", 1).unwrap();
    assert_eq!(window.routes().len(), 3);
    assert_ne!(
        window.source(first).unwrap().terminal_token("p0").unwrap(),
        tokens[1]
    );
}

#[test]
fn grouped_navigation_detach_and_source_cleanup_choose_surviving_neighbor() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let owner = window.lifecycle_token();
    let local = window.local_source().source_id();
    catalog(&mut window, local, &["local"]);
    let first = remote(&mut window, "one.test");
    let second = remote(&mut window, "two.test");
    catalog(&mut window, first, &["a", "b"]);
    catalog(&mut window, second, &["a"]);
    let a = open(&mut window, first, "a");
    let b = open(&mut window, first, "b");
    let next = open(&mut window, second, "a");
    let local_key = window.local_source().screen_key("local");
    window
        .set_host_order(owner, &["two.test".into(), "one.test".into()])
        .unwrap();
    assert_eq!(
        window
            .opened()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec![local_key.clone(), next.clone(), a.clone(), b.clone()]
    );
    window.select(owner, &local_key).unwrap();
    assert!(!window.cycle(owner, TabStep::Previous).unwrap());
    assert!(window.cycle(owner, TabStep::Next).unwrap());
    assert_eq!(window.shown_key(), Some(next.clone()));
    assert!(window.cycle(owner, TabStep::Next).unwrap());
    assert_eq!(window.shown_key(), Some(a.clone()));
    assert!(window
        .select(window.source(first).unwrap().source_token(), &local_key)
        .is_err());
    assert_eq!(window.shown_key(), Some(a.clone()));
    window.set_host_order(owner, &[]).unwrap();
    window.select(owner, &a).unwrap();
    window.detach(owner, &a).unwrap();
    assert_eq!(window.shown_key(), Some(b.clone()));
    window.detach(owner, &b).unwrap();
    assert_eq!(window.shown_key(), Some(next.clone()));
    window.begin_close(second).unwrap();
    window.finish_close(second).unwrap();
    assert_eq!(
        window.shown_key(),
        Some(window.local_source().screen_key("local"))
    );
    let a = open(&mut window, first, "a");
    open(&mut window, first, "b");
    window
        .select(owner, &window.local_source().screen_key("local"))
        .unwrap();
    catalog(&mut window, local, &[]);
    assert_eq!(window.shown_key(), Some(a));
    assert_eq!(window.source(first).unwrap().shown_screen_id(), Some("a"));
}

#[test]
fn opening_and_source_reservations_are_held_until_owned_cleanup_finishes() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let owner = window.lifecycle_token();
    let ids: Vec<_> = (0..MAX_PENDING_REMOTE_OPENS)
        .map(|i| {
            window
                .admit_remote(owner, &format!("host{i}.test"))
                .unwrap()
        })
        .collect();
    assert_eq!(window.admit_remote(owner, "host0.test").unwrap(), ids[0]);
    assert_eq!(window.source_for_target("host0.test"), Some(ids[0]));
    assert!(
        window.source(ids[0]).is_none(),
        "preparation never invents a Client"
    );
    window.begin_close(ids[0]).unwrap();
    assert!(
        window.disconnect(ids[0]).is_err(),
        "a late read failure cannot reopen Closing state"
    );
    window.disconnect(ids[1]).unwrap();
    assert!(window.retry(ids[1]).is_err());
    assert!(window.admit_remote(owner, "next.test").is_err());
    window.finish_attempt(ids[1]).unwrap();
    window.admit_remote(owner, "next.test").unwrap();
    assert!(window.retry(ids[1]).is_err());
    window.finish_close(ids[0]).unwrap();
    window.retry(ids[1]).unwrap();
    for id in ids.into_iter().skip(1) {
        window
            .bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())
            .unwrap();
        window
            .confirm_ready(window.source(id).unwrap().source_token())
            .unwrap();
    }
    for i in 0..4 {
        remote(&mut window, &format!("ready{i}.test"));
    }
    assert_eq!(window.remotes.len(), MAX_REMOTE_SOURCES);
    assert!(window.admit_remote(owner, "overflow.test").is_err());
    assert!(window.admit_remote(owner, " ssh://bad ").is_err());
    assert!(window
        .admit_remote(
            SourceWorkspace::new(Uuid::new_v4()).source_token(),
            "foreign.test"
        )
        .is_err());
}

#[test]
fn window_admission_counts_offline_sources_and_releases_only_detached_models() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let first = remote(&mut window, "one.test");
    let second = remote(&mut window, "two.test");
    catalog(&mut window, first, &["same"]);
    catalog(&mut window, second, &["same"]);
    let key = open(&mut window, first, "same");
    open(&mut window, second, "same");
    snapshot(&mut window, first, "same", MAX_ATTACHMENTS).unwrap();
    window.disconnect(first).unwrap();
    assert!(snapshot(&mut window, second, "same", 1).is_err());
    assert!(window
        .source(second)
        .unwrap()
        .screen("same")
        .unwrap()
        .snapshot()
        .is_none());
    window.detach(window.lifecycle_token(), &key).unwrap();
    snapshot(&mut window, second, "same", 1).unwrap();
    assert_eq!(window.routes().len(), 1);
}

#[test]
fn remote_catalog_charge_survives_view_clone_and_rejected_replacement() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let id = window
        .admit_remote(window.lifecycle_token(), "one.test")
        .unwrap();
    let budget = ConnectionBudget::default();
    window
        .bind_remote_parts(id, Uuid::new_v4(), budget.clone())
        .unwrap();
    window
        .confirm_ready(window.source(id).unwrap().source_token())
        .unwrap();
    catalog(&mut window, id, &["a", "b"]);
    let charged = budget.used();
    assert!(charged > 0);
    let view = window.clone();
    let mut source = window.source_mut(id).unwrap();
    source.mark_catalog_dirty();
    source.flush_catalog_request(|| Ok(())).unwrap();
    let connection = source.connection_id();
    assert!(source.apply_catalog(connection, &json!({"type":"mux.workspace_snapshot", "screens":[{"id":"duplicate"},{"id":"duplicate"}]})).is_err());
    assert_eq!(budget.used(), charged);
    assert_eq!(source.live_catalog().unwrap().len(), 2);
    drop(source);
    window.begin_close(id).unwrap();
    window.finish_close(id).unwrap();
    assert_eq!(budget.used(), charged);
    drop(view);
    assert_eq!(budget.used(), 0);
}

#[test]
fn aggregate_screen_allowance_rejects_open_without_partial_reference() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let local = window.local_source().source_id();
    let names: Vec<_> = (0..catalog::MAX_LOCAL_SCREENS)
        .map(|index| format!("screen{index}"))
        .collect();
    let names: Vec<_> = names.iter().map(String::as_str).collect();
    catalog(&mut window, local, &names);
    let id = remote(&mut window, "one.test");
    catalog(&mut window, id, &["extra"]);
    let source = window.source(id).unwrap();
    let key = source.screen_key("extra");
    assert!(window.open_existing(source.source_token(), &key).is_err());
    assert!(window.source(id).unwrap().screens().is_empty());
    assert!(window.source(id).unwrap().screen("extra").is_none());
    catalog(&mut window, local, &names[..names.len() - 1]);
    open(&mut window, id, "extra");
    assert_eq!(window.opened().len(), catalog::MAX_LOCAL_SCREENS);
    let action = window.host(id).unwrap().action_target().clone();
    assert!(window
        .prepare_remote_screen_creation(&action, window.execution_token(id).unwrap())
        .is_err());
}

#[test]
fn unarranged_groups_follow_first_open_not_browse_order() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let owner = window.lifecycle_token();
    let a = remote(&mut window, "a.test");
    let b = remote(&mut window, "b.test");
    catalog(&mut window, a, &["same"]);
    catalog(&mut window, b, &["same"]);
    let b_key = open(&mut window, b, "same");
    let a_key = open(&mut window, a, "same");
    assert_eq!(
        window
            .opened()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec![b_key.clone(), a_key.clone()]
    );
    window.set_host_order(owner, &["a.test".into()]).unwrap();
    assert_eq!(
        window
            .opened()
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec![a_key, b_key]
    );
}

#[test]
fn retained_failed_catalog_cannot_admit_new_reference_but_keeps_existing_navigation() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let id = remote(&mut window, "freshness.test");
    catalog(&mut window, id, &["opened", "retained"]);
    let existing = open(&mut window, id, "opened");
    let owner = window.execution_token(id).unwrap();
    let retained = window.source(id).unwrap().screen_key("retained");
    let action = window.host(id).unwrap().action_target().clone();
    for state in [
        CatalogState::Failed,
        CatalogState::Loading,
        CatalogState::Unrequested,
    ] {
        window.host_mut(&action).unwrap().catalog = state;
        assert!(window.open_existing(owner, &retained).is_err());
        assert_eq!(window.source(id).unwrap().screens().len(), 1);
        assert_eq!(window.shown_key(), Some(existing.clone()));
        window.open_existing(owner, &existing).unwrap();
    }
    window.host_mut(&action).unwrap().catalog = CatalogState::Ready;
    window.open_existing(owner, &retained).unwrap();
    assert_eq!(window.source(id).unwrap().screens().len(), 2);
    assert_eq!(window.shown_key(), Some(retained));
}

#[test]
fn reconnect_keeps_valid_tab_and_focus_choices_but_revokes_routes_and_old_revisions() {
    fn layout(revision: u64, keep_right: bool) -> serde_json::Value {
        let mut panes = serde_json::json!({
            "left-a":{"id":"left-a","sessionId":"s-a"},
            "left-b":{"id":"left-b","sessionId":"s-b"}
        });
        let mut sessions = serde_json::json!({
            "s-a":{"id":"s-a","status":"running"},
            "s-b":{"id":"s-b","status":"running"}
        });
        let left = json!({"type":"slot","id":"left","paneIds":["left-a","left-b"],"activePaneId":"left-a"});
        let root = if keep_right {
            panes["right"] = json!({"id":"right","sessionId":"s-right"});
            sessions["s-right"] = json!({"id":"s-right","status":"running"});
            json!({"type":"split","id":"root","axis":"horizontal","sizes":[0.5,0.5],"children":[left,{"type":"slot","id":"right-slot","paneIds":["right"],"activePaneId":"right"}]})
        } else {
            left
        };
        json!({"type":"mux.screen_snapshot","screen":{"id":"a","revision":revision,"root":root,"panes":panes,"sessions":sessions}})
    }
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let id = remote(&mut window, "focus.test");
    catalog(&mut window, id, &["a"]);
    let key = open(&mut window, id, "a");
    let old_owner = window.execution_token(id).unwrap();
    {
        let mut source = window.source_mut(id).unwrap();
        source
            .apply_snapshot(old_owner.connection_id(), &layout(77, true))
            .unwrap();
        assert!(source.select_tab("left-b"));
        assert!(source.focus_pane("right"));
        source.observe_attached(old_owner.connection_id(), "s-right");
    }
    let old_route = window.source(id).unwrap().terminal_token("right").unwrap();
    assert!(window.accepts_terminal(&old_route));
    for keep_right in [true, false] {
        window.disconnect(id).unwrap();
        window.finish_attempt(id).unwrap();
        window.retry(id).unwrap();
        window
            .bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())
            .unwrap();
        let owner = window.source(id).unwrap().source_token();
        assert!(window.routes().is_empty());
        assert!(window.shown().unwrap().snapshot().is_none());
        assert!(!window.contains_terminal_binding(&old_route));
        assert!(!window.accepts_terminal(&old_route));
        window.confirm_ready(owner).unwrap();
        catalog(&mut window, id, &["a"]);
        window
            .source_mut(id)
            .unwrap()
            .apply_snapshot(owner.connection_id(), &layout(1, keep_right))
            .unwrap();
        assert_eq!(window.shown_key(), Some(key.clone()));
        assert_eq!(window.shown().unwrap().snapshot().unwrap().revision, 1);
        assert_eq!(
            window.shown().unwrap().focused_pane(),
            Some(if keep_right { "right" } else { "left-b" })
        );
        assert!(window
            .shown()
            .unwrap()
            .displayed_panes()
            .contains(&"left-b".into()));
        assert!(!window.accepts_terminal(&old_route));
        if keep_right {
            let route = window.source(id).unwrap().terminal_token("right").unwrap();
            assert_ne!(route.source_token(), old_owner);
            assert!(
                !window.accepts_terminal(&route),
                "fresh attachment still required"
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn confirmed_forget_directory_reconciliation_preserves_open_remote_bindings() {
    use crate::hub::{
        HostDirectoryState, HostManagementOutcome, HostManagementRequest, HubClient, HubEvent,
    };
    let (runtime, path) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    state
        .hub
        .update_host_preference(json!({"target":"open.test","alias":"Open work"}))
        .unwrap();
    state
        .hub
        .set_host_order(json!({"order":["open.test"]}))
        .unwrap();
    let service: crate::owned_service::OwnedService =
        crate::EmbeddedService::from_runtime(runtime).into();
    let local = service.connect().await.unwrap();
    let mut window = WindowWorkspace::new(local.connection_id());
    let owner = window.lifecycle_token();
    let id = remote(&mut window, "open.test");
    catalog(&mut window, id, &["keep"]);
    let key = open(&mut window, id, "keep");
    snapshot(&mut window, id, "keep", 1).unwrap();
    let route = window.routes()[0].terminal().clone();
    let action = window.host(id).unwrap().action_target().clone();
    let mut hub = HubClient::new(service.api_stream().await.unwrap(), owner.connection_id());
    hub.enqueue(
        owner,
        Uuid::new_v4(),
        HostManagementRequest::forget("open.test").unwrap(),
    )
    .unwrap();
    let HubEvent::Management(result) = hub.next_event().await else {
        panic!("metadata ack")
    };
    assert_eq!(result.outcome(), HostManagementOutcome::Applied);
    loop {
        if let HostDirectoryState::Ready(snapshot) = hub.next_update().await.state {
            assert!(snapshot.directory().hosts.is_empty());
            assert!(snapshot.directory().order.is_empty());
            window
                .set_host_order(owner, &snapshot.directory().order)
                .unwrap();
            break;
        }
    }
    assert_eq!(window.shown_key(), Some(key));
    assert_eq!(window.host(id).unwrap().action_target(), &action);
    assert_eq!(window.routes()[0].terminal(), &route);
    assert!(window.host(id).unwrap().is_browsing());
    assert!(state
        .hub
        .update_catalog(json!({"target":"open.test","screens":[{"id":"keep","panes":1}]}))
        .is_err());
    assert!(
        state.hub.directory().hosts.is_empty(),
        "live observations cannot resurrect forgotten metadata"
    );
    hub.finish().await.unwrap();
    local.finish().await.unwrap();
    service.close().await.unwrap();
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn disconnect_and_rebind_clear_live_port_observations_without_detaching_screens() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let id = remote(&mut window, "ports.test");
    catalog(&mut window, id, &["work"]);
    let key = open(&mut window, id, "work");
    let owner = window.source(id).unwrap().source_token();
    window
        .source_mut(id)
        .unwrap()
        .observe_ports(
            owner,
            &json!({"ports":[{"port":3000,"sessionId":"session","process":"node"}]}),
            &ConnectionBudget::default(),
        )
        .unwrap();
    assert!(window.source(id).unwrap().port_observation().is_some());
    window.disconnect(id).unwrap();
    assert!(window.source(id).unwrap().port_observation().is_none());
    assert_eq!(window.shown_key(), Some(key));
    window.finish_attempt(id).unwrap();
    window.retry(id).unwrap();
    window
        .bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())
        .unwrap();
    let replacement = window.source(id).unwrap().source_token();
    window.confirm_ready(replacement).unwrap();
    assert!(!window
        .source_mut(id)
        .unwrap()
        .observe_ports(owner, &json!({"ports":[]}), &ConnectionBudget::default())
        .unwrap());
    assert!(window.source(id).unwrap().port_observation().is_none());
}

#[test]
fn listed_screens_open_together_and_a_host_detaches_as_a_whole() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let owner = window.lifecycle_token();
    let host = remote(&mut window, "one.test");
    let other = remote(&mut window, "two.test");
    catalog(&mut window, host, &["a", "b", "c"]);
    catalog(&mut window, other, &["x"]);
    let kept = open(&mut window, other, "x");
    let token = window.source(host).unwrap().source_token();
    let key = |window: &WindowWorkspace, id: &str| window.source(host).unwrap().screen_key(id);
    let (a, c, ended) = (key(&window, "a"), key(&window, "c"), key(&window, "ended"));
    let on_host = |window: &WindowWorkspace| {
        window
            .opened()
            .into_iter()
            .map(|entry| entry.key)
            .filter(|key| key.source_id() == host)
            .collect::<Vec<_>>()
    };

    // A Screen that has ended is left out, and the first listed one is shown.
    assert!(window
        .open_listed(token, &[a.clone(), ended.clone(), c.clone()])
        .unwrap());
    assert_eq!(on_host(&window), [a.clone(), c.clone()]);
    assert_eq!(window.shown_key(), Some(a.clone()));
    assert!(!window.open_listed(token, &[c.clone(), a.clone()]).unwrap());
    assert_eq!(window.shown_key(), Some(c.clone()));
    assert!(window.open_listed(token, &[ended]).is_err());
    assert!(window
        .open_listed(token, &[a.clone(), kept.clone()])
        .is_err());
    assert!(window.open_listed(token, &[]).is_err());
    assert_eq!(on_host(&window), [a, c]);

    // Closing the host removes its references and nothing else.
    assert!(window.detach_all(token, host).is_err());
    window.detach_all(owner, host).unwrap();
    assert!(on_host(&window).is_empty());
    assert_eq!(window.shown_key(), Some(kept));
    assert_eq!(
        window.source(host).unwrap().live_catalog().unwrap().len(),
        3
    );
}

/// Reuse the canonical catalog/open/snapshot fixtures for other feature tests.
/// A repeated target deliberately replaces its connection and retains Screen IDs.
pub(crate) fn remote_terminal_fixture(
    window: &mut WindowWorkspace,
    target: &str,
) -> (ScreenKey, TerminalToken) {
    window.ui_test_remote_terminal_fixture(target).unwrap()
}

#[test]
fn remote_screen_placement_changes_only_reference_order_across_offline_refresh_and_reconnect() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let owner = window.lifecycle_token();
    catalog(&mut window, owner.source_id(), &["local"]);
    let id = remote(&mut window, "order.test");
    catalog(&mut window, id, &["a", "b", "c"]);
    let a = open(&mut window, id, "a");
    let b = open(&mut window, id, "b");
    let c = open(&mut window, id, "c");
    snapshot(&mut window, id, "a", 1).unwrap();
    window.select(owner, &a).unwrap();
    let token = window.source(id).unwrap().terminal_token("p0").unwrap();
    let route_generation = window.source(id).unwrap().route_generation();
    let browse = window.host(id).unwrap().is_browsing();
    assert!(window.move_opened_screen(owner, &c, Some(&a)).unwrap());
    let order = |window: &WindowWorkspace| {
        window
            .opened()
            .into_iter()
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(&order(&window)[1..], &[c.clone(), a.clone(), b.clone()]);
    assert_eq!(window.shown_key(), Some(a.clone()));
    assert_eq!(window.source(id).unwrap().terminal_token("p0"), Some(token));
    assert_eq!(
        window.source(id).unwrap().route_generation(),
        route_generation
    );
    assert_eq!(
        window
            .source(id)
            .unwrap()
            .live_catalog()
            .unwrap()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(window.host(id).unwrap().is_browsing(), browse);
    catalog(&mut window, id, &["a", "c", "d"]); // b is missing but still arranged.
    assert!(window.source(id).unwrap().is_missing("b"));
    window.disconnect(id).unwrap();
    window.finish_attempt(id).unwrap();
    assert!(window.can_move_screen(&b));
    assert!(window.move_opened_screen(owner, &b, Some(&c)).unwrap());
    assert_eq!(window.source_status(id), Some(SourceStatus::Offline));
    assert_eq!(window.shown_key(), Some(a.clone()));
    let arranged = order(&window);
    window.retry(id).unwrap();
    window
        .bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())
        .unwrap();
    window
        .confirm_ready(window.source(id).unwrap().source_token())
        .unwrap();
    catalog(&mut window, id, &["d", "c", "a", "b"]);
    assert_eq!(order(&window), arranged);
    let d = open(&mut window, id, "d");
    assert_eq!(order(&window).last(), Some(&d));
    assert_eq!(window.shown_key(), Some(d.clone())); // explicit open alone selects.
    let stable = order(&window);
    let local = window.local_source().screen_key("local");
    assert!(window
        .validate_screen_move(owner, &b, Some(&local))
        .is_err());
    let stale = WindowWorkspace::new(Uuid::new_v4()).lifecycle_token();
    assert!(window.move_opened_screen(stale, &b, None).is_err());
    assert_eq!(order(&window), stable);
    window.detach(owner, &c).unwrap();
    assert!(window.move_opened_screen(owner, &b, Some(&c)).is_err());
    window.begin_close(id).unwrap();
    assert!(!window.can_move_screen(&b));
    assert!(window.move_opened_screen(owner, &b, None).is_err());
}

#[test]
fn offline_terminal_focus_is_retained_but_never_upgrades_transport_or_stale_binding() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let id = remote(&mut window, "me@192.0.2.1");
    catalog(&mut window, id, &["a"]);
    let key = open(&mut window, id, "a");
    let owner = window.source(id).unwrap().source_token();
    window.source_mut(id).unwrap().apply_snapshot(owner.connection_id(), &json!({
        "type":"mux.screen_snapshot","screen":{"id":"a","revision":1,
        "root":{"type":"split","id":"split","axis":"horizontal","sizes":[0.5,0.5],"children":[
            {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
            {"type":"slot","id":"right","paneIds":["p1","hidden"],"activePaneId":"p1"}]},
        "panes":{"p0":{"id":"p0","sessionId":"s0"},"p1":{"id":"p1","sessionId":"s1"},"hidden":{"id":"hidden","sessionId":"s2"}},
        "sessions":{"s0":{"id":"s0","status":"running"},"s1":{"id":"s1","status":"running"},"s2":{"id":"s2","status":"running"}}}
    })).unwrap();
    let route = window.source(id).unwrap().terminal_token("p1").unwrap();
    let hidden = window.source(id).unwrap().terminal_token("hidden").unwrap();
    window.source_mut(id).unwrap().focus_pane("p0");
    window.disconnect(id).unwrap();
    assert!(window.focus_terminal(&key, &route).unwrap());
    assert_eq!(window.execution_token(id), None);
    assert!(!window.accepts_terminal(&route));
    // A different source publication clones the same worker-owned focus state.
    let _other = remote(&mut window, "me@192.0.2.2");
    let published = window.clone();
    assert_eq!(published.source(id).unwrap().focused_pane(), Some("p1"));
    assert!(window.focus_terminal(&key, &hidden).is_err());
    for obsolete in [
        TerminalToken::new(
            SourceWorkspace::new(Uuid::new_v4()).source_token(),
            "p1",
            "s1",
        ),
        TerminalToken::new(owner, "p1", "replaced"),
    ] {
        assert!(window.focus_terminal(&key, &obsolete).is_err());
    }
    assert_eq!(window.source(id).unwrap().focused_pane(), Some("p1"));
    window.begin_close(id).unwrap();
    assert!(window.focus_terminal(&key, &route).is_err());
    window.finish_close(id).unwrap();
    assert!(window.focus_terminal(&key, &route).is_err());
}

// Reuse real in-process session Clients as a remote source without an SSH setup.
// Transport-purpose enforcement has its own real adapter rejection coverage.
#[cfg(not(target_os = "windows"))]
impl WindowWorkspace {
    pub(crate) fn test_bind_remote_client(&mut self, id: SourceId, client: &Client) {
        self.bind_remote_parts(id, client.connection_id(), client.sender().context().budget)
            .unwrap();
    }
}

#[test]
fn remote_creation_admission_uses_exact_current_source_and_aggregate_capacity() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let first = remote(&mut window, "one.test");
    let second = remote(&mut window, "two.test");
    catalog(&mut window, first, &["same"]);
    catalog(&mut window, second, &["same"]);
    let first_key = open(&mut window, first, "same");
    let second_key = open(&mut window, second, "same");
    snapshot(&mut window, first, "same", MAX_ATTACHMENTS).unwrap();
    let action = window.host(second).unwrap().action_target().clone();
    let token = window.execution_token(second).unwrap();
    assert!(window
        .prepare_remote_screen_creation(&action, token)
        .is_err());
    window.disconnect(first).unwrap();
    assert!(
        window
            .prepare_remote_screen_creation(&action, token)
            .is_err(),
        "offline retained models still own capacity"
    );
    window.detach(window.lifecycle_token(), &first_key).unwrap();
    snapshot(&mut window, second, "same", 1).unwrap();
    assert!(window
        .prepare_remote_screen_creation(&action, token)
        .is_ok());
    let replacement = Uuid::new_v4();
    // Same logical host/attempt, different execution: a callback cannot upgrade.
    window
        .source_mut(second)
        .unwrap()
        .reset_connection(replacement);
    let current = window.execution_token(second).unwrap();
    assert_ne!(token, current);
    assert!(window
        .prepare_remote_screen_creation(&action, token)
        .is_err());
    catalog(&mut window, second, &["same"]);
    snapshot(&mut window, second, "same", 1).unwrap();
    assert!(window
        .prepare_remote_screen_creation(&action, current)
        .is_ok());
    window
        .detach(window.lifecycle_token(), &second_key)
        .unwrap();
    assert!(
        window
            .prepare_remote_screen_creation(&action, current)
            .is_ok(),
        "a live catalog allows creating the first opened Screen from Hosts"
    );
    open(&mut window, second, "same");
    window.begin_close(second).unwrap();
    assert!(window
        .prepare_remote_screen_creation(&action, current)
        .is_err());
}

fn local_catalog(window: &mut WindowWorkspace, ids: &[&str]) {
    let id = window.local_source().source_id();
    catalog(window, id, ids);
}
fn listed(window: &WindowWorkspace) -> Vec<String> {
    window
        .opened()
        .iter()
        .map(|entry| entry.key().screen_id().to_owned())
        .collect()
}
fn local_key(window: &WindowWorkspace, screen: &str) -> ScreenKey {
    window.local_source().screen_key(screen)
}
const SECOND: WindowId = WindowId::new(1);

#[test]
fn every_screen_is_held_by_one_window_and_each_window_shows_its_own() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    local_catalog(&mut window, &["work", "logs", "notes"]);
    let owner = window.lifecycle_token();
    window.select(owner, &local_key(&window, "work")).unwrap();
    assert!(window.single_window());
    assert_eq!(
        window.holder(&local_key(&window, "notes")),
        Some(WindowId::FIRST)
    );

    // A new window holds nothing and changes nothing of the first.
    window.open_window(SECOND).unwrap();
    assert!(window.open_window(SECOND).is_err());
    assert_eq!(listed(&window), ["work", "logs", "notes"]);
    assert_eq!(window.shown_key(), Some(local_key(&window, "work")));
    assert!(window.enter(SECOND));
    assert!(listed(&window).is_empty());
    assert_eq!(window.shown_key(), None);
    assert!(!window.enter(WindowId::new(7)));
    assert!(window.enter(WindowId::FIRST));

    // A moved Screen is shown where it goes; no source hears of the move.
    let mutation = window.local_source().mutation_generation();
    let notes = local_key(&window, "notes");
    window.move_screen_to_window(&notes, SECOND).unwrap();
    assert_eq!(window.local_source().mutation_generation(), mutation);
    assert_eq!(listed(&window), ["work", "logs"]);
    assert_eq!(window.shown_key(), Some(local_key(&window, "work")));
    assert_eq!(window.holder(&notes), Some(SECOND));
    assert_eq!(window.held_elsewhere(&notes), Some(SECOND));
    assert_eq!(window.shown_in(SECOND), Some(notes.clone()));
    assert_eq!(window.take_losers(), [WindowId::FIRST]);

    // Each window's choice stays its own, and a Screen another window holds
    // cannot be shown here.
    window.select(owner, &local_key(&window, "logs")).unwrap();
    assert!(window.select(owner, &notes).is_err());
    assert_eq!(window.shown_in(SECOND), Some(notes.clone()));
    assert!(window.enter(SECOND));
    assert_eq!(listed(&window), ["notes"]);
    assert_eq!(window.shown_key(), Some(notes.clone()));
    assert!(window.select(owner, &local_key(&window, "work")).is_err());
    assert!(window.enter(WindowId::FIRST));
    assert_eq!(window.shown_key(), Some(local_key(&window, "logs")));

    // Each window reads a model of its own Screens.
    let first = window.projection(WindowId::FIRST).unwrap();
    assert!(first.single_window());
    assert_eq!(listed(&first), ["work", "logs"]);
    assert_eq!(first.local_source().screens().len(), 2);
    assert_eq!(first.shown_key(), Some(local_key(&window, "logs")));
    let second = window.projection(SECOND).unwrap();
    assert_eq!(second.window(), SECOND);
    assert_eq!(listed(&second), ["notes"]);
    assert_eq!(second.shown_key(), Some(notes));
    assert_eq!(second.local_source().shown_screen_id(), Some("notes"));
    assert!(window.projection(WindowId::new(7)).is_none());
}

#[test]
fn a_window_shows_the_neighbor_of_a_screen_that_left_it() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    local_catalog(&mut window, &["work", "logs", "notes", "mail"]);
    let owner = window.lifecycle_token();
    window.open_window(SECOND).unwrap();
    window.select(owner, &local_key(&window, "logs")).unwrap();
    window.settle();

    // The shown Screen moves away: the next one in the window's order shows.
    window
        .move_screen_to_window(&local_key(&window, "logs"), SECOND)
        .unwrap();
    assert_eq!(window.shown_key(), Some(local_key(&window, "notes")));

    // The other window's shown Screen is removed on its server: it shows the
    // neighbor it holds, and the first window keeps its own.
    window
        .move_screen_to_window(&local_key(&window, "mail"), SECOND)
        .unwrap();
    assert_eq!(window.shown_in(SECOND), Some(local_key(&window, "mail")));
    local_catalog(&mut window, &["work", "logs", "notes"]);
    window.settle();
    assert_eq!(window.shown_in(SECOND), Some(local_key(&window, "logs")));
    assert_eq!(window.shown_key(), Some(local_key(&window, "notes")));
    assert_eq!(window.holder(&local_key(&window, "mail")), None);

    // A window that loses its last Screen shows none.
    local_catalog(&mut window, &["work", "notes"]);
    window.settle();
    assert_eq!(window.shown_in(SECOND), None);
    assert!(window.projection(SECOND).unwrap().opened().is_empty());
}

#[test]
fn a_screen_nobody_here_asked_for_goes_to_the_window_active_last() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    local_catalog(&mut window, &["work"]);
    window.open_window(SECOND).unwrap();
    local_catalog(&mut window, &["work", "first"]);
    window.settle();
    assert_eq!(
        window.holder(&local_key(&window, "first")),
        Some(WindowId::FIRST)
    );
    window.set_intake(SECOND);
    local_catalog(&mut window, &["work", "first", "second"]);
    window.settle();
    assert_eq!(window.holder(&local_key(&window, "second")), Some(SECOND));
    assert_eq!(window.shown_in(SECOND), Some(local_key(&window, "second")));
    assert_eq!(listed(&window), ["work", "first"]);
    // A window that does not exist takes nothing.
    window.set_intake(WindowId::new(7));
    local_catalog(&mut window, &["work", "first", "second", "third"]);
    window.settle();
    assert_eq!(window.holder(&local_key(&window, "third")), Some(SECOND));
}

#[test]
fn a_closed_window_hands_its_screens_to_its_heir() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    local_catalog(&mut window, &["work", "logs", "notes"]);
    let owner = window.lifecycle_token();
    window.select(owner, &local_key(&window, "work")).unwrap();
    window.open_window(SECOND).unwrap();
    window
        .move_screen_to_window(&local_key(&window, "logs"), SECOND)
        .unwrap();
    assert!(window.close_window(SECOND, SECOND).is_err());
    assert!(window
        .close_window(WindowId::new(7), WindowId::FIRST)
        .is_err());

    // The heir keeps the Screen it shows and lists the Screens in their order.
    window.close_window(SECOND, WindowId::FIRST).unwrap();
    assert!(window.single_window());
    assert_eq!(listed(&window), ["work", "logs", "notes"]);
    assert_eq!(window.shown_key(), Some(local_key(&window, "work")));
    assert!(!window.has_window(SECOND));

    // The window in view can close too; its heir comes into view.
    window.open_window(SECOND).unwrap();
    window
        .move_screen_to_window(&local_key(&window, "notes"), SECOND)
        .unwrap();
    window.close_window(WindowId::FIRST, SECOND).unwrap();
    assert_eq!(window.window(), SECOND);
    assert_eq!(listed(&window), ["work", "logs", "notes"]);
    assert_eq!(window.shown_key(), Some(local_key(&window, "notes")));
}

#[test]
fn the_close_of_the_application_covers_the_screens_of_every_window() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let local = window.local_source().source_id();
    local_catalog(&mut window, &["work", "logs"]);
    // The fixture's pane IDs are the same on every Screen, so only the Screen
    // the other window gets has a snapshot.
    snapshot(&mut window, local, "logs", 2).unwrap();
    window.open_window(SECOND).unwrap();
    window
        .move_screen_to_window(&local_key(&window, "logs"), SECOND)
        .unwrap();
    let whole = window
        .local_source()
        .assess_close(1, CloseTarget::Window)
        .unwrap();
    assert!(whole.affected.len() > 2);
    for id in [WindowId::FIRST, SECOND] {
        let projection = window.projection(id).unwrap();
        let plan = projection
            .local_source()
            .assess_close(1, CloseTarget::Window)
            .unwrap();
        assert_eq!(plan.affected, whole.affected);
        // The worker checks a window's plan against every window's Screens.
        window.local_source().validate_close(&plan, true).unwrap();
    }
}

#[test]
fn a_host_serves_every_window_and_its_screens_stay_where_they_are_held() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    local_catalog(&mut window, &["work"]);
    let host = remote(&mut window, "alice@example.test");
    catalog(&mut window, host, &["api", "infra"]);
    let api = open(&mut window, host, "api");
    window.open_window(SECOND).unwrap();
    assert!(window.enter(SECOND));

    // A Screen another window holds is not opened a second time.
    let token = window.source(host).unwrap().source_token();
    assert!(window.open_existing(token, &api).is_err());
    assert_eq!(window.held_elsewhere(&api), Some(WindowId::FIRST));
    let infra = open(&mut window, host, "infra");
    assert_eq!(window.holder(&infra), Some(SECOND));
    assert_eq!(listed(&window), ["infra"]);
    assert_eq!(window.windows_showing(host), [WindowId::FIRST, SECOND]);

    // Each window's model shows the host with the Screen it holds.
    let second = window.projection(SECOND).unwrap();
    assert_eq!(second.source(host).unwrap().screens().len(), 1);
    assert!(second.host(host).is_some());
    let first = window.projection(WindowId::FIRST).unwrap();
    assert_eq!(listed(&first), ["work", "api"]);

    // Detaching in one window leaves the other's reference.
    let owner = window.lifecycle_token();
    window.detach(owner, &infra).unwrap();
    window.settle();
    assert_eq!(window.holder(&api), Some(WindowId::FIRST));
    assert_eq!(window.source(host).unwrap().screens().len(), 1);

    // Closing the host takes its Screens out of every window.
    window.detach_all(owner, host).unwrap();
    window.settle();
    assert!(window.source(host).unwrap().screens().is_empty());
    assert_eq!(window.holder(&api), None);
    assert!(window.enter(WindowId::FIRST));
    assert_eq!(listed(&window), ["work"]);
}

#[test]
fn a_window_lists_the_screens_it_holds_in_their_sources_order() {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    local_catalog(&mut window, &["work", "notes", "logs"]);
    let owner = window.lifecycle_token();
    window.open_window(SECOND).unwrap();
    window
        .move_screen_to_window(&local_key(&window, "notes"), SECOND)
        .unwrap();
    assert_eq!(listed(&window), ["work", "logs"]);
    // The service moved `logs` before `work` in its own order, as a
    // placement within the window asks: the window lists them so.
    let (logs, work) = (local_key(&window, "logs"), local_key(&window, "work"));
    local_catalog(&mut window, &["logs", "work", "notes"]);
    window.settle();
    assert_eq!(listed(&window), ["logs", "work"]);
    assert!(window.enter(SECOND));
    assert_eq!(listed(&window), ["notes"]);
    // Relative navigation stays within the window.
    assert!(!window.cycle(owner, TabStep::Next).unwrap());
    assert!(window.enter(WindowId::FIRST));
    window.select(owner, &logs).unwrap();
    assert!(window.cycle(owner, TabStep::Next).unwrap());
    assert_eq!(window.shown_key(), Some(work));
    assert!(!window.cycle(owner, TabStep::Next).unwrap());
}
