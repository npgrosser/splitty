//! Unit tests of the command gate: a command whose capability the server did
//! not announce yields the window's message instead, for the local service
//! and for a host's daemon alike.
use super::*;
use crate::workspace::{MovePlacement, WindowWorkspace};
use uuid::Uuid;

/// A group move on a local Screen with two groups.
fn group_move() -> Command {
    let owner = Uuid::new_v4();
    let mut window = WindowWorkspace::new(owner);
    window
        .ui_test_screen_order_fixture(&[("work", "Work")], None)
        .unwrap();
    let token = window.lifecycle_token();
    let pane = |id: &str| serde_json::json!({"id": id, "sessionId": format!("s-{id}")});
    let session = |id: &str| serde_json::json!({"id": format!("s-{id}"), "status": "running"});
    let mut source = window.source_mut(token.source_id()).unwrap();
    source
        .apply_snapshot(
            owner,
            &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                "id":"work","revision":2,
                "root":{"type":"split","id":"root","axis":"horizontal","sizes":[0.5,0.5],"children":[
                    {"type":"slot","id":"left","paneIds":["p0"]},
                    {"type":"slot","id":"right","paneIds":["p1"]}]},
                "panes":{"p0":pane("p0"),"p1":pane("p1")},
                "sessions":{"s-p0":session("p0"),"s-p1":session("p1")}
            }}),
        )
        .unwrap();
    let drag = source.capture_group_drag("left").unwrap();
    Command::new(
        token,
        CommandKind::MoveGroup {
            drag,
            target: "right".into(),
            placement: MovePlacement::Tab,
            before: None,
        },
    )
}

#[test]
fn a_command_without_its_capability_is_refused_with_one_message() {
    let command = group_move();
    let older = vec![capability::TAB_PLACEMENT.to_owned()];
    for (remote, says) in [
        (false, "local Splitty service"),
        (true, "host's Splitty daemon"),
    ] {
        let updates = refusal(&command, &older, remote).expect("refused");
        assert!(
            matches!(updates.as_slice(), [Update::ServerOutdated(message)]
                if message.contains(says) && message.contains("update")),
            "one message for {says}"
        );
    }
    // Before any welcome nothing is known to be announced either.
    assert!(refusal(&command, &[], false).is_some());
}

#[test]
fn a_command_with_its_capability_or_without_a_need_passes() {
    let command = group_move();
    let current = vec![capability::LAYOUT_ARRANGE.to_owned()];
    assert!(refusal(&command, &current, true).is_none());
    let plain = Command::new(command.source.unwrap(), CommandKind::RefreshBackendVersion);
    assert!(required_capability(&plain.kind).is_none());
    assert!(refusal(&plain, &[], false).is_none());
}
