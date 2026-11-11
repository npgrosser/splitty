use super::*;
use serde_json::json;

fn catalog(ids: &[&str]) -> Value {
    json!({"type":"mux.workspace_snapshot", "screens": ids.iter().map(|id| json!({"id":id})).collect::<Vec<_>>(), "activeScreenId":ids.first()})
}

fn snapshot(revision: u64, left: &[&str], right: &[&str]) -> Value {
    let panes: Vec<_> = left.iter().chain(right).copied().collect();
    json!({"type":"mux.screen_snapshot", "screen":{
        "id":"screen", "revision":revision,
        "root":{"type":"split","id":"root","axis":"horizontal","sizes":[0.7,0.3],"children":[
            {"type":"slot","id":"left","paneIds":left,"activePaneId":left.first()},
            {"type":"slot","id":"right","paneIds":right,"activePaneId":right.first()}
        ]},
        "panes":panes.iter().map(|pane| (pane.to_string(),json!({"id":pane,"sessionId":format!("session-{pane}")}))).collect::<serde_json::Map<_,_>>(),
        "sessions":panes.iter().map(|pane| (format!("session-{pane}"),json!({"id":format!("session-{pane}"),"status":"running"}))).collect::<serde_json::Map<_,_>>()
    }})
}

fn workspace() -> SourceWorkspace {
    let mut model = SourceWorkspace::new(Uuid::new_v4());
    model.set_catalog_supported(true);
    model.set_tab_placement_supported(true);
    model.catalog_in_flight = Some(0);
    model
        .apply_catalog(model.connection_id, &catalog(&["screen", "other"]))
        .unwrap();
    model
        .apply_snapshot(
            model.connection_id,
            &snapshot(1, &["a", "b", "c"], &["d", "e"]),
        )
        .unwrap();
    model
}

fn admit(model: &mut SourceWorkspace, pane: &str, target: &str, before: Option<&str>) {
    let drag = model.capture_drag(pane).unwrap();
    assert!(model
        .place_tab_with(
            drag,
            target.into(),
            before.map(str::to_owned),
            |_, _| Ok(())
        )
        .unwrap());
}

fn fence(model: &mut SourceWorkspace) -> WorkspaceDelta {
    model.catalog_in_flight = Some(model.mutation);
    model
        .apply_catalog(model.connection_id, &catalog(&["screen", "other"]))
        .unwrap()
}

fn apply(
    model: &mut SourceWorkspace,
    revision: u64,
    left: &[&str],
    right: &[&str],
) -> WorkspaceDelta {
    model
        .apply_snapshot(model.connection_id, &snapshot(revision, left, right))
        .unwrap()
}

#[test]
fn placement_admission_is_capability_guarded_atomic_and_noop_aware() {
    let mut model = workspace();
    let drag = model.capture_drag("a").unwrap();
    assert!(!model.check_place_tab(&drag, "left", Some("a")).unwrap());
    assert!(!model.check_place_tab(&drag, "left", Some("b")).unwrap());
    assert!(model.check_place_tab(&drag, "left", None).unwrap());
    assert!(model.check_place_tab(&drag, "right", Some("b")).is_err());
    assert!(model.check_place_tab(&drag, "gone", None).is_err());
    model.set_tab_placement_supported(false);
    assert_eq!(
        model
            .check_place_tab(&drag, "right", None)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Unsupported
    );
    model.set_tab_placement_supported(true);
    assert!(model.toggle_maximize_focused());
    let before = serde_json::to_value(model.snapshot()).unwrap();
    let mutation = model.mutation;
    let error = model
        .place_tab_with(drag, "right".into(), None, |_, _| {
            Err(io::ErrorKind::WouldBlock.into())
        })
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(model.mutation, mutation);
    assert!(!model.placement_pending());
    assert_eq!(model.maximized_slot_id(), Some("left"));
    assert_eq!(serde_json::to_value(model.snapshot()).unwrap(), before);
    admit(&mut model, "a", "right", None);
    assert_eq!(model.maximized_slot_id(), None);
}

