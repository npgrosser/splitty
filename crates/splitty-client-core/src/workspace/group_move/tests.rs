//! Unit tests: the arrangement a group drop sends, over a model fed with
//! snapshots (`workspace-model`, Native Tab Group Move).
use super::*;
use serde_json::json;

/// `[slot-a: a | slot-b: b1, b2 | slot-c: c]` side by side, `slot-b`
/// showing `b2` in this window and naming no active tab on the server.
fn three_groups() -> ScreenModel {
    let mut model = ScreenModel::new(
        ScreenIds {
            screen_id: "screen".into(),
            pane_id: "a".into(),
        },
        Uuid::new_v4(),
    );
    let pane = |id: &str| json!({"id":id,"sessionId":format!("s-{id}")});
    let session = |id: &str| json!({"id":format!("s-{id}"),"status":"running"});
    let event = json!({"type":"mux.screen_snapshot", "screen": {
        "id":"screen", "revision":1,
        "root":{"type":"split","id":"root","axis":"horizontal","sizes":[0.2,0.3,0.5],"children":[
            {"type":"slot","id":"slot-a","paneIds":["a"]},
            {"type":"slot","id":"slot-b","paneIds":["b1","b2"]},
            {"type":"slot","id":"slot-c","paneIds":["c"]}]},
        "panes":{"a":pane("a"),"b1":pane("b1"),"b2":pane("b2"),"c":pane("c")},
        "sessions":{"s-a":session("a"),"s-b1":session("b1"),"s-b2":session("b2"),"s-c":session("c")}
    }});
    model.apply_snapshot(model.connection_id, &event).unwrap();
    model.select_tab("b2");
    model.select_tab("a");
    model
}

fn arranged(op: Option<ScreenOp>) -> serde_json::Value {
    match op {
        Some(ScreenOp::ArrangePanes { root }) => serde_json::to_value(root).unwrap(),
        other => panic!("expected arrange_panes, got {other:?}"),
    }
}

fn slot(panes: &[&str], active: &str) -> serde_json::Value {
    json!({"type":"slot","paneIds":panes,"activePaneId":active})
}

#[test]
fn a_group_dropped_into_another_lands_before_the_marked_tab_and_shows_its_shown_tab() {
    let model = three_groups();
    let drag = model.capture_group_drag("slot-b").unwrap();
    assert_eq!(drag.shown_pane(), "b2");
    assert_eq!(drag.tab_count(), 2);
    let op = model
        .group_move_op(&drag, "slot-a", MovePlacement::Tab, Some("a"))
        .unwrap();
    let root = arranged(op);
    assert_eq!(
        root,
        json!({"type":"split","axis":"horizontal","sizes":[0.2 / 0.7, 0.5 / 0.7],"children":[
            slot(&["b1", "b2", "a"], "b2"),
            slot(&["c"], "c"),
        ]})
    );
    // Without an anchor the tabs go last.
    let op = model
        .group_move_op(&drag, "slot-c", MovePlacement::Tab, None)
        .unwrap();
    assert_eq!(arranged(op)["children"][1], slot(&["c", "b1", "b2"], "b2"));
}

#[test]
fn a_group_dropped_beside_another_splits_it_in_halves() {
    let model = three_groups();
    let drag = model.capture_group_drag("slot-a").unwrap();
    let op = model
        .group_move_op(&drag, "slot-c", MovePlacement::Below, None)
        .unwrap();
    assert_eq!(
        arranged(op),
        json!({"type":"split","axis":"horizontal","sizes":[0.3 / 0.8, 0.5 / 0.8],"children":[
            slot(&["b1", "b2"], "b2"),
            {"type":"split","axis":"vertical","sizes":[0.5, 0.5],"children":[
                slot(&["c"], "c"),
                slot(&["a"], "a"),
            ]},
        ]})
    );
}

#[test]
fn a_group_dropped_at_the_layout_edge_goes_beside_all_others() {
    let model = three_groups();
    let drag = model.capture_group_drag("slot-c").unwrap();
    let op = model
        .group_move_op(&drag, "root", MovePlacement::Above, None)
        .unwrap();
    let root = arranged(op);
    assert_eq!(root["axis"], "vertical");
    assert_eq!(root["children"][0], slot(&["c"], "c"));
    assert_eq!(root["children"][1]["axis"], "horizontal");
}

