//! workspace-model, Moving a Pane to Another Screen, through the control API:
//! the nudges for both screens, a moved shell's ops following its pane, and
//! job placements of the owner and of an agent.
use super::*;
use crate::jobs::{Actor, Anchor, Grant};
use splitty_core::ids::{AgentId, JobId};
use splitty_core::pty::testing::pty_available;

/// The next text message of `expected` type, answering pings meanwhile.
async fn message_type(socket: &mut Socket, expected: &str) -> serde_json::Value {
    loop {
        match next(socket).await {
            Message::Text(text) => {
                let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
                if value["type"] == expected {
                    return value;
                }
            }
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.expect("pong"),
            _ => {}
        }
    }
}

/// The state `serve` builds, with the detected shell profiles, so panes start shells.
fn shell_state(addr: &str, dir: &Path) -> TestState {
    let state = state(addr, dir);
    *state.shell_profiles.write().expect("profiles") =
        splitty_core::shell_profiles::state_from_settings(&Default::default());
    state
}

/// Two screens, `s1` with `p1` and `p3`, `s2` with `p2`; `p3` moves to `s2`.
async fn moved(app: &axum::Router, dir: &Path) -> serde_json::Value {
    for (screen, pane) in [("s1", "p1"), ("s2", "p2")] {
        let op =
            json!({"screenId":"","op":{"type":"create_screen","screenId":screen,"newPaneId":pane}});
        assert_eq!(
            status(app, api("POST", "/api/mux/op", Some(&op))).await,
            StatusCode::OK
        );
    }
    let split = json!({"screenId":"s1","op":{"type":"split_pane","paneId":"p1","axis":"horizontal","newPaneId":"p3","cwd":dir}});
    assert_eq!(
        status(app, api("POST", "/api/mux/op", Some(&split))).await,
        StatusCode::OK
    );
    let op = json!({"screenId":"s1","op":{"type":"move_pane_to_screen","paneId":"p3","targetScreenId":"s2"}});
    let (code, body) = exchange(app, api("POST", "/api/mux/op", Some(&op))).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    body
}

/// control-api, Operation Effects and Client Notification (Move from the CLI)
/// and Operation Targeting (Split in a moved shell).
#[tokio::test]
async fn a_cli_move_nudges_both_screens_and_a_moved_shell_still_splits() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served =
        Served::start_with(hub(&host, dir.path()), |addr| shell_state(addr, dir.path())).await;
    // The CLI's own gateway over the same service.
    let cli = TestState {
        listen_addr: ADDR.into(),
        allowed_origins: Arc::new(allowed_origins(ADDR)),
        ..served.state.clone()
    };
    let app = router(&cli, hub(&host, dir.path()));
    let mut ui = served.open_local().await;
    message_type(&mut ui, "mux.welcome").await;

    moved(&app, dir.path()).await;
    let mut nudged = Vec::new();
    while nudged.len() < 2 || !nudged.contains(&json!("s2")) {
        let nudge = message_type(&mut ui, "mux.nudge").await;
        assert_eq!(nudge["reason"], "cli_op");
        nudged.push(nudge["screenId"].clone());
    }
    assert!(
        nudged.contains(&json!("s1")) && nudged.contains(&json!("s2")),
        "{nudged:?}"
    );

    // The shell of p3 still has SPLITTY_SCREEN_ID=s1.
    let split = json!({"screenId":"s1","op":{"type":"split_pane","paneId":"p3","axis":"vertical","newPaneId":"p4","cwd":dir.path()}});
    let (code, body) = exchange(&app, api("POST", "/api/mux/op", Some(&split))).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["screenId"], "s2");
    let store = &served.state.mux_store;
    assert_eq!(
        store.screen_of_movable_pane(&splitty_core::ids::PaneId::from("p4")),
        Some(splitty_core::ids::ScreenId::from("s2"))
    );
    served.state.client_state.dispatcher.backend().close_all();
}