#[test]
fn captured_tab_placement_allows_metadata_but_rejects_replaced_identity_and_layout() {
    let mut model = workspace();
    let drag = model.capture_drag("a").unwrap();
    let mut metadata = snapshot(2, &["a", "b", "c"], &["d", "e"]);
    metadata["screen"]["panes"]["a"]["title"] = json!("new title");
    model
        .apply_snapshot(model.connection_id, &metadata)
        .unwrap();
    assert!(model.check_place_tab(&drag, "right", Some("e")).unwrap());
    apply(&mut model, 3, &["a", "c", "b"], &["d", "e"]);
    assert!(model.check_place_tab(&drag, "right", Some("e")).is_err());
    let fresh = model.capture_drag("a").unwrap();
    model.reset_connection(Uuid::new_v4());
    assert!(model.check_place_tab(&fresh, "right", None).is_err());
}

#[test]
fn exact_position_waits_for_fence_and_preserves_same_group_selection_and_maximize() {
    let mut model = workspace();
    model.select_tab("b");
    assert!(model.toggle_maximize_focused());
    admit(&mut model, "a", "left", None);
    assert!(model.placement_pending());
    // The desired position alone is insufficient before the ordered fence.
    assert_eq!(
        apply(&mut model, 2, &["b", "c", "a"], &["d", "e"]).placement,
        None
    );
    model.catalog_in_flight = Some(0);
    assert!(model
        .apply_catalog(model.connection_id, &catalog(&["screen", "other"]))
        .unwrap()
        .placement
        .is_none());
    assert!(model.placement_pending());
    fence(&mut model);
    assert_eq!(
        apply(&mut model, 3, &["b", "c", "a"], &["d", "e"]).placement,
        Some(PlacementOutcome::Confirmed)
    );
    assert!(!model.placement_pending());
    assert_eq!(model.focused_pane(), Some("b"));
    assert_eq!(model.maximized_slot_id(), Some("left"));
    assert_eq!(model.session_for("a"), Some("session-a"));
}

#[test]
fn tab_failure_is_retained_until_fence_and_cannot_reject_a_newer_intent() {
    let mut model = workspace();
    admit(&mut model, "a", "right", Some("e"));
    let failure = json!({"type":"mux.op_failed","op":"move_tab","screenId":"screen"});
    model.observe_placement_failure(Uuid::new_v4(), &failure);
    assert!(model.pending_placement.as_ref().unwrap().failure.is_none());
    model.observe_placement_failure(model.connection_id, &failure);
    assert!(model
        .check_place_tab(&model.capture_drag("b").unwrap(), "right", None)
        .is_err());
    model.catalog_in_flight = Some(0);
    model
        .apply_catalog(model.connection_id, &catalog(&["screen", "other"]))
        .unwrap();
    assert!(model.placement_pending());
    assert!(matches!(
        fence(&mut model).placement,
        Some(PlacementOutcome::Rejected(_))
    ));
    admit(&mut model, "b", "right", None);
    assert!(model.pending_placement.as_ref().unwrap().failure.is_none());
    fence(&mut model);
    assert_eq!(
        apply(&mut model, 2, &["a", "c"], &["d", "e", "b"]).placement,
        Some(PlacementOutcome::Confirmed)
    );
    assert_eq!(model.focused_pane(), Some("b"));
}

#[test]
fn cross_group_focus_can_be_superseded_without_discarding_settlement() {
    let mut model = workspace();
    admit(&mut model, "a", "right", Some("e"));
    model.select_tab("c");
    assert!(model.placement_pending());
    fence(&mut model);
    assert_eq!(
        apply(&mut model, 2, &["b", "c"], &["d", "a", "e"]).placement,
        Some(PlacementOutcome::Confirmed)
    );
    assert_eq!(model.focused_pane(), Some("c"));
    admit(&mut model, "b", "right", None);
    fence(&mut model);
    // Another valid position after the fence is a terminal outcome, not a retry.
    assert!(matches!(
        apply(&mut model, 3, &["c"], &["b", "d", "a", "e"]).placement,
        Some(PlacementOutcome::Rejected(_))
    ));
    assert!(!model.placement_pending());
    // The moved tab took focus when it was dropped.
    assert_eq!(model.focused_pane(), Some("b"));
}

