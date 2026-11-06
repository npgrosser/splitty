//! viewer-annotations, Agents Read And Reply To Handed-Over Threads, through
//! the control API: neither the HTTP listener, the control socket nor the
//! native endpoint returns a viewer pane's annotations with the workspace or
//! changes them through `POST /api/mux/op`; the comment endpoints return what
//! the user handed over on the caller's screen and add an agent's reply.
use super::conformance::native;
use super::*;
use crate::service::catalog::Operation;
use serde_json::Value;
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
                "id": ANNOTATION, "kind": "comment", "body": "Why not five times?",
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
/// Layout Operations (Annotations only through the window, No client writes
/// an agent's reply); viewer-annotations, Agents Read And Reply To
/// Handed-Over Threads (An agent reads the workspace, An agent tries to
/// annotate): on the HTTP listener, the control socket and the native
/// endpoint, also for a pane with a wait for its review.
#[tokio::test]
async fn the_control_api_neither_shows_nor_changes_annotations() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    with_annotation(&state);
    state
        .mux_store
        .dispatch(Op::StartReviewWait {
            screen_id: ScreenId::from("s1"),
            pane_id: PaneId::from("v1"),
            wait: splitty_core::mux::ReviewWait {
                id: "wait-0123456789abcdef".into(),
                pane_id: PaneId::from("p1"),
                title: "claude".into(),
            },
            revision: None,
        })
        .expect("a wait");
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

        assert!(!screen.to_string().contains("wait-"), "{channel}");
        for op in [
            json!({
                "type": "put_annotations", "paneId": "v1",
                "annotations": [{"id": "ann-00000000000000ff", "kind": "comment", "body": "from a pane"}],
            }),
            json!({"type": "remove_annotations", "paneId": "v1", "ids": [ANNOTATION]}),
            json!({
                "type": "put_replies", "paneId": "v1", "annotationId": ANNOTATION,
                "replies": [{"id": "rep-00000000000000ff", "author": "user", "body": "from a pane"}],
            }),
            json!({
                "type": "put_replies", "paneId": "v1", "annotationId": ANNOTATION,
                "replies": [{"id": "rep-00000000000000fe", "author": "agent", "body": "from a pane"}],
            }),
            json!({
                "type": "remove_replies", "paneId": "v1", "annotationId": ANNOTATION,
                "ids": ["rep-00000000000000ff"],
            }),
            json!({"type": "mark_sent", "paneId": "v1", "entries": [{"id": ANNOTATION}]}),
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
        // A trusted operation has no wire form a client could submit.
        for op in [
            json!({
                "type": "agent_reply", "paneId": "v1", "number": 1,
                "reply": {"id": "rep-00000000000000fd", "author": "agent", "body": "x",
                          "from": {"paneId": "p1", "title": "claude"}},
            }),
            json!({
                "type": "start_review_wait", "paneId": "v1",
                "wait": {"id": "wait-00000000000000fd", "paneId": "p1", "title": "claude"},
            }),
            json!({"type": "end_review_wait", "paneId": "v1", "waitId": "wait-0123456789abcdef"}),
        ] {
            let body = json!({"screenId": "s1", "op": op});
            let (code, answer) = exchange(&app, api("POST", "/api/mux/op", Some(&body))).await;
            assert_eq!(code, StatusCode::BAD_REQUEST, "{channel}: {answer}");
            assert_eq!(answer["ok"], false, "{channel}: {answer}");
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

/// Annotation `id` with `remark`, `sent` or pending, on a passage of `line`.
fn comment(id: &str, remark: &str, line: u32) -> Value {
    json!({
        "id": id, "kind": "comment", "body": remark,
        "target": {
            "view": "source", "quote": "retry three times", "prefix": "On failure, ",
            "suffix": ".", "startLine": line, "endLine": line,
        },
    })
}

fn dispatch(state: &AppState, screen: &str, op: Value) {
    let op: ScreenOp = serde_json::from_value(op).expect("wire op");
    state
        .mux_store
        .dispatch_screen(&ScreenId::from(screen), op)
        .expect("op");
}

/// Screen `s1` with the terminal `p1`, the viewer `v1` of PLAN.md and the
/// text viewer `v2`; screen `s2` with the terminal `p2`. `v1` holds #1 and #2,
/// which the user handed over with a reply of the user, #3, which the user
/// did not, a draft reply of the user under #1, and #4, handed over and
/// resolved. An agent replied to #1. `v2` holds a handed-over comment on a
/// passage of its text.
fn with_review(state: &AppState) {
    for (screen, pane) in [("s1", "p1"), ("s2", "p2")] {
        state
            .mux_store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from(screen)),
                new_pane_id: Some(PaneId::from(pane)),
                shell_profile_id: None,
            })
            .expect("screen");
    }
    let quoted = json!({
        "id": "ann-0000000000000005", "kind": "comment", "body": "Which plan?",
        "target": {"view": "source", "quote": "agent's", "prefix": "the ", "suffix": " plan",
                   "startLine": 1, "endLine": 1},
    });
    for op in [
        json!({
            "type": "open_viewer", "paneId": "p1", "newPaneId": "v1",
            "source": {"kind": "file", "path": PLAN},
        }),
        json!({
            "type": "open_viewer", "paneId": "p1", "newPaneId": "v2", "placement": "tab",
            "source": {"kind": "text", "title": "Plan", "text": "the agent's plan"},
        }),
        json!({"type": "put_annotations", "paneId": "v1", "annotations": [
            comment("ann-0000000000000001", "Why not five times?", 30),
            {"id": "ann-0000000000000002", "kind": "change", "replacement": "",
             "target": {"view": "rendered", "quote": "The old migration path.", "prefix": "",
                        "suffix": "", "startLine": 41, "endLine": 41}},
            {"id": "ann-0000000000000004", "kind": "comment", "body": "Settled."},
        ]}),
        json!({"type": "put_replies", "paneId": "v1", "annotationId": "ann-0000000000000001",
               "replies": [{"id": "rep-00000000000000b1", "author": "user", "body": "Or with backoff."}]}),
        json!({"type": "mark_sent", "paneId": "v1", "entries": [
            comment("ann-0000000000000001", "Why not five times?", 30),
            {"id": "ann-0000000000000002", "kind": "change", "replacement": "",
             "target": {"view": "rendered", "quote": "The old migration path.", "prefix": "",
                        "suffix": "", "startLine": 41, "endLine": 41}},
            {"id": "ann-0000000000000004", "kind": "comment", "body": "Settled."},
            {"id": "rep-00000000000000b1", "body": "Or with backoff."},
        ]}),
        json!({"type": "put_annotations", "paneId": "v1", "annotations": [
            {"id": "ann-0000000000000004", "kind": "comment", "body": "Settled.",
             "status": "resolved", "sent": true},
            comment("ann-0000000000000003", "Not sent yet.", 12),
        ]}),
        json!({"type": "put_replies", "paneId": "v1", "annotationId": "ann-0000000000000001",
               "replies": [{"id": "rep-00000000000000b2", "author": "user", "body": "A draft."}]}),
        json!({"type": "put_annotations", "paneId": "v2", "annotations": [quoted]}),
        json!({"type": "mark_sent", "paneId": "v2", "entries": [quoted]}),
    ] {
        dispatch(state, "s1", op);
    }
    state
        .mux_store
        .dispatch(Op::AgentReply {
            screen_id: ScreenId::from("s1"),
            pane_id: PaneId::from("v1"),
            number: 1,
            reply: splitty_core::mux::AnnotationReply {
                id: "rep-00000000000000a1".into(),
                author: splitty_core::mux::ReplyAuthor::Agent,
                body: "Three keeps the worst case under a minute.".into(),
                sent: false,
                from: Some(splitty_core::mux::ReplyFrom {
                    pane_id: PaneId::from("p1"),
                    title: "claude".into(),
                }),
            },
        })
        .expect("agent's reply");
}

