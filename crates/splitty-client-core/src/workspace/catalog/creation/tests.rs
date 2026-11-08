use super::*;
use crate::workspace::{CatalogState, WindowWorkspace};
use serde_json::json;

fn catalog(ids: &[&str]) -> Value {
    json!({"type":"mux.workspace_snapshot", "screens":ids.iter().map(|id| json!({"id":id,"name":format!("Name {id}")})).collect::<Vec<_>>()})
}
fn snapshot(id: &str, pane: &str) -> Value {
    json!({"type":"mux.screen_snapshot","screen":{"id":id,"revision":1,
        "root":{"type":"slot","id":format!("slot-{id}"),"paneIds":[pane],"activePaneId":pane},
        "panes":{pane:{"id":pane,"sessionId":format!("s-{pane}")}},
        "sessions":{format!("s-{pane}"):{"id":format!("s-{pane}"),"status":"running"}}}})
}
fn ready() -> (WindowWorkspace, RemoteScreenCreateRequest) {
    let mut window = WindowWorkspace::new(Uuid::new_v4());
    let id = window
        .admit_remote(window.lifecycle_token(), "me@192.0.2.10")
        .unwrap();
    window
        .bind_remote_parts(id, Uuid::new_v4(), ConnectionBudget::default())
        .unwrap();
    let source = window.source(id).unwrap().source_token();
    window.confirm_ready(source).unwrap();
    {
        let mut model = window.source_mut(id).unwrap();
        model.set_catalog_supported(true);
        model.catalog_in_flight = Some(0);
        model
            .apply_catalog(source.connection_id(), &catalog(&["old", "unopened"]))
            .unwrap();
    }
    let target = window.host(id).unwrap().action_target().clone();
    window.host_mut(&target).unwrap().catalog = CatalogState::Ready;
    let key = window.source(id).unwrap().screen_key("old");
    window.open_existing(source, &key).unwrap();
    window
        .source_mut(id)
        .unwrap()
        .apply_snapshot(source.connection_id(), &snapshot("old", "old-pane"))
        .unwrap();
    let target = window.host(id).unwrap().action_target().clone();
    window.host_mut(&target).unwrap().catalog = CatalogState::Ready;
    let request = window
        .prepare_remote_screen_creation(&target, source)
        .unwrap();
    (window, request)
}
fn admit(window: &mut WindowWorkspace, request: &RemoteScreenCreateRequest) -> ScreenIds {
    let mut ids = None;
    window
        .source_mut(request.source().source_id())
        .unwrap()
        .create_remote_screen_with(request.clone(), |sent| {
            ids = Some(sent.clone());
            Ok(())
        })
        .unwrap();
    ids.unwrap()
}
fn observe(model: &mut SourceWorkspace, ids: &[&str], new: Option<&ScreenIds>) -> WorkspaceDelta {
    let mut refresh = super::super::refresh::WorkspaceRefresh::default();
    refresh.begin(model).unwrap();
    let generation = model.mutation;
    model.catalog_in_flight = Some(generation);
    refresh
        .apply_catalog(model, model.connection_id, &catalog(ids))
        .unwrap();
    if ids.contains(&"old") {
        let delta = refresh
            .apply_snapshot(model, model.connection_id, &snapshot("old", "old-pane"))
            .unwrap();
        if delta.reconciliation_error.is_some() {
            return delta;
        }
    }
    if let Some(new) = new {
        let delta = refresh
            .apply_snapshot(
                model,
                model.connection_id,
                &snapshot(&new.screen_id, &new.pane_id),
            )
            .unwrap();
        if delta.reconciliation_error.is_some() {
            return delta;
        }
    }
    refresh.test_trailing(model);
    refresh
        .apply_catalog(model, model.connection_id, &catalog(ids))
        .unwrap()
}

#[test]
fn remote_creation_admission_rolls_back_and_bounds_one_request_without_retargeting() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let before = window.source(id).unwrap().admission_count();
    let mutation = window.source(id).unwrap().mutation;
    assert!(window
        .source_mut(id)
        .unwrap()
        .create_remote_screen_with(request.clone(), |_| Err(io::ErrorKind::WouldBlock.into()))
        .is_err());
    assert_eq!(window.source(id).unwrap().admission_count(), before);
    assert_eq!(window.source(id).unwrap().mutation, mutation);
    assert!(window
        .source(id)
        .unwrap()
        .remote_screen_creation()
        .is_none());
    let ids = admit(&mut window, &request);
    assert_eq!(window.opened().len(), 2);
    assert!(window.source(id).unwrap().screen("unopened").is_none());
    assert_eq!(window.source(id).unwrap().admission_count(), before + 1);
    assert!(window
        .validate_remote_screen_creation(request.target(), request.source())
        .is_err());
    assert!(window
        .source_mut(id)
        .unwrap()
        .create_remote_screen_with(request.clone(), |_| panic!("duplicate must not send"))
        .is_err());
    assert!(window
        .source(id)
        .unwrap()
        .remote_creation_is_request(&request));
    let newer = window.source(id).unwrap().mutation;
    {
        let mut model = window.source_mut(id).unwrap();
        model.catalog_in_flight = Some(newer - 1);
        model
            .apply_catalog(
                request.source().connection_id(),
                &catalog(&["old", "unopened"]),
            )
            .unwrap();
        assert!(model.remote_screen_creation().unwrap().is_unresolved());
        assert!(!model.is_missing(&ids.screen_id));
        assert!(model.screen(&ids.screen_id).is_some());
        assert_eq!(model.admission_count(), before + 1);
        assert!(
            model.catalog_dirty,
            "newer creation still requires a fresh pass"
        );
    }
}