#[test]
fn malformed_verification_removed_screen_and_refresh_pressure_release_or_retain_correctly() {
    let mut model = workspace();
    admit(&mut model, "a", "right", None);
    assert!(model
        .flush_catalog_request(|| Err(io::ErrorKind::WouldBlock.into()))
        .is_err());
    assert!(model.catalog_dirty && model.placement_pending());
    model.flush_catalog_request(|| Ok(())).unwrap();
    let bad = json!({"type":"mux.workspace_snapshot","screens":"invalid"});
    assert!(model.apply_catalog(model.connection_id, &bad).is_err());
    assert!(!model.placement_pending());
    admit(&mut model, "a", "right", None);
    fence(&mut model);
    assert!(model
        .apply_snapshot(
            model.connection_id,
            &json!({"type":"mux.screen_snapshot","screen":{"id":"screen"}})
        )
        .is_err());
    assert!(!model.placement_pending());
    admit(&mut model, "a", "right", None);
    model.catalog_in_flight = Some(model.mutation);
    let delta = model
        .apply_catalog(model.connection_id, &catalog(&["other"]))
        .unwrap();
    assert!(matches!(
        delta.placement,
        Some(PlacementOutcome::Rejected(_))
    ));
    assert!(!model.placement_pending());
}

#[test]
fn cycle_tabs_uses_latest_selection_clamps_and_rejects_stale_captures() {
    let mut model = workspace();
    let owner = model.connection_id;
    assert!(!model
        .cycle_tab(owner, "screen", "left", TabStep::Previous)
        .unwrap());
    assert!(model
        .cycle_tab(owner, "screen", "left", TabStep::Next)
        .unwrap());
    assert_eq!(model.focused_pane(), Some("b"));
    assert!(model
        .cycle_tab(owner, "screen", "left", TabStep::Next)
        .unwrap());
    assert_eq!(model.focused_pane(), Some("c"));
    assert!(!model
        .cycle_tab(owner, "screen", "left", TabStep::Next)
        .unwrap());
    assert!(model
        .cycle_tab(Uuid::new_v4(), "screen", "left", TabStep::Previous)
        .is_err());
    assert!(model
        .cycle_tab(owner, "other", "left", TabStep::Previous)
        .is_err());
    model.select_tab("d");
    assert!(model
        .cycle_tab(owner, "screen", "left", TabStep::Previous)
        .is_err());
    assert_eq!(model.focused_pane(), Some("d"));
}

#[test]
fn hovered_group_navigation_uses_its_own_selection_and_rejects_stale_targets() {
    let mut model = workspace();
    let owner = model.connection_id;
    assert_eq!(model.focused_slot_id(), Some("left"));
    assert!(model
        .cycle_tab_in_slot(owner, "screen", "right", TabStep::Next)
        .unwrap());
    assert_eq!(model.focused_pane(), Some("e"));
    assert_eq!(model.focused_slot_id(), Some("right"));
    assert!(!model
        .cycle_tab_in_slot(owner, "screen", "right", TabStep::Next)
        .unwrap());
    assert!(model
        .cycle_tab_in_slot(owner, "screen", "left", TabStep::Next)
        .unwrap());
    assert_eq!(model.focused_pane(), Some("b"));
    assert!(model
        .cycle_tab(owner, "screen", "right", TabStep::Previous)
        .is_err());
    assert_eq!(model.focused_pane(), Some("b"));
    assert!(model
        .cycle_tab_in_slot(Uuid::new_v4(), "screen", "left", TabStep::Next)
        .is_err());
    assert!(model
        .cycle_tab_in_slot(owner, "other", "left", TabStep::Next)
        .is_err());
    assert!(model
        .cycle_tab_in_slot(owner, "screen", "gone", TabStep::Next)
        .is_err());
    assert_eq!(model.focused_pane(), Some("b"));
    assert_eq!(
        model.selected_pane(
            model
                .snapshot()
                .unwrap()
                .root
                .find_node(&NodeId::from("right"))
                .unwrap()
        ),
        Some("e")
    );
    model
        .apply_snapshot(
            owner,
            &json!({"type":"mux.screen_snapshot", "screen":{
                "id":"screen", "revision":2,
                "root":{"type":"slot","id":"left","paneIds":["a","b","c"],"activePaneId":"a"},
                "panes":{
                    "a":{"id":"a"},"b":{"id":"b"},"c":{"id":"c"}
                },
                "sessions":{}
            }}),
        )
        .unwrap();
    assert!(model
        .cycle_tab_in_slot(owner, "screen", "right", TabStep::Previous)
        .is_err());
}

