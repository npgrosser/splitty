//! In-process adapter conformance: native operations and the HTTP/control
//! adapters observe the same service, without starting a second workspace.
use super::*;
use crate::service::catalog::{Operation, Request as NativeRequest, Role};
use serde_json::Value;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::{Op, ScreenOp};

#[tokio::test]
async fn conditional_daemon_shutdown_is_owner_only_and_bound_to_the_accepting_instance() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).server_mode = "daemon".into();
    let owner = crate::http::build_control_router(state.clone());
    let request = |instance: &str| {
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/daemon/stop")
            .header("content-type", "application/json")
            .header("host", ADDR)
            .header("authorization", "Bearer process-token")
            .body(axum::body::Body::from(
                json!({"instance":instance}).to_string(),
            ))
            .unwrap()
    };
    let reply = owner
        .clone()
        .oneshot(request(&"f".repeat(32)))
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::CONFLICT);
    assert!(
        tokio::time::timeout(Duration::from_millis(1), state.owner_shutdown.notified())
            .await
            .is_err()
    );
    let browser = crate::http::build_router(state.clone());
    assert_eq!(
        browser
            .oneshot(request(&state.instance))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(1), state.owner_shutdown.notified())
            .await
            .is_err()
    );
    let reply = owner.oneshot(request(&state.instance)).await.unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_millis(50), state.owner_shutdown.notified())
        .await
        .unwrap();
}

async fn framed(
    state: &AppState,
    role: Role,
) -> (
    splitty_transport::peer::Peer,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    use splitty_transport::{negotiation, peer, queue, wire};
    let (client, server) = tokio::io::duplex(128 * 1024);
    let (read, write) = tokio::io::split(server);
    let service = state.service.clone();
    let task = tokio::spawn(crate::native::serve(
        read,
        write,
        service,
        crate::native::Assignment {
            role,
            ..Default::default()
        },
    ));
    let (mut read, mut write) = tokio::io::split(client);
    let context = queue::Context::default();
    negotiation::client(&mut read, &mut write, &context.budget)
        .await
        .unwrap();
    let ready = negotiation::accept(
        &mut read,
        &mut write,
        &context.budget,
        negotiation::Expected {
            instance: Some(&state.instance),
            purpose: &wire::Purpose::Direct,
            mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
            preparation: WAIT,
        },
        |_| panic!("direct connection has no preparation prelude"),
    )
    .await
    .unwrap();
    (peer::Peer::start(read, write, &ready, context), task)
}

fn framed_call(peer: &splitty_transport::peer::Peer, id: u32, operation: &str, args: Value) {
    peer.sender
        .json(
            splitty_transport::wire::Kind::Request,
            &splitty_transport::wire::Request {
                id,
                operation: operation.into(),
                args: args.as_object().unwrap().clone(),
            },
        )
        .unwrap();
}

async fn framed_reply(peer: &mut splitty_transport::peer::Peer) -> splitty_transport::wire::Reply {
    tokio::time::timeout(WAIT, async {
        loop {
            let frame = peer.recv().await.expect("native reply");
            if frame.frame.kind == splitty_transport::wire::Kind::Reply {
                return frame.frame.decode().unwrap();
            }
        }
    })
    .await
    .expect("reply deadline")
}

