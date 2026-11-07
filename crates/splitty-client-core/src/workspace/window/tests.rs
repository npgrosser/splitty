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
