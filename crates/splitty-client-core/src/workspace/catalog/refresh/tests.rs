use super::*;
use serde_json::json;
fn cat(ids: &[&str]) -> Value {
    json!({"type":"mux.workspace_snapshot","screens":ids.iter().map(|id|json!({"id":id})).collect::<Vec<_>>(),"activeScreenId":ids.first()})
}
fn snap(id: &str, panes: &[&str]) -> Value {
    json!({"type":"mux.screen_snapshot","screen":{"id":id,"revision":2,
    "root":{"type":"slot","id":format!("slot-{id}"),"paneIds":panes,"activePaneId":panes.first()},
    "panes":panes.iter().map(|p|(p.to_string(),json!({"id":p,"sessionId":format!("s-{p}")}))).collect::<serde_json::Map<_,_>>(),
    "sessions":panes.iter().map(|p|(format!("s-{p}"),json!({"id":format!("s-{p}"),"status":"running"}))).collect::<serde_json::Map<_,_>>()}})
}
fn model() -> LocalWorkspace {
    let mut model = LocalWorkspace::new(Uuid::new_v4());
    model.set_catalog_supported(true);
    model.catalog_in_flight = Some(0);
    model
        .apply_catalog(model.connection_id, &cat(&["a", "b"]))
        .unwrap();
    model
        .apply_snapshot(model.connection_id, &snap("a", &["p", "q"]))
        .unwrap();
    model
        .apply_snapshot(model.connection_id, &snap("b", &["r"]))
        .unwrap();
    model.observe_attached(model.connection_id, "s-q");
    model
}
fn admit(model: &mut LocalWorkspace, new: bool) -> Uuid {
    let id = Uuid::new_v4();
    let source = model.capture_pane_action("q").unwrap();
    let target = if new {
        ScreenTransferTarget::New {
            screen_id: "new".into(),
        }
    } else {
        ScreenTransferTarget::Existing {
            screen: model.screen_key("b"),
        }
    };
    model
        .transfer_with(id, source, target, |screen, pane, _, _| {
            assert_eq!((screen, pane), ("a", "q"));
            Ok(())
        })
        .unwrap();
    id
}
fn opening(refresh: &mut WorkspaceRefresh, model: &mut LocalWorkspace, ids: &[&str]) {
    refresh.begin(model).unwrap();
    model.catalog_in_flight = Some(refresh.batch.as_ref().unwrap().generation);
    refresh
        .apply_catalog(model, model.connection_id, &cat(ids))
        .unwrap();
}
fn stage(
    refresh: &mut WorkspaceRefresh,
    model: &mut LocalWorkspace,
    id: &str,
    panes: &[&str],
) -> WorkspaceDelta {
    refresh
        .apply_snapshot(model, model.connection_id, &snap(id, panes))
        .unwrap()
}
fn ending(
    refresh: &mut WorkspaceRefresh,
    model: &mut LocalWorkspace,
    ids: &[&str],
) -> WorkspaceDelta {
    let batch = refresh.batch.as_mut().unwrap();
    batch.closing = true;
    model.catalog_in_flight = Some(batch.generation);
    refresh
        .apply_catalog(model, model.connection_id, &cat(ids))
        .unwrap()
}
#[test]
fn captured_hidden_tab_admission_rejects_stale_conflicts_and_queue_failure_atomically() {
    let mut model = model();
    let source = model.capture_pane_action("q").unwrap();
    assert_eq!(model.focused_pane(), Some("p"));
    let target = ScreenTransferTarget::Existing {
        screen: model.screen_key("b"),
    };
    let before = model.mutation;
    assert!(model
        .transfer_with(
            Uuid::new_v4(),
            source.clone(),
            target.clone(),
            |_, _, _, _| Err(io::ErrorKind::WouldBlock.into())
        )
        .is_err());
    assert!(!model.reconciling);
    assert_eq!(model.mutation, before);
    assert!(!model.transfer_pending());
    let mut stale = source.clone();
    stale.session_id = "replacement".into();
    assert!(model.validate_pane_action(&stale).is_err());
    stale = source.clone();
    stale.connection_id = Uuid::new_v4();
    assert!(model.validate_pane_action(&stale).is_err());
    for target in [
        ScreenTransferTarget::Existing {
            screen: model.screen_key("a"),
        },
        ScreenTransferTarget::Existing {
            screen: model.screen_key("gone"),
        },
        ScreenTransferTarget::New {
            screen_id: "b".into(),
        },
    ] {
        assert!(model
            .transfer_with(Uuid::new_v4(), source.clone(), target, |_, _, _, _| panic!(
                "invalid admitted"
            ))
            .is_err());
    }
    let mut job = snap("a", &["p", "q"]);
    job["screen"]["panes"]["q"]["jobId"] = json!("job");
    model.apply_snapshot(model.connection_id, &job).unwrap();
    assert!(model.validate_pane_action(&source).is_ok());
    assert!(!model.can_transfer_pane(&source));
    model
        .apply_snapshot(model.connection_id, &snap("a", &["p", "q"]))
        .unwrap();
    admit(&mut model, false);
    assert!(model
        .transfer_with(Uuid::new_v4(), source, target, |_, _, _, _| panic!(
            "second mutation"
        ))
        .is_err());
    assert!(model
        .assess_close(
            1,
            CloseTarget::Pane {
                screen: "a".into(),
                pane: "q".into()
            }
        )
        .is_err());
    let close = model.assess_close(2, CloseTarget::Window).unwrap();
    assert!(close.needs_confirmation());
    assert!(model.validate_close(&close, false).is_err());
    assert!(model.validate_close(&close, true).is_ok());
}
#[test]
fn every_snapshot_order_preserves_ready_route_until_atomic_confirmed_move() {
    for reverse in [false, true] {
        let mut model = model();
        let request = admit(&mut model, false);
        let mut refresh = WorkspaceRefresh::default();
        opening(&mut refresh, &mut model, &["a", "b"]);
        let updates = if reverse {
            [("b", vec!["r", "q"]), ("a", vec!["p"])]
        } else {
            [("a", vec!["p"]), ("b", vec!["r", "q"])]
        };
        for (screen, panes) in updates {
            let delta = stage(&mut refresh, &mut model, screen, &panes);
            assert!(delta.removed.is_empty());
            assert_eq!(model.screen_for("q"), Some("a"));
            assert!(model.session_ready("s-q"));
        }
        let delta = ending(&mut refresh, &mut model, &["a", "b"]);
        assert!(delta.removed.is_empty());
        assert!(delta.added.is_empty());
        assert_eq!(
            delta.transfer.unwrap(),
            ScreenTransferResult {
                connection_id: model.connection_id,
                request_id: request,
                outcome: ScreenTransferOutcome::Confirmed
            }
        );
        assert_eq!(model.screen_for("q"), Some("b"));
        assert!(model.session_ready("s-q"));
        assert_eq!(model.shown_screen_id(), Some("b"));
        assert_eq!(model.focused_pane(), Some("q"));
        assert_eq!(model.route_generation(), 1);
    }
}
#[test]
fn new_target_empty_source_and_superseding_navigation_preserve_canonical_choices() {
    let mut model = model();
    model
        .apply_snapshot(model.connection_id, &snap("a", &["q"]))
        .unwrap();
    admit(&mut model, true);
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "b", "new"]);
    // The source disappears after opening and sends no response. Ending catalog
    // is sufficient; no wait for a nonexistent source snapshot is necessary.
    stage(&mut refresh, &mut model, "b", &["r"]);
    stage(&mut refresh, &mut model, "new", &["q"]);
    model.shown = Some("b".into());
    model.cancel_placement_focus();
    model.toggle_maximize_focused();
    let delta = ending(&mut refresh, &mut model, &["b", "new"]);
    assert!(matches!(
        delta.transfer.unwrap().outcome,
        ScreenTransferOutcome::Confirmed
    ));
    assert_eq!(model.shown_screen_id(), Some("b"));
    assert_eq!(model.maximized_slot_id(), Some("slot-b"));
    assert_eq!(model.screen_for("q"), Some("new"));
    assert!(model.screen("a").is_none());
    assert!(delta.removed.is_empty());
}
#[test]
fn external_source_target_and_catalog_first_changes_use_same_route_preserving_path() {
    for first in 0..3 {
        let mut model = model();
        model
            .apply_snapshot(model.connection_id, &snap("a", &["q"]))
            .unwrap();
        let mut refresh = WorkspaceRefresh::default();
        let delta = match first {
            0 => stage(&mut refresh, &mut model, "a", &[]),
            1 => stage(&mut refresh, &mut model, "b", &["r", "q"]),
            _ => {
                model.catalog_in_flight = Some(model.mutation);
                let owner = model.connection_id;
                refresh
                    .apply_catalog(&mut model, owner, &cat(&["b"]))
                    .unwrap()
            }
        };
        assert!(delta.removed.is_empty());
        assert_eq!(model.screen_for("q"), Some("a"));
        opening(&mut refresh, &mut model, &["b"]);
        stage(&mut refresh, &mut model, "b", &["r", "q"]);
        let delta = ending(&mut refresh, &mut model, &["b"]);
        assert!(delta.transfer.is_none());
        assert!(delta.removed.is_empty());
        assert!(model.session_ready("s-q"));
        assert_eq!(model.screen_for("q"), Some("b"));
        assert_eq!(model.focused_pane(), Some("r"));
    }
}
#[test]
fn failed_collection_drops_staging_and_drains_late_fence_without_retry_loop() {
    let mut model = model();
    admit(&mut model, false);
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "b"]);
    stage(&mut refresh, &mut model, "a", &["p"]);
    let batch = refresh.batch.as_mut().unwrap();
    batch.closing = true;
    model.catalog_in_flight = Some(batch.generation);
    let mut invalid = snap("b", &["r", "q"]);
    invalid["screen"]["panes"] = json!(null);
    let owner = model.connection_id;
    let delta = refresh.apply_snapshot(&mut model, owner, &invalid).unwrap();
    assert!(matches!(
        delta.transfer.unwrap().outcome,
        ScreenTransferOutcome::Unverified(_)
    ));
    assert!(refresh.batch.is_none());
    assert!(refresh.draining.is_some());
    stage(&mut refresh, &mut model, "a", &["p"]);
    assert!(
        refresh.batch.is_none(),
        "old snapshot cannot restart failed collection"
    );
    assert_eq!(model.screen_for("q"), Some("a"));
    refresh
        .apply_catalog(&mut model, owner, &cat(&["a", "b"]))
        .unwrap();
    assert!(refresh.draining.is_none());
    assert!(refresh.batch.is_none());
    assert!(!model.catalog_dirty);
    // An explicit later invalidation permits read recovery, never mutation replay.
    model.mark_catalog_dirty();
    opening(&mut refresh, &mut model, &["a", "b"]);
    stage(&mut refresh, &mut model, "a", &["p"]);
    stage(&mut refresh, &mut model, "b", &["r", "q"]);
    ending(&mut refresh, &mut model, &["a", "b"]);
    assert_eq!(model.screen_for("q"), Some("b"));
}
#[test]
fn nudge_during_inconsistent_collection_rearms_once_and_keeps_failure_correlation() {
    let mut model = model();
    admit(&mut model, false);
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "b"]);
    stage(&mut refresh, &mut model, "a", &["p", "q"]);
    stage(&mut refresh, &mut model, "b", &["r", "q"]);
    model.mark_catalog_dirty();
    let delta = ending(&mut refresh, &mut model, &["a", "b"]);
    assert!(delta.transfer.is_none());
    assert!(refresh.batch.as_ref().unwrap().repeated);
    assert!(model.transfer_pending());
    opening(&mut refresh, &mut model, &["a", "b"]);
    stage(&mut refresh, &mut model, "a", &["p"]);
    stage(&mut refresh, &mut model, "b", &["r", "q"]);
    let owner = model.connection_id;
    model.observe_transfer_failure(
        owner,
        &json!({"type":"mux.op_failed","screenId":"a","op":"move_pane_to_screen"}),
    );
    let delta = ending(&mut refresh, &mut model, &["a", "b"]);
    assert!(matches!(
        delta.transfer.unwrap().outcome,
        ScreenTransferOutcome::Rejected(_)
    ));
    assert!(!model.transfer_pending());
}
#[test]
fn aggregate_staged_counts_and_text_are_admitted_before_retaining_next_snapshot() {
    let mut model = model();
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "b"]);
    let names: Vec<String> = (0..MAX_ATTACHMENTS).map(|i| format!("p{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    stage(&mut refresh, &mut model, "a", &refs);
    let delta = stage(&mut refresh, &mut model, "b", &["r"]);
    assert!(delta.reconciliation_error.is_some());
    assert!(refresh.batch.is_none());
    assert_eq!(model.routes().len(), 3);
    opening(&mut refresh, &mut model, &["a", "b"]);
    let mut first = snap("a", &["p", "q"]);
    first["screen"]["panes"]["p"]["title"] = json!("x".repeat(MAX_FRAME_BYTES / 2 + 1));
    let owner = model.connection_id;
    refresh.apply_snapshot(&mut model, owner, &first).unwrap();
    let mut second = snap("b", &["r"]);
    second["screen"]["panes"]["r"]["title"] = json!("y".repeat(MAX_FRAME_BYTES / 2));
    let delta = refresh.apply_snapshot(&mut model, owner, &second).unwrap();
    assert!(delta.reconciliation_error.is_some());
    assert!(refresh.batch.is_none());
    assert_eq!(model.routes().len(), 3);
}