#[test]
fn stale_hovered_group_does_not_cancel_pending_placement_focus() {
    let mut model = workspace();
    let owner = model.connection_id;
    admit(&mut model, "a", "right", None);
    assert!(model.pending_placement.as_ref().unwrap().focus);
    assert!(model
        .cycle_tab_in_slot(owner, "screen", "gone", TabStep::Next)
        .is_err());
    assert!(model.pending_placement.as_ref().unwrap().focus);
}

#[test]
fn post_fence_equal_revision_confirms_but_lower_revision_rejects_without_losing_state() {
    let mut model = workspace();
    admit(&mut model, "a", "left", None);
    apply(&mut model, 2, &["b", "c", "a"], &["d", "e"]);
    fence(&mut model);
    assert_eq!(
        apply(&mut model, 2, &["b", "c", "a"], &["d", "e"]).placement,
        Some(PlacementOutcome::Confirmed)
    );
    // A rejected placement returns the window to this confirmed state.
    let before = serde_json::to_value(model.snapshot()).unwrap();
    admit(&mut model, "b", "right", None);
    fence(&mut model);
    let old = snapshot(1, &["b", "c", "a"], &["d", "e"]);
    assert!(model
        .apply_snapshot(Uuid::new_v4(), &old)
        .unwrap()
        .placement
        .is_none());
    assert!(model.placement_pending());
    let mut other = old.clone();
    other["screen"]["id"] = json!("unrelated");
    assert!(model
        .apply_snapshot(model.connection_id, &other)
        .unwrap()
        .placement
        .is_none());
    assert!(model.placement_pending());
    let delta = model.apply_snapshot(model.connection_id, &old).unwrap();
    assert!(matches!(
        delta.placement,
        Some(PlacementOutcome::Rejected(_))
    ));
    assert!(!model.placement_pending());
    assert!(!delta.snapshot_changed);
    assert_eq!(serde_json::to_value(model.snapshot()).unwrap(), before);
}

#[test]
fn coherent_cross_screen_refresh_settles_an_earlier_tab_placement() {
    for superseded in [false, true] {
        let mut model = workspace();
        admit(&mut model, "a", "right", None);
        let mut refresh = crate::workspace::WorkspaceRefresh::default();
        refresh.begin(&mut model).unwrap();
        let owner = model.connection_id;
        model.catalog_in_flight = Some(model.mutation);
        refresh
            .apply_catalog(&mut model, owner, &catalog(&["screen", "other"]))
            .unwrap();
        let right = if superseded {
            vec!["d", "e"]
        } else {
            vec!["d", "e", "a"]
        };
        refresh
            .apply_snapshot(&mut model, owner, &snapshot(2, &["b", "c"], &right))
            .unwrap();
        let other = if superseded { vec!["a"] } else { vec![] };
        let event = json!({"type":"mux.screen_snapshot","screen":{"id":"other","revision":2,
            "root":{"type":"slot","id":"other-slot","paneIds":other,"activePaneId":other.first()},
            "panes":other.iter().map(|pane|(pane.to_string(),json!({"id":pane,"sessionId":format!("session-{pane}")}))).collect::<serde_json::Map<_,_>>(),
            "sessions":other.iter().map(|pane|(format!("session-{pane}"),json!({"id":format!("session-{pane}"),"status":"running"}))).collect::<serde_json::Map<_,_>>()}});
        refresh.apply_snapshot(&mut model, owner, &event).unwrap();
        // Admit the collector's trailing fence through its real bounded sender
        // in production; this pure test marks that phase after all snapshots.
        refresh.test_trailing(&mut model);
        let delta = refresh
            .apply_catalog(&mut model, owner, &catalog(&["screen", "other"]))
            .unwrap();
        assert!(!model.placement_pending());
        assert_eq!(
            matches!(delta.placement, Some(PlacementOutcome::Confirmed)),
            !superseded
        );
        assert_eq!(model.session_for("a"), Some("session-a"));
    }
}