#[test]
fn remote_creation_confirms_only_coherent_snapshot_and_follow_is_revocable_without_queue() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let ids = admit(&mut window, &request);
    let mut model = window.source_mut(id).unwrap();
    model
        .apply_snapshot(
            request.source().connection_id(),
            &snapshot(&ids.screen_id, &ids.pane_id),
        )
        .unwrap();
    assert!(model.remote_screen_creation().unwrap().is_unresolved());
    assert!(model.remote_creation_follow().is_none());
    let clone = request.clone();
    drop(clone);
    assert!(
        request.is_follow_requested(),
        "dropping a snapshot/capture is not cancellation"
    );
    let delta = observe(&mut model, &["old", "unopened", &ids.screen_id], Some(&ids));
    assert!(delta.coherent_refresh);
    assert_eq!(
        model.remote_screen_creation().unwrap().status(),
        RemoteScreenCreateStatus::Present
    );
    assert_eq!(model.admission_count(), 2);
    assert!(model.screen("unopened").is_none());
    request.cancel_follow();
    assert!(model.remote_creation_follow().is_none());
}

#[test]
fn remote_creation_reset_retains_reservation_until_current_presence_or_absence() {
    for present in [true, false] {
        let (mut window, request) = ready();
        let id = request.source().source_id();
        let ids = admit(&mut window, &request);
        window.disconnect(id).unwrap();
        assert!(!request.is_follow_requested());
        window.finish_attempt(id).unwrap();
        window.retry(id).unwrap();
        let connection = Uuid::new_v4();
        window
            .bind_remote_parts(id, connection, ConnectionBudget::default())
            .unwrap();
        let replacement = window.source(id).unwrap().source_token();
        window.confirm_ready(replacement).unwrap();
        let mut model = window.source_mut(id).unwrap();
        model.set_catalog_supported(true);
        assert_eq!(
            model.remote_screen_creation().unwrap().status(),
            RemoteScreenCreateStatus::Uncertain
        );
        assert_eq!(
            model.admission_count(),
            1,
            "old routes are gone, unresolved reservation remains"
        );
        model
            .apply_snapshot(
                request.source().connection_id(),
                &snapshot(&ids.screen_id, &ids.pane_id),
            )
            .unwrap();
        assert!(model.remote_screen_creation().unwrap().is_unresolved());
        model.remote_creation_uncertain("read failed");
        assert_eq!(model.admission_count(), 1);
        if present {
            observe(&mut model, &["old", &ids.screen_id], Some(&ids));
            assert_eq!(
                model.remote_screen_creation().unwrap().status(),
                RemoteScreenCreateStatus::Present
            );
            assert_eq!(model.admission_count(), 2);
        } else {
            observe(&mut model, &["old"], None);
            assert_eq!(
                model.remote_screen_creation().unwrap().status(),
                RemoteScreenCreateStatus::Missing
            );
            assert!(model.is_missing(&ids.screen_id));
            assert_eq!(model.admission_count(), 1);
            assert_eq!(
                model.screens().len(),
                2,
                "missing reference retains its Screen slot"
            );
        }
        assert!(model.remote_screen_creation().unwrap().was_uncertain());
        assert!(!model.remote_screen_creation().unwrap().is_unresolved());
        assert!(model.remote_creation_follow().is_none());
    }
}