/// control-api, Operation Targeting: an owner's tab placement follows a
/// moved pane; an agent grant anchored on it fails (access-control).
#[tokio::test]
async fn an_owner_placement_follows_a_moved_pane_and_an_agent_anchor_does_not() {
    if !pty_available() {
        return;
    }
    // Short enough for the jobs' sockets on macOS.
    let dir = tempfile::tempdir_in("/tmp").expect("dir");
    let state = shell_state(ADDR, dir.path());
    let host = ScriptedHost::new(dir.path());
    let app = router(&state, hub(&host, dir.path()));
    moved(&app, dir.path()).await;

    let launch = |placement: serde_json::Value| json!({"requestId":JobId::new(),"argv":["/bin/sh","-c","read line"],"cwd":dir.path(),"placement":placement,"background":true});
    let owner = launch(json!({"kind":"tab","screenId":"s1","paneId":"p3"}));
    let control = super::super::build_control_router(state.service.clone());
    let (code, job) = exchange(&control, api("POST", "/api/jobs", Some(&owner))).await;
    assert_eq!(code, StatusCode::CREATED, "{job}");
    assert_eq!(job["screenId"], "s2");

    let agent = AgentId::new();
    state
        .client_state
        .jobs
        .register_grant(Grant::new(
            "agent".into(),
            dir.path().join("agent.sock").to_string_lossy().into_owned(),
            Some(Anchor {
                screen_id: "s1".into(),
                pane_id: "p3".into(),
            }),
            agent.clone(),
        ))
        .expect_err("an anchor that left its screen is refused");
    // A grant anchored before the move fails afterwards.
    let grant_dir = tempfile::tempdir_in("/tmp").expect("dir");
    let grant_state = shell_state(ADDR, grant_dir.path());
    let grant_app = router(&grant_state, hub(&host, grant_dir.path()));
    for (screen, pane) in [("s1", "p1"), ("s2", "p2")] {
        let op =
            json!({"screenId":"","op":{"type":"create_screen","screenId":screen,"newPaneId":pane}});
        assert_eq!(
            status(&grant_app, api("POST", "/api/mux/op", Some(&op))).await,
            StatusCode::OK
        );
    }
    grant_state
        .client_state
        .jobs
        .register_grant(Grant::new(
            "agent".into(),
            grant_dir
                .path()
                .join("agent.sock")
                .to_string_lossy()
                .into_owned(),
            Some(Anchor {
                screen_id: "s1".into(),
                pane_id: "p1".into(),
            }),
            agent.clone(),
        ))
        .expect("grant");
    let op = json!({"screenId":"s1","op":{"type":"move_pane_to_screen","paneId":"p1","targetScreenId":"s2"}});
    assert_eq!(
        status(&grant_app, api("POST", "/api/mux/op", Some(&op))).await,
        StatusCode::OK
    );
    let scoped: axum::Router = super::super::jobs::routes(
        Arc::downgrade(&grant_state.client_state.jobs),
        Actor::Agent(agent),
    );
    let (code, body) = exchange(
        &scoped,
        api("POST", "/api/jobs", Some(&launch(json!({"kind":"tab"})))),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{body}");

    state.client_state.dispatcher.backend().close_all();
    grant_state.client_state.dispatcher.backend().close_all();
}

/// The owner control API shares exact tab validation and reduction with session clients.
#[tokio::test]
async fn exact_tab_placement_via_control_preserves_records_and_rejects_stale_source() {
    use splitty_core::{
        ids::{PaneId, ScreenId},
        mux::{Op, ScreenOp},
    };
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let screen_id = ScreenId::from("screen");
    state
        .mux_store
        .dispatch(Op::CreateScreen {
            screen_id: Some(screen_id.clone()),
            name: None,
            new_pane_id: Some("p1".into()),
            shell_profile_id: None,
        })
        .expect("seed screen without running effects");
    let split: ScreenOp = serde_json::from_value(
        json!({"type":"split_pane", "paneId":"p1", "axis":"horizontal", "newPaneId":"p2"}),
    )
    .expect("split");
    state
        .mux_store
        .dispatch_screen(&screen_id, split)
        .expect("seed split");
    let before = state.mux_store.get_screen(&screen_id).expect("screen");
    let source = before
        .root
        .find_slot(&PaneId::from("p2"))
        .expect("source")
        .id();
    let target = before
        .root
        .find_slot(&PaneId::from("p1"))
        .expect("target")
        .id();
    let body = json!({"screenId":"screen", "op":{
        "type":"move_tab", "paneId":"p2", "sourceSlotId":source, "targetSlotId":target, "beforePaneId":"p1"
    }});
    let control = super::super::build_control_router(state.service.clone());
    let (code, reply) = exchange(&control, api("POST", "/api/mux/op", Some(&body))).await;
    assert_eq!(code, StatusCode::OK, "{reply}");
    let after = state.mux_store.get_screen(&screen_id).expect("screen");
    assert_eq!(after.root.id(), target);
    assert_eq!(
        after.root.collect_pane_ids(),
        ["p2", "p1"].map(PaneId::from)
    );
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(
        serde_json::to_value(&after.panes).expect("panes"),
        serde_json::to_value(&before.panes).expect("panes")
    );
    assert_eq!(
        serde_json::to_value(&after.sessions).expect("sessions"),
        serde_json::to_value(&before.sessions).expect("sessions")
    );
    let (code, _) = exchange(&control, api("POST", "/api/mux/op", Some(&body))).await;
    assert!(!code.is_success());
    assert_eq!(
        serde_json::to_value(state.mux_store.get_screen(&screen_id)).expect("screen"),
        serde_json::to_value(Some(after)).expect("screen")
    );
}

/// control-api, Workspace Operations: a tab group moves whole through the
/// owner control API; a malformed request is a 400, a rejected one leaves
/// the workspace as it was.
#[tokio::test]
async fn a_tab_group_moves_through_the_control_api() {
    use splitty_core::{
        ids::{PaneId, ScreenId},
        mux::{LayoutNode, Op, ScreenOp},
    };
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    for (screen, pane) in [("s1", "p1"), ("s2", "p2")] {
        state
            .mux_store
            .dispatch(Op::CreateScreen {
                screen_id: Some(ScreenId::from(screen)),
                name: None,
                new_pane_id: Some(PaneId::from(pane)),
                shell_profile_id: None,
            })
            .expect("seed screen without running effects");
    }
    for op in [
        json!({"type":"split_pane", "paneId":"p1", "axis":"horizontal", "newPaneId":"p3"}),
        json!({"type":"new_tab", "paneId":"p3", "newPaneId":"p4"}),
    ] {
        let op: ScreenOp = serde_json::from_value(op).expect("op");
        state
            .mux_store
            .dispatch_screen(&ScreenId::from("s1"), op)
            .expect("seed");
    }
    let slot = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen")
        .root
        .find_slot(&PaneId::from("p3"))
        .expect("group")
        .id()
        .clone();
    let control = super::super::build_control_router(state.service.clone());
    let post = |op: serde_json::Value| {
        let body = json!({"screenId":"s1","op":op});
        exchange(&control, api("POST", "/api/mux/op", Some(&body)))
    };

    let (code, body) = post(json!({"type":"move_tab_group_to_screen","slotId":slot})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    let before = serde_json::to_value(state.mux_store.get_workspace_state()).expect("state");
    let (code, body) =
        post(json!({"type":"move_tab_group_to_screen","slotId":"absent","targetScreenId":"s2"}))
            .await;
    assert!(!code.is_success(), "{body}");
    assert_eq!(
        serde_json::to_value(state.mux_store.get_workspace_state()).expect("state"),
        before
    );

    let (code, body) = post(json!({"type":"move_tab_group_to_screen","slotId":slot,"activePaneId":"p3","targetScreenId":"s2"})).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let target = state
        .mux_store
        .get_screen(&ScreenId::from("s2"))
        .expect("target");
    assert!(matches!(
        target.root.find_slot(&PaneId::from("p3")),
        Some(LayoutNode::Slot { pane_ids, active_pane_id, .. })
            if pane_ids == &["p2", "p3", "p4"].map(PaneId::from)
                && active_pane_id.as_ref() == Some(&PaneId::from("p3"))
    ));
    let source = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("source");
    assert_eq!(source.root.collect_pane_ids(), [PaneId::from("p1")]);
}