#[test]
fn the_arrangement_is_one_the_service_accepts_without_pane_or_session_changes() {
    let model = three_groups();
    let drag = model.capture_group_drag("slot-b").unwrap();
    let op = model
        .group_move_op(&drag, "slot-c", MovePlacement::Left, None)
        .unwrap()
        .unwrap();
    let before = model.snapshot().unwrap().clone();
    let (after, effects) = mux::reduce_screen(before.clone(), op).unwrap();
    assert!(effects.is_empty());
    assert_eq!(after.panes.len(), before.panes.len());
    assert_eq!(after.sessions.len(), before.sessions.len());
}

#[test]
fn a_drop_on_its_own_group_or_one_that_changes_nothing_sends_nothing() {
    let model = three_groups();
    let drag = model.capture_group_drag("slot-b").unwrap();
    for placement in [
        MovePlacement::Tab,
        MovePlacement::Left,
        MovePlacement::Below,
    ] {
        assert!(model
            .group_move_op(&drag, "slot-b", placement, None)
            .unwrap()
            .is_none());
    }
    // Two halves side by side: the right group dropped at the right edge
    // again changes nothing; at the left edge it trades places.
    let mut two = three_groups();
    let drag_a = two.capture_group_drag("slot-a").unwrap();
    let joined = two
        .group_move_op(&drag_a, "slot-b", MovePlacement::Tab, None)
        .unwrap()
        .unwrap();
    two.preview(&joined);
    let root = two.snapshot().unwrap().root.clone();
    let LayoutNode::Split { id, children, .. } = &root else {
        panic!("two groups side by side");
    };
    two.preview(&ScreenOp::ResizeSplit {
        node_id: id.clone(),
        sizes: vec![0.5, 0.5],
    });
    let right = children[1].id().to_string();
    let drag = two.capture_group_drag(&right).unwrap();
    assert!(two
        .group_move_op(&drag, id.as_str(), MovePlacement::Right, None)
        .unwrap()
        .is_none());
    assert!(two
        .group_move_op(&drag, id.as_str(), MovePlacement::Left, None)
        .unwrap()
        .is_some());
}

#[test]
fn a_stale_drag_or_a_split_moves_nothing() {
    let mut model = three_groups();
    let drag = model.capture_group_drag("slot-b").unwrap();
    // The layout changed in between: the drag no longer applies.
    let (mut changed, _) = mux::reduce_screen(
        model.snapshot().unwrap().clone(),
        ScreenOp::ResizeSplit {
            node_id: NodeId::from("root"),
            sizes: vec![0.4, 0.3, 0.3],
        },
    )
    .unwrap();
    changed.revision = 2;
    model
        .apply_snapshot(
            model.connection_id,
            &json!({"type":"mux.screen_snapshot","screen":changed}),
        )
        .unwrap();
    assert!(model
        .group_move_op(&drag, "slot-a", MovePlacement::Tab, None)
        .is_err());
    assert!(model.capture_group_drag("absent").is_none());
    assert!(
        model.capture_group_drag("root").is_none(),
        "only a group is dragged, not a split"
    );
}

#[test]
fn a_moved_group_shows_at_once_with_its_shown_tab_focused() {
    let mut model = three_groups();
    let drag = model.capture_group_drag("slot-b").unwrap();
    let op = model
        .group_move_op(&drag, "slot-a", MovePlacement::Tab, None)
        .unwrap()
        .unwrap();
    model.preview(&op);
    model.select_tab(drag.shown_pane());
    let screen = model.snapshot().unwrap();
    let joined = screen.root.find_slot(&PaneId::from("a")).unwrap();
    assert_eq!(model.selected_pane(joined), Some("b2"));
    assert_eq!(model.focused_pane(), Some("b2"));
    // A rejection brings the service's layout back.
    assert!(model.drop_preview());
    assert!(model
        .snapshot()
        .unwrap()
        .root
        .find_node(&NodeId::from("slot-b"))
        .is_some());
}