#[test]
fn remote_creation_detach_rejects_late_snapshot_and_fresh_creation_uses_new_ids() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let ids = admit(&mut window, &request);
    let key = window.source(id).unwrap().screen_key(&ids.screen_id);
    window.detach(window.lifecycle_token(), &key).unwrap();
    assert!(!request.is_follow_requested());
    let mut model = window.source_mut(id).unwrap();
    assert!(model.remote_screen_creation().is_none());
    model
        .apply_snapshot(
            request.source().connection_id(),
            &snapshot(&ids.screen_id, &ids.pane_id),
        )
        .unwrap();
    observe(&mut model, &["old", &ids.screen_id], None);
    assert!(model.screen(&ids.screen_id).is_none());
    assert_eq!(model.screens().len(), 1);
    drop(model);
    let next = window
        .prepare_remote_screen_creation(request.target(), request.source())
        .unwrap();
    let next_ids = admit(&mut window, &next);
    assert_ne!(next_ids.screen_id, ids.screen_id);
    assert_ne!(next_ids.pane_id, ids.pane_id);
    assert_eq!(window.source(id).unwrap().admission_count(), 2);
}

#[test]
fn remote_creation_candidate_converts_reservation_after_initial_pane_replacement_at_capacity() {
    for empty in [false, true] {
        let (mut window, request) = ready();
        let id = request.source().source_id();
        let ids = admit(&mut window, &request);
        let mut edit = window.source_mut(id).unwrap();
        let model: &mut SourceWorkspace = &mut edit;
        model.limits.panes = 2;
        let mut refresh = super::super::refresh::WorkspaceRefresh::default();
        refresh.begin(model).unwrap();
        model.catalog_in_flight = Some(model.mutation);
        let connection = model.connection_id;
        let cat = catalog(&["old", &ids.screen_id]);
        refresh.apply_catalog(model, connection, &cat).unwrap();
        refresh
            .apply_snapshot(model, connection, &snapshot("old", "old-pane"))
            .unwrap();
        let changed = if empty {
            json!({"type":"mux.screen_snapshot","screen":{"id":ids.screen_id,"revision":3,"root":{"type":"slot","id":"empty","paneIds":[]},"panes":{},"sessions":{}}})
        } else {
            snapshot(&ids.screen_id, "replacement-pane")
        };
        let staged = refresh.apply_snapshot(model, connection, &changed).unwrap();
        assert!(
            staged.reconciliation_error.is_none(),
            "{:?}",
            staged.reconciliation_error
        );
        refresh.test_trailing(model);
        let result = refresh.apply_catalog(model, connection, &cat).unwrap();
        assert!(result.coherent_refresh, "{:?}", result.reconciliation_error);
        assert_eq!(
            model.remote_screen_creation().unwrap().status(),
            RemoteScreenCreateStatus::Present
        );
        assert_eq!(model.admission_count(), if empty { 1 } else { 2 });
        assert!(request.is_follow_requested());
    }
}

#[test]
fn remote_creation_invalid_candidate_retains_current_reservation_and_follow_permission() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let ids = admit(&mut window, &request);
    let mut edit = window.source_mut(id).unwrap();
    let model: &mut SourceWorkspace = &mut edit;
    model.limits.panes = 1; // Remaining window allowance shrank before this observation.
    let delta = observe(model, &["old", &ids.screen_id], Some(&ids));
    assert!(!delta.coherent_refresh);
    assert!(model.routes_reconciling());
    assert!(model.remote_screen_creation().unwrap().is_unresolved());
    assert_eq!(model.admission_count(), 2);
    assert!(model.screen(&ids.screen_id).unwrap().snapshot().is_none());
    assert!(
        request.is_follow_requested(),
        "failed candidate must not mutate shared permission"
    );
}

#[test]
fn remote_creation_captured_token_owner_and_late_follow_are_revalidated() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let foreign = WindowWorkspace::new(Uuid::new_v4()).lifecycle_token();
    assert!(window
        .prepare_remote_screen_creation(request.target(), foreign)
        .is_err());
    assert!(WindowWorkspace::new(Uuid::new_v4())
        .prepare_remote_screen_creation(request.target(), request.source())
        .is_err());
    let ids = admit(&mut window, &request);
    observe(
        &mut window.source_mut(id).unwrap(),
        &["old", &ids.screen_id],
        Some(&ids),
    );
    let effect = window.source(id).unwrap().remote_creation_follow().unwrap();
    assert!(
        request.is_follow_requested(),
        "preparing effects does not consume follow"
    );
    let selected = window.shown_key();
    request.cancel_follow(); // UI destination change after source effects were prepared.
    assert!(!window.follow_remote_creation(&effect).unwrap());
    assert_eq!(window.shown_key(), selected);
    window.disconnect(id).unwrap();
    assert!(window
        .prepare_remote_screen_creation(request.target(), request.source())
        .is_err());
}