#[tokio::test]
async fn framed_json_amplification_disconnects_only_its_client() {
    use splitty_transport::wire::Kind;
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (mut healthy, good_server) = framed(&state, Role::Main).await;
    for kind in [Kind::Text, Kind::Request] {
        let (mut peer, server) = framed(&state, Role::Main).await;
        let count = if kind == Kind::Text {
            1_048_576
        } else {
            262_144
        };
        let array = format!("[{}0]", "0,".repeat(count));
        let input = if kind == Kind::Text {
            format!(r#"{{"type":"resize","padding":{array}}}"#)
        } else {
            format!(r#"{{"id":1,"operation":"home.update","args":{{"body":{array}}}}}"#)
        };
        peer.sender.data(kind, input.as_bytes()).unwrap();
        tokio::time::timeout(WAIT, async { while peer.recv().await.is_some() {} })
            .await
            .unwrap();
        assert!(
            server.await.unwrap().is_err(),
            "over-budget parse must end the carrier"
        );
        peer.shutdown().await;
    }
    framed_call(&healthy, 1, "mux.status", json!({}));
    assert_eq!(framed_reply(&mut healthy).await.status, 200);
    healthy.shutdown().await;
    good_server.await.unwrap().unwrap();
}

#[tokio::test]
async fn framed_maximum_image_survives_parser_and_worker_admission() {
    use base64::Engine as _;
    use splitty_transport::wire::Kind;
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (mut peer, server) = framed(&state, Role::Main).await;
    let mut bytes = vec![0xa5; splitty_core::protocol::paste::MAX_IMAGE_BYTES];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    let input = json!({"type":"save_image", "requestId":"maximum-image", "data":base64::engine::general_purpose::STANDARD.encode(&bytes)}).to_string();
    peer.sender.data(Kind::Text, input.as_bytes()).unwrap();
    drop(input);
    let saved = tokio::time::timeout(WAIT, async {
        loop {
            let frame = peer
                .recv()
                .await
                .expect("image response, not a closed client");
            if frame.frame.kind != Kind::Text {
                continue;
            }
            let value: Value = frame.frame.decode().unwrap();
            if value["requestId"] == "maximum-image" {
                break value;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(saved["type"], "image_saved", "{saved}");
    let path = Path::new(saved["path"].as_str().unwrap());
    assert_eq!(path.parent().unwrap(), dir.path().join("paste"));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    framed_call(&peer, 1, "mux.status", json!({}));
    assert_eq!(framed_reply(&mut peer).await.status, 200);
    peer.shutdown().await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn framed_native_welcomes_before_replies_and_enforces_the_fixed_role_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    // Seed visible local management state so forbidden Forget/Stop/Update
    // operations cannot pass unnoticed merely because their targets are absent.
    for (operation, body) in [
        (
            Operation::HostPreferencesUpdate,
            json!({"target":"vm1","alias":"Original"}),
        ),
        (Operation::HostOrderUpdate, json!({"order":["vm1","vm2"]})),
        (
            Operation::RemoteScreensUpdate,
            json!({"version":1,"screens":[{"target":"vm1","screenId":"original"}]}),
        ),
        (
            Operation::CatalogUpdate,
            json!({"target":"vm1","screens":[{"id":"original","panes":1}]}),
        ),
        (
            Operation::ForwardPreferencesSave,
            json!({"target":"vm1","port":5173,"local":5174,"label":"Original"}),
        ),
        (
            Operation::ReversePreferencesSave,
            json!({"target":"vm1","port":18431,"local":18432}),
        ),
    ] {
        let response = native(&state, operation, json!({"body":body})).await;
        assert_eq!(
            response.0,
            StatusCode::OK,
            "seed {operation:?}: {response:?}"
        );
    }
    async fn management_state(state: &AppState) -> Vec<(&'static str, (StatusCode, Value))> {
        let mut snapshot = Vec::new();
        for operation in crate::service::catalog::OPERATIONS.iter().filter(|entry| {
            entry.scope == crate::service::catalog::Scope::LocalHub && entry.method == "GET"
        }) {
            snapshot.push((
                operation.name,
                native(state, operation.operation, json!({})).await,
            ));
        }
        snapshot
    }
    let before = management_state(&state).await;
    let (mut peer, server) = framed(&state, Role::Remote).await;
    framed_call(&peer, 1, "mux.status", json!({}));
    let welcome = peer.recv().await.unwrap();
    assert_eq!(welcome.frame.kind, splitty_transport::wire::Kind::Text);
    assert_eq!(
        welcome.frame.decode::<Value>().unwrap()["type"],
        "mux.welcome"
    );
    drop(welcome);
    let reply = framed_reply(&mut peer).await;
    assert_eq!(reply.id, 1);
    assert_eq!(reply.status, 200);
    let status: Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(status["instance"], state.instance);
    assert!(status["listenAddr"].is_null());
    let mut id = 1;
    for operation in crate::service::catalog::OPERATIONS
        .iter()
        .filter(|entry| entry.scope == crate::service::catalog::Scope::LocalHub)
    {
        let args = if operation.method == "GET" {
            json!({})
        } else {
            let body = match operation.operation {
                Operation::HostForget => json!({"target":"vm1"}),
                Operation::HostPreferencesUpdate => json!({"target":"vm1","alias":"Forbidden"}),
                Operation::HostOrderUpdate => json!({"order":["vm2","vm1"]}),
                Operation::RemoteScreensUpdate => {
                    json!({"version":1,"screens":[{"target":"vm2","screenId":"forbidden"}]})
                }
                Operation::CatalogUpdate => {
                    json!({"target":"vm1","screens":[{"id":"forbidden","panes":2}]})
                }
                Operation::ForwardOpen => json!({"target":"vm1","port":5173,"local":5175}),
                Operation::ForwardPreferencesSave => {
                    json!({"target":"vm1","port":5173,"local":5175,"label":"Forbidden"})
                }
                Operation::ForwardStop | Operation::ForwardPreferencesForget => {
                    json!({"target":"vm1","port":5173})
                }
                Operation::ReverseOpen | Operation::ReversePreferencesSave => {
                    json!({"target":"vm1","port":18431,"local":18433})
                }
                Operation::ReverseStop | Operation::ReversePreferencesForget => {
                    json!({"target":"vm1","port":18431})
                }
                _ => panic!(
                    "new LocalHub mutation needs a meaningful Remote-denial fixture: {}",
                    operation.name
                ),
            };
            json!({"body":body})
        };
        NativeRequest {
            operation: operation.operation,
            args: args.as_object().unwrap().clone(),
        }
        .validate(Role::Main)
        .expect("denial input has the allowed role's argument shape");
        id += 1;
        framed_call(&peer, id, operation.name, args);
        let reply = framed_reply(&mut peer).await;
        assert_eq!(reply.id, id);
        assert_eq!(reply.status, 403, "{}: {}", operation.name, reply.body);
        assert_eq!(
            management_state(&state).await,
            before,
            "{} changed local management state",
            operation.name
        );
        assert!(
            host.asked().is_empty(),
            "{} invoked a connector: {:?}",
            operation.name,
            host.asked()
        );
    }
    // Backend authority remains useful on the same assigned Remote stream.
    // Compare a real template mutation/read and domain failures with browser
    // dispatch; these must not be mistaken for forbidden local-hub operations.
    let browser = crate::http::build_router(state.clone());
    let template = json!({"name":"Remote template","panes":{"shell":{"command":"printf remote"}},"layout":{"slot":"shell"}});
    for (operation, args, method, path, body) in [
        ("home.get", json!({}), "GET", "/api/home/settings", None),
        ("build.get", json!({}), "GET", "/api/build", None),
        (
            "template.save",
            json!({"slug":"remote-template","body":template}),
            "POST",
            "/api/templates/remote-template",
            Some(template),
        ),
        (
            "template.get",
            json!({"slug":"remote-template"}),
            "GET",
            "/api/templates/remote-template",
            None,
        ),
        (
            "template.get",
            json!({"slug":"missing"}),
            "GET",
            "/api/templates/missing",
            None,
        ),
        (
            "session.send",
            json!({"id":"missing","body":{"data":"hello\n"}}),
            "POST",
            "/api/session/missing/send",
            Some(json!({"data":"hello\n"})),
        ),
    ] {
        id += 1;
        framed_call(&peer, id, operation, args);
        let reply = framed_reply(&mut peer).await;
        let actual = (
            StatusCode::from_u16(reply.status).unwrap(),
            serde_json::from_str(&reply.body).unwrap_or_else(|_| json!(reply.body)),
        );
        let expected = exchange(&browser, api(method, path, body.as_ref())).await;
        assert_eq!(actual, expected, "Remote backend parity: {operation}");
    }
    id += 1;
    framed_call(&peer, id, "agents.list", json!({}));
    assert_eq!(framed_reply(&mut peer).await.status, 400);
    id += 1;
    framed_call(&peer, id, "mux.status", json!({"target":"other-host"}));
    assert_eq!(framed_reply(&mut peer).await.status, 400);
    assert_eq!(management_state(&state).await, before);
    assert!(host.asked().is_empty());
    peer.shutdown().await;
    server.await.unwrap().unwrap();
    assert!(state.client_state.clients.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stale_discovery_and_early_requests_create_no_service_client() {
    use splitty_transport::{codec, negotiation, queue, wire};
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    for early in [true, false] {
        let (client, server) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(server);
        let task = tokio::spawn(crate::native::serve(
            read,
            write,
            state.service.clone(),
            Default::default(),
        ));
        let (mut read, mut write) = tokio::io::split(client);
        let context = queue::Context::default();
        negotiation::client(&mut read, &mut write, &context.budget)
            .await
            .unwrap();
        if early {
            let frame = codec::Frame::data(
                wire::Kind::Text,
                wire::Delivery::new(1),
                br#"{"type":"create","id":"forbidden"}"#,
            )
            .unwrap();
            codec::write(&mut write, &frame).await.unwrap();
        } else {
            assert!(negotiation::accept(
                &mut read,
                &mut write,
                &context.budget,
                negotiation::Expected {
                    instance: Some(&"f".repeat(32)),
                    purpose: &wire::Purpose::Direct,
                    mux_protocol: 1,
                    preparation: WAIT,
                },
                |_| {}
            )
            .await
            .is_err());
        }
        drop(read);
        drop(write);
        assert!(task.await.unwrap().is_err());
        assert!(state.client_state.clients.lock().unwrap().is_empty());
        assert_eq!(state.mux_store.screen_count(), 0);
    }
}

#[tokio::test]
async fn framed_request_flood_is_bounded_and_cancel_releases_a_real_long_poll() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (_, launched) = native(&state, Operation::JobLaunch, json!({"body":{
        "requestId":splitty_core::ids::JobId::new(),"argv":["/bin/sh","-c","read line"],"cwd":dir.path(),"placement":{"kind":"screen"},"background":true
    }})).await;
    let id = launched["jobId"].as_str().expect("job id");
    let (mut peer, server) = framed(&state, Role::Main).await;
    for request in 1..=65 {
        framed_call(
            &peer,
            request,
            "job.logs",
            json!({"id":id,"tail":"0","waitMs":"30000"}),
        );
    }
    let busy = framed_reply(&mut peer).await;
    assert_eq!((busy.id, busy.status), (65, 503));
    peer.sender
        .control(
            splitty_transport::wire::Kind::Cancel,
            &splitty_transport::wire::Cancel {
                generation: peer.sender.generation(),
                id: 1,
            },
        )
        .unwrap();
    let cancelled = framed_reply(&mut peer).await;
    assert_eq!(
        (cancelled.id, cancelled.status, cancelled.uncertain),
        (1, 499, false)
    );
    framed_call(&peer, 66, "mux.status", json!({}));
    let next = framed_reply(&mut peer).await;
    assert_eq!((next.id, next.status), (66, 200));
    peer.shutdown().await;
    server.await.unwrap().unwrap();
    assert!(state.client_state.clients.lock().unwrap().is_empty());
    native(&state, Operation::JobStop, json!({"id":id})).await;
    state.client_state.jobs.shutdown().await;
}

#[tokio::test]
async fn cancelling_an_admitted_mutation_reports_uncertainty_without_replaying_it() {
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let gate = host.pause_forward(5173);
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    let (mut peer, server) = framed(&state, Role::Main).await;
    framed_call(
        &peer,
        1,
        "forward.open",
        json!({"body":{"target":"vm1","port":5173}}),
    );
    gate.entered().await;
    peer.sender
        .control(
            splitty_transport::wire::Kind::Cancel,
            &splitty_transport::wire::Cancel {
                generation: peer.sender.generation(),
                id: 1,
            },
        )
        .unwrap();
    let cancelled = framed_reply(&mut peer).await;
    assert_eq!(
        (cancelled.id, cancelled.status, cancelled.uncertain),
        (1, 499, true)
    );
    gate.release();
    tokio::time::timeout(WAIT, async {
        while state.hub.forwards().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("admitted mutation completes");
    framed_call(&peer, 2, "mux.status", json!({}));
    let next = framed_reply(&mut peer).await;
    assert_eq!(
        next.id, 2,
        "the cancelled operation must not produce a second reply"
    );
    assert_eq!(state.hub.forwards().len(), 1);
    state.hub.stop_forward("vm1", 5173).await.unwrap();
    peer.shutdown().await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_immediate_abort_cannot_be_lost_when_control_overtakes_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    let (mut peer, server) = framed(&state, Role::Main).await;
    // No yield between enqueue and abort: the reserved control writer can
    // deliver Cancel before the data frame, as a frozen renderer can too.
    framed_call(
        &peer,
        1,
        "forward.open",
        json!({"body":{"target":"vm1","port":5173}}),
    );
    peer.sender
        .control(
            splitty_transport::wire::Kind::Cancel,
            &splitty_transport::wire::Cancel {
                generation: peer.sender.generation(),
                id: 1,
            },
        )
        .unwrap();
    let cancelled = framed_reply(&mut peer).await;
    assert_eq!(
        (cancelled.id, cancelled.status, cancelled.uncertain),
        (1, 499, false)
    );
    framed_call(&peer, 2, "mux.status", json!({}));
    assert_eq!(framed_reply(&mut peer).await.id, 2);
    assert!(
        host.asked().is_empty(),
        "cancel before admission cannot start SSH"
    );
    peer.shutdown().await;
    server.await.unwrap().unwrap();
}

pub(super) async fn native(
    state: &AppState,
    operation: Operation,
    args: Value,
) -> (StatusCode, Value) {
    let request = NativeRequest {
        operation,
        args: args.as_object().expect("argument object").clone(),
    };
    let response = state.service.execute(request, Role::Main).await;
    (
        StatusCode::from_u16(response.status).expect("service status"),
        if response.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&response.body).expect("JSON")
        },
    )
}

#[tokio::test]
async fn queries_errors_and_service_identity_match_browser_and_native() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    let browser = crate::http::build_router(state.clone());
    for operation in crate::service::catalog::OPERATIONS.iter().filter(|entry| {
        entry.method == "GET" && entry.arguments.iter().all(|arg| arg.ends_with('?'))
    }) {
        let expected = exchange(&browser, api("GET", operation.path, None)).await;
        let actual = native(&state, operation.operation, json!({})).await;
        assert_eq!(actual, expected, "{}", operation.name);
    }
    for (operation, args, method, path, body) in [
        (
            Operation::MuxScreen,
            json!({"id":"missing"}),
            "GET",
            "/api/mux/screen/missing",
            None,
        ),
        (
            Operation::TemplateGet,
            json!({"slug":"missing"}),
            "GET",
            "/api/templates/missing",
            None,
        ),
        (
            Operation::MuxOp,
            json!({"body":{"screenId":"missing","op":{"type":"explode"}}}),
            "POST",
            "/api/mux/op",
            Some(json!({"screenId":"missing","op":{"type":"explode"}})),
        ),
        (
            Operation::SessionSend,
            json!({"id":"missing","body":{"data":"hello\n"}}),
            "POST",
            "/api/session/missing/send",
            Some(json!({"data":"hello\n"})),
        ),
        (
            Operation::EditorOpen,
            json!({"body":{"sessionId":"missing","path":"/tmp/file","editor":"zed"}}),
            "POST",
            "/api/editor/open",
            Some(json!({"sessionId":"missing","path":"/tmp/file","editor":"zed"})),
        ),
        (
            Operation::FileOffer,
            json!({"body":{"sessionId":"missing","path":"/tmp/file"}}),
            "POST",
            "/api/file/offer",
            Some(json!({"sessionId":"missing","path":"/tmp/file"})),
        ),
        (
            Operation::ViewerCommentReply,
            json!({"body":{"screenId":"missing","paneId":"p1","document":"/tmp/file","number":1,"body":"x"}}),
            "POST",
            "/api/viewer/comments/reply",
            Some(
                json!({"screenId":"missing","paneId":"p1","document":"/tmp/file","number":1,"body":"x"}),
            ),
        ),
        (
            Operation::ViewerReviewWait,
            json!({"body":{"screenId":"missing","paneId":"p1","viewerPaneId":"v1"}}),
            "POST",
            "/api/viewer/review/wait",
            Some(json!({"screenId":"missing","paneId":"p1","viewerPaneId":"v1"})),
        ),
        (
            Operation::JobInspect,
            json!({"id":"missing"}),
            "GET",
            "/api/jobs/missing",
            None,
        ),
    ] {
        assert_eq!(
            native(&state, operation, args).await,
            exchange(&browser, api(method, path, body.as_ref())).await,
            "{operation:?}"
        );
    }
    let (_, status) = native(&state, Operation::MuxStatus, json!({})).await;
    assert_eq!(status["listenAddr"], Value::Null);
    assert_eq!(status["instance"], state.instance);
    assert_eq!(status["transportVersion"], 1);
    assert!(
        host.asked().is_empty(),
        "queries must not establish SSH connections"
    );

    let other = AppState {
        listen_addr: "127.0.0.1:1".into(),
        ..state.clone()
    };
    let owner = crate::http::build_control_router(other);
    assert_eq!(
        exchange(
            &owner,
            Request::get("/api/mux/status")
                .body(Body::empty())
                .expect("request")
        )
        .await
        .1,
        status
    );
}

#[tokio::test]
async fn native_mutations_are_visible_to_cli_without_exposing_scratchpads() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    state
        .mux_store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some(ScreenId::from("shared")),
            new_pane_id: Some(PaneId::from("pane")),
            shell_profile_id: None,
        })
        .expect("screen");
    let session = state
        .mux_store
        .session_id_for_pane(&PaneId::from("pane"))
        .expect("session");
    state
        .mux_store
        .dispatch_screen(
            &ScreenId::from("shared"),
            ScreenOp::SetScratchpad {
                session_id: session.clone(),
                text: "private draft".into(),
            },
        )
        .expect("scratchpad");
    let rename =
        json!({"screenId":"shared","op":{"type":"rename_screen","name":"changed natively"}});
    assert_eq!(
        native(&state, Operation::MuxOp, json!({"body":rename}))
            .await
            .0,
        StatusCode::OK
    );
    let owner = crate::http::build_control_router(state.clone());
    let (_, screen) = exchange(
        &owner,
        Request::get("/api/mux/screen/shared")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(screen["name"], "changed natively");
    let (_, native_screen) = native(&state, Operation::MuxScreen, json!({"id":"shared"})).await;
    assert_eq!(screen, native_screen);
    assert!(native_screen["sessions"][session.as_str()]
        .get("scratchpad")
        .is_none());
    let rejected = json!({"screenId":"shared","op":{"type":"set_scratchpad","sessionId":session,"text":"overwrite"}});
    assert_eq!(
        native(&state, Operation::MuxOp, json!({"body":rejected}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        state
            .mux_store
            .get_screen(&ScreenId::from("shared"))
            .expect("screen")
            .sessions[&session]
            .scratchpad,
        "private draft"
    );
}

#[tokio::test]
async fn immediate_native_drop_and_independent_closes_release_only_their_client() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    for _ in 0..20 {
        drop(crate::client_session::connect(state.client_state.clone()).expect("native admission"));
    }
    let mut a =
        crate::client_session::connect(state.client_state.clone()).expect("native admission");
    let mut b =
        crate::client_session::connect(state.client_state.clone()).expect("native admission");
    let first = tokio::time::timeout(WAIT, a.output.recv())
        .await
        .expect("welcome")
        .expect("open");
    let second = tokio::time::timeout(WAIT, b.output.recv())
        .await
        .expect("welcome")
        .expect("open");
    let crate::client_session::Message::Text(first) = first.value else {
        panic!("welcome must precede binary output")
    };
    let crate::client_session::Message::Text(second) = second.value else {
        panic!("welcome must precede binary output")
    };
    let a_welcome: Value = serde_json::from_str(&first).expect("JSON");
    let b_welcome: Value = serde_json::from_str(&second).expect("JSON");
    assert_eq!(a_welcome["type"], "mux.welcome");
    assert_eq!(b_welcome["type"], "mux.welcome");
    assert_ne!(a_welcome["clientId"], b_welcome["clientId"]);
    a.shutdown().await;
    assert_eq!(state.client_state.clients.lock().expect("clients").len(), 1);
    b.input
        .send(crate::client_session::Message::Text(
            json!({"type":"mux.resync"}).to_string(),
        ))
        .expect("remaining client is live");
    assert!(tokio::time::timeout(WAIT, b.output.recv())
        .await
        .expect("reply")
        .is_some());
    b.shutdown().await;
    assert!(state
        .client_state
        .clients
        .lock()
        .expect("clients")
        .is_empty());
}

#[tokio::test]
async fn native_jobs_keep_shared_idempotency_ownership_and_error_contracts() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let browser = crate::http::build_router(state.clone());
    let launch = json!({"requestId":splitty_core::ids::JobId::new(),"argv":["/bin/sh","-c","read line"],"cwd":dir.path(),"placement":{"kind":"screen"},"background":true});
    let (status, launched) = native(&state, Operation::JobLaunch, json!({"body":launch})).await;
    assert_eq!(status, StatusCode::CREATED, "{launched}");
    let (replayed_status, replayed) =
        exchange(&browser, api("POST", "/api/jobs", Some(&launch))).await;
    assert_eq!(replayed_status, StatusCode::OK, "{replayed}");
    assert_eq!(launched["jobId"], replayed["jobId"]);
    assert_eq!(
        native(&state, Operation::JobList, json!({})).await,
        exchange(&browser, api("GET", "/api/jobs", None)).await
    );
    let id = launched["jobId"].as_str().expect("job");
    assert_eq!(
        native(&state, Operation::JobInspect, json!({"id":id})).await,
        exchange(&browser, api("GET", &format!("/api/jobs/{id}"), None)).await
    );
    let native_logs = native(
        &state,
        Operation::JobLogs,
        json!({"id":id,"waitMs":"31000"}),
    )
    .await;
    let browser_logs = exchange(
        &browser,
        api("GET", &format!("/api/jobs/{id}/logs?waitMs=31000"), None),
    )
    .await;
    assert_eq!(native_logs, browser_logs);
    assert_eq!(native_logs.0, StatusCode::BAD_REQUEST);
    let unauthorized_actor = NativeRequest {
        operation: Operation::JobList,
        args: json!({"actor":"owner"})
            .as_object()
            .expect("object")
            .clone(),
    };
    assert_eq!(
        state
            .service
            .execute(unauthorized_actor, Role::Main)
            .await
            .status,
        400
    );
    assert_eq!(
        native(&state, Operation::JobStop, json!({"id":id})).await.0,
        StatusCode::OK
    );
    state.client_state.jobs.shutdown().await;
}

#[tokio::test]
async fn a_native_reverse_request_cannot_share_a_fresh_browser_gateway() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let browser = crate::http::build_router(served.state.clone());
    // Building another gateway must not replace the first port's exclusion.
    let other_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("second gateway listener");
    let other_address = other_listener.local_addr().expect("address");
    let _other_gateway = crate::http::build_router(AppState {
        listen_addr: other_address.to_string(),
        ..served.state.clone()
    });
    let body = json!({"target":"vm1","port":8080,"local":served.port});
    for (operation, method, path) in [
        (
            Operation::ReverseOpen,
            "POST",
            "/api/remote/reverse-forwards",
        ),
        (
            Operation::ReversePreferencesSave,
            "PUT",
            "/api/remote/reverse-forward-preferences",
        ),
    ] {
        // Native first, before any browser reverse/remote handler is invoked.
        let result = native(&served.state, operation, json!({"body":body})).await;
        assert_eq!(result.0, StatusCode::CONFLICT, "{result:?}");
        assert_eq!(result.1["code"], "local_port_reserved");
        assert_eq!(
            result,
            exchange(&browser, api(method, path, Some(&body))).await
        );
    }
    assert!(
        host.asked().is_empty(),
        "reserved destinations must never spawn SSH"
    );
    let other = native(
        &served.state,
        Operation::ReverseOpen,
        json!({"body":{"target":"vm1","port":8080,"local":other_address.port()}}),
    )
    .await;
    assert_eq!(other.0, StatusCode::CONFLICT);
    assert_eq!(other.1["code"], "local_port_reserved");
    assert!(!dir
        .path()
        .join("config/remote-reverse-forwards.json")
        .exists());
}

#[tokio::test]
async fn framed_host_catalog_and_saved_forward_intents_preserve_passive_and_stop_forget_semantics()
{
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    let (mut peer, server) = framed(&state, Role::Main).await;
    let forward_local = free_port();
    let reverse_local = free_port();
    let reverse_remote = free_port();
    let other_local = free_port();
    let requests = [
        (
            "hostPreferences.update",
            json!({"target":"vm1","alias":"Primary"}),
        ),
        ("hostOrder.update", json!({"order":["vm2","vm1"]})),
        (
            "remoteScreens.update",
            json!({"version":1,"screens":[{"target":"vm1","screenId":"s1"}]}),
        ),
        (
            "catalog.update",
            json!({"target":"vm1","screens":[{"id":"s1","panes":1}]}),
        ),
        (
            "forwardPreferences.save",
            json!({"target":"vm1","port":5173,"local":forward_local,"label":"Frontend"}),
        ),
        (
            "forwardPreferences.save",
            json!({"target":"vm2","port":5173,"local":other_local}),
        ),
        (
            "reversePreferences.save",
            json!({"target":"vm1","port":reverse_remote,"local":reverse_local}),
        ),
    ];
    let mut id = 0;
    for (operation, body) in requests {
        id += 1;
        framed_call(&peer, id, operation, json!({"body":body}));
        let reply = framed_reply(&mut peer).await;
        assert_eq!(reply.status, 200, "{operation}: {}", reply.body);
    }
    for operation in [
        "host.list",
        "hostPreferences.get",
        "hostOrder.get",
        "remoteScreens.get",
        "catalog.get",
        "forward.list",
        "forwardPreferences.get",
        "reverse.list",
        "reversePreferences.get",
    ] {
        id += 1;
        framed_call(&peer, id, operation, json!({}));
        let reply = framed_reply(&mut peer).await;
        assert_eq!(reply.status, 200, "{operation}: {}", reply.body);
    }
    assert!(
        host.asked().is_empty(),
        "native reads and preference edits must never contact SSH"
    );
    for local in [forward_local, other_local, reverse_local, reverse_remote] {
        assert!(!held(local), "passive metadata bound {local}");
    }
    let connector = crate::native::client::Connector::default();
    let mut remote = connector
        .host(
            state.service.clone(),
            splitty_remote::ssh::SshTarget::parse("vm1").unwrap(),
            false,
        )
        .await
        .unwrap();
    loop {
        let message = remote.recv().await.unwrap();
        if message.frame.decode::<Value>().unwrap()["type"] == "remote.ready" {
            break;
        }
    }
    tokio::time::timeout(WAIT, async {
        while state.hub.forwards().len() != 1 || state.hub.reverses().len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !held(other_local),
        "only the explicitly connected host may restore saved mappings"
    );
    assert!(host.asked().iter().all(|call| !call.contains("vm2")));
    assert_eq!(state.hub.forwards()[0].target, "vm1");
    for (operation, body) in [
        ("host.forget", json!({"target":"vm1"})),
        (
            "forwardPreferences.forget",
            json!({"target":"vm1","port":5173}),
        ),
        (
            "reversePreferences.forget",
            json!({"target":"vm1","port":reverse_remote}),
        ),
    ] {
        id += 1;
        framed_call(&peer, id, operation, json!({"body":body}));
        let reply = framed_reply(&mut peer).await;
        assert_eq!(reply.status, 204, "{operation}: {}", reply.body);
    }
    assert_eq!(state.hub.forwards().len(), 1, "Forget keeps a live forward");
    assert_eq!(state.hub.reverses().len(), 1, "Forget keeps a live share");
    for (operation, port) in [("forward.stop", 5173), ("reverse.stop", reverse_remote)] {
        id += 1;
        framed_call(
            &peer,
            id,
            operation,
            json!({"body":{"target":"vm1","port":port}}),
        );
        let reply = framed_reply(&mut peer).await;
        assert_eq!(reply.status, 204, "{operation}: {}", reply.body);
    }
    assert!(state.hub.forwards().is_empty());
    assert!(state.hub.reverses().is_empty());
    assert!(!held(forward_local));
    assert!(!held(reverse_remote));
    remote.shutdown().await;
    peer.shutdown().await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn embedded_connector_retains_owner_budget_and_revocation_through_ready() {
    use crate::native::client::Connector;
    use splitty_transport::{
        budget::{ConnectionBudget, ProcessBudget},
        queue, wire,
    };
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let connector = Connector::default();
    let process = ProcessBudget::new(wire::PROCESS_BYTES, wire::PROCESS_CONTROL_BYTES);
    let context = queue::Context::new(ConnectionBudget::new(
        process.clone(),
        wire::CONNECTION_BYTES,
        wire::CONNECTION_CONTROL_BYTES,
    ));
    let pending = connector.admit_context(context.clone()).unwrap();
    let mut client = connector
        .embedded_admitted(state.service.clone(), Role::Main, pending)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &context.budget.data,
        &client.sender().context().budget.data
    ));
    assert!(Arc::ptr_eq(
        &context.budget.control,
        &client.sender().context().budget.control
    ));
    let reply = client
        .request(Operation::MuxStatus, serde_json::Map::new())
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    drop(reply);
    assert!(
        process.used() >= 2 * 64 * 1024,
        "pipe buffers remain charged through their actual lifetime"
    );
    context.close();
    assert!(client.sender().context().is_closed());
    assert!(tokio::time::timeout(WAIT, client.recv())
        .await
        .unwrap()
        .is_none());
    client.shutdown().await;
    assert_eq!(
        process.used(),
        0,
        "completion includes peer and owned service cleanup"
    );
    assert!(connector.admit_context(context).is_err());

    let context = queue::Context::default();
    let pending = connector.admit_context(context.clone()).unwrap();
    context.close();
    let error = match connector
        .embedded_admitted(state.service.clone(), Role::Main, pending)
        .await
    {
        Ok(_) => panic!("revoked setup must not create a peer"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    assert_eq!(context.budget.used(), 0);
    let healthy = connector
        .embedded(state.service.clone(), Role::Main)
        .await
        .unwrap();
    healthy.shutdown().await;
}
