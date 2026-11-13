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
fn model() -> SourceWorkspace {
    let mut model = SourceWorkspace::new(Uuid::new_v4());
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
fn admit(model: &mut SourceWorkspace, new: bool) -> Uuid {
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
fn opening(refresh: &mut WorkspaceRefresh, model: &mut SourceWorkspace, ids: &[&str]) {
    refresh.begin(model).unwrap();
    model.catalog_in_flight = Some(refresh.batch.as_ref().unwrap().generation);
    refresh
        .apply_catalog(model, model.connection_id, &cat(ids))
        .unwrap();
}
fn stage(
    refresh: &mut WorkspaceRefresh,
    model: &mut SourceWorkspace,
    id: &str,
    panes: &[&str],
) -> WorkspaceDelta {
    refresh
        .apply_snapshot(model, model.connection_id, &snap(id, panes))
        .unwrap()
}
fn ending(
    refresh: &mut WorkspaceRefresh,
    model: &mut SourceWorkspace,
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

#[test]
fn simultaneous_sources_share_staging_lease_until_commit_failure_or_drop() {
    let mut first = model();
    let mut second = model();
    let bytes = codec::json_size(&snap("a", &["p", "q"])["screen"], MAX_FRAME_BYTES).unwrap();
    let shared = Budget::new(bytes);
    first.staged = shared.clone();
    second.staged = shared.clone();
    let mut first_refresh = WorkspaceRefresh::default();
    let mut second_refresh = WorkspaceRefresh::default();
    opening(&mut first_refresh, &mut first, &["a"]);
    opening(&mut second_refresh, &mut second, &["a"]);
    stage(&mut first_refresh, &mut first, "a", &["p", "q"]);
    assert_eq!(shared.used(), bytes);
    let failed = stage(&mut second_refresh, &mut second, "a", &["p", "q"]);
    assert!(failed.reconciliation_error.is_some());
    assert_eq!(shared.used(), bytes);
    assert_eq!(second.routes().len(), 3);
    ending(&mut first_refresh, &mut first, &["a"]);
    assert_eq!(shared.used(), 0);
    opening(&mut second_refresh, &mut second, &["a"]);
    stage(&mut second_refresh, &mut second, "a", &["p", "q"]);
    assert_eq!(shared.used(), bytes);
    drop(second_refresh);
    assert_eq!(shared.used(), 0);
    assert_eq!(second.routes().len(), 3);
}

#[test]
fn pending_new_transfer_reserves_a_screen_without_duplicating_its_panes() {
    let mut model = model();
    assert_eq!(model.screen_count(), 2);
    assert_eq!(model.admission_count(), 3);
    admit(&mut model, true);
    assert_eq!(model.screen_count(), 3);
    assert_eq!(model.admission_count(), 3);
    model.finish_transfer(Some("rejected".into()));
    assert_eq!(model.screen_count(), 2);
}

#[test]
fn refresh_preserves_newer_pending_local_creation_without_requiring_an_old_snapshot() {
    let mut model = model();
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "b"]);
    let generation = model.mutation + 1;
    let screen = ScreenModel::new(
        ScreenIds {
            screen_id: "new".into(),
            pane_id: "new-pane".into(),
        },
        model.connection_id,
    );
    model.models.insert("new".into(), screen);
    model.pending_screens.insert(
        "new".into(),
        PendingScreen {
            mutation: generation,
        },
    );
    model.mutation = generation;
    stage(&mut refresh, &mut model, "a", &["p", "q"]);
    stage(&mut refresh, &mut model, "b", &["r"]);
    let delta = ending(&mut refresh, &mut model, &["a", "b"]);
    assert!(delta.coherent_refresh);
    assert!(model.models.contains_key("new"));
    assert_eq!(model.admission_count(), 4);
}

#[test]
fn remote_opening_catalog_holds_its_decoding_charge_until_collection_drops() {
    let budget = ConnectionBudget::default();
    let mut model = SourceWorkspace::remote(
        SourceId::initial(Uuid::new_v4()),
        Uuid::new_v4(),
        "host.test".into(),
        budget.clone(),
    );
    model.set_catalog_supported(true);
    model.catalog_in_flight = Some(0);
    model
        .apply_catalog(model.connection_id, &cat(&["a"]))
        .unwrap();
    model.open_existing("a").unwrap();
    let accepted_charge = budget.used();
    assert!(accepted_charge > 0);
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a"]);
    assert!(budget.used() > accepted_charge);
    drop(refresh);
    assert_eq!(budget.used(), accepted_charge);
    drop(model);
    assert_eq!(budget.used(), 0);
}

#[test]
fn failed_collection_is_quiet_until_a_new_explicit_pass_is_active() {
    let mut model = model();
    let mut refresh = WorkspaceRefresh::default();
    refresh.begin(&mut model).unwrap();
    assert!(refresh.observation_active());
    // The runtime must distinguish an active new pass from an old ordered
    // response still being drained after timeout.
    model.catalog_in_flight = Some(model.mutation);
    refresh.abort_observation(&mut model, "timed out".into());
    assert!(!refresh.observation_active());
    assert!(refresh.observation_pending(&model));
    let connection = model.connection_id();
    refresh
        .apply_catalog(&mut model, connection, &cat(&["a", "b"]))
        .unwrap();
    assert!(!refresh.observation_active());
    refresh.begin(&mut model).unwrap();
    assert!(refresh.observation_active());
}

fn remote_model(ids: &[&str]) -> SourceWorkspace {
    let mut model = SourceWorkspace::remote(
        SourceId::initial(Uuid::new_v4()),
        Uuid::new_v4(),
        "host.test".into(),
        ConnectionBudget::default(),
    );
    model.set_catalog_supported(true);
    model.catalog_in_flight = Some(0);
    model.apply_catalog(model.connection_id, &cat(ids)).unwrap();
    model
}

#[test]
fn remote_absence_is_missing_but_present_silent_resync_exhausts_one_recollection() {
    let mut model = remote_model(&["a", "unopened"]);
    model.open_existing("a").unwrap();
    model
        .apply_snapshot(model.connection_id, &snap("a", &["p"]))
        .unwrap();
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "unopened"]);
    let first = ending(&mut refresh, &mut model, &["a", "unopened"]);
    assert!(first.reconciliation_error.is_none());
    assert!(refresh.batch.as_ref().unwrap().repeated);
    let failed = ending(&mut refresh, &mut model, &["a", "unopened"]);
    assert!(failed.reconciliation_error.is_some());
    assert!(!refresh.observation_active());
    assert!(
        !refresh.needs_flush(&model),
        "failed present resync does not loop"
    );
    assert_eq!(model.session_for("p"), Some("s-p"));
    assert!(model.screen("unopened").is_none());
    opening(&mut refresh, &mut model, &["unopened"]);
    let absent = ending(&mut refresh, &mut model, &["unopened"]);
    assert!(absent.coherent_refresh);
    assert!(model.is_missing("a"));
    assert!(model.screen("a").is_none());
    assert!(model.screen("unopened").is_none());
    assert_eq!(
        model.screens().len(),
        1,
        "missing reference is retained for Detach"
    );
}

