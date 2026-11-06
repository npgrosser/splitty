//! Viewer panes through the control API, a window's connection and
//! `splitty open` (`viewer-panes`, `control-api`, `cli`): `open_viewer`
//! answers the pane it showed the document in, the control API leaves the
//! text of a `text` document out, and a window reads a document over its own
//! connection. Screens are built in the store directly; no shell is started.
use super::*;
use base64::Engine as _;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::{Op, ScreenOp, ViewerSource};

const PLAN: &str = "/home/me/app/PLAN.md";

/// Screen `s1` with the terminal pane `p1`.
fn with_screen(state: &AppState) {
    state
        .mux_store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some(ScreenId::from("s1")),
            new_pane_id: Some(PaneId::from("p1")),
            shell_profile_id: None,
        })
        .expect("screen");
}

/// Open a viewer on `s1` as a window does, with the op's wire form.
fn open_viewer(state: &AppState, op: serde_json::Value) {
    let op: ScreenOp = serde_json::from_value(op).expect("wire op");
    state
        .mux_store
        .dispatch_screen(&ScreenId::from("s1"), op)
        .expect("viewer");
}

fn viewer_panes(state: &AppState) -> usize {
    state
        .mux_store
        .get_workspace_state()
        .screens
        .values()
        .flat_map(|screen| screen.panes.values())
        .filter(|pane| pane.viewer.is_some())
        .count()
}

/// control-api, Submitting Layout Operations (Open a viewer, Open the same
/// file again), on the HTTP listener and the owner control socket, with the
/// viewer's revision each opening showed.
#[tokio::test]
async fn open_viewer_answers_its_pane_and_shows_the_same_file_there_again() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    for channel in ["HTTP", "control socket"] {
        let state = state(ADDR, dir.path());
        with_screen(&state);
        let app = if channel == "HTTP" {
            router(&state, hub(&host, dir.path()))
        } else {
            crate::http::build_control_router(state.clone())
        };
        let op = json!({"screenId": "s1", "op": {"type": "open_viewer", "paneId": "p1", "source": {"kind": "file", "path": PLAN}}});

        let (code, first) = exchange(&app, api("POST", "/api/mux/op", Some(&op))).await;
        assert_eq!(code, StatusCode::OK, "{channel}: {first}");
        assert_eq!(first["ok"], true, "{channel}: {first}");
        assert_eq!(first["screenId"], "s1");
        assert_eq!(first["created"], json!([]), "no session");
        assert_eq!(first["reused"], false);
        assert_eq!(first["viewerRevision"], 1, "{channel}: {first}");
        let pane = first["paneId"].as_str().expect("paneId").to_owned();
        let screen = state
            .mux_store
            .get_screen(&ScreenId::from("s1"))
            .expect("screen");
        assert_eq!(first["revision"], screen.revision);
        assert_eq!(screen.sessions.len(), 1, "{channel}");
        assert_eq!(
            screen.panes[&PaneId::from(pane.as_str())]
                .viewer
                .as_ref()
                .map(|viewer| viewer.source.clone()),
            Some(ViewerSource::File { path: PLAN.into() })
        );

        let (code, again) = exchange(&app, api("POST", "/api/mux/op", Some(&op))).await;
        assert_eq!(code, StatusCode::OK, "{channel}: {again}");
        assert_eq!(again["paneId"], pane.as_str());
        assert_eq!(again["reused"], true);
        assert_eq!(
            again["viewerRevision"], 2,
            "{channel}: the revision shown now"
        );
        assert_eq!(viewer_panes(&state), 1, "{channel}: no new pane");

        let relative = json!({"screenId": "s1", "op": {"type": "open_viewer", "paneId": "p1", "source": {"kind": "file", "path": "docs/plan.md"}}});
        let (code, body) = exchange(&app, api("POST", "/api/mux/op", Some(&relative))).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{channel}: {body}");
        assert_eq!(body["ok"], false);
        assert_eq!(viewer_panes(&state), 1);
    }
}

