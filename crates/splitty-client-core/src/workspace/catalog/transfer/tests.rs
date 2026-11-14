//! Unit tests of a tab group's transfer intent: what is captured, what a
//! later snapshot invalidates, what is sent and which failure rejects it.
use super::*;
use serde_json::json;

fn cat() -> Value {
    json!({"type":"mux.workspace_snapshot","screens":[{"id":"a"},{"id":"b"}],"activeScreenId":"a"})
}

/// Screen `a`: `left` with `l`, the group `group` with `tabs` (the last
/// active), and a floating group `float-slot` with `floats`.
fn snapshot(tabs: &[&str], floats: &[&str]) -> Value {
    let panes: Vec<_> = ["l"].iter().chain(tabs).chain(floats).copied().collect();
    json!({"type":"mux.screen_snapshot","screen":{"id":"a","revision":3,
        "root":{"type":"split","id":"root","axis":"horizontal","sizes":[0.5,0.5],"children":[
            {"type":"slot","id":"left","paneIds":["l"]},
            {"type":"slot","id":"group","paneIds":tabs,"activePaneId":tabs.last()}]},
        "floating":if floats.is_empty(){vec![]}else{vec![json!({"id":"float","bounds":{"x":0.25,"y":0.25,"w":0.5,"h":0.5},"root":{"type":"slot","id":"float-slot","paneIds":floats,"activePaneId":floats.first()}})]},
        "panes":panes.iter().map(|p|(p.to_string(),json!({"id":p,"sessionId":format!("s-{p}")}))).collect::<serde_json::Map<_,_>>(),
        "sessions":panes.iter().map(|p|(format!("s-{p}"),json!({"id":format!("s-{p}"),"status":"running"}))).collect::<serde_json::Map<_,_>>()}})
}

fn workspace(tabs: &[&str], floats: &[&str]) -> SourceWorkspace {
    let mut model = SourceWorkspace::new(Uuid::new_v4());
    model.set_catalog_supported(true);
    model.set_tab_selection_supported(true);
    model.catalog_in_flight = Some(0);
    model.apply_catalog(model.connection_id, &cat()).unwrap();
    model
        .apply_snapshot(model.connection_id, &snapshot(tabs, floats))
        .unwrap();
    model
}

#[test]
fn a_group_is_captured_from_any_tab_with_the_tab_this_window_shows() {
    let model = workspace(&["g1", "g2", "g3"], &["f1", "f2"]);
    let group = model.capture_group_action("g1").unwrap();
    assert_eq!(group.slot_id, "group");
    assert_eq!(
        group.tabs,
        ["g1", "g2", "g3"].map(|p| (p.to_string(), format!("s-{p}")))
    );
    assert_eq!(
        group.shown.pane_id, "g3",
        "the group's shown tab, not the clicked one"
    );
    assert_eq!(group.shown.screen_id, "a");

    let floating = model.capture_group_action("f2").unwrap();
    assert_eq!(floating.slot_id, "float-slot");
    assert_eq!(floating.tab_count(), 2);
    assert_eq!(floating.shown.pane_id, "f1");
}

#[test]
fn a_changed_group_or_one_with_a_managed_job_terminal_does_not_move() {
    let mut model = workspace(&["g1", "g2"], &[]);
    let group = model.capture_group_action("g2").unwrap();
    assert!(model.can_transfer_group(&group));
    // A tab joined the group since: the captured intent is stale.
    model.catalog_in_flight = Some(model.mutation);
    model.apply_catalog(model.connection_id, &cat()).unwrap();
    model
        .apply_snapshot(model.connection_id, &snapshot(&["g1", "g2", "g4"], &[]))
        .unwrap();
    assert!(!model.can_transfer_group(&group));
    assert!(model
        .transfer_group_with(
            Uuid::new_v4(),
            group,
            ScreenTransferTarget::Existing {
                screen: model.screen_key("b"),
            },
            |_, _, _, _, _| panic!("stale group sent"),
        )
        .is_err());

    let mut jobs = snapshot(&["g1", "g2"], &[]);
    jobs["screen"]["panes"]["g1"]["jobId"] = json!("managed");
    let mut model = workspace(&["g1", "g2"], &[]);
    model.apply_snapshot(model.connection_id, &jobs).unwrap();
    let error = model.capture_group_action("g2").unwrap_err();
    assert!(error.to_string().contains("managed job"), "{error}");
}

#[test]
fn a_group_transfer_sends_once_and_only_its_own_failure_rejects_it() {
    let mut model = workspace(&["g1", "g2"], &[]);
    let group = model.capture_group_action("g1").unwrap();
    let request = Uuid::new_v4();
    let mut sent = None;
    model
        .transfer_group_with(
            request,
            group.clone(),
            ScreenTransferTarget::New {
                screen_id: "c".into(),
            },
            |screen, slot, shown, target, create| {
                sent = Some((
                    screen.to_owned(),
                    slot.to_owned(),
                    shown.to_owned(),
                    target.to_owned(),
                    create,
                ));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(
        sent,
        Some(("a".into(), "group".into(), "g2".into(), "c".into(), true))
    );
    assert!(model.transfer_request_pending(request));
    assert!(model
        .transfer_group_with(
            Uuid::new_v4(),
            group,
            ScreenTransferTarget::Existing {
                screen: model.screen_key("b"),
            },
            |_, _, _, _, _| panic!("second transfer admitted"),
        )
        .is_err());

    let connection = model.connection_id;
    let failed = |op: &str| json!({"type":"mux.op_failed","op":op,"screenId":"a","error":"x"});
    model.observe_transfer_failure(connection, &failed("move_pane_to_screen"));
    assert!(model.pending_transfer.as_ref().unwrap().failure.is_none());
    model.observe_transfer_failure(connection, &failed("move_tab_group_to_screen"));
    let result = model.finish_transfer(None).unwrap();
    assert_eq!(result.request_id, request);
    assert!(matches!(
        result.outcome,
        ScreenTransferOutcome::Rejected(ref reason) if reason.contains("rejected by the service")
    ));
}