#[test]
fn remote_creation_rejection_is_exact_and_never_frees_unobserved_capacity() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let ids = admit(&mut window, &request);
    let mut model = window.source_mut(id).unwrap();
    let failure = json!({"type":"mux.op_failed","screenId":"","op":"create_screen","paneId":ids.pane_id,"error":"spawn failed"});
    model.observe_remote_creation_failure(Uuid::new_v4(), &failure);
    model.observe_remote_creation_failure(
        request.source().connection_id(),
        &json!({"op":"create_screen","paneId":"other"}),
    );
    assert_eq!(
        model.remote_screen_creation().unwrap().status(),
        RemoteScreenCreateStatus::Pending
    );
    model.observe_remote_creation_failure(request.source().connection_id(), &failure);
    assert_eq!(
        model.remote_screen_creation().unwrap().status(),
        RemoteScreenCreateStatus::Rejected
    );
    assert!(model.remote_screen_creation().unwrap().is_unresolved());
    assert_eq!(model.admission_count(), 2);
    assert!(!request.is_follow_requested());
    observe(&mut model, &["old"], None);
    assert!(!model.remote_screen_creation().unwrap().is_unresolved());
    assert_eq!(model.admission_count(), 1);
    assert_eq!(
        model.remote_screen_creation().unwrap().status(),
        RemoteScreenCreateStatus::Rejected
    );
}

#[test]
fn remote_creation_old_fence_and_missing_snapshot_preserve_reservation_until_bounded_failure() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    window.source_mut(id).unwrap().catalog_in_flight = Some(0);
    let ids = admit(&mut window, &request);
    let mut edit = window.source_mut(id).unwrap();
    let model: &mut SourceWorkspace = &mut edit;
    let connection = model.connection_id;
    let mut refresh = super::super::refresh::WorkspaceRefresh::default();
    refresh.begin(model).unwrap();
    refresh
        .apply_catalog(model, connection, &catalog(&["old"]))
        .unwrap();
    assert!(model.screen(&ids.screen_id).is_some());
    assert!(model.remote_screen_creation().unwrap().is_unresolved());
    assert_eq!(model.admission_count(), 2);
    let cat = catalog(&["old", &ids.screen_id]);
    model.catalog_in_flight = Some(model.mutation);
    refresh.apply_catalog(model, connection, &cat).unwrap();
    refresh
        .apply_snapshot(model, connection, &snapshot("old", "old-pane"))
        .unwrap();
    refresh.test_trailing(model);
    let first = refresh.apply_catalog(model, connection, &cat).unwrap();
    assert!(!first.coherent_refresh);
    assert!(first.reconciliation_error.is_none());
    refresh
        .apply_snapshot(model, connection, &snapshot("old", "old-pane"))
        .unwrap();
    refresh.test_trailing(model);
    let second = refresh.apply_catalog(model, connection, &cat).unwrap();
    assert!(second.reconciliation_error.is_some());
    assert_eq!(
        model.remote_screen_creation().unwrap().status(),
        RemoteScreenCreateStatus::Uncertain
    );
    assert_eq!(model.admission_count(), 2);
    assert!(model.screen(&ids.screen_id).unwrap().snapshot().is_none());
    assert!(!model.route_observation_complete());
}

#[test]
fn remote_creation_missing_reference_reappearance_requires_capacity_and_does_not_reopen_detached() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let ids = admit(&mut window, &request);
    observe(&mut window.source_mut(id).unwrap(), &["old"], None);
    let mut edit = window.source_mut(id).unwrap();
    let model: &mut SourceWorkspace = &mut edit;
    model.limits.panes = 1;
    let result = observe(model, &["old", &ids.screen_id], Some(&ids));
    assert!(result.reconciliation_error.is_some());
    assert!(model.is_missing(&ids.screen_id));
    assert!(model.screen(&ids.screen_id).is_none());
    assert_eq!(model.admission_count(), 1);
    assert!(!request.is_follow_requested());
}

#[test]
fn remote_creation_live_catalog_bound_and_ui_fixture_use_same_admission() {
    let (mut window, request) = ready();
    let id = request.source().source_id();
    let entries = (0..MAX_REMOTE_CATALOG)
        .map(|n| format!("remote-{n}"))
        .collect::<Vec<_>>();
    let refs = entries.iter().map(String::as_str).collect::<Vec<_>>();
    {
        let mut model = window.source_mut(id).unwrap();
        model.catalog_in_flight = Some(model.mutation);
        model
            .apply_catalog(request.source().connection_id(), &catalog(&refs))
            .unwrap();
    }
    assert!(window
        .prepare_remote_screen_creation(request.target(), request.source())
        .is_err());
    for missing in [false, true] {
        let (mut window, request) = ready();
        window
            .ui_test_remote_creation_fixture(&request, missing)
            .unwrap();
        let creation = window
            .source(request.source().source_id())
            .unwrap()
            .remote_screen_creation()
            .unwrap();
        assert_eq!(
            creation.status(),
            if missing {
                RemoteScreenCreateStatus::Missing
            } else {
                RemoteScreenCreateStatus::Pending
            }
        );
        assert_eq!(creation.request_id(), request.request_id());
    }
}