/// control-api, Workspace and Screen Queries (Viewer text omitted), and
/// access-control, Viewer Documents Stay With Their Connection (A process in
/// a pane reads the workspace): the record without the selected text.
#[tokio::test]
async fn the_control_api_leaves_out_the_text_a_viewer_shows() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    with_screen(&state);
    open_viewer(
        &state,
        json!({
            "type": "open_viewer", "paneId": "p1", "newPaneId": "v1", "cwd": "/home/me/app",
            "source": {"kind": "text", "title": "Selection from claude", "text": "the selected answer"},
        }),
    );
    let record = json!({
        "source": {"kind": "text", "title": "Selection from claude"},
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
        assert!(!workspace.to_string().contains("the selected answer"));

        let (code, screen) = exchange(&app, api("GET", "/api/mux/screen/s1", None)).await;
        assert_eq!(code, StatusCode::OK, "{channel}: {screen}");
        assert_eq!(screen["panes"]["v1"]["viewer"], record, "{channel}");
        assert!(!screen.to_string().contains("the selected answer"));
    }
    assert_eq!(
        state.mux_store.viewer_source(&PaneId::from("v1")),
        Some(ViewerSource::Text {
            title: "Selection from claude".into(),
            text: "the selected answer".into(),
            format: None,
        }),
        "the record keeps the text"
    );
}

async fn message_type(socket: &mut Socket, expected: &str) -> serde_json::Value {
    loop {
        if let Message::Text(text) = next(socket).await {
            let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
            if value["type"] == expected {
                return value;
            }
        }
    }
}

async fn read(socket: &mut Socket, read: serde_json::Value) -> serde_json::Value {
    socket
        .send(Message::Text(read.to_string().into()))
        .await
        .expect("send");
    loop {
        let answer = json_of(next(socket).await);
        if answer["type"]
            .as_str()
            .is_some_and(|kind| kind.starts_with("viewer."))
        {
            assert_eq!(answer["requestId"], read["requestId"], "{answer}");
            return answer;
        }
    }
}

/// viewer-panes, Reading A Document (A file in two chunks, Nothing changed)
/// and Viewer Capability, through the connection handler on both carriers:
/// the WebSocket of the window and a native connection.
#[tokio::test]
async fn a_window_reads_a_document_over_its_own_connection() {
    for native in [false, true] {
        let dir = tempfile::tempdir().expect("dir");
        let host = ScriptedHost::new(dir.path());
        let served = Served::start(hub(&host, dir.path()), dir.path()).await;
        with_screen(&served.state);
        let path = dir.path().join("PLAN.md");
        let bytes: Vec<u8> = (0..1_500_000u32)
            .map(|i| u8::try_from(i % 251).expect("byte"))
            .collect();
        std::fs::write(&path, &bytes).expect("file");
        let path = path.to_str().expect("UTF-8 path");
        open_viewer(
            &served.state,
            json!({"type": "open_viewer", "paneId": "p1", "newPaneId": "v1", "source": {"kind": "file", "path": path}}),
        );
        open_viewer(
            &served.state,
            json!({"type": "open_viewer", "paneId": "p1", "newPaneId": "v2", "source": {"kind": "text", "title": "Selection", "text": "an answer"}}),
        );
        let mut socket = served.open_local_for(native).await;
        let welcome = message_type(&mut socket, "mux.welcome").await;
        assert!(
            welcome["capabilities"]
                .as_array()
                .is_some_and(|all| all.contains(&json!("viewer-v1"))),
            "{welcome}"
        );

        let first = read(
            &mut socket,
            json!({"type": "viewer.read", "requestId": "r1", "paneId": "v1", "offset": 0}),
        )
        .await;
        assert_eq!(first["type"], "viewer.chunk", "native {native}: {first}");
        assert_eq!(first["size"], bytes.len());
        assert_eq!(first["eof"], false);
        let decode = |chunk: &serde_json::Value| {
            base64::engine::general_purpose::STANDARD
                .decode(chunk["data"].as_str().expect("data"))
                .expect("base64")
        };
        let mut received = decode(&first);
        let second = read(
            &mut socket,
            json!({
                "type": "viewer.read", "requestId": "r2", "paneId": "v1",
                "offset": received.len(), "version": first["version"],
            }),
        )
        .await;
        assert_eq!(second["eof"], true, "{second}");
        received.extend(decode(&second));
        assert_eq!(received, bytes);

        let unchanged = read(
            &mut socket,
            json!({"type": "viewer.read", "requestId": "r3", "paneId": "v1", "offset": 0, "ifVersion": first["version"]}),
        )
        .await;
        assert_eq!(unchanged["type"], "viewer.unchanged", "{unchanged}");
        assert!(unchanged.get("data").is_none());

        let text = read(
            &mut socket,
            json!({"type": "viewer.read", "requestId": "r4", "paneId": "v2", "offset": 0}),
        )
        .await;
        assert_eq!(decode(&text), b"an answer");
        let terminal = read(
            &mut socket,
            json!({"type": "viewer.read", "requestId": "r5", "paneId": "p1", "offset": 0}),
        )
        .await;
        assert_eq!(terminal["code"], "unknown_pane");
        assert_eq!(
            served
                .state
                .mux_store
                .get_screen(&ScreenId::from("s1"))
                .expect("screen")
                .revision,
            3,
            "reading changes nothing"
        );
    }
}