/// The comment endpoints on the three transports the control API has.
fn channels(
    state: &AppState,
    host: &ScriptedHost,
    dir: &Path,
) -> Vec<(&'static str, axum::Router)> {
    vec![
        ("HTTP", router(state, hub(host, dir))),
        (
            "control socket",
            crate::http::build_control_router(state.clone()),
        ),
    ]
}

/// `GET /api/viewer/comments` with `query`, on `channel`, or natively.
async fn comments(
    channel: Option<&axum::Router>,
    state: &AppState,
    query: &[(&str, &str)],
) -> (StatusCode, Value) {
    match channel {
        Some(app) => {
            let query: Vec<String> = query
                .iter()
                .map(|(name, value)| format!("{name}={}", encode(value)))
                .collect();
            exchange(
                app,
                api(
                    "GET",
                    &format!("/api/viewer/comments?{}", query.join("&")),
                    None,
                ),
            )
            .await
        }
        None => {
            let args: serde_json::Map<String, Value> = query
                .iter()
                .map(|(name, value)| ((*name).to_owned(), json!(value)))
                .collect();
            native(state, Operation::ViewerComments, Value::Object(args)).await
        }
    }
}

/// control-api, Viewer Comment Requests (Comments of a screen),
/// viewer-annotations, Agents Read And Reply To Handed-Over Threads (An agent
/// reads a sent review, Another screen) and access-control, Viewer Documents
/// Stay With Their Connection (A handed-over quote): the viewer panes of the
/// caller's screen in layout order, each with what the user handed over, open
/// only unless `all`; the user's drafts stay out, and of a text only its
/// quoted passage; another screen sees none of it. The same on the HTTP
/// listener, the control socket and the native endpoint.
#[tokio::test]
async fn an_agent_reads_what_the_user_handed_over_on_its_screen() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    with_review(&state);
    let first = json!({
        "number": 1, "kind": "comment", "status": "open", "body": "Why not five times?",
        "target": {"view": "source", "quote": "retry three times", "startLine": 30, "endLine": 30},
        "replies": [
            {"author": "user", "body": "Or with backoff."},
            {"author": "agent", "body": "Three keeps the worst case under a minute.",
             "from": {"paneId": "p1", "title": "claude"}},
        ],
    });
    let second = json!({
        "number": 2, "kind": "change", "status": "open", "replacement": "",
        "target": {"view": "rendered", "quote": "The old migration path.", "startLine": 41, "endLine": 41},
        "replies": [],
    });
    let resolved = json!({
        "number": 3, "kind": "comment", "status": "resolved", "body": "Settled.", "replies": [],
    });
    // Layout order, not the order of opening: `v2` is a tab in the slot of
    // `p1`, which comes before `v1`. Of its text only the quoted passage.
    let text = json!({"paneId": "v2", "source": {"kind": "text", "title": "Plan"}, "annotations": [{
        "number": 1, "kind": "comment", "status": "open", "body": "Which plan?",
        "target": {"view": "source", "quote": "agent's", "startLine": 1, "endLine": 1},
        "replies": [],
    }]});
    let plan = json!({"paneId": "v1", "source": {"kind": "file", "path": PLAN}, "annotations": [first, second]});
    let open = json!({"documents": [text, plan]});
    let all = json!({"documents": [
        text,
        {"paneId": "v1", "source": {"kind": "file", "path": PLAN}, "annotations": [first, second, resolved]},
    ]});
    let channels = channels(&state, &host, dir.path());
    let channels: Vec<Option<&axum::Router>> = channels
        .iter()
        .map(|(_, app)| Some(app))
        .chain([None])
        .collect();
    for (index, channel) in channels.into_iter().enumerate() {
        let caller = [("screenId", "s1"), ("paneId", "p1")];
        let (code, listing) = comments(channel, &state, &caller).await;
        assert_eq!(code, StatusCode::OK, "{index}: {listing}");
        assert_eq!(listing, open, "{index}");
        assert!(!listing.to_string().contains("A draft."), "{index}");
        assert!(!listing.to_string().contains("Not sent yet."), "{index}");
        assert!(!listing.to_string().contains("the agent's plan"), "{index}");

        let (_, listing) = comments(
            channel,
            &state,
            &[("screenId", "s1"), ("paneId", "p1"), ("all", "true")],
        )
        .await;
        assert_eq!(listing, all, "{index}");
        for document in [PLAN, "v1"] {
            let (code, listing) = comments(
                channel,
                &state,
                &[("screenId", "s1"), ("paneId", "p1"), ("document", document)],
            )
            .await;
            assert_eq!(code, StatusCode::OK, "{index} {document}: {listing}");
            assert_eq!(listing["documents"], json!([plan]), "{index} {document}");
        }

        // A pane that moved still names its old screen; the pane decides.
        let (code, listing) =
            comments(channel, &state, &[("screenId", "gone"), ("paneId", "p1")]).await;
        assert_eq!((code, &listing), (StatusCode::OK, &open), "{index}");

        let (code, listing) =
            comments(channel, &state, &[("screenId", "s2"), ("paneId", "p2")]).await;
        assert_eq!(code, StatusCode::OK, "{index}: {listing}");
        assert_eq!(listing, json!({"documents": []}), "{index}: another screen");

        for (query, status, code_name) in [
            (vec![("paneId", "p1")], StatusCode::BAD_REQUEST, "invalid"),
            (
                vec![("screenId", "s1"), ("paneId", "p9")],
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                vec![
                    ("screenId", "s1"),
                    ("paneId", "p1"),
                    ("document", "/home/me/app/NOTES.md"),
                ],
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                vec![
                    ("screenId", "s1"),
                    ("paneId", "p1"),
                    ("document", "viewer-0123456789abcdef"),
                ],
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                vec![
                    ("screenId", "s1"),
                    ("paneId", "p1"),
                    ("document", "PLAN.md"),
                ],
                StatusCode::BAD_REQUEST,
                "invalid",
            ),
            (
                vec![("screenId", "s2"), ("paneId", "p2"), ("document", PLAN)],
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                vec![("screenId", "s1"), ("paneId", "p1"), ("all", "yes")],
                StatusCode::BAD_REQUEST,
                "invalid",
            ),
        ] {
            let (code, answer) = comments(channel, &state, &query).await;
            assert_eq!(code, status, "{index} {query:?}: {answer}");
            assert_eq!(answer["code"], code_name, "{index} {query:?}: {answer}");
            assert!(
                answer["error"].as_str().is_some_and(|e| !e.is_empty()),
                "{answer}"
            );
        }
    }
}

