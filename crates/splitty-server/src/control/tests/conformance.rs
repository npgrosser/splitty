//! In-process adapter conformance: native operations and the HTTP/control
//! adapters observe the same service, without starting a second workspace.
use super::*;
use crate::service::catalog::{Operation, Request as NativeRequest, Role};
use axum::response::IntoResponse;
use serde_json::Value;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::{Op, ScreenOp};
use splitty_core::native_api::directory::Directory;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn conditional_daemon_shutdown_is_owner_only_and_bound_to_the_accepting_instance() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).server_mode = "daemon".into();
    let owner = crate::control::build_control_router(state.service.clone());
    let request = |instance: &str| {
        axum::http::Request::builder()
            .method("POST")
            .uri("/api/daemon/stop")
            .header("content-type", "application/json")
            .header("host", ADDR)
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
    let reply = owner.oneshot(request(&state.instance)).await.unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_millis(50), state.owner_shutdown.notified())
        .await
        .unwrap();
}

pub(super) async fn framed(
    state: &TestState,
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

/// The next text frame of `kind` on `peer`, skipping others.
pub(super) async fn next_text(peer: &mut splitty_transport::peer::Peer, kind: &str) -> Value {
    tokio::time::timeout(WAIT, async {
        loop {
            let frame = peer.recv().await.expect("open connection");
            if frame.frame.kind == splitty_transport::wire::Kind::Text {
                let value: Value = frame.frame.decode().unwrap();
                if value["type"] == kind {
                    return value;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind}"))
}

// remote-hosts, SSH Questions Of The Local Service: a question the service's
// askpass helper publishes is announced to local main connections, read and
// answered over the native API, and kept from remote-window connections.
#[tokio::test]
async fn ssh_prompts_notify_local_connections_and_are_answered_over_the_native_api() {
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    let service = Arc::get_mut(&mut state.service).unwrap();
    service.hub = Arc::new(hub(&host, dir.path()));
    Arc::get_mut(&mut service.client_state).unwrap().hub = Some(service.hub.clone());
    let capability = json!(splitty_core::protocol::capability::SSH_PROMPTS);
    let (mut local, local_task) = framed(&state, Role::Main).await;
    let welcome = next_text(&mut local, "mux.welcome").await;
    assert!(welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&capability));
    let (mut remote, remote_task) = framed(&state, Role::Remote).await;
    let welcome = next_text(&mut remote, "mux.welcome").await;
    assert!(!welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&capability));

    // The broker is process-wide: this test's question has a target of its own.
    let target = format!("demo-{:016x}@conformance", rand::random::<u64>());
    let answer = splitty_remote::askpass::testing::ask(&target, "demo@host's password:");
    next_text(&mut local, "ssh.prompts.changed").await;
    framed_call(&local, 1, "ssh.prompts.get", json!({}));
    let reply = framed_reply(&mut local).await;
    assert_eq!(reply.status, 200);
    let prompts: Vec<Value> = serde_json::from_str(&reply.body).unwrap();
    let prompt = prompts
        .iter()
        .find(|prompt| prompt["target"] == target.as_str())
        .expect("the published question");
    assert_eq!(prompt["prompt"], "demo@host's password:");
    assert_eq!(prompt["confirm"], false);
    let id = prompt["id"].as_str().unwrap().to_owned();

    // A connection opened while the question waits is told after its greeting.
    let (mut late, late_task) = framed(&state, Role::Main).await;
    next_text(&mut late, "mux.welcome").await;
    next_text(&mut late, "ssh.prompts.changed").await;

    assert!(
        tokio::time::timeout(Duration::from_millis(50), async {
            loop {
                let frame = remote.recv().await.unwrap();
                if frame.frame.kind == splitty_transport::wire::Kind::Text
                    && frame.frame.decode::<Value>().unwrap()["type"] == "ssh.prompts.changed"
                {
                    break;
                }
            }
        })
        .await
        .is_err(),
        "remote-window connections must not learn of local SSH questions"
    );
    for (request, call, args) in [
        (1, "ssh.prompts.get", json!({})),
        (
            2,
            "ssh.prompts.answer",
            json!({"body":{"id":id,"answer":"stolen"}}),
        ),
    ] {
        framed_call(&remote, request, call, args);
        assert_eq!(framed_reply(&mut remote).await.status, 403, "{call}");
    }

    framed_call(
        &local,
        2,
        "ssh.prompts.answer",
        json!({"body":{"id":id,"answer":"private answer"}}),
    );
    let reply = framed_reply(&mut local).await;
    assert_eq!(reply.status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&reply.body).unwrap()["accepted"],
        true
    );
    assert_eq!(
        answer.recv_timeout(WAIT).unwrap().as_deref(),
        Some("private answer")
    );
    next_text(&mut local, "ssh.prompts.changed").await;
    framed_call(&local, 3, "ssh.prompts.get", json!({}));
    let reply = framed_reply(&mut local).await;
    let prompts: Vec<Value> = serde_json::from_str(&reply.body).unwrap();
    assert!(prompts.iter().all(|prompt| prompt["id"] != id.as_str()));

    for peer in [local, remote, late] {
        peer.shutdown().await;
    }
    for task in [local_task, remote_task, late_task] {
        task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn hub_observations_notify_other_local_clients_and_respect_assigned_authority() {
    use splitty_transport::wire::Kind;
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    let service = Arc::get_mut(&mut state.service).unwrap();
    service.hub = Arc::new(hub(&host, dir.path()));
    Arc::get_mut(&mut service.client_state).unwrap().hub = Some(service.hub.clone());
    let (observer, observer_task) = framed(&state, Role::Main).await;
    let (mut viewer, viewer_task) = framed(&state, Role::Main).await;
    let welcome = viewer
        .recv()
        .await
        .unwrap()
        .frame
        .decode::<Value>()
        .unwrap();
    assert_eq!(welcome["type"], "mux.welcome");
    assert!(welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("hub-host-decisions-v1")));
    assert!(welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!(splitty_core::protocol::capability::HUB_AUTO_FORWARD)));
    assert!(welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!(splitty_core::protocol::capability::HUB_HOST_STATE)));
    // An observation without pane counts keeps the stored ones.
    for (index, (screens, stored)) in [
        (
            json!([{"id":"s1","name":"Renamed","panes":2}]),
            json!([{"id":"s1","name":"Renamed","panes":2}]),
        ),
        (
            json!([{"id":"s1","name":"Again"},{"id":"s2"}]),
            json!([{"id":"s1","name":"Again","panes":2}]),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        observer
            .sender
            .json(
                Kind::Text,
                &json!({"type":"hub.catalog.observe","target":"vm1","screens":screens}),
            )
            .unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                let frame = viewer.recv().await.unwrap();
                if frame.frame.kind == Kind::Text
                    && frame.frame.decode::<Value>().unwrap()["type"] == "hub.changed"
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        framed_call(&viewer, index as u32 + 1, "remote.directory.get", json!({}));
        let reply = framed_reply(&mut viewer).await;
        assert_eq!(reply.status, 200);
        let native_wire: Value = serde_json::from_str(&reply.body).unwrap();
        let native: Directory = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(native.hosts[0].target, "vm1");
        assert_eq!(
            native.hosts[0].screens.len(),
            stored.as_array().unwrap().len()
        );
        assert_eq!(serde_json::to_value(&native).unwrap(), native_wire);
        let router = service_router(&state);
        let (status, http_wire) =
            exchange(&router, api("GET", "/api/remote/directory", None)).await;
        assert_eq!(status, StatusCode::OK);
        let http: Directory = serde_json::from_value(http_wire.clone()).unwrap();
        assert_eq!(http, native);
        assert_eq!(http_wire, native_wire);
        assert_eq!(http_wire["hosts"][0]["screens"], stored);
        assert!(
            host.asked().is_empty(),
            "passive reads must not open a host"
        );
    }
    let restored = router(&state, hub(&host, dir.path()));
    let (status, directory) = exchange(&restored, api("GET", "/api/remote/directory", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(directory["hosts"][0]["target"], "vm1");
    assert_eq!(
        directory["hosts"][0]["screens"],
        json!([{"id":"s1","name":"Again","panes":2}])
    );
    let (mut remote, remote_task) = framed(&state, Role::Remote).await;
    let remote_welcome = remote
        .recv()
        .await
        .unwrap()
        .frame
        .decode::<Value>()
        .unwrap();
    assert!(!remote_welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("hub-host-decisions-v1")));
    assert!(!remote_welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!(splitty_core::protocol::capability::HUB_AUTO_FORWARD)));
    assert!(!remote_welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!(splitty_core::protocol::capability::HUB_HOST_STATE)));
    state.hub.set_host_order(json!({"order":["vm1"]})).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), async {
            loop {
                let frame = remote.recv().await.unwrap();
                if frame.frame.kind == Kind::Text
                    && frame.frame.decode::<Value>().unwrap()["type"] == "hub.changed"
                {
                    break;
                }
            }
        })
        .await
        .is_err(),
        "remote-only connections must not learn local hub activity"
    );
    remote.sender.json(Kind::Text, &json!({"type":"hub.catalog.observe","target":"vm1","screens":[{"id":"forbidden","panes":1}]})).unwrap();
    tokio::time::timeout(WAIT, async { while remote.recv().await.is_some() {} })
        .await
        .unwrap();
    assert!(remote_task.await.unwrap().is_err());
    assert_eq!(state.hub.directory().hosts[0].screens[0].id, "s1");
    state.hub.forget_host(json!({"target":"vm1"})).unwrap();
    let observation =
        json!({"type":"hub.catalog.observe","target":"vm1","screens":[{"id":"hidden","panes":1}]});
    observer.sender.json(Kind::Text, &observation).unwrap();
    // A resync fences the observation on the same stream.
    let mut observer = observer;
    observer
        .sender
        .json(Kind::Text, &json!({"type":"mux.workspace_resync"}))
        .unwrap();
    next_text(&mut observer, "mux.workspace_snapshot").await;
    assert!(!state
        .hub
        .directory()
        .hosts
        .iter()
        .any(|host| host.target == "vm1"));
    let control = crate::control::build_control_router(state.service.clone());
    assert_eq!(
        exchange(
            &control,
            api("PUT", "/api/remote/catalog", Some(&observation))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert!(host.asked().is_empty());
    remote.shutdown().await;
    observer.shutdown().await;
    viewer.shutdown().await;
    observer_task.await.unwrap().unwrap();
    viewer_task.await.unwrap().unwrap();
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
async fn removed_operations_answer_errors_without_ending_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (mut peer, task) = framed(&state, Role::Main).await;
    next_text(&mut peer, "mux.welcome").await;
    for (index, name) in [
        "shellProfiles.update",
        "host.list",
        "hostPreferences.get",
        "hostOrder.get",
        "catalog.get",
        "catalog.update",
    ]
    .into_iter()
    .enumerate()
    {
        let id = u32::try_from(index).unwrap() * 2 + 1;
        framed_call(&peer, id, name, json!({}));
        let reply = framed_reply(&mut peer).await;
        assert_eq!(reply.status, 400, "{name}: {}", reply.body);
        assert_eq!(reply.id, id);
        framed_call(&peer, id + 1, "remote.directory.get", json!({}));
        let directory = framed_reply(&mut peer).await;
        assert_eq!(directory.status, 200);
        assert_eq!(directory.id, id + 1);
    }
    peer.shutdown().await;
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn native_welcome_leaves_screen_order_to_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    for id in ["second", "first"] {
        state
            .mux_store
            .dispatch(splitty_core::mux::Op::CreateScreen {
                name: None,
                screen_id: Some(id.into()),
                new_pane_id: None,
                shell_profile_id: None,
            })
            .unwrap();
    }
    let (mut peer, task) = framed(&state, Role::Main).await;
    let welcome = next_text(&mut peer, "mux.welcome").await;
    assert!(welcome.get("screens").is_none());
    peer.sender
        .json(
            splitty_transport::wire::Kind::Text,
            &json!({"type":"mux.workspace_resync"}),
        )
        .unwrap();
    let catalog = next_text(&mut peer, "mux.workspace_snapshot").await;
    assert_eq!(catalog["screens"][0]["id"], "second");
    assert_eq!(catalog["screens"][1]["id"], "first");
    peer.shutdown().await;
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn standalone_session_creation_is_refused_after_welcome() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (mut peer, task) = framed(&state, Role::Main).await;
    next_text(&mut peer, "mux.welcome").await;
    peer.sender
        .json(
            splitty_transport::wire::Kind::Text,
            &json!({"type":"create"}),
        )
        .unwrap();
    assert!(
        tokio::time::timeout(WAIT, async { while peer.recv().await.is_some() {} })
            .await
            .is_ok(),
        "the invalid session stream should end"
    );
    peer.shutdown().await;
    let _ = task.await.unwrap();
    assert!(state
        .client_state
        .dispatcher
        .backend()
        .sessions()
        .is_empty());
}

#[tokio::test]
async fn framed_native_welcomes_before_replies_and_enforces_the_fixed_role_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    state
        .hub
        .update_catalog(json!({"target":"vm1","screens":[{"id":"original","panes":1}]}))
        .unwrap();
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
    async fn management_state(state: &TestState) -> Vec<(&'static str, (StatusCode, Value))> {
        let mut snapshot = Vec::new();
        // The SSH questions are the process's, not this state's: a test that
        // runs beside this one publishes its own. That a Remote connection
        // cannot answer one is checked where a question is asked.
        for operation in crate::service::catalog::OPERATIONS.iter().filter(|entry| {
            entry.scope == crate::service::catalog::Scope::LocalHub
                && entry.method == "GET"
                && entry.operation != Operation::SshPrompts
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
    assert!(status.get("listenAddr").is_none());
    assert_eq!(status["serverMode"], "embedded");
    let mut id = 1;
    for operation in crate::service::catalog::OPERATIONS
        .iter()
        .filter(|entry| entry.scope == crate::service::catalog::Scope::LocalHub)
    {
        let args = if operation.method == "GET" {
            json!({})
        } else {
            let body = match operation.operation {
                Operation::SshAnswer => json!({"id":"f".repeat(64),"answer":"yes"}),
                Operation::HostForget => json!({"target":"vm1"}),
                Operation::HostPreferencesUpdate => json!({"target":"vm1","alias":"Forbidden"}),
                Operation::HostOrderUpdate => json!({"order":["vm2","vm1"]}),
                Operation::RemoteScreensUpdate => {
                    json!({"version":1,"screens":[{"target":"vm2","screenId":"forbidden"}]})
                }
                Operation::RemoteScreensUpdateHost => {
                    json!({"target":"vm2","screens":[{"screenId":"forbidden"}]})
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
    // Compare a real template mutation/read and domain failures with service
    // dispatch; these must not be mistaken for forbidden local-hub operations.
    let service = service_router(&state);
    let template = json!({"name":"Remote template","panes":{"shell":{"command":"printf remote"}},"layout":{"slot":"shell"}});
    for (operation, args, method, path, body) in [
        ("home.get", json!({}), "GET", "/api/home/settings", None),
        (
            "home.metadata",
            json!({}),
            "GET",
            "/api/home/settings/metadata",
            None,
        ),
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
        let expected = exchange(&service, api(method, path, body.as_ref())).await;
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
    drop(service);
    assert!(
        splitty_core::pty::testing::wait_until(WAIT, || state
            .client_state
            .clients
            .lock()
            .unwrap()
            .is_empty())
        .await
    );
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
                br#"{"type":"attach","id":"forbidden"}"#,
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
    state: &TestState,
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

pub(super) async fn framed_mux_op(state: &TestState, body: Value) -> (StatusCode, Value) {
    let (mut peer, task) = framed(state, Role::Main).await;
    framed_call(&peer, 1, "mux.op", json!({"body":body}));
    let reply = framed_reply(&mut peer).await;
    peer.shutdown().await;
    task.await.unwrap().unwrap();
    (
        StatusCode::from_u16(reply.status).expect("mux op status"),
        serde_json::from_str(&reply.body).expect("mux op JSON"),
    )
}

#[tokio::test]
async fn settings_metadata_and_rejection_match_http_and_native() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let service = service_router(&state);
    let (status, metadata) =
        exchange(&service, api("GET", "/api/home/settings/metadata", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(metadata["defaults"]["drawer"]["sidebarWidth"], 200);
    assert!(metadata["limits"]
        .get("terminal.xterm.scrollSensitivity")
        .is_none());
    assert_eq!(metadata["defaults"]["appearance"]["zoom"], 1.0);
    assert_eq!(
        native(&state, Operation::HomeMetadata, json!({})).await,
        (status, metadata)
    );
    let before = state.config.read().unwrap().clone();
    let invalid = json!({"editors":{"extensions":[{"extension":"../bad","editor":"zed"}]}});
    let (status, body) =
        exchange(&service, api("PATCH", "/api/home/settings", Some(&invalid))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["field"], "editors.extensions.0.extension");
    assert_eq!(
        native(&state, Operation::HomeUpdate, json!({"body":invalid})).await,
        (status, body)
    );
    assert_eq!(*state.config.read().unwrap(), before);
}

#[tokio::test]
async fn queries_errors_and_service_identity_match_control_and_native() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    let service = service_router(&state);
    // A custom source may deliberately share a built-in display name. The
    // resolved identifier must describe the actual source on both adapters.
    std::fs::create_dir_all(&state.themes_dir).unwrap();
    std::fs::write(
        state.themes_dir.join("alias-palette.toml"),
        include_str!("../../../../splitty-core/src/theme/bundled/splitty-dark.toml"),
    )
    .unwrap();
    for (requested, resolved, label) in [
        ("splitty-dark", "splitty-dark", "Splitty Dark"),
        ("alias-palette", "alias-palette", "Splitty Dark"),
        ("missing-palette", "splitty-dark", "Splitty Dark"),
    ] {
        let native_theme = native(&state, Operation::ThemeGet, json!({"name": requested})).await;
        let service_theme = exchange(
            &service,
            api("GET", &format!("/api/home/theme?name={requested}"), None),
        )
        .await;
        assert_eq!(native_theme, service_theme);
        assert_eq!(native_theme.0, StatusCode::OK);
        assert_eq!(native_theme.1["id"], resolved);
        assert_eq!(native_theme.1["name"], label);
        assert_eq!(native_theme.1["terminalPalette"], resolved);
    }
    // settings, Named palette in the web window: the stored terminal palette
    // gives the answer its terminal colors on both adapters.
    let (_, light) = native(
        &state,
        Operation::ThemeGet,
        json!({"name": "splitty-light"}),
    )
    .await;
    state.config.write().unwrap().terminal.theme = "splitty-light".into();
    let native_theme = native(&state, Operation::ThemeGet, json!({"name": "splitty-dark"})).await;
    let service_theme = exchange(
        &service,
        api("GET", "/api/home/theme?name=splitty-dark", None),
    )
    .await;
    assert_eq!(native_theme, service_theme);
    assert_eq!(native_theme.1["id"], "splitty-dark");
    assert_eq!(native_theme.1["terminalPalette"], "splitty-light");
    assert_eq!(native_theme.1["terminal"], light["terminal"]);
    assert_ne!(native_theme.1["ui"], light["ui"]);
    state.config.write().unwrap().terminal.theme = "default".into();
    for operation in crate::service::catalog::OPERATIONS
        .iter()
        .filter(|entry| entry.method == "GET" && entry.arguments.iter().all(|arg| arg.optional))
    {
        let expected = exchange(&service, api("GET", operation.path, None)).await;
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
            exchange(&service, api(method, path, body.as_ref())).await,
            "{operation:?}"
        );
    }
    let (_, status) = native(&state, Operation::MuxStatus, json!({})).await;
    assert!(status.get("listenAddr").is_none());
    assert_eq!(status["instance"], state.instance);
    assert_eq!(status["transportVersion"], 1);
    assert!(
        host.asked().is_empty(),
        "queries must not establish SSH connections"
    );

    let other = TestState {
        listen_addr: "127.0.0.1:1".into(),
        ..state.clone()
    };
    let owner = crate::control::build_control_router(other.service);
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
                if_version: None,
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
    let owner = crate::control::build_control_router(state.service.clone());
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
async fn session_and_control_layout_ops_create_the_same_split_and_viewer() {
    use splitty_core::mux::LayoutNode;
    use splitty_core::pty::testing::pty_available;
    use splitty_transport::wire::Kind;

    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let native = state(ADDR, dir.path());
    let owner = state(ADDR, dir.path());
    for state in [&native, &owner] {
        state
            .mux_store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some("shared".into()),
                new_pane_id: Some("anchor".into()),
                shell_profile_id: None,
            })
            .unwrap();
        use_test_shell(state);
    }
    let control = crate::control::build_control_router(owner.service.clone());
    let (mut peer, task) = framed(&native, Role::Main).await;
    next_text(&mut peer, "mux.welcome").await;
    for (op, expected_spawns) in [
        (
            json!({"type":"split_pane","paneId":"anchor","axis":"horizontal","newPaneId":"shell"}),
            1,
        ),
        (
            json!({"type":"open_viewer","paneId":"shell","placement":"tab","newPaneId":"viewer",
            "source":{"kind":"file","path":"/tmp/plan.md"}}),
            0,
        ),
    ] {
        peer.sender
            .json(
                Kind::Text,
                &json!({"type":"mux.op","screenId":"shared","op":op}),
            )
            .unwrap();
        let (status, reply) = exchange(
            &control,
            api(
                "POST",
                "/api/mux/op",
                Some(&json!({"screenId":"shared","op":op})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["created"].as_array().unwrap().len(), expected_spawns);
        if expected_spawns == 1 {
            let spawned = next_text(&mut peer, "mux.pane_spawned").await;
            assert_eq!(spawned["paneId"], "shell");
        }
    }
    peer.sender
        .json(
            Kind::Text,
            &json!({"type":"mux.resync","screenId":"shared"}),
        )
        .unwrap();
    next_text(&mut peer, "mux.screen_snapshot").await;

    let layout = |state: &TestState| {
        let workspace = state.mux_store.get_screen(&"shared".into()).unwrap();
        let LayoutNode::Split { axis, children, .. } = &workspace.root else {
            panic!("split pane must produce a split");
        };
        let slots = children
            .iter()
            .map(LayoutNode::collect_pane_ids)
            .collect::<Vec<_>>();
        let viewer = workspace.panes[&PaneId::from("viewer")]
            .viewer
            .clone()
            .unwrap();
        let backend = state.client_state.dispatcher.backend();
        assert_eq!(backend.sessions().len(), 1);
        backend.close_all();
        (axis.clone(), slots, viewer.source, viewer.origin_pane_id)
    };
    let native_layout = layout(&native);
    assert_eq!(
        native_layout.1,
        vec![
            vec![PaneId::from("anchor")],
            vec![PaneId::from("shell"), PaneId::from("viewer")]
        ]
    );
    assert_eq!(native_layout, layout(&owner));
    peer.shutdown().await;
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn relative_screen_placement_matches_native_and_control_decoders() {
    let native_dir = tempfile::tempdir().expect("native dir");
    let control_dir = tempfile::tempdir().expect("control dir");
    let native_state = state(ADDR, native_dir.path());
    let control_state = state(ADDR, control_dir.path());
    for service in [&native_state, &control_state] {
        for id in ["A", "B", "C"] {
            service
                .mux_store
                .dispatch(Op::CreateScreen {
                    name: None,
                    screen_id: Some(ScreenId::from(id)),
                    new_pane_id: None,
                    shell_profile_id: None,
                })
                .expect("seed Screen");
        }
    }
    let mut observer =
        crate::client_session::connect(native_state.client_state.clone()).expect("observer");
    let welcome = observer.output.recv().await.expect("welcome");
    let crate::client_session::Message::Text(welcome) = welcome.value else {
        panic!("welcome must be text")
    };
    let welcome: Value = serde_json::from_str(&welcome).expect("welcome JSON");
    assert!(welcome["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("screen-placement-v1")));
    while observer.output.try_recv().is_ok() {}

    let request = json!({"screenId":"C","op":{"type":"move_screen","beforeScreenId":"A"}});
    assert_eq!(
        framed_mux_op(&native_state, request.clone()).await.0,
        StatusCode::OK
    );
    let control = crate::control::build_control_router(control_state.service.clone());
    assert_eq!(
        exchange(&control, api("POST", "/api/mux/op", Some(&request)))
            .await
            .0,
        StatusCode::OK
    );
    let order = |service: &TestState| {
        service
            .mux_store
            .get_workspace_state()
            .screen_order
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&native_state), vec!["C", "A", "B"]);
    assert_eq!(order(&native_state), order(&control_state));
    let removed =
        json!({"screenId":"C","op":{"type":"reorder_screens","screenOrder":["A","B","C"]}});
    assert_eq!(
        framed_mux_op(&native_state, removed.clone()).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(&control, api("POST", "/api/mux/op", Some(&removed))).await,
        StatusCode::BAD_REQUEST
    );
    async fn next_kind(observer: &mut crate::client_session::Connection, kind: &str) -> Value {
        loop {
            let event = tokio::time::timeout(WAIT, observer.output.recv())
                .await
                .expect("event within test bound")
                .expect("connected output");
            let crate::client_session::Message::Text(text) = event.value else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).expect("event JSON");
            if value["type"] == kind {
                return value;
            }
        }
    }
    let nudge = next_kind(&mut observer, "mux.nudge").await;
    assert!(nudge["screenId"].is_null());

    observer
        .input
        .send(crate::client_session::Message::Text(
            json!({"type":"mux.workspace_resync"}).to_string(),
        ))
        .expect("catalog request");
    let catalog = next_kind(&mut observer, "mux.workspace_snapshot").await;
    assert_eq!(catalog["screens"][0]["id"], "C");
    assert_eq!(catalog["screens"][1]["id"], "A");
    assert_eq!(catalog["screens"][2]["id"], "B");

    for (request, expected) in [
        (
            json!({"screenId":"C","op":{"type":"move_screen","beforeScreenId":null}}),
            vec!["A", "B", "C"],
        ),
        (
            json!({"screenId":"A","op":{"type":"move_screen"}}),
            vec!["B", "C", "A"],
        ),
    ] {
        assert_eq!(
            native(
                &native_state,
                Operation::MuxOp,
                json!({"body":request.clone()})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            exchange(&control, api("POST", "/api/mux/op", Some(&request)))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(order(&native_state), expected);
        assert_eq!(order(&native_state), order(&control_state));
        assert!(next_kind(&mut observer, "mux.nudge").await["screenId"].is_null());
    }

    let unchanged = json!({"screenId":"C","op":{"type":"move_screen","beforeScreenId":"A"}});
    assert_eq!(
        native(&native_state, Operation::MuxOp, json!({"body":unchanged}))
            .await
            .0,
        StatusCode::OK
    );
    while let Ok(event) = observer.output.try_recv() {
        if let crate::client_session::Message::Text(text) = event.value {
            let value: Value = serde_json::from_str(&text).expect("event JSON");
            assert_ne!(value["type"], "mux.nudge", "no-op has no change nudge");
        }
    }
    let before = serde_json::to_value(native_state.mux_store.get_workspace_state()).unwrap();
    for anchor in [
        json!(true),
        json!(17),
        json!([]),
        json!({}),
        json!("missing"),
    ] {
        let bad = json!({"screenId":"A","op":{"type":"move_screen","beforeScreenId":anchor}});
        assert_eq!(
            native(&native_state, Operation::MuxOp, json!({"body":bad.clone()}))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            exchange(&control, api("POST", "/api/mux/op", Some(&bad)))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            serde_json::to_value(native_state.mux_store.get_workspace_state()).unwrap(),
            before
        );
        while let Ok(event) = observer.output.try_recv() {
            if let crate::client_session::Message::Text(text) = event.value {
                let value: Value = serde_json::from_str(&text).expect("event JSON");
                assert_ne!(value["type"], "mux.nudge", "reject has no change nudge");
            }
        }
    }
    observer.shutdown().await;
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
    assert!(a_welcome.get("clientId").is_none());
    assert!(b_welcome.get("clientId").is_none());
    assert_eq!(state.client_state.clients.lock().expect("clients").len(), 2);
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
    let service = service_router(&state);
    let launch = json!({"requestId":splitty_core::ids::JobId::new(),"argv":["/bin/sh","-c","read line"],"cwd":dir.path(),"placement":{"kind":"screen"},"background":true});
    let (status, launched) = native(&state, Operation::JobLaunch, json!({"body":launch})).await;
    assert_eq!(status, StatusCode::CREATED, "{launched}");
    let (replayed_status, replayed) =
        exchange(&service, api("POST", "/api/jobs", Some(&launch))).await;
    assert_eq!(replayed_status, StatusCode::OK, "{replayed}");
    assert_eq!(launched["jobId"], replayed["jobId"]);
    assert_eq!(
        native(&state, Operation::JobList, json!({})).await,
        exchange(&service, api("GET", "/api/jobs", None)).await
    );
    let id = launched["jobId"].as_str().expect("job");
    assert_eq!(
        native(&state, Operation::JobInspect, json!({"id":id})).await,
        exchange(&service, api("GET", &format!("/api/jobs/{id}"), None)).await
    );
    let native_logs = native(
        &state,
        Operation::JobLogs,
        json!({"id":id,"waitMs":"31000"}),
    )
    .await;
    let control_logs = exchange(
        &service,
        api("GET", &format!("/api/jobs/{id}/logs?waitMs=31000"), None),
    )
    .await;
    assert_eq!(native_logs, control_logs);
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
            "remoteScreens.updateHost",
            json!({"target":"vm2","screens":[{"screenId":"s2"}],"closed":true}),
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
        "remote.directory.get",
        "remoteScreens.get",
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
async fn framed_manual_forward_preserves_paused_metadata_and_conflicting_listener() {
    async fn call(
        peer: &mut splitty_transport::peer::Peer,
        id: &mut u32,
        operation: &str,
        args: Value,
    ) -> (u16, Value) {
        *id += 1;
        framed_call(peer, *id, operation, args);
        let reply = framed_reply(peer).await;
        assert_eq!(reply.id, *id);
        let body = if reply.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&reply.body).unwrap()
        };
        (reply.status, body)
    }

    let dir = tempfile::tempdir().unwrap();
    let host = ScriptedHost::new(dir.path());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    let (mut peer, server) = framed(&state, Role::Main).await;
    let mut id = 0;
    let before = call(&mut peer, &mut id, "mux.state", json!({})).await;
    assert_eq!(before.0, 200);
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let occupied = busy.local_addr().unwrap().port();
    let key = json!({"target":"vm1","port":5173});
    let saved = call(
        &mut peer,
        &mut id,
        "forwardPreferences.save",
        json!({"body":{"target":"vm1","port":5173,"local":occupied,"label":"Frontend"}}),
    )
    .await;
    assert_eq!(saved.0, 200);
    assert_eq!(saved.1["enabled"], true);
    let paused = call(&mut peer, &mut id, "forward.stop", json!({"body":key})).await;
    assert_eq!(paused.0, 204, "stopping a saved-only rule is durable");
    for (body, label) in [
        (key.clone(), "Frontend"),
        (json!({"target":"vm1","port":5173,"label":"API"}), "API"),
        (json!({"target":"vm1","port":5173,"label":""}), ""),
    ] {
        let reply = call(
            &mut peer,
            &mut id,
            "forwardPreferences.save",
            json!({"body":body}),
        )
        .await;
        assert_eq!(reply.0, 200);
        assert_eq!(reply.1["local"], occupied);
        assert_eq!(reply.1["enabled"], false);
        assert_eq!(reply.1["label"], label);
    }
    assert!(
        host.asked().is_empty(),
        "metadata edits must not contact SSH"
    );
    let conflict = call(&mut peer, &mut id, "forward.open", json!({"body":key})).await;
    assert_eq!(conflict.0, 409);
    assert_eq!(conflict.1["code"], "local_port_conflict");
    assert!(
        host.asked().is_empty(),
        "a busy saved port must not start SSH"
    );
    let unchanged = call(&mut peer, &mut id, "forwardPreferences.get", json!({})).await;
    assert_eq!(unchanged.0, 200);
    assert_eq!(unchanged.1["forwards"][0]["local"], occupied);
    assert_eq!(unchanged.1["forwards"][0]["enabled"], false);

    let alternate = free_port();
    let opened = call(
        &mut peer,
        &mut id,
        "forward.open",
        json!({"body":{"target":"vm1","port":5173,"local":alternate}}),
    )
    .await;
    assert_eq!(opened.0, 200, "{}", opened.1);
    assert_eq!(opened.1["target"], "vm1");
    assert_eq!(opened.1["remote"], 5173);
    assert_eq!(opened.1["local"], alternate);
    assert_eq!(opened.1["state"], "up");
    assert_eq!(opened.1["url"], format!("http://localhost:{alternate}"));
    assert_eq!(
        tokio::task::spawn_blocking(move || through(alternate))
            .await
            .unwrap(),
        "forwarded 5173"
    );
    let resumed = call(&mut peer, &mut id, "forwardPreferences.get", json!({})).await;
    assert_eq!(resumed.0, 200);
    assert_eq!(resumed.1["forwards"][0]["local"], alternate);
    assert_eq!(resumed.1["forwards"][0]["enabled"], true);
    assert_eq!(resumed.1["forwards"][0]["label"], "");
    let after = call(&mut peer, &mut id, "mux.state", json!({})).await;
    assert_eq!(after.0, 200);
    assert_eq!(after.1, before.1, "manual forwarding leaves Screens alone");

    let forgotten = call(
        &mut peer,
        &mut id,
        "forwardPreferences.forget",
        json!({"body":key}),
    )
    .await;
    assert_eq!(forgotten.0, 204);
    assert_eq!(
        tokio::task::spawn_blocking(move || through(alternate))
            .await
            .unwrap(),
        "forwarded 5173",
        "Forget keeps the tunnel"
    );
    assert!(state.hub.forward_preferences().forwards.is_empty());
    // Remember is a separate metadata request after a confirmed opening.
    // Its storage failure must not undo the already usable, unsaved tunnel.
    let config = dir.path().join("config");
    let backup = dir.path().join("config-backup");
    std::fs::rename(&config, &backup).unwrap();
    std::fs::write(&config, b"owned storage failure fixture").unwrap();
    let failed_save = call(
        &mut peer,
        &mut id,
        "forwardPreferences.save",
        json!({"body":key}),
    )
    .await;
    assert_eq!(failed_save.0, 500);
    assert_eq!(failed_save.1["code"], "storage_failure");
    let still_live = call(&mut peer, &mut id, "forward.list", json!({})).await;
    assert_eq!(still_live.0, 200);
    assert_eq!(still_live.1, json!([opened.1]));
    assert!(state.hub.forward_preferences().forwards.is_empty());
    assert_eq!(
        tokio::task::spawn_blocking(move || through(alternate))
            .await
            .unwrap(),
        "forwarded 5173",
        "a failed Remember must not undo a confirmed forward"
    );
    std::fs::remove_file(&config).unwrap();
    std::fs::rename(&backup, &config).unwrap();
    let stopped = call(&mut peer, &mut id, "forward.stop", json!({"body":key})).await;
    assert_eq!(stopped.0, 204);
    assert!(!held(alternate));
    // Accept on the same owned listener to prove neither conflict recovery nor
    // Stop closed/replaced the competing service's socket.
    let competing_client = std::net::TcpStream::connect(("127.0.0.1", occupied)).unwrap();
    let (_, competing_peer) = busy.accept().unwrap();
    assert_eq!(competing_peer, competing_client.local_addr().unwrap());
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

#[tokio::test]
async fn template_extraction_matches_native_and_http_and_control_cannot_apply_or_extract() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let template = json!({"name":"T", "panes":{"a":{"command":"editor", "dir":"/project/src", "restore_mode":"close"}}, "layout":{"slot":"a"}});
    state
        .mux_store
        .dispatch(Op::ApplyTemplate {
            screen_id: "screen".into(),
            request: Box::new(
                serde_json::from_value(
                    json!({"template":template,"createScreen":true,"paneIds":{"a":"pane"}}),
                )
                .unwrap(),
            ),
        })
        .unwrap();
    state
        .mux_store
        .dispatch(Op::CreateScreen {
            name: None,
            screen_id: Some("empty".into()),
            new_pane_id: Some("gone".into()),
            shell_profile_id: None,
        })
        .unwrap();
    // A screen without a terminal: only a viewer pane is left on it.
    let viewer: ScreenOp = serde_json::from_value(json!({
        "type": "open_viewer", "paneId": "gone", "newPaneId": "viewer",
        "source": {"kind": "file", "path": "/project/PLAN.md"},
    }))
    .unwrap();
    for op in [
        viewer,
        ScreenOp::ClosePane {
            pane_id: "gone".into(),
            resulting_sizes: None,
        },
    ] {
        state
            .mux_store
            .dispatch_screen(&"empty".into(), op)
            .unwrap();
    }
    let service = service_router(&state);
    let owner = crate::control::build_control_router(state.service.clone());
    let (mut peer, task) = framed(&state, Role::Remote).await;
    for (id, screen_id, expected) in [(1, "screen", 200), (2, "missing", 404), (3, "empty", 400)] {
        let body = json!({"screenId":screen_id,"name":"Saved","root":"/project"});
        framed_call(&peer, id, "template.extract", json!({"body":body}));
        let native = framed_reply(&mut peer).await;
        let http = exchange(
            &service,
            api("POST", "/api/templates/extract/screen", Some(&body)),
        )
        .await;
        assert_eq!(native.status, expected);
        assert_eq!(native.status, http.0.as_u16());
        assert_eq!(serde_json::from_str::<Value>(&native.body).unwrap(), http.1);
        if expected == 200 {
            assert_eq!(http.1["panes"]["pane"]["dir"], "./src");
            assert_eq!(http.1["panes"]["pane"]["restore_mode"], "close");
        }
        // control-api, Template Extraction Endpoint (Window of an earlier
        // release): the deprecated operation answers what a current window
        // gets on the Screen's session connection.
        let request = format!("session-{id}");
        let mut message = body.clone();
        message["type"] = json!("mux.extract_template");
        message["requestId"] = json!(request);
        peer.sender
            .json(splitty_transport::wire::Kind::Text, &message)
            .unwrap();
        let session = next_text(&mut peer, "mux.template_extracted").await;
        assert_eq!(session["requestId"], request.as_str());
        match expected {
            200 => assert_eq!(session["template"], http.1),
            // The endpoint keeps its own text for a missing screen.
            404 => assert_eq!(session["error"], "screen missing not found"),
            _ => assert_eq!(session["error"], http.1["error"]),
        }
        assert_eq!(
            exchange(
                &owner,
                api("POST", "/api/templates/extract/screen", Some(&body))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let apply = json!({"screenId":"screen", "op":{"type":"apply_template", "template":template}});
    assert_eq!(
        exchange(&owner, api("POST", "/api/mux/op", Some(&apply)))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    framed_call(&peer, 4, "mux.op", json!({"body":apply}));
    assert_eq!(framed_reply(&mut peer).await.status, 400);
    let scoped = crate::control::jobs::routes(
        Arc::downgrade(&state.client_state.jobs),
        crate::jobs::Actor::Agent("restricted".into()),
    );
    for path in ["/api/mux/op", "/api/templates/extract/screen"] {
        assert_eq!(
            exchange(&scoped, api("POST", path, Some(&apply))).await.0,
            StatusCode::NOT_FOUND
        );
    }
    peer.shutdown().await;
    task.await.unwrap().unwrap();
}

/// templates, Saved Template Names (Same name from both windows, Confirmed
/// overwrite, Name without letters or digits): `POST /api/templates` and
/// native `template.create` answer alike, and the server names the file.
#[tokio::test]
async fn a_save_by_name_answers_alike_over_http_and_native() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let service = service_router(&state);
    let template = |name: &str| json!({"name": name, "panes": {"shell": {"command": "make"}}, "layout": {"slot": "shell"}});
    let save =
        |name: &str, overwrite: bool| json!({"template": template(name), "overwrite": overwrite});
    assert_eq!(
        exchange(
            &service,
            api("POST", "/api/templates", Some(&save("v1.2", false)))
        )
        .await,
        (StatusCode::OK, json!({"slug": "v1-2", "replaced": false}))
    );
    let taken = (
        StatusCode::CONFLICT,
        json!({"error": "template exists", "slug": "v1-2"}),
    );
    assert_eq!(
        native(
            &state,
            Operation::TemplateCreate,
            json!({"body": save("V1 2", false)})
        )
        .await,
        taken
    );
    assert_eq!(
        exchange(
            &service,
            api("POST", "/api/templates", Some(&save("V1 2", false)))
        )
        .await,
        taken
    );
    assert_eq!(
        exchange(&service, api("GET", "/api/templates/v1-2", None))
            .await
            .1["name"],
        "v1.2",
        "a refused save changes nothing"
    );
    for (channel, answer) in [
        (
            "native",
            native(
                &state,
                Operation::TemplateCreate,
                json!({"body": save("V1 2", true)}),
            )
            .await,
        ),
        (
            "HTTP",
            exchange(
                &service,
                api("POST", "/api/templates", Some(&save("V1 2", true))),
            )
            .await,
        ),
    ] {
        assert_eq!(
            answer,
            (StatusCode::OK, json!({"slug": "v1-2", "replaced": true})),
            "{channel}"
        );
    }
    let unnamed = native(
        &state,
        Operation::TemplateCreate,
        json!({"body": save("...", false)}),
    )
    .await;
    assert_eq!(unnamed.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        unnamed,
        exchange(
            &service,
            api("POST", "/api/templates", Some(&save("...", false)))
        )
        .await
    );
    let (_, list) = exchange(&service, api("GET", "/api/templates", None)).await;
    assert_eq!(list, json!([{"slug": "v1-2", "displayName": "V1 2"}]));
}

/// access-control, Control Socket Route Set: the owner control socket routes a
/// catalog operation exactly when its scope is `Control`; the job operations
/// come with the job router it shares with the agent grant sockets.
#[tokio::test]
async fn the_control_socket_routes_exactly_the_catalogs_control_operations() {
    use crate::service::catalog::{Scope, OPERATIONS};
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let control = crate::control::build_control_router(state.service.clone());
    for entry in OPERATIONS {
        if entry.name.starts_with("job.") {
            continue;
        }
        let path = entry.path.replace("{id}", "x").replace("{slug}", "x");
        // No content type and an undefined query argument: a routed operation
        // refuses it, an unrouted path is not found.
        let request = Request::builder()
            .method(entry.method)
            .uri(format!("{path}?undefined=1"))
            .body(Body::empty())
            .expect("request");
        let expected = match entry.scope {
            Scope::Control if entry.takes_body() => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Scope::Control => StatusCode::BAD_REQUEST,
            Scope::Backend | Scope::LocalHub => StatusCode::NOT_FOUND,
        };
        assert_eq!(status(&control, request).await, expected, "{}", entry.name);
    }
}

/// control-api, Two Control Transports (Malformed request on both
/// transports), and access-control, JSON Request Bodies: the control socket
/// decoder refuses malformed requests and non-JSON content types before
/// changing the workspace.
#[tokio::test]
async fn malformed_control_requests_leave_the_workspace_unchanged() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
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
    let control = crate::control::build_control_router(state.service.clone());
    let request = |method: &str, path: &str, content_type: Option<&str>, body: &str| {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", ADDR);
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        request.body(Body::from(body.to_owned())).expect("request")
    };
    let workspace = || serde_json::to_value(state.mux_store.get_workspace_state()).expect("state");
    let before = workspace();
    let send = format!("/api/session/{session}/send");
    let json = Some("application/json");
    for (method, path, content_type, body, expected) in [
        (
            "POST",
            "/api/mux/op",
            json,
            r#"{"op":{"type":"close_screen"}}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "POST",
            "/api/mux/op",
            json,
            "{",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "GET",
            "/api/mux/neighbor?undefined=1",
            None,
            "",
            StatusCode::BAD_REQUEST,
        ),
        (
            "POST",
            "/api/mux/op",
            Some("text/plain"),
            r#"{"screenId":"s1","op":{"type":"close_screen","screenId":"s1"}}"#,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "POST",
            send.as_str(),
            None,
            r#"{"data":"printf leaked\n"}"#,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "POST",
            "/api/viewer/comments/reply",
            Some("application/json; charset=utf-8"),
            r#"{"screenId":"s1"}"#,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let over_socket = exchange(&control, request(method, path, content_type, body)).await;
        assert_eq!(over_socket.0, expected, "{method} {path}: {over_socket:?}");
        assert!(over_socket.1["error"].is_string(), "{over_socket:?}");
    }
    assert_eq!(workspace(), before, "no request changed the workspace");
}

/// Count arguments stay decimal strings on the native wire and an operation
/// reports its own decoding errors in its style: job operations as
/// `invalid_request`, viewer operations as `invalid`, others as 422.
#[tokio::test]
async fn native_arguments_and_bodies_are_decoded_in_the_operations_error_style() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    assert_eq!(
        native(&state, Operation::JobLogs, json!({"id":"job-1","tail":"x"})).await,
        (
            StatusCode::BAD_REQUEST,
            json!({"code":"invalid_request","error":"invalid tail"})
        )
    );
    let (status, body) = native(&state, Operation::JobLogs, json!({"id":"job-1","limit":3})).await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"unsupported or malformed native request: limit must be a string"})
        )
    );
    let (status, body) = native(&state, Operation::JobLaunch, json!({"body":{}})).await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid_request"))
    );
    let (status, body) = native(
        &state,
        Operation::ViewerReviewWait,
        json!({"body":{"screenId":"s1"}}),
    )
    .await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid"))
    );
    assert!(body["error"]
        .as_str()
        .expect("error")
        .starts_with("invalid request: "));
    let (status, body) = native(&state, Operation::MuxOp, json!({"body":{"op":{}}})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"].is_string() && body.get("code").is_none(),
        "{body}"
    );
}

#[tokio::test]
async fn daemon_endpoint_rejects_host_intent_in_the_hello_without_starting_ssh() {
    use splitty_transport::{
        codec::{self, Frame},
        wire::{self, Kind},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let dir = tempfile::tempdir().unwrap();
    let mut state = state(ADDR, dir.path());
    let host = ScriptedHost::new(dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(hub(&host, dir.path()));
    let endpoint = dir.path().join("client.sock");
    let native = Native::start(&state, &endpoint).await;
    let mut stream = crate::native::endpoint::connect(&endpoint).await.unwrap();
    stream.write_all(wire::MAGIC).await.unwrap();
    let hello = Frame::json(
        Kind::Hello,
        &json!({
            "transportVersion": wire::VERSION,
            "openHost": {"ownerInstance":state.instance,"target":"vm1","update":false}
        }),
    )
    .unwrap();
    codec::write(&mut stream, &hello).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.is_empty(), "no acceptance frame");
    assert!(host.asked().is_empty(), "no SSH preparation");
    native.stop().await;
}

#[tokio::test]
async fn transport_worker_exhaustion_is_busy_in_both_control_and_native_reply_shapes() {
    use crate::hub::{ForwardError, HubError};
    use splitty_remote::{ssh::SshError, tunnel::PortTunnelError};
    for error in [
        HubError::Capacity("process ceiling".into()),
        HubError::NativeBridge(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
        HubError::Forward(PortTunnelError::Ssh(SshError::Spawn(std::io::Error::from(
            std::io::ErrorKind::WouldBlock,
        )))),
    ] {
        let reply = crate::service::operations::forward_failed("vm", ForwardError::Failed(error));
        assert_eq!(reply.status, 503);
        assert_eq!(
            serde_json::from_slice::<Value>(&reply.body).unwrap()["code"],
            "busy"
        );
        let response = reply.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            serde_json::from_slice::<Value>(
                &axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap()
            )
            .unwrap()["code"],
            "busy"
        );
    }
}

pub(super) struct Native {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Native {
    pub(super) async fn start(state: &TestState, path: &Path) -> Self {
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let service = state.service.clone();
        let task = tokio::spawn(crate::native::listen(listener, service, async {
            let _ = stopped.await;
        }));
        Self { stop, task }
    }
    pub(super) async fn stop(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap().unwrap();
    }
}

/// control-api: native build, host listing and command delivery remain available
/// independently of the control socket's restricted routes.
#[tokio::test]
async fn native_build_hosts_and_commands_keep_their_response_and_delivery_contract() {
    let dir = tempfile::tempdir().expect("fixture");
    let state = state(ADDR, dir.path());
    let (status, build) = native(&state, Operation::BuildGet, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(build["version"], "test");
    for field in ["commit", "buildTime"] {
        assert!(build[field].is_string(), "{build}");
    }
    splitty_remote::hosts::record_recent(
        &dir.path().join("config"),
        &splitty_remote::ssh::SshTarget::parse("vm1").unwrap(),
    )
    .expect("recent host");
    let (status, hosts) = native(&state, Operation::HostDirectory, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        hosts["hosts"]
            .as_array()
            .expect("hosts")
            .iter()
            .any(|host| host["target"] == "vm1" && host["source"] == "recent"),
        "{hosts}"
    );
    let command = json!({"body":{"type":"pane.split","direction":"right"}});
    assert_eq!(
        native(&state, Operation::CommandSend, command.clone()).await,
        (StatusCode::OK, json!({"ok":true,"sent":0}))
    );
    let before = state.mux_store.get_workspace_state();
    let mut client =
        Socket::Native(crate::client_session::connect(state.client_state.clone()).expect("client"));
    assert_eq!(json_of(next(&mut client).await)["type"], "mux.welcome");
    assert_eq!(
        native(&state, Operation::CommandSend, command.clone()).await,
        (StatusCode::OK, json!({"ok":true,"sent":1}))
    );
    let event = tokio::time::timeout(WAIT, async {
        loop {
            let event = json_of(next(&mut client).await);
            if event["type"] == "command" {
                break event;
            }
        }
    })
    .await
    .expect("command delivery");
    assert_eq!(event["command"], command["body"]);
    assert!(event["id"].is_string());
    assert!(event["timestamp"].is_number());
    let (status, _) = native(
        &state,
        Operation::CommandSend,
        json!({"body":{"type":"pane.explode"}}),
    )
    .await;
    assert!(status.is_client_error());
    assert_eq!(
        serde_json::to_value(state.mux_store.get_workspace_state()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    client.close().await.expect("close");
}
