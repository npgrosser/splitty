//! Waits for a review and agents' replies through the served router,
//! answered by real WebSocket windows: control-api, Review Requests;
//! viewer-annotations, Waiting For A Review; mux-protocol, Review Answers and
//! Change Propagation to Other Clients. The waiting request is held on the
//! HTTP listener and on the owner control socket; no shell is started.
use super::*;
use crate::client_session::reviews::MAX_WAITS;
use serde_json::Value;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::{Op, ScreenOp};
use tokio::io::AsyncWriteExt;

const PLAN: &str = "/home/me/app/PLAN.md";
const WAIT_PATH: &str = "/api/viewer/review/wait";

/// Screen `s1` with the terminal `p1` and the viewer `v1` of PLAN.md, whose
/// annotation #1 the user handed over.
fn workspace(state: &TestState) {
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
        json!({"type": "put_annotations", "paneId": "v1", "annotations": [
            {"id": "ann-0000000000000001", "kind": "comment", "body": "Why not five times?"},
        ]}),
        json!({"type": "mark_sent", "paneId": "v1", "entries": [
            {"id": "ann-0000000000000001", "kind": "comment", "body": "Why not five times?"},
        ]}),
    ] {
        let op: ScreenOp = serde_json::from_value(op).expect("wire op");
        state
            .mux_store
            .dispatch_screen(&ScreenId::from("s1"), op)
            .expect("op");
    }
}

/// The router served with [`workspace`].
async fn serve(host: &ScriptedHost, dir: &Path) -> Served {
    Served::start_with(hub(host, dir), |addr| {
        let state = state(addr, dir);
        workspace(&state);
        state
    })
    .await
}

/// A window of the served router, past its welcome.
async fn window(served: &Served) -> Socket {
    let mut socket = served.open_local().await;
    message_type(&mut socket, "mux.welcome").await;
    socket
}

async fn send(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("send");
}

/// The next message of each of the `expected` types, in the order of
/// `expected`, whatever order they arrive in; other messages are passed over.
async fn messages(socket: &mut Socket, expected: &[&str]) -> Vec<Value> {
    let mut found: Vec<Option<Value>> = vec![None; expected.len()];
    tokio::time::timeout(WAIT, async {
        while found.iter().any(Option::is_none) {
            match next(socket).await {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).expect("JSON");
                    let slot = expected
                        .iter()
                        .zip(found.iter_mut())
                        .find(|(kind, slot)| slot.is_none() && value["type"] == **kind);
                    if let Some((_, slot)) = slot {
                        *slot = Some(value);
                    }
                }
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.expect("pong"),
                _ => {}
            }
        }
    })
    .await
    .expect("the messages in time");
    found.into_iter().flatten().collect()
}

async fn message_type(socket: &mut Socket, expected: &str) -> Value {
    messages(socket, &[expected]).await.remove(0)
}

/// `message` is the nudge a change of screen `s1` gives every window.
fn assert_nudge(message: &Value) {
    assert_eq!(message["type"], "mux.nudge", "{message}");
    assert_eq!(message["reason"], "mux_op", "{message}");
    assert_eq!(message["screenId"], "s1", "{message}");
}

async fn nudged(socket: &mut Socket) {
    assert_nudge(&message_type(socket, "mux.nudge").await);
}

/// `v1`'s viewer record as a window sees it.
async fn record(socket: &mut Socket) -> Value {
    send(socket, json!({"type": "mux.resync", "screenId": "s1"})).await;
    message_type(socket, "mux.screen_snapshot").await["screen"]["panes"]["v1"]["viewer"].clone()
}

/// A window's answer to the wait `wait` of `v1`.
fn answer(wait: &Value, outcome: Value) -> Value {
    let mut answer = json!({
        "type": "viewer.review_answer", "screenId": "s1", "paneId": "v1", "waitId": wait,
    });
    if let (Some(answer), Some(outcome)) = (answer.as_object_mut(), outcome.as_object()) {
        answer.extend(outcome.clone());
    }
    answer
}