/// `POST /api/viewer/comments/reply` with `body`, on `channel`, or natively.
async fn reply(
    channel: Option<&axum::Router>,
    state: &AppState,
    body: Value,
) -> (StatusCode, Value) {
    match channel {
        Some(app) => exchange(app, api("POST", "/api/viewer/comments/reply", Some(&body))).await,
        None => native(state, Operation::ViewerCommentReply, json!({"body": body})).await,
    }
}

/// The replies of annotation `number` of `v1`.
fn thread(state: &AppState, number: u32) -> Vec<splitty_core::mux::AnnotationReply> {
    let screen = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    screen.panes[&PaneId::from("v1")]
        .viewer
        .as_ref()
        .expect("viewer")
        .annotations
        .iter()
        .find(|annotation| annotation.number == number)
        .expect("annotation")
        .replies
        .clone()
}

/// control-api, Viewer Comment Requests (A reply, Unknown number) and
/// viewer-annotations, Agents Read And Reply To Handed-Over Threads (An agent
/// replies, A reply to an annotation not sent): the reply lands in the
/// thread with author `agent`, the caller's pane and its title, by path or
/// pane ID, open or resolved, on every transport; what breaks a rule changes
/// nothing and is answered with its code, `limit` for a full thread and
/// `ambiguous` for a path two viewer panes show.
#[tokio::test]
async fn an_agent_replies_to_a_handed_over_annotation() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    with_review(&state);
    let channels = channels(&state, &host, dir.path());
    let channels: Vec<Option<&axum::Router>> = channels
        .iter()
        .map(|(_, app)| Some(app))
        .chain([None])
        .collect();
    let request = |document: &str, number: u32, body: &str| json!({"screenId": "s1", "paneId": "p1", "document": document, "number": number, "body": body});
    for (index, channel) in channels.into_iter().enumerate() {
        let before = thread(&state, 2).len();
        let (code, answer) = reply(
            channel,
            &state,
            request(PLAN, 2, "Switched to on-demand computation."),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{index}: {answer}");
        assert_eq!(answer, json!({"ok": true, "paneId": "v1", "number": 2}));
        let replies = thread(&state, 2);
        assert_eq!(replies.len(), before + 1);
        let added = replies.last().expect("reply");
        assert_eq!(added.author, splitty_core::mux::ReplyAuthor::Agent);
        assert_eq!(added.body, "Switched to on-demand computation.");
        assert!(
            splitty_core::mux::reply_id_is_valid(&added.id),
            "{}",
            added.id
        );
        let from = added.from.as_ref().expect("from");
        // No process runs in `p1` here: the pane's title is the default.
        assert_eq!(
            (from.pane_id.as_str(), from.title.as_str()),
            ("p1", "agent")
        );

        let (code, answer) = reply(
            channel,
            &state,
            request("v1", 3, "Resolved, and still answered."),
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{index}: {answer}");

        let revision = state
            .mux_store
            .get_screen(&ScreenId::from("s1"))
            .expect("screen")
            .revision;
        let workspace =
            serde_json::to_value(state.mux_store.get_workspace_state()).expect("workspace");
        for (body, status, code_name) in [
            (request(PLAN, 9, "x"), StatusCode::NOT_FOUND, "not_found"),
            (request(PLAN, 4, "x"), StatusCode::NOT_FOUND, "not_found"),
            (
                request("/home/me/app/NOTES.md", 1, "x"),
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                request(PLAN, 1, " \n\t"),
                StatusCode::BAD_REQUEST,
                "invalid",
            ),
            (request(PLAN, 1, ""), StatusCode::BAD_REQUEST, "invalid"),
            (
                request(PLAN, 1, &"x".repeat(16 * 1024 + 1)),
                StatusCode::BAD_REQUEST,
                "invalid",
            ),
            (
                json!({"screenId": "s1", "paneId": "p1", "document": PLAN, "number": -1, "body": "x"}),
                StatusCode::BAD_REQUEST,
                "invalid",
            ),
            (
                json!({"screenId": "s1", "paneId": "p1", "document": PLAN, "body": "x"}),
                StatusCode::BAD_REQUEST,
                "invalid",
            ),
            (
                json!({"screenId": "s2", "paneId": "p2", "document": PLAN, "number": 1, "body": "x"}),
                StatusCode::NOT_FOUND,
                "not_found",
            ),
        ] {
            let (code, answer) = reply(channel, &state, body.clone()).await;
            assert_eq!(code, status, "{index} {body}: {answer}");
            assert_eq!(answer["code"], code_name, "{index}: {answer}");
        }
        assert_eq!(
            serde_json::to_value(state.mux_store.get_workspace_state()).expect("workspace"),
            workspace,
            "{index}: nothing changed"
        );
        assert_eq!(
            state
                .mux_store
                .get_screen(&ScreenId::from("s1"))
                .expect("screen")
                .revision,
            revision
        );
    }

    // A thread takes 50 replies; a path two viewer panes show names neither.
    let http = router(&state, hub(&host, dir.path()));
    let more: Vec<Value> = (thread(&state, 3).len()..splitty_core::mux::REPLY_COUNT_LIMIT)
        .map(|n| json!({"id": format!("rep-{n:016x}"), "author": "user", "body": "More."}))
        .collect();
    dispatch(
        &state,
        "s1",
        json!({"type": "put_replies", "paneId": "v1", "annotationId": "ann-0000000000000004", "replies": more}),
    );
    let (code, answer) = reply(Some(&http), &state, request("v1", 3, "One too many.")).await;
    assert_eq!(code, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "limit", "{answer}");
    dispatch(
        &state,
        "s1",
        json!({
            "type": "open_viewer", "paneId": "p1", "newPaneId": "v3", "placement": "tab",
            "source": {"kind": "file", "path": PLAN},
        }),
    );
    let (code, answer) = reply(Some(&http), &state, request(PLAN, 1, "Which one?")).await;
    assert_eq!(code, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "ambiguous", "{answer}");
    let named = answer["error"].as_str().unwrap_or_default();
    assert!(named.contains("v1") && named.contains("v3"), "{answer}");
    let (code, answer) = comments(
        Some(&http),
        &state,
        &[("screenId", "s1"), ("paneId", "p1"), ("document", PLAN)],
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT, "{answer}");
    assert_eq!(answer["code"], "ambiguous", "{answer}");
    let (code, answer) = reply(Some(&http), &state, request("v1", 1, "This one.")).await;
    assert_eq!(code, StatusCode::OK, "by its ID: {answer}");

    // Numbers follow the order in which the server added the annotations:
    // the resolved one is #3, the pending one #4.
    let screen = state
        .mux_store
        .get_screen(&ScreenId::from("s1"))
        .expect("screen");
    let numbers: Vec<(String, u32, bool)> = screen.panes[&PaneId::from("v1")]
        .viewer
        .as_ref()
        .expect("viewer")
        .annotations
        .iter()
        .map(|annotation| (annotation.id.clone(), annotation.number, annotation.sent))
        .collect();
    assert_eq!(
        numbers,
        [
            ("ann-0000000000000001".to_owned(), 1, true),
            ("ann-0000000000000002".to_owned(), 2, true),
            ("ann-0000000000000004".to_owned(), 3, true),
            ("ann-0000000000000003".to_owned(), 4, false),
        ]
    );
}