#[test]
fn remote_external_move_to_unopened_screen_never_adopts_destination_or_late_routes() {
    let mut model = remote_model(&["a", "b", "unopened"]);
    model.open_existing("a").unwrap();
    model.open_existing("b").unwrap();
    model
        .apply_snapshot(model.connection_id, &snap("a", &["p"]))
        .unwrap();
    model
        .apply_snapshot(model.connection_id, &snap("b", &["q", "r"]))
        .unwrap();
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "b", "unopened"]);
    stage(&mut refresh, &mut model, "a", &["p"]);
    stage(&mut refresh, &mut model, "b", &["r"]);
    stage(&mut refresh, &mut model, "unopened", &["q"]);
    let committed = ending(&mut refresh, &mut model, &["a", "b", "unopened"]);
    assert!(committed.coherent_refresh);
    assert_eq!(model.session_for("p"), Some("s-p"));
    assert_eq!(model.session_for("r"), Some("s-r"));
    assert!(model.session_for("q").is_none());
    assert!(model.screen("unopened").is_none());
    opening(&mut refresh, &mut model, &["a", "b", "unopened"]);
    model.detach_reference("a").unwrap();
    stage(&mut refresh, &mut model, "a", &["p"]);
    stage(&mut refresh, &mut model, "b", &["r"]);
    let committed = ending(&mut refresh, &mut model, &["a", "b", "unopened"]);
    assert!(committed.coherent_refresh);
    assert!(model.session_for("p").is_none());
    assert!(model.screen("a").is_none());
    assert_eq!(model.screens().len(), 1);
    assert_eq!(model.session_for("r"), Some("s-r"));
}