/// `p1` waits for the review of `v1`.
fn request() -> Value {
    json!({"screenId": "s1", "paneId": "p1", "viewerPaneId": "v1"})
}

/// The wait, held over the HTTP listener as the CLI holds it.
fn wait_over_http(served: &Served) -> tokio::task::JoinHandle<(u16, Value)> {
    let url = format!("http://127.0.0.1:{}{WAIT_PATH}", served.port);
    tokio::spawn(async move {
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client")
            .post(url)
            .bearer_auth("process-token")
            .json(&request())
            .send()
            .await
            .expect("HTTP");
        let status = response.status().as_u16();
        (status, response.json().await.expect("JSON"))
    })
}

/// The wait, held over the owner control router.
fn wait_over_control(control: &axum::Router) -> tokio::task::JoinHandle<(StatusCode, Value)> {
    let control = control.clone();
    tokio::spawn(async move { exchange(&control, api("POST", WAIT_PATH, Some(&request()))).await })
}

/// The wait's request as an HTTP/1.1 client writes it to `host`.
fn raw_wait(host: &str) -> Vec<u8> {
    let body = request().to_string();
    format!(
        "POST {WAIT_PATH} HTTP/1.1\r\nhost: {host}\r\nauthorization: Bearer process-token\r\n\
         content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// control-api, Review Requests (Approved, Changes requested, Second wait);
/// viewer-annotations, Waiting For A Review (Approved, Two windows answer);
/// mux-protocol, Review Answers (The first answer, An approval with notes, A
/// stale answer) and Change Propagation to Other Clients: a wait held on the
/// HTTP listener names itself, the waiting pane and its title in the record
/// and nudges every window; a second wait for the pane is refused while the
/// first keeps waiting; the first answer ends the wait with its outcome and
/// nudges the windows again; a later answer is not accepted. The same with a
/// request for changes, held on the owner control socket, and with an
/// approval that carries notes.
#[tokio::test]
async fn a_wait_ends_with_the_first_answer() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = serve(&host, dir.path()).await;
    let control = crate::http::build_control_router(served.state.service.clone());
    let v1 = PaneId::from("v1");
    let mut a = window(&served).await;
    let mut b = window(&served).await;

    let waiting = wait_over_http(&served);
    nudged(&mut a).await;
    nudged(&mut b).await;
    let held = record(&mut a).await["reviewWait"].clone();
    assert_eq!(held["paneId"], "p1", "{held}");
    // No process runs in `p1` here: the pane's title is the default.
    assert_eq!(held["title"], "agent", "{held}");
    let wait = held["id"].clone();
    assert!(
        wait.as_str().is_some_and(|id| id.starts_with("wait-")),
        "{held}"
    );

    let (code, refused) = exchange(&control, api("POST", WAIT_PATH, Some(&request()))).await;
    assert_eq!(code, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "busy", "{refused}");
    assert!(!waiting.is_finished(), "the first keeps waiting");
    assert_eq!(
        served.state.mux_store.review_wait(&v1).map(|held| held.id),
        wait.as_str().map(str::to_owned)
    );

    send(&mut b, answer(&wait, json!({"outcome": "approved"}))).await;
    let got = messages(&mut b, &["viewer.review_answered", "mux.nudge"]).await;
    assert_eq!(
        got[0],
        json!({"type": "viewer.review_answered", "paneId": "v1", "waitId": wait, "accepted": true})
    );
    assert_nudge(&got[1]);
    let (status, outcome) = tokio::time::timeout(WAIT, waiting)
        .await
        .expect("an answer in time")
        .expect("request");
    assert_eq!((status, outcome), (200, json!({"outcome": "approved"})));
    nudged(&mut a).await;
    send(
        &mut a,
        answer(
            &wait,
            json!({"outcome": "changes_requested", "review": "Too late."}),
        ),
    )
    .await;
    let late = message_type(&mut a, "viewer.review_answered").await;
    assert_eq!(late["accepted"], false, "{late}");
    let viewer = record(&mut a).await;
    assert!(viewer.get("reviewWait").is_none(), "{viewer}");
    assert_eq!(served.state.client_state.reviews.held(), 0);

    let waiting = wait_over_control(&control);
    nudged(&mut a).await;
    nudged(&mut b).await;
    let next_wait = record(&mut b).await["reviewWait"]["id"].clone();
    assert_ne!(next_wait, wait, "a wait has an ID of its own");
    let review = "# Review of PLAN.md\n\n1. Line 30: Why not five times?\n";
    send(
        &mut a,
        answer(
            &next_wait,
            json!({"outcome": "changes_requested", "review": review}),
        ),
    )
    .await;
    let got = messages(&mut a, &["viewer.review_answered", "mux.nudge"]).await;
    assert_eq!(got[0]["accepted"], true, "{}", got[0]);
    assert_nudge(&got[1]);
    let (code, outcome) = tokio::time::timeout(WAIT, waiting)
        .await
        .expect("an answer in time")
        .expect("request");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        outcome,
        json!({"outcome": "changes_requested", "review": review})
    );
    nudged(&mut b).await;
    send(&mut b, answer(&next_wait, json!({"outcome": "approved"}))).await;
    let late = message_type(&mut b, "viewer.review_answered").await;
    assert_eq!(late["accepted"], false, "{late}");

    let waiting = wait_over_control(&control);
    nudged(&mut a).await;
    let notes_wait = record(&mut a).await["reviewWait"]["id"].clone();
    let notes = "# Review of PLAN.md\n\nVerdict: approved\n";
    send(
        &mut a,
        answer(&notes_wait, json!({"outcome": "approved", "review": notes})),
    )
    .await;
    let got = message_type(&mut a, "viewer.review_answered").await;
    assert_eq!(got["accepted"], true, "{got}");
    let (code, outcome) = tokio::time::timeout(WAIT, waiting)
        .await
        .expect("an answer in time")
        .expect("request");
    assert_eq!(code, StatusCode::OK);
    assert_eq!(outcome, json!({"outcome": "approved", "review": notes}));
}

