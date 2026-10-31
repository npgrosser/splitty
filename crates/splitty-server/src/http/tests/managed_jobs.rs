use super::*;
use crate::jobs::Actor;
use splitty_core::ids::{AgentId, JobId};
use splitty_core::pty::testing::{pty_available, wait_until};

fn job_state(dir: &Path) -> AppState {
    let mut state = state(ADDR, dir);
    let old = &state.ws_state;
    let (events, _) = tokio::sync::broadcast::channel(256);
    let ws = crate::ws::WsState::new(
        state.mux_store.clone(),
        old.dispatcher.clone(),
        state.shell_profiles.clone(),
        old.effect_runner.clone(),
        splitty_core::mux::effect_runner::EnvContext {
            control_socket: dir.join("run/owner.sock").to_string_lossy().into_owned(),
        },
        events,
        "embedded".into(),
    );
    state.ws_state = Arc::new(ws);
    *state.shell_profiles.write().expect("profiles") =
        splitty_core::shell_profiles::state_from_settings(&Default::default());
    state
}
fn launch(dir: &Path, script: &str) -> serde_json::Value {
    json!({"requestId":JobId::new(),"argv":["/bin/sh","-c",script],"cwd":dir,"placement":{"kind":"screen"},"background":true})
}
async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    exchange(router, api(method, path, body)).await
}