/// `snap`, with each session showing `title`, at `revision`.
fn titled(id: &str, panes: &[&str], title: &str, revision: u64) -> Value {
    let mut event = snap(id, panes);
    event["screen"]["revision"] = json!(revision);
    for session in event["screen"]["sessions"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        session["displayTitle"] = json!(title);
    }
    event
}

/// A host source with `a` showing `p` and `b` showing `q`, both opened and
/// settled by a coherent refresh.
fn settled_remote() -> (WorkspaceRefresh, SourceWorkspace) {
    let mut model = remote_model(&["a", "b"]);
    model.open_existing("b").unwrap();
    model.open_existing("a").unwrap();
    let mut refresh = WorkspaceRefresh::default();
    opening(&mut refresh, &mut model, &["a", "b"]);
    stage(&mut refresh, &mut model, "a", &["p"]);
    stage(&mut refresh, &mut model, "b", &["q"]);
    assert!(ending(&mut refresh, &mut model, &["a", "b"]).coherent_refresh);
    for screen in model.models.values_mut() {
        // The flush that sent the reads cleared them.
        screen.resync_pending = false;
    }
    assert!(!refresh.needs_flush(&model));
    (refresh, model)
}

#[test]
fn host_metadata_read_of_one_screen_keeps_the_routes_and_their_generation() {
    let (mut refresh, mut model) = settled_remote();
    let generation = model.route_generation();
    model.mark_screen_metadata_dirty("a");
    assert!(refresh.needs_flush(&model), "the worker reads the Screen");
    assert!(
        !refresh.needs_coherent(&model),
        "a metadata change alone does not refresh every opened Screen"
    );
    for (revision, title) in [(3, "Working"), (4, "Working."), (5, "Working..")] {
        let connection = model.connection_id;
        let delta = refresh
            .apply_snapshot(
                &mut model,
                connection,
                &titled("a", &["p"], title, revision),
            )
            .unwrap();
        assert!(delta.snapshot_changed && !delta.coherent_refresh);
        assert!(delta.added.is_empty() && delta.removed.is_empty());
        assert!(!refresh.observation_active(), "no coherent refresh begins");
        let snapshot = model.screen("a").unwrap().snapshot().unwrap();
        assert_eq!(
            snapshot.sessions[&"s-p".into()].display_title.as_deref(),
            Some(title)
        );
    }
    assert_eq!(model.route_generation(), generation);
    assert_eq!(model.session_for("p"), Some("s-p"));
    assert_eq!(model.session_for("q"), Some("s-q"));
}

#[test]
fn host_metadata_read_that_shows_a_layout_change_refreshes_coherently() {
    // A pane closed on exit, a split another client made and a pane moved in
    // from another opened Screen all change the read Screen's layout.
    for panes in [&[][..], &["p", "n"][..], &["p", "q"][..]] {
        let (mut refresh, mut model) = settled_remote();
        let generation = model.route_generation();
        model.mark_screen_metadata_dirty("a");
        let connection = model.connection_id;
        let delta = refresh
            .apply_snapshot(&mut model, connection, &titled("a", panes, "exited", 3))
            .unwrap();
        assert!(delta.snapshot_changed && !delta.coherent_refresh);
        assert!(refresh.observation_active(), "{panes:?} begins the refresh");
        assert_eq!(
            model.session_for("p"),
            Some("s-p"),
            "routes stay until it commits"
        );
        assert_eq!(model.route_generation(), generation);
    }
}

#[test]
fn host_metadata_change_waits_for_the_coherent_refresh_while_routes_are_unsettled() {
    let (refresh, mut model) = settled_remote();
    model.mark_screen_metadata_dirty("a");
    model.mark_screen_dirty("b");
    assert!(
        refresh.needs_coherent(&model),
        "a structural nudge takes the metadata read along"
    );
    let (refresh, mut model) = settled_remote();
    model.mark_screen_metadata_dirty("a");
    model.mark_catalog_dirty();
    assert!(refresh.needs_coherent(&model));
    let (refresh, mut model) = settled_remote();
    model.mark_screen_metadata_dirty("a");
    model.reconciling = true;
    assert!(
        refresh.needs_coherent(&model),
        "an unsettled source recovers through the coherent refresh"
    );
}

#[test]
fn local_metadata_nudge_reads_its_screen_as_before() {
    let mut model = model();
    model.mark_screen_metadata_dirty("a");
    assert!(model.screen("a").unwrap().resync_pending);
    assert!(!model.screen("a").unwrap().metadata_pending);
}
