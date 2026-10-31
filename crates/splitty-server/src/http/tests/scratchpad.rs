//! session-scratchpad, The Window Is The Only Channel, through the control
//! API: neither the HTTP listener nor the control socket hands a scratchpad's
//! text out or changes it.
use super::*;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::{Op, ScreenOp};

/// A screen `s1` whose session holds scratch text, built in the store directly
/// (a window saves it over its mux connection); no shell is started.
fn with_scratchpad(state: &AppState) -> String {
    state
        .mux_store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some(ScreenId::from("s1")),
            new_pane_id: Some(PaneId::from("p1")),
            shell_profile_id: None,
        })
        .expect("screen");
    let session = state
        .mux_store
        .session_id_for_pane(&PaneId::from("p1"))
        .expect("session");
    state
        .mux_store
        .dispatch_screen(
            &ScreenId::from("s1"),
            ScreenOp::SetScratchpad {
                session_id: session.clone(),
                text: "ask why the test is skipped".into(),
            },
        )
        .expect("save");
    session.as_str().to_owned()
}

/// control-api, Workspace and Screen Queries (Scratchpad omitted) and
/// Submitting Layout Operations (Scratchpad refused); workspace-model, Layout
/// Operations (Scratchpad only through the window); session-scratchpad (Agent
/// in a pane reads the workspace): on both channels.
#[tokio::test]
async fn the_control_api_neither_shows_nor_sets_a_scratchpad() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let session = with_scratchpad(&state);
    let channels = [
        ("HTTP", router(&state, hub(&host, dir.path()))),
        (
            "control socket",
            crate::http::build_control_router(state.clone()),
        ),
    ];
    for (channel, app) in channels {
        let (code, workspace) = exchange(&app, api("GET", "/api/mux/state", None)).await;
        assert_eq!(code, StatusCode::OK, "{channel}: {workspace}");
        let record = &workspace["screens"]["s1"]["sessions"][session.as_str()];
        assert!(record.is_object(), "{channel}: {workspace}");
        assert!(record.get("scratchpad").is_none(), "{channel}: {record}");

        let (code, screen) = exchange(&app, api("GET", "/api/mux/screen/s1", None)).await;
        assert_eq!(code, StatusCode::OK, "{channel}: {screen}");
        let record = &screen["sessions"][session.as_str()];
        assert!(record.is_object(), "{channel}: {screen}");
        assert!(record.get("scratchpad").is_none(), "{channel}: {record}");

        let op = json!({"screenId": "s1", "op": {"type": "set_scratchpad", "sessionId": session, "text": "from a pane"}});
        let (code, body) = exchange(&app, api("POST", "/api/mux/op", Some(&op))).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{channel}: {body}");
        assert_eq!(body["ok"], false, "{channel}: {body}");
    }
    let screen = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    assert_eq!(
        screen.sessions.values().next().expect("session").scratchpad,
        "ask why the test is skipped",
        "the window's text is unchanged"
    );
}