/// control-api, Review Requests (Caller gone) and viewer-annotations, Waiting
/// For A Review (The agent stops waiting): when the connection of a waiting
/// request ends, on the owner control socket or on the HTTP listener, the
/// wait leaves the registry and the record, the windows are nudged and the
/// annotations stay as they are. A wait whose viewer pane closes answers 410
/// `closed`; 16 waits are the most a server holds; a request that names no
/// viewer pane of the caller's screen is refused.
#[tokio::test]
async fn a_wait_ends_with_its_caller_or_its_pane() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = serve(&host, dir.path()).await;
    let control = crate::http::build_control_router(served.state.service.clone());
    let v1 = PaneId::from("v1");
    let reviews = served.state.client_state.reviews.clone();
    let mut window = window(&served).await;
    let annotations = record(&mut window).await["annotations"].clone();

    let control_socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&control_socket).expect("control socket");
    let serving = tokio::spawn({
        let control = control.clone();
        async move { axum::serve(listener, control).await }
    });
    let mut caller = tokio::net::UnixStream::connect(&control_socket)
        .await
        .expect("connect");
    caller
        .write_all(&raw_wait("localhost"))
        .await
        .expect("request");
    nudged(&mut window).await;
    assert!(served.state.mux_store.review_wait(&v1).is_some());
    drop(caller);
    nudged(&mut window).await;
    assert!(
        served.state.mux_store.review_wait(&v1).is_none(),
        "the record no longer names the wait"
    );
    assert_eq!(reviews.held(), 0);
    serving.abort();

    let mut caller = tokio::net::TcpStream::connect(("127.0.0.1", served.port))
        .await
        .expect("connect");
    caller
        .write_all(&raw_wait(&format!("127.0.0.1:{}", served.port)))
        .await
        .expect("request");
    nudged(&mut window).await;
    assert!(served.state.mux_store.review_wait(&v1).is_some());
    drop(caller);
    nudged(&mut window).await;
    assert!(served.state.mux_store.review_wait(&v1).is_none());
    assert_eq!(reviews.held(), 0);
    let viewer = record(&mut window).await;
    assert!(viewer.get("reviewWait").is_none(), "{viewer}");
    assert_eq!(
        viewer["annotations"], annotations,
        "the annotations stay as they are"
    );

    let held = (0..MAX_WAITS)
        .map(|n| reviews.register(PaneId::from(format!("viewer-{n:016x}"))))
        .collect::<Result<Vec<_>, _>>()
        .expect("held");
    let (code, refused) = exchange(&control, api("POST", WAIT_PATH, Some(&request()))).await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["code"], "busy", "{refused}");
    drop(held);

    for (body, status, name) in [
        (
            json!({"screenId": "s1", "paneId": "p1", "viewerPaneId": "p1"}),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            json!({"screenId": "s1", "paneId": "p1", "viewerPaneId": "viewer-0123456789abcdef"}),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            json!({"screenId": "s1", "paneId": "p9", "viewerPaneId": "v1"}),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            json!({"screenId": "s1", "paneId": "p1"}),
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
    ] {
        let (code, refused) = exchange(&control, api("POST", WAIT_PATH, Some(&body))).await;
        assert_eq!(code, status, "{body}: {refused}");
        assert_eq!(refused["code"], name, "{body}: {refused}");
    }
    assert_eq!(reviews.held(), 0);
    assert!(served.state.mux_store.review_wait(&v1).is_none());

    let waiting = wait_over_control(&control);
    nudged(&mut window).await;
    send(
        &mut window,
        json!({"type": "mux.op", "screenId": "s1", "op": {"type": "close_pane", "paneId": "v1"}}),
    )
    .await;
    let (code, closed) = tokio::time::timeout(WAIT, waiting)
        .await
        .expect("an answer in time")
        .expect("request");
    assert_eq!(code, StatusCode::GONE, "{closed}");
    assert_eq!(closed["code"], "closed", "{closed}");
    assert_eq!(reviews.held(), 0);
}