/// The owner control router on a Unix socket of `dir`, as a pane's process
/// reaches it.
fn control_socket(dir: &Path, app: axum::Router) -> crate::cli::ServerInfo {
    let socket = dir.join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("control socket");
    tokio::spawn(async move { axum::serve(listener, app).await });
    crate::cli::ServerInfo {
        transport: crate::cli::Transport::LocalSocket(socket.to_string_lossy().into_owned()),
        source: "test",
    }
}

/// cli, Open In A Viewer Pane (A plan next to the agent, Opened again after
/// an edit, As a tab): the command's request through the control router.
#[tokio::test]
async fn splitty_open_shows_a_file_beside_its_pane_and_again_in_the_same_pane() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    with_screen(&state);
    let server = control_socket(dir.path(), crate::http::build_control_router(state.clone()));
    let plan = dir.path().join("PLAN.md");
    std::fs::write(&plan, "# Plan").expect("file");
    let resolved = crate::cli::viewer::resolve(&plan).expect("resolved");

    let request = crate::cli::viewer::prepare(&plan, "s1", "p1", false).expect("request");
    crate::cli::viewer::submit(&server, &request)
        .await
        .expect("opened");
    let screen = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    let splitty_core::mux::LayoutNode::Split { axis, children, .. } = &screen.root else {
        panic!("a split: {:?}", screen.root);
    };
    assert_eq!(axis, &splitty_core::mux::Axis::Horizontal);
    assert_eq!(children[0].collect_pane_ids(), [PaneId::from("p1")]);
    let viewer = children[1].collect_pane_ids();
    assert_eq!(
        screen.panes[&viewer[0]]
            .viewer
            .as_ref()
            .map(|viewer| (viewer.source.clone(), viewer.revision)),
        Some((ViewerSource::File { path: resolved }, 1))
    );

    crate::cli::viewer::submit(&server, &request)
        .await
        .expect("again");
    let again = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    assert_eq!(again.panes.len(), 2, "no further pane");
    assert_eq!(
        again.panes[&viewer[0]].viewer.as_ref().map(|v| v.revision),
        Some(2)
    );

    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "notes").expect("file");
    let request = crate::cli::viewer::prepare(&notes, "s1", "p1", true).expect("request");
    crate::cli::viewer::submit(&server, &request)
        .await
        .expect("tab");
    let screen = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    let slot = screen.root.find_slot(&PaneId::from("p1")).expect("slot");
    assert!(
        matches!(slot, splitty_core::mux::LayoutNode::Slot { pane_ids, active_pane_id: Some(active), .. }
            if pane_ids.len() == 2 && pane_ids[1] == *active && screen.panes[active].viewer.is_some()),
        "{slot:?}"
    );
}

/// cli, Open In A Viewer Pane: a server that does not know `open_viewer`
/// answers as an older server does, and the command says to update it.
#[tokio::test]
async fn splitty_open_against_an_older_server_asks_for_an_update() {
    let dir = tempfile::tempdir().expect("dir");
    let older = axum::Router::new().route(
        "/api/mux/op",
        axum::routing::post(|| async {
            (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"ok": false, "error": "invalid op: unknown variant `open_viewer`, expected one of `split_pane`, `new_tab`"})),
            )
        }),
    );
    let server = control_socket(dir.path(), older);
    let plan = dir.path().join("PLAN.md");
    std::fs::write(&plan, "# Plan").expect("file");
    let request = crate::cli::viewer::prepare(&plan, "s1", "p1", false).expect("request");
    match crate::cli::viewer::submit(&server, &request).await {
        Err(crate::cli::CliError::Failed(message)) => {
            assert!(message.contains("Update"), "{message}");
        }
        other => panic!("not an update hint: {other:?}"),
    }
}
