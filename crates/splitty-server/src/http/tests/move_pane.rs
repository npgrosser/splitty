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
fn shell_state(addr: &str, dir: &Path) -> AppState {
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
    let app = router(&served.state, hub(&host, dir.path()));
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
    served.state.ws_state.dispatcher.backend().close_all();
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
    let control = super::super::build_control_router(state.clone());
    let (code, job) = exchange(&control, api("POST", "/api/jobs", Some(&owner))).await;
    assert_eq!(code, StatusCode::CREATED, "{job}");
    assert_eq!(job["screenId"], "s2");

    let agent = AgentId::new();
    state
        .ws_state
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
        .ws_state
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
        Arc::downgrade(&grant_state.ws_state.jobs),
        Actor::Agent(agent),
    );
    let (code, body) = exchange(
        &scoped,
        api("POST", "/api/jobs", Some(&launch(json!({"kind":"tab"})))),
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{body}");

    state.ws_state.dispatcher.backend().close_all();
    grant_state.ws_state.dispatcher.backend().close_all();
}