/// control-api, Review Requests (Replaced before the wait): two reviews of a
/// plan started at once from one pane; the second text takes the pane before
/// the first command waits, so the first wait, bound to the revision its
/// command showed, is refused with 409 `changed` and leaves no wait behind,
/// while the wait on the revision the pane shows is held.
#[tokio::test]
async fn a_wait_on_a_replaced_text_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = serve(&host, dir.path()).await;
    let control = crate::http::build_control_router(served.state.service.clone());
    let reviews = served.state.client_state.reviews.clone();
    let show = |text: &str| {
        json!({"screenId": "s1", "op": {
            "type": "open_viewer", "paneId": "p1", "replace": true,
            "source": {"kind": "text", "title": "Plan", "text": text, "format": "markdown"},
        }})
    };
    let (code, first) = exchange(
        &control,
        api("POST", "/api/mux/op", Some(&show("1. Cache\n"))),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{first}");
    let (code, second) = exchange(
        &control,
        api("POST", "/api/mux/op", Some(&show("1. Cache with a TTL\n"))),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{second}");
    assert_eq!(second["paneId"], first["paneId"], "{second}");
    assert_eq!(second["reused"], true, "{second}");
    assert!(
        first["viewerRevision"].as_u64() < second["viewerRevision"].as_u64(),
        "{first} {second}"
    );
    let viewer = PaneId::from(first["paneId"].as_str().expect("paneId"));
    let wait = |shown: &Value| {
        json!({"screenId": "s1", "paneId": "p1", "viewerPaneId": viewer,
               "viewerRevision": shown["viewerRevision"]})
    };

    let (code, refused) = exchange(&control, api("POST", WAIT_PATH, Some(&wait(&first)))).await;
    assert_eq!(code, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "changed", "{refused}");
    assert!(served.state.mux_store.review_wait(&viewer).is_none());
    assert_eq!(reviews.held(), 0);

    let mut window = window(&served).await;
    let waiting = tokio::spawn({
        let (control, body) = (control.clone(), wait(&second));
        async move { exchange(&control, api("POST", WAIT_PATH, Some(&body))).await }
    });
    nudged(&mut window).await;
    assert!(
        served.state.mux_store.review_wait(&viewer).is_some(),
        "the wait on the revision shown is held"
    );
    waiting.abort();
}

/// mux-protocol, Change Propagation to Other Clients (Reply of an agent): an
/// agent's reply through the control API nudges every window, and the
/// windows find it in the thread with the caller's pane and title.
#[tokio::test]
async fn an_agents_reply_nudges_every_window() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = serve(&host, dir.path()).await;
    let control = crate::http::build_control_router(served.state.service.clone());
    let mut a = window(&served).await;
    let mut b = window(&served).await;
    let body = json!({
        "screenId": "s1", "paneId": "p1", "document": PLAN, "number": 1,
        "body": "Three keeps the worst case under a minute.",
    });
    let (code, replied) = exchange(
        &control,
        api("POST", "/api/viewer/comments/reply", Some(&body)),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{replied}");
    nudged(&mut a).await;
    nudged(&mut b).await;
    let thread = record(&mut b).await["annotations"][0]["replies"].clone();
    assert_eq!(thread[0]["author"], "agent", "{thread}");
    assert_eq!(thread[0]["body"], body["body"], "{thread}");
    assert_eq!(
        thread[0]["from"],
        json!({"paneId": "p1", "title": "agent"}),
        "{thread}"
    );
}

/// mux-protocol, Review Texts And Anchors (A review on request); and
/// viewer-annotations, The Review (Added just before the review): a window
/// that saves a comment and asks for the review right after it, over the
/// WebSocket and over a native carrier, gets a review that holds the comment
/// with its number.
#[tokio::test]
async fn a_review_holds_the_comment_a_window_saved_just_before() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = serve(&host, dir.path()).await;
    for (native, pane) in [(false, "v2"), (true, "v3")] {
        let mut socket = served.open_local_for(native).await;
        let welcome = message_type(&mut socket, "mux.welcome").await;
        assert!(
            welcome["capabilities"]
                .as_array()
                .is_some_and(|all| all.contains(&json!("viewer-reviews-v1"))),
            "{welcome}"
        );
        for op in [
            json!({"type": "open_viewer", "paneId": "p1", "newPaneId": pane, "placement": "tab",
                   "source": {"kind": "text", "title": "Plan", "text": "one\nretry three times\n"}}),
            json!({"type": "put_annotations", "paneId": pane, "annotations": [
                {"id": "ann-00000000000000aa", "kind": "comment", "body": "Split it."},
                {"id": "ann-00000000000000bb", "kind": "comment", "body": "Why three?",
                 "target": {"view": "source", "quote": "retry three times", "prefix": "one\n", "suffix": "\n",
                            "startLine": 1, "endLine": 1}},
            ]}),
        ] {
            send(&mut socket, json!({"type": "mux.op", "screenId": "s1", "op": op})).await;
        }
        send(&mut socket, json!({"type": "viewer.anchor", "requestId": "a1", "paneId": pane})).await;
        send(&mut socket, json!({"type": "viewer.review", "requestId": "r1", "paneId": pane})).await;
        let [anchored, reviewed] = <[Value; 2]>::try_from(
            messages(&mut socket, &["viewer.anchored", "viewer.reviewed"]).await,
        )
        .expect("both answers");
        assert_eq!(anchored["anchors"][1]["startLine"], 2, "{anchored}");
        assert_eq!(
            reviewed["text"],
            format!("# Review of Plan\n\n## General comments\n\n- #1 Split it.\n\n## #2 Line 2, comment\n> retry three times\n\nWhy three?\n\nReply in a thread: splitty comments reply {pane} <number> '<text>'\n"),
            "native: {native}"
        );
        assert_eq!(reviewed["count"], 2);
    }
}