#[tokio::test]
async fn managed_jobs_routes_enforce_transport_scope_and_payload_bounds() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let state = job_state(dir.path());
    let tcp = super::super::build_router(state.clone());
    let owner = super::super::build_control_router(state.clone());
    let (status, grant) = call(
        &owner,
        "POST",
        "/api/agents",
        Some(&json!({"name":"agent"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{grant}");
    let id = AgentId::from(grant["agentId"].as_str().expect("grant"));
    let scoped: axum::Router = super::super::jobs::routes(
        Arc::downgrade(&state.ws_state.jobs),
        Actor::Agent(id.clone()),
    );
    for path in [
        "/api/mux/state",
        "/api/session/session/send",
        "/api/bootstrap",
        "/api/ws-ticket",
        "/api/agents",
        "/ws",
    ] {
        let method =
            if path.contains("send") || path.contains("bootstrap") || path.contains("ticket") {
                "POST"
            } else {
                "GET"
            };
        assert_eq!(
            call(&scoped, method, path, Some(&json!({}))).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        call(
            &tcp,
            "POST",
            "/api/agents",
            Some(&json!({"name":"forbidden"}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let request = Request::get("/api/jobs")
        .header("host", ADDR)
        .body(Body::empty())
        .expect("anonymous");
    assert_eq!(exchange(&tcp, request).await.0, StatusCode::UNAUTHORIZED);
    let mut request = api("GET", "/api/jobs", None);
    request
        .headers_mut()
        .insert("origin", "https://example.invalid".parse().expect("header"));
    assert_eq!(exchange(&tcp, request).await.0, StatusCode::FORBIDDEN);
    let mut bad = launch(dir.path(), "exit 0");
    bad["owner"] = json!("forged");
    assert_eq!(
        call(&scoped, "POST", "/api/jobs", Some(&bad)).await.0,
        StatusCode::BAD_REQUEST
    );
    bad = launch(dir.path(), "exit 0");
    bad["argv"] = json!(["nul\0program"]);
    assert_eq!(
        call(&scoped, "POST", "/api/jobs", Some(&bad)).await.0,
        StatusCode::BAD_REQUEST
    );
    bad = launch(dir.path(), "exit 0");
    bad["cwd"] = json!(dir.path().join("missing"));
    assert_eq!(
        call(&scoped, "POST", "/api/jobs", Some(&bad)).await.0,
        StatusCode::BAD_REQUEST
    );
    bad = launch(dir.path(), "exit 0");
    bad["name"] = json!("x".repeat(201));
    assert_eq!(
        call(&scoped, "POST", "/api/jobs", Some(&bad)).await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, job) = call(
        &scoped,
        "POST",
        "/api/jobs",
        Some(&launch(
            dir.path(),
            "printf READY; read line; printf DONE; exit 23",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{job}");
    let job_id = job["jobId"].as_str().expect("job");
    let base = format!("/api/jobs/{job_id}");
    assert_eq!(
        call(
            &scoped,
            "POST",
            &format!("{base}/input"),
            Some(&json!({"data":"x".repeat(16385)}))
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    for query in [
        "limit=0",
        "limit=999999",
        "cursor=wrong",
        "waitMs=30001",
        "format=raw&tail=1",
        "tail=10001",
    ] {
        assert_eq!(
            call(&scoped, "GET", &format!("{base}/logs?{query}"), None)
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    let (status, _) = call(
        &scoped,
        "POST",
        &format!("{base}/input"),
        Some(&json!({"data":"finish\n"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let key = JobId::from(job_id);
    assert!(
        wait_until(WAIT, || state.mux_store.job(&key).is_some_and(
            |j| j.state == splitty_core::mux::jobs::JobState::Exited
        ))
        .await
    );
    assert_eq!(call(&scoped, "GET", &base, None).await.1["exitCode"], 23);
    assert_eq!(
        call(
            &scoped,
            "POST",
            &format!("{base}/input"),
            Some(&json!({"data":"again"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let logs = call(&scoped, "GET", &format!("{base}/logs"), None).await.1;
    assert!(logs["data"].as_str().expect("text").contains("DONE"));
    let list = call(&scoped, "GET", "/api/jobs", None).await.1;
    assert_eq!(list["jobs"].as_array().expect("list").len(), 1);
    assert!(list["capabilities"]
        .as_array()
        .expect("caps")
        .contains(&json!("managed-jobs-v1")));
    let raw = call(&scoped, "GET", &format!("{base}/logs?format=raw"), None)
        .await
        .1;
    assert_eq!(raw["encoding"], "base64");
    state.ws_state.jobs.shutdown().await;
    assert!(!Path::new(grant["socket"].as_str().expect("socket")).exists());
}

#[tokio::test]
async fn managed_job_spawn_events_and_mux_queries_use_actual_ids_and_outcomes() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let state = job_state(dir.path());
    let app = super::super::build_control_router(state.clone());
    let (_, ordinary) = call(
        &app,
        "POST",
        "/api/mux/op",
        Some(&json!({"screenId":"","op":{"type":"create_screen","name":"user"}})),
    )
    .await;
    assert_eq!(ordinary["revision"], 1);
    assert!(ordinary["screenId"].as_str().is_some_and(|s| !s.is_empty()));
    let mut events = state.ws_state.notify_tx.subscribe();
    let request = launch(dir.path(), "printf END; exit 7");
    let (status, job) = call(&app, "POST", "/api/jobs", Some(&request)).await;
    assert_eq!(status, StatusCode::CREATED);
    let mut wire = Vec::new();
    while let Ok(event) = events.try_recv() {
        wire.push(serde_json::from_str::<serde_json::Value>(&event).expect("event"));
    }
    assert_eq!(wire[0]["type"], "mux.pane_spawned");
    assert_eq!(wire[1]["type"], "mux.nudge");
    assert_eq!(wire[1]["screenId"], job["screenId"]);
    let mut op = request.clone();
    op["type"] = json!("create_job");
    let (status, replay) = call(
        &app,
        "POST",
        "/api/mux/op",
        Some(&json!({"screenId":"","op":op})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["jobId"], job["jobId"]);
    let mut failed = launch(dir.path(), "");
    failed["argv"] = json!(["/no/such/program-for-splitty-test"]);
    let (status, failure) = call(&app, "POST", "/api/jobs", Some(&failed)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failure["code"], "spawn_failed");
    let snapshot = call(
        &app,
        "GET",
        &format!(
            "/api/mux/screen/{}",
            failure["screenId"].as_str().expect("screen")
        ),
        None,
    )
    .await
    .1;
    assert_eq!(
        snapshot["jobs"][failure["jobId"].as_str().expect("job")]["state"],
        "failed"
    );
    state.ws_state.jobs.shutdown().await;
    state.ws_state.dispatcher.backend().close_all();
}

#[tokio::test]
async fn managed_grant_socket_is_private_resumable_and_never_promotes_to_owner() {
    if !pty_available() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("fixture");
    let state = job_state(dir.path());
    let grant =
        crate::control::spawn_grant(&state.ws_state.jobs, "agent".into(), None).expect("grant");
    let path = grant["socket"].as_str().expect("socket");
    assert_eq!(
        std::fs::metadata(path)
            .expect("socket mode")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(Path::new(path).parent().expect("parent"))
            .expect("dir mode")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let server = crate::cli::ServerInfo {
        transport: crate::cli::Transport::LocalSocket(path.into()),
        source: "agent",
    };
    assert_eq!(
        crate::cli::request(
            &server,
            "POST",
            "/api/editor/open",
            Some(json!({"sessionId":"s","path":"/tmp"}))
        )
        .await
        .expect("editor route denied")
        .0,
        404
    );
    // access-control, File offer on the owner socket: not on a grant socket.
    assert_eq!(
        crate::cli::request(
            &server,
            "POST",
            "/api/file/offer",
            Some(json!({"sessionId":"s","path":"/tmp/x"}))
        )
        .await
        .expect("file offer route denied")
        .0,
        404
    );
    let (status, list) = crate::cli::request(&server, "GET", "/api/jobs", None)
        .await
        .expect("first client");
    assert_eq!(status, 200);
    assert_eq!(list["jobs"], json!([]));
    assert_eq!(
        crate::cli::request(&server, "GET", "/api/jobs", None)
            .await
            .expect("fresh client")
            .0,
        200
    );
    assert_eq!(
        crate::cli::request(&server, "GET", "/api/mux/state", None)
            .await
            .expect("route denied")
            .0,
        404
    );
    state
        .ws_state
        .jobs
        .revoke(&AgentId::from(grant["agentId"].as_str().expect("id")))
        .await
        .expect("revoke");
    assert!(!Path::new(path).exists());
    assert!(crate::cli::request(&server, "GET", "/api/jobs", None)
        .await
        .is_err());
    state.ws_state.jobs.shutdown().await;
}

#[tokio::test]
async fn invalid_launches_and_rejected_closes_have_no_effects() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let state = job_state(dir.path());
    let app = super::super::build_control_router(state.clone());
    let base = launch(dir.path(), "read line");
    for (field, value) in [
        ("argv", json!([])),
        ("argv", json!([""])),
        ("argv", json!(vec!["arg"; 257])),
        ("argv", json!(["x".repeat(65537)])),
        ("cwd", json!(format!("/{}", "x".repeat(4096)))),
        ("newPaneId", json!("nul\0id")),
        ("background", json!(false)),
        ("placement", json!({"kind":"screen","direction":"left"})),
        ("placement", json!({"kind":"tab","screenId":"s"})),
    ] {
        let mut bad = base.clone();
        bad[field] = value;
        let (status, error) = call(&app, "POST", "/api/jobs", Some(&bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {error}");
        assert_eq!(error["code"], "invalid_request");
    }
    let malformed = Request::post("/api/jobs")
        .header("content-type", "application/json")
        .body(Body::from("{not json"))
        .expect("request");
    assert_eq!(exchange(&app, malformed).await.0, StatusCode::BAD_REQUEST);
    let oversized = Request::post("/api/jobs")
        .header("content-type", "application/json")
        .body(Body::from(" ".repeat(128 * 1024 + 1)))
        .expect("request");
    let (status, error) = exchange(&app, oversized).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error["code"], "invalid_request");
    assert!(state.mux_store.jobs().is_empty());
    assert!(state.ws_state.dispatcher.backend().sessions().is_empty());
    let (_, job) = call(&app, "POST", "/api/jobs", Some(&base)).await;
    let id = JobId::from(job["jobId"].as_str().expect("job"));
    let before = serde_json::to_value(state.mux_store.get_workspace_state()).expect("state");
    let mut events = state.ws_state.notify_tx.subscribe();
    let (status, _) = call(
        &app,
        "POST",
        "/api/mux/op",
        Some(&json!({"screenId":"wrong-screen","op":{"type":"close_pane","paneId":job["paneId"]}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        serde_json::to_value(state.mux_store.get_workspace_state()).expect("state"),
        before
    );
    assert!(events.try_recv().is_err());
    assert_eq!(
        state.mux_store.job(&id).expect("job").state,
        splitty_core::mux::jobs::JobState::Running
    );
    call(
        &app,
        "POST",
        "/api/mux/op",
        Some(
            &json!({"screenId":job["screenId"],"op":{"type":"close_pane","paneId":job["paneId"]}}),
        ),
    )
    .await;
    assert_eq!(
        state.mux_store.job(&id).expect("job").state,
        splitty_core::mux::jobs::JobState::Exited
    );
    state.ws_state.jobs.shutdown().await;
}

#[tokio::test]
async fn log_handlers_page_independently_report_loss_and_keep_draining_output() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let state = job_state(dir.path());
    let app = super::super::build_control_router(state.clone());
    let (_,job)=call(&app,"POST","/api/jobs",Some(&launch(dir.path(),"stty -echo; printf READY; read line; head -c 1100000 /dev/zero | tr '\\000' x; printf '\\nTAIL\\n'; read line"))).await;
    let id = job["jobId"].as_str().expect("id");
    let url = format!("/api/jobs/{id}/logs");
    let (_, first) = call(&app, "GET", &format!("{url}?waitMs=5000"), None).await;
    assert_eq!(first["data"], "READY");
    let old = first["nextCursor"].as_str().expect("cursor");
    call(
        &app,
        "POST",
        &format!("/api/jobs/{id}/input"),
        Some(&json!({"data":"go\n"})),
    )
    .await;
    let session = state
        .ws_state
        .dispatcher
        .backend()
        .get(job["sessionId"].as_str().expect("session"))
        .expect("live");
    assert!(wait_until(WAIT, || session.scrollback().ends_with(b"TAIL\r\n")).await);
    let (_, a) = call(&app, "GET", &format!("{url}?cursor={old}&limit=64"), None).await;
    let (_, b) = call(&app, "GET", &format!("{url}?cursor={old}&limit=64"), None).await;
    assert_eq!(a, b);
    assert_eq!(a["data"].as_str().expect("data").len(), 64);
    assert_eq!(a["hasMore"], true);
    assert!(a["lostBytes"].as_u64().expect("loss") > 0);
    let (_, next) = call(
        &app,
        "GET",
        &format!(
            "{url}?cursor={}&limit=64",
            a["nextCursor"].as_str().expect("next")
        ),
        None,
    )
    .await;
    assert_eq!(next["lostBytes"], 0);
    assert_ne!(next["nextCursor"], a["nextCursor"]);
    let (_, tail) = call(&app, "GET", &format!("{url}?tail=1"), None).await;
    assert_eq!(tail["data"], "TAIL\n");
    assert_eq!(
        call(&app, "GET", &format!("{url}?format=raw&cursor={old}"), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, raw) = call(&app, "GET", &format!("{url}?format=raw&limit=64"), None).await;
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(raw["data"].as_str().expect("base64"))
            .expect("decode"),
        vec![b'x'; 64]
    );
    let cursor = tail["nextCursor"].as_str().expect("end");
    let started = tokio::time::Instant::now();
    let (_, empty) = call(
        &app,
        "GET",
        &format!("{url}?cursor={cursor}&waitMs=10"),
        None,
    )
    .await;
    assert_eq!(empty["data"], "");
    assert!(!empty["outputComplete"].as_bool().expect("completion"));
    assert!(started.elapsed() >= Duration::from_millis(10));
    state.ws_state.jobs.shutdown().await;
}

#[tokio::test]
async fn grants_filter_every_route_and_anchors_never_grant_terminal_access() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let state = job_state(dir.path());
    let app = super::super::build_control_router(state.clone());
    let (_, anchor) = call(
        &app,
        "POST",
        "/api/jobs",
        Some(&launch(dir.path(), "printf PRIVATE_ANCHOR; read line")),
    )
    .await;
    let grant = crate::control::spawn_grant(
        &state.ws_state.jobs,
        "same-label".into(),
        Some(crate::jobs::Anchor {
            screen_id: anchor["screenId"].as_str().expect("screen").into(),
            pane_id: anchor["paneId"].as_str().expect("pane").into(),
        }),
    )
    .expect("grant");
    let other = crate::control::spawn_grant(&state.ws_state.jobs, "same-label".into(), None)
        .expect("other grant");
    let actor = Actor::Agent(grant["agentId"].as_str().expect("grant").into());
    let scoped: axum::Router =
        super::super::jobs::routes(Arc::downgrade(&state.ws_state.jobs), actor);
    let second: axum::Router = super::super::jobs::routes(
        Arc::downgrade(&state.ws_state.jobs),
        Actor::Agent(other["agentId"].as_str().expect("grant").into()),
    );
    for router in [&scoped, &second] {
        for (method, suffix) in [
            ("GET", ""),
            ("GET", "/logs"),
            ("POST", "/input"),
            ("POST", "/stop"),
            ("POST", "/forget"),
        ] {
            let (status, error) = call(
                router,
                method,
                &format!(
                    "/api/jobs/{}{suffix}",
                    anchor["jobId"].as_str().expect("anchor")
                ),
                Some(&json!({"data":"forbidden"})),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(error.get("jobId").is_none());
        }
        assert_eq!(
            call(router, "GET", "/api/jobs", None).await.1["jobs"],
            json!([])
        );
    }
    let mut request = launch(dir.path(), "printf PUBLIC_JOB; read line");
    request["placement"] = json!({"kind":"tab"});
    let (status, job) = call(&scoped, "POST", "/api/jobs", Some(&request)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(job["screenId"], anchor["screenId"]);
    assert_eq!(
        call(&scoped, "GET", "/api/jobs", None).await.1["jobs"]
            .as_array()
            .expect("list")
            .len(),
        1
    );
    let id = job["jobId"].as_str().expect("id");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/jobs/{id}/owner"),
            Some(&json!({"agentId":other["agentId"]}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&scoped, "POST", "/api/jobs", Some(&request)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&second, "GET", &format!("/api/jobs/{id}"), None)
            .await
            .1["sessionId"],
        job["sessionId"]
    );
    call(&app,"POST","/api/mux/op",Some(&json!({"screenId":anchor["screenId"],"op":{"type":"close_pane","paneId":anchor["paneId"]}}))).await;
    request["requestId"] = json!(JobId::new());
    assert_eq!(
        call(&scoped, "POST", "/api/jobs", Some(&request)).await.0,
        StatusCode::NOT_FOUND
    );
    request["placement"] = json!({"kind":"screen"});
    assert_eq!(
        call(&scoped, "POST", "/api/jobs", Some(&request)).await.0,
        StatusCode::CREATED
    );
    state.ws_state.jobs.shutdown().await;
}

#[tokio::test]
async fn grant_bind_failures_rollback_and_shared_services_restart_empty() {
    let dir = tempfile::tempdir().expect("fixture");
    let too_long = dir.path().join("x".repeat(100));
    let state = job_state(&too_long);
    assert!(crate::control::spawn_grant(&state.ws_state.jobs, "long".into(), None).is_err());
    assert_eq!(state.ws_state.jobs.grants()["agents"], json!([]));
    let state = job_state(dir.path());
    assert!(crate::control::spawn_grant(
        &state.ws_state.jobs,
        "bad".into(),
        Some(crate::jobs::Anchor {
            screen_id: "missing".into(),
            pane_id: "missing".into()
        })
    )
    .is_err());
    assert_eq!(
        std::fs::read_dir(dir.path().join("run"))
            .expect("runtime")
            .count(),
        0
    );
    let grant =
        crate::control::spawn_grant(&state.ws_state.jobs, "lifetime".into(), None).expect("grant");
    let old_path = grant["socket"].as_str().expect("path");
    state.ws_state.jobs.shutdown().await;
    assert!(!Path::new(old_path).exists());
    drop(state);
    let replacement = job_state(dir.path());
    let new = crate::control::spawn_grant(&replacement.ws_state.jobs, "lifetime".into(), None)
        .expect("new grant");
    assert_ne!(grant["agentId"], new["agentId"]);
    assert_ne!(grant["socket"], new["socket"]);
    assert!(replacement.mux_store.jobs().is_empty());
    assert!(tokio::net::UnixStream::connect(old_path).await.is_err());
    replacement.ws_state.jobs.shutdown().await;
}

#[tokio::test]
async fn disconnected_router_reads_release_quota_without_stopping_the_job() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let state = job_state(dir.path());
    let app = super::super::build_control_router(state.clone());
    let (_, job) = call(
        &app,
        "POST",
        "/api/jobs",
        Some(&launch(dir.path(), "read line")),
    )
    .await;
    let router = app.clone();
    let id = job["jobId"].as_str().expect("id").to_owned();
    let read = tokio::spawn(async move {
        call(
            &router,
            "GET",
            &format!("/api/jobs/{id}/logs?waitMs=30000"),
            None,
        )
        .await
    });
    assert!(wait_until(WAIT, || state.ws_state.jobs.waiting_reads() == 1).await);
    read.abort();
    let _ = read.await;
    assert_eq!(state.ws_state.jobs.waiting_reads(), 0);
    assert_eq!(
        state
            .mux_store
            .job(&job["jobId"].as_str().expect("id").into())
            .expect("job")
            .state,
        splitty_core::mux::jobs::JobState::Running
    );
    state.ws_state.jobs.shutdown().await;
}

#[tokio::test]
async fn websocket_job_spawns_precede_the_next_resync_and_failures_name_allocated_ids() {
    if !pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start_with(hub(&host, dir.path()), |addr| {
        let mut state = job_state(dir.path());
        state.allowed_origins = Arc::new(allowed_origins(addr));
        state.listen_addr = addr.into();
        state
    })
    .await;
    let mut socket = served.open_local().await;
    let welcome = json_of(next(&mut socket).await);
    assert!(welcome["capabilities"]
        .as_array()
        .expect("capabilities")
        .contains(&json!("managed-jobs-v1")));
    let owner = super::super::build_control_router(served.state.clone());
    assert_eq!(
        call(&owner, "GET", "/api/mux/status", None).await.1["capabilities"],
        json!(["managed-jobs-v1"])
    );
    let mut request = launch(dir.path(), "read line");
    request["newScreenId"] = json!("ordered-job-screen");
    request["type"] = json!("create_job");
    socket
        .send(Message::Text(
            json!({"type":"mux.op","screenId":"","op":request})
                .to_string()
                .into(),
        ))
        .await
        .expect("launch");
    socket
        .send(Message::Text(
            json!({"type":"mux.resync","screenId":"ordered-job-screen"})
                .to_string()
                .into(),
        ))
        .await
        .expect("immediate resync");
    let mut kinds = Vec::new();
    loop {
        let message = next(&mut socket).await;
        if !message.is_text() {
            continue;
        }
        let event = json_of(message);
        if event["type"]
            .as_str()
            .is_some_and(|s| s.starts_with("mux."))
        {
            kinds.push(event["type"].as_str().expect("type").to_owned());
        }
        if event["type"] == "mux.screen_snapshot" {
            break;
        }
    }
    assert_eq!(
        kinds,
        ["mux.pane_spawned", "mux.nudge", "mux.screen_snapshot"]
    );
    let mut failed = launch(dir.path(), "");
    failed["argv"] = json!(["/no/such/splitty-program"]);
    failed["type"] = json!("create_job");
    socket
        .send(Message::Text(
            json!({"type":"mux.op","screenId":"","op":failed})
                .to_string()
                .into(),
        ))
        .await
        .expect("failed start");
    loop {
        let message = next(&mut socket).await;
        if !message.is_text() {
            continue;
        }
        let event = json_of(message);
        assert_ne!(event["type"], "mux.pane_spawned");
        if event["type"] == "mux.op_failed" {
            assert!(event["screenId"].as_str().is_some_and(|s| !s.is_empty()));
            assert!(event["paneId"].as_str().is_some_and(|s| !s.is_empty()));
            break;
        }
    }
    let (status, ordinary) = call(
        &owner,
        "POST",
        "/api/mux/op",
        Some(&json!({"screenId":"ignored-envelope","op":{"type":"create_screen"}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ordinary["revision"], 1);
    let first = loop {
        let message = next(&mut socket).await;
        if message.is_text() {
            break json_of(message);
        }
    };
    assert_eq!(first["type"], "mux.pane_spawned");
    let nudge = json_of(next(&mut socket).await);
    assert_eq!(nudge["screenId"], ordinary["screenId"]);
    assert_eq!(nudge["reason"], "cli_op");
    let (_,split)=call(&owner,"POST","/api/mux/op",Some(&json!({"screenId":ordinary["screenId"],"op":{"type":"split_pane","paneId":ordinary["created"][0]["paneId"],"axis":"horizontal"}}))).await;
    assert_eq!(split["revision"], 2);
    assert_eq!(split["created"].as_array().expect("mapping").len(), 1);
    assert_eq!(
        json_of(next(&mut socket).await)["paneId"],
        split["created"][0]["paneId"]
    );
    assert_eq!(json_of(next(&mut socket).await)["type"], "mux.nudge");
    call(&owner,"POST","/api/mux/op",Some(&json!({"screenId":ordinary["screenId"],"op":{"type":"rename_screen","name":"Renamed"}}))).await;
    assert_eq!(json_of(next(&mut socket).await)["type"], "mux.nudge");
    assert_eq!(
        served
            .state
            .mux_store
            .get_screen(&ordinary["screenId"].as_str().expect("screen").into())
            .expect("screen")
            .name
            .as_deref(),
        Some("Renamed")
    );
    let profile = {
        let mut profiles = served.state.shell_profiles.write().expect("profiles");
        let profile = profiles
            .profiles
            .iter_mut()
            .find(|p| p.enabled && p.available)
            .expect("default profile");
        profile.command = "/no/such/ordinary-test-shell".into();
        profile.id.clone()
    };
    let (status, failed) = call(
        &owner,
        "POST",
        "/api/mux/op",
        Some(&json!({"screenId":"","op":{"type":"create_screen","shellProfileId":profile}})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failed["code"], "spawn_failed");
    assert_eq!(json_of(next(&mut socket).await)["type"], "mux.nudge");
    served.state.ws_state.jobs.shutdown().await;
    served.state.ws_state.dispatcher.backend().close_all();
    let _ = socket.close(None).await;
}
