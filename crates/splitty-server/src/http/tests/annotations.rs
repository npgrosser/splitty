//! viewer-annotations, The Window Is The Only Channel For Annotations,
//! through the control API: neither the HTTP listener, the control socket nor
//! the native endpoint returns a viewer pane's annotations or changes them.
use super::conformance::native;
use super::*;
use crate::service::catalog::Operation;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::{Op, ScreenOp};

const PLAN: &str = "/home/me/app/PLAN.md";
const ANNOTATION: &str = "ann-0123456789abcdef";

/// Screen `s1` with the terminal `p1` and the viewer `v1`, which holds one
/// annotation, built in the store directly (a window saves annotations over
/// its mux connection); no shell is started.
fn with_annotation(state: &AppState) {
    state
        .mux_store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some(ScreenId::from("s1")),
            new_pane_id: Some(PaneId::from("p1")),
            shell_profile_id: None,
        })
        .expect("screen");
    for op in [
        json!({
            "type": "open_viewer", "paneId": "p1", "newPaneId": "v1",
            "source": {"kind": "file", "path": PLAN},
        }),
        json!({
            "type": "put_annotations", "paneId": "v1", "annotations": [{
                "id": ANNOTATION, "kind": "question", "body": "Why not five times?",
                "target": {
                    "view": "source", "quote": "retry three times", "prefix": "On failure, ",
                    "suffix": ".", "startLine": 30, "endLine": 30,
                },
            }],
        }),
    ] {
        let op: ScreenOp = serde_json::from_value(op).expect("wire op");
        state
            .mux_store
            .dispatch_screen(&ScreenId::from("s1"), op)
            .expect("op");
    }
}

/// control-api, Workspace and Screen Queries (Annotations omitted) and
/// Submitting Layout Operations (Annotations refused); workspace-model,
/// Layout Operations (Annotations only through the window);
/// viewer-annotations, The Window Is The Only Channel For Annotations (An
/// agent reads the workspace, An agent tries to annotate): on the HTTP
/// listener, the control socket and the native endpoint.
#[tokio::test]
async fn the_control_api_neither_shows_nor_changes_annotations() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    with_annotation(&state);
    let before = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    let record = json!({
        "source": {"kind": "file", "path": PLAN},
        "originPaneId": "p1",
        "revision": 1,
    });
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
        assert_eq!(
            workspace["screens"]["s1"]["panes"]["v1"]["viewer"], record,
            "{channel}"
        );
        assert!(!workspace.to_string().contains(ANNOTATION), "{channel}");

        let (code, screen) = exchange(&app, api("GET", "/api/mux/screen/s1", None)).await;
        assert_eq!(code, StatusCode::OK, "{channel}: {screen}");
        assert_eq!(screen["panes"]["v1"]["viewer"], record, "{channel}");
        assert!(
            !screen.to_string().contains("Why not five times?"),
            "{channel}"
        );

        for op in [
            json!({
                "type": "put_annotations", "paneId": "v1",
                "annotations": [{"id": "ann-00000000000000ff", "kind": "comment", "body": "from a pane"}],
            }),
            json!({"type": "remove_annotations", "paneId": "v1", "ids": [ANNOTATION]}),
        ] {
            let kind = op["type"].clone();
            let body = json!({"screenId": "s1", "op": op});
            let (code, answer) = exchange(&app, api("POST", "/api/mux/op", Some(&body))).await;
            assert_eq!(code, StatusCode::BAD_REQUEST, "{channel} {kind}: {answer}");
            assert_eq!(answer["ok"], false, "{channel}: {answer}");
            assert!(
                answer["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("only accepted from a window")),
                "{channel}: {answer}"
            );
        }
    }
    let (code, workspace) = native(&state, Operation::MuxState, json!({})).await;
    assert_eq!(code, StatusCode::OK, "native: {workspace}");
    assert_eq!(workspace["screens"]["s1"]["panes"]["v1"]["viewer"], record);
    let (code, screen) = native(&state, Operation::MuxScreen, json!({"id": "s1"})).await;
    assert_eq!(code, StatusCode::OK, "native: {screen}");
    assert_eq!(screen["panes"]["v1"]["viewer"], record);
    let put = json!({"screenId": "s1", "op": {
        "type": "put_annotations", "paneId": "v1",
        "annotations": [{"id": "ann-00000000000000ff", "kind": "comment", "body": "from a pane"}],
    }});
    let (code, answer) = native(&state, Operation::MuxOp, json!({"body": put})).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "native: {answer}");

    let after = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    assert_eq!(after.revision, before.revision);
    assert_eq!(
        after.panes[&PaneId::from("v1")].viewer,
        before.panes[&PaneId::from("v1")].viewer,
        "the window's annotation is unchanged"
    );
    let annotations = after.panes[&PaneId::from("v1")]
        .viewer
        .as_ref()
        .map(|viewer| viewer.annotations.len());
    assert_eq!(annotations, Some(1));
}
