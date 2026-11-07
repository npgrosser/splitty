use super::*;
use serde_json::json;
fn cat() -> Value {
    json!({"type":"mux.workspace_snapshot","screens":[{"id":"a"},{"id":"b"}],"activeScreenId":"a"})
}
fn snapshot(root: &[&str], floats: &[&str]) -> Value {
    let panes: Vec<_> = root.iter().chain(floats).copied().collect();
    json!({"type":"mux.screen_snapshot","screen":{"id":"a","revision":3,
        "root":{"type":"slot","id":"tile","paneIds":root,"activePaneId":root.first()},
        "floating":if floats.is_empty(){vec![]}else{vec![json!({"id":"float","bounds":{"x":0.25,"y":0.25,"w":0.5,"h":0.5},"root":{"type":"slot","id":"float-slot","paneIds":floats,"activePaneId":floats.first()}})]},
        "panes":panes.iter().map(|p|(p.to_string(),json!({"id":p,"sessionId":format!("s-{p}")}))).collect::<serde_json::Map<_,_>>(),
        "sessions":panes.iter().map(|p|(format!("s-{p}"),json!({"id":format!("s-{p}"),"status":"running"}))).collect::<serde_json::Map<_,_>>()}})
}
fn workspace(root: &[&str], floats: &[&str]) -> SourceWorkspace {
    let mut model = SourceWorkspace::new(Uuid::new_v4());
    model.set_catalog_supported(true);
    model.set_tab_selection_supported(true);
    model.catalog_in_flight = Some(0);
    model.apply_catalog(model.connection_id, &cat()).unwrap();
    model
        .apply_snapshot(model.connection_id, &snapshot(root, floats))
        .unwrap();
    model
}
fn fence(model: &mut SourceWorkspace) -> WorkspaceDelta {
    model.catalog_in_flight = Some(model.mutation);
    model.apply_catalog(model.connection_id, &cat()).unwrap()
}
#[test]
fn projection_cycle_maximize_and_pending_tab_focus_share_all_canonical_slots() {
    let mut model = workspace(&["a", "b"], &["c", "d"]);
    let routes = model.routes().len();
    assert_eq!(model.displayed_panes(), ["a", "c"]);
    assert_eq!(model.presented_floats().len(), 1);
    model.select_tab("d");
    assert_eq!(model.focused_slot_id(), Some("float-slot"));
    assert!(model.is_displayed_pane("d"));
    assert!(!model.is_displayed_pane("c"));
    assert!(model.toggle_maximize_focused());
    assert_eq!(
        model.presentation_root().unwrap().id().as_str(),
        "float-slot"
    );
    assert!(model.presented_floats().is_empty());
    assert_eq!(model.displayed_panes(), ["d"]);
    assert!(model
        .cycle_tab(model.connection_id, "a", "float-slot", TabStep::Previous)
        .unwrap());
    assert_eq!(model.displayed_panes(), ["c"]);
    assert_eq!(model.maximized_slot_id(), Some("float-slot"));
    model.shown = Some("b".into());
    model.shown = Some("a".into());
    assert_eq!(model.maximized_slot_id(), Some("float-slot"));
    assert!(model.toggle_maximize_focused());
    assert_eq!(model.displayed_panes(), ["a", "c"]);
    model.models.get_mut("a").unwrap().pending_focus = Some("e".into());
    model
        .models
        .get_mut("a")
        .unwrap()
        .pending_panes
        .insert("e".into());
    model
        .apply_snapshot(
            model.connection_id,
            &snapshot(&["a", "b"], &["c", "d", "e"]),
        )
        .unwrap();
    assert_eq!(model.focused_pane(), Some("e"));
    assert_eq!(model.focused_slot_id(), Some("float-slot"));
    assert_eq!(model.routes().len(), routes + 1);
    // Observed nested floats render all visible children and support focus/max.
    let mut nested = snapshot(&["a"], &["c", "d"]);
    nested["screen"]["floating"][0]["root"] = json!({"type":"split","id":"nested","axis":"horizontal","sizes":[0.4,0.6],"children":[{"type":"slot","id":"fl","paneIds":["c"]},{"type":"slot","id":"fr","paneIds":["d"]}]});
    model.apply_snapshot(model.connection_id, &nested).unwrap();
    assert_eq!(model.displayed_panes(), ["a", "c", "d"]);
    model.select_tab("d");
    assert!(model.toggle_maximize_focused());
    assert_eq!(model.displayed_panes(), ["d"]);
}
#[test]
fn capture_tolerates_metadata_but_rejects_bounds_structure_identity_and_noop() {
    let mut model = workspace(&["a"], &["c", "d"]);
    let target = model.capture_float("float").unwrap();
    let mut metadata = snapshot(&["a"], &["c", "d"]);
    metadata["screen"]["revision"] = json!(10);
    metadata["screen"]["panes"]["c"]["title"] = json!("changed");
    metadata["screen"]["floating"][0]["root"]["activePaneId"] = json!("d");
    model
        .apply_snapshot(model.connection_id, &metadata)
        .unwrap();
    assert!(model.validate_float(&target).is_ok());
    assert!(!model
        .move_float_with(
            Uuid::new_v4(),
            target.clone(),
            FloatBounds::default(),
            |_, _| panic!("unchanged move submitted")
        )
        .unwrap());
    assert!(model
        .move_float_with(
            Uuid::new_v4(),
            target.clone(),
            FloatBounds {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 1.0
            },
            |_, _| panic!("invalid move")
        )
        .is_err());
    let changed = FloatBounds {
        x: -0.1,
        y: 0.2,
        w: 1.5,
        h: 0.6,
    };
    let before = model.mutation;
    assert!(model
        .move_float_with(Uuid::new_v4(), target.clone(), changed, |_, _| Err(
            io::ErrorKind::WouldBlock.into()
        ))
        .is_err());
    assert_eq!(model.mutation, before);
    assert!(!model.floating_action_pending());
    for variation in 0..4 {
        let mut invalid = metadata.clone();
        match variation {
            0 => invalid["screen"]["floating"][0]["bounds"]["x"] = json!(0.3),
            1 => invalid["screen"]["floating"][0]["root"]["paneIds"] = json!(["d", "c"]),
            2 => invalid["screen"]["panes"]["c"]["sessionId"] = json!("different"),
            _ => invalid["screen"]["floating"][0]["id"] = json!("replacement"),
        }
        model.apply_snapshot(model.connection_id, &invalid).unwrap();
        assert!(model.validate_float(&target).is_err());
    }
}
#[test]
fn float_and_dock_confirm_only_after_fence_and_follow_can_be_cancelled() {
    let mut model = workspace(&["a", "b"], &[]);
    let source = model.capture_pane_action("b").unwrap();
    let request = Uuid::new_v4();
    assert!(model.can_float_pane(&source));
    model
        .float_with(request, source.clone(), |_, op| {
            assert!(matches!(op, ScreenOp::FloatPane { .. }));
            Ok(())
        })
        .unwrap();
    let delta = model
        .apply_snapshot(model.connection_id, &snapshot(&["a"], &["b"]))
        .unwrap();
    assert!(delta.floating.is_none());
    assert!(model.floating_action_pending());
    model.select_tab("a");
    fence(&mut model);
    let delta = model
        .apply_snapshot(model.connection_id, &snapshot(&["a"], &["b"]))
        .unwrap();
    assert_eq!(
        delta.floating.unwrap().outcome,
        FloatingActionOutcome::Confirmed
    );
    assert_eq!(model.focused_pane(), Some("a"));
    assert!(!model.can_float_pane(&source));
    let target = model.capture_float("float").unwrap();
    model
        .dock_with(Uuid::new_v4(), target, |_, op| {
            assert!(matches!(op,ScreenOp::DockPane{pane_id,..} if pane_id.as_str()=="a"));
            Ok(())
        })
        .unwrap();
    fence(&mut model);
    let delta = model
        .apply_snapshot(model.connection_id, &snapshot(&["a", "b"], &[]))
        .unwrap();
    assert_eq!(
        delta.floating.unwrap().outcome,
        FloatingActionOutcome::Confirmed
    );
    assert_eq!(model.focused_pane(), Some("b"));
    let single = workspace(&["a"], &[]);
    assert!(!single.can_float_pane(&single.capture_pane_action("a").unwrap()));
}
#[test]
fn empty_root_dock_and_late_failures_and_malformed_verification_do_not_stick() {
    let mut model = workspace(&[], &["c", "d"]);
    let target = model.capture_float("float").unwrap();
    model
        .dock_with(Uuid::new_v4(), target, |_, op| {
            assert!(matches!(op,ScreenOp::DockPane{pane_id,..} if pane_id.as_str()=="c"));
            Ok(())
        })
        .unwrap();
    fence(&mut model);
    let delta = model
        .apply_snapshot(model.connection_id, &snapshot(&["c", "d"], &[]))
        .unwrap();
    assert!(matches!(
        delta.floating.unwrap().outcome,
        FloatingActionOutcome::Confirmed
    ));
    model
        .apply_snapshot(model.connection_id, &snapshot(&["a"], &["c"]))
        .unwrap();
    let target = model.capture_float("float").unwrap();
    model
        .move_float_with(
            Uuid::new_v4(),
            target,
            FloatBounds {
                x: 0.1,
                ..FloatBounds::default()
            },
            |_, _| Ok(()),
        )
        .unwrap();
    model.observe_floating_failure(
        model.connection_id,
        &json!({"type":"mux.op_failed","screenId":"a","op":"move_float"}),
    );
    assert!(model.floating_action_pending());
    let delta = fence(&mut model);
    assert!(matches!(
        delta.floating.unwrap().outcome,
        FloatingActionOutcome::Rejected(_)
    ));
    assert!(!model.floating_action_pending());
    let target = model.capture_float("float").unwrap();
    model
        .move_float_with(
            Uuid::new_v4(),
            target,
            FloatBounds {
                x: 0.1,
                ..FloatBounds::default()
            },
            |_, _| Ok(()),
        )
        .unwrap();
    fence(&mut model);
    let mut refresh = WorkspaceRefresh::default();
    let owner = model.connection_id;
    let delta = refresh
        .apply_snapshot(
            &mut model,
            owner,
            &json!({"type":"mux.screen_snapshot","screen":{"id":"a"}}),
        )
        .unwrap();
    assert!(matches!(
        delta.floating.unwrap().outcome,
        FloatingActionOutcome::Unverified(_)
    ));
    assert!(!model.floating_action_pending());
}
#[test]
fn transfer_accepts_floating_sources_and_close_keeps_whole_window_available() {
    let mut model = workspace(&["a"], &["c"]);
    let source = model.capture_pane_action("c").unwrap();
    assert!(!model.can_float_pane(&source));
    let target = model.capture_float("float").unwrap();
    model
        .move_float_with(
            Uuid::new_v4(),
            target,
            FloatBounds {
                x: 0.1,
                ..FloatBounds::default()
            },
            |_, _| Ok(()),
        )
        .unwrap();
    assert!(model
        .assess_close(
            1,
            CloseTarget::Pane {
                screen: "a".into(),
                pane: "c".into()
            }
        )
        .is_err());
    let window = model.assess_close(2, CloseTarget::Window).unwrap();
    assert!(model.validate_close(&window, true).is_ok());
    fence(&mut model);
    let mut moved = snapshot(&["a"], &["c"]);
    moved["screen"]["floating"][0]["bounds"]["x"] = json!(0.1);
    model.apply_snapshot(model.connection_id, &moved).unwrap();
    model
        .transfer_with(
            Uuid::new_v4(),
            source,
            ScreenTransferTarget::Existing {
                screen: model.screen_key("b"),
            },
            |screen, pane, target, _| {
                assert_eq!((screen, pane, target), ("a", "c", "b"));
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn managed_tabs_float_but_cannot_transfer_and_old_refresh_cannot_settle() {
    let mut model = workspace(&["a", "b"], &[]);
    let mut jobs = snapshot(&["a", "b"], &[]);
    jobs["screen"]["panes"]["b"]["jobId"] = json!("managed");
    model.apply_snapshot(model.connection_id, &jobs).unwrap();
    let source = model.capture_pane_action("b").unwrap();
    assert!(!model.can_transfer_pane(&source));
    assert!(model.can_float_pane(&source));
    assert!(model
        .transfer_with(
            Uuid::new_v4(),
            source.clone(),
            ScreenTransferTarget::Existing {
                screen: model.screen_key("b")
            },
            |_, _, _, _| panic!("managed transfer admitted")
        )
        .is_err());
    model
        .float_with(Uuid::new_v4(), source, |_, _| Ok(()))
        .unwrap();
    assert!(model
        .settle_floating_after_refresh(model.mutation - 1)
        .is_none());
    assert!(model.floating_action_pending());
    let mut refresh = WorkspaceRefresh::default();
    refresh.begin(&mut model).unwrap();
    let connection = model.connection_id;
    // A coherent collection can include the admitted float without losing settlement.
    let only_a =
        json!({"type":"mux.workspace_snapshot","screens":[{"id":"a"}],"activeScreenId":"a"});
    model.catalog_in_flight = Some(model.mutation);
    refresh
        .apply_catalog(&mut model, connection, &only_a)
        .unwrap();
    refresh
        .apply_snapshot(&mut model, connection, &snapshot(&["a"], &["b"]))
        .unwrap();
    refresh.test_trailing(&mut model);
    let delta = refresh
        .apply_catalog(&mut model, connection, &only_a)
        .unwrap();
    assert!(delta.coherent_refresh);
    assert_eq!(
        delta.floating.unwrap().outcome,
        FloatingActionOutcome::Confirmed
    );
    assert!(!model.floating_action_pending());
}
