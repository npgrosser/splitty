//! Adapter integration with real Unix sockets; no SSH host or daemon process
//! is needed to verify per-request identity, credential isolation and epochs.
use super::*;
use crate::http::{gateway, BrowserState};
use axum::response::IntoResponse;
use serde_json::Value;
use splitty_core::daemon::DaemonInfo;
use std::os::unix::fs::PermissionsExt;

static WS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Native {
    info: DaemonInfo,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Native {
    async fn start(state: &AppState, path: &Path) -> Self {
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let info = discovery(state, path);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let service = state.service.clone();
        let task = tokio::spawn(crate::native::listen(listener, service, async {
            let _ = stopped.await;
        }));
        Self { info, stop, task }
    }
    async fn stop(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap().unwrap();
    }
}

fn discovery(state: &AppState, path: &Path) -> DaemonInfo {
    DaemonInfo {
        pid: std::process::id(),
        format: Some(2),
        port: None,
        client_socket: Some(path.to_string_lossy().into()),
        instance: Some(state.instance.clone()),
        transport_version: Some(1),
        socket: String::new(),
        started_at: "1".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: 1,
        keeps_sessions: true,
    }
}

fn browser(address: &str) -> BrowserState {
    BrowserState {
        token: "gateway-only-token".into(),
        sessions: Arc::new(SessionStore::isolated()),
        listen_addr: address.into(),
        allowed_origins: Arc::new(vec![format!("http://{address}")]),
    }
}
fn request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("host", ADDR)
        .header("authorization", "Bearer gateway-only-token")
        .body(Body::empty())
        .unwrap()
}
async fn value(response: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn browser_assignment_is_authenticated_fixed_and_available_without_an_upstream() {
    use splitty_remote::{
        ssh::{Ssh, SshTarget},
        tunnel::SshRemote,
    };
    use splitty_transport::admission;
    let remote = Arc::new(SshRemote::new(
        Ssh::new(SshTarget::parse("me@192.0.2.1").unwrap()),
        env!("CARGO_PKG_VERSION"),
        1,
        |_, _| Ok(None),
        admission::Assignment::new(admission::Backend::new(), admission::Domain::new()),
    ));
    // No discovery/SSH is needed for this locally assigned identity.
    let backend = gateway::Backend::remote(remote);
    let browser = browser(ADDR);
    let gateway_id = browser.sessions.cookie_name().to_owned();
    let cookie = browser
        .sessions
        .redeem_bootstrap(&browser.sessions.issue_bootstrap())
        .unwrap();
    let cookie = format!("{}={cookie}", browser.sessions.cookie_name());
    let app = gateway::router(backend.clone(), browser);
    let reply = app
        .clone()
        .oneshot(request("/api/client-assignment?role=main&target=other"))
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(reply.headers()["cache-control"], "no-store");
    assert_eq!(
        value(reply).await,
        json!({"gateway":gateway_id, "role":"remote", "target":"me@192.0.2.1"})
    );
    backend.set_remote(None);
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/client-assignment")
                .header("host", ADDR)
                .header("cookie", &cookie)
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(value(reply).await["role"], "remote");
    for (headers, expected) in [
        (vec![], StatusCode::UNAUTHORIZED),
        (
            vec![
                ("authorization", "Bearer gateway-only-token"),
                ("origin", "http://localhost:1"),
            ],
            StatusCode::FORBIDDEN,
        ),
        (
            vec![
                ("cookie", cookie.as_str()),
                ("sec-fetch-site", "cross-site"),
            ],
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = Request::builder()
            .uri("/api/client-assignment")
            .header("host", ADDR);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let reply = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(reply.status(), expected);
    }
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let browser = super::gateway::browser(ADDR);
    let gateway_id = browser.sessions.cookie_name().to_owned();
    let app = gateway::router(gateway::Backend::embedded(state.service.clone()), browser);
    assert_eq!(
        value(
            app.oneshot(request("/api/client-assignment?host=forged"))
                .await
                .unwrap()
        )
        .await,
        json!({"gateway":gateway_id, "role":"main"})
    );
}

#[tokio::test]
async fn a_cookie_from_another_tab_cannot_retarget_an_assigned_documents_requests() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let previous = browser(ADDR);
    let current = browser(ADDR);
    let current_id = current.sessions.cookie_name().to_owned();
    let session = current
        .sessions
        .redeem_bootstrap(&current.sessions.issue_bootstrap())
        .unwrap();
    let cookie = format!("{current_id}={session}");
    let app = gateway::router(gateway::Backend::embedded(state.service.clone()), current);
    for (method, path) in [
        ("GET", "/api/home/settings"),
        ("PUT", "/api/home/settings"),
        ("POST", "/api/ws-ticket"),
    ] {
        let reply = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", ADDR)
                    .header("cookie", &cookie)
                    .header("sec-fetch-site", "same-origin")
                    .header("x-splitty-gateway", previous.sessions.cookie_name())
                    .header("content-type", "application/json")
                    .body(Body::from("invalid JSON must not reach a handler"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::CONFLICT, "{method} {path}");
        assert_eq!(reply.headers()["x-splitty-gateway-changed"], "1");
    }
    let reply = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ws-ticket")
                .header("host", ADDR)
                .header("cookie", &cookie)
                .header("sec-fetch-site", "same-origin")
                .header("x-splitty-gateway", &current_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    let ticket = value(reply).await["ticket"].as_str().unwrap().to_owned();
    assert!(!previous.sessions.redeem_ticket(&ticket));
}

#[tokio::test]
async fn gateway_routes_fixed_catalog_calls_without_forwarding_browser_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let native = Native::start(&state, &dir.path().join("client.sock")).await;
    let backend = gateway::Backend::daemon(native.info.clone());
    let app = gateway::router(backend, browser(ADDR));
    let reply = app
        .clone()
        .oneshot(request("/api/mux/status"))
        .await
        .unwrap();
    let code = reply.status();
    let status = value(reply).await;
    assert_eq!(code, StatusCode::OK, "{status}");
    assert_eq!(status["instance"], state.instance);
    assert!(status["listenAddr"].is_null());
    let reply = app
        .clone()
        .oneshot(request("/api/home/settings?target=other-host"))
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::BAD_REQUEST);
    let reply = app.clone().oneshot(request("/api/agents")).await.unwrap();
    assert_eq!(reply.status(), StatusCode::NOT_FOUND);
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/mux/status")
                .header("host", "localhost:1")
                .header("authorization", "Bearer gateway-only-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::FORBIDDEN);
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/mux/status")
                .header("host", ADDR)
                .header("origin", "http://localhost:1")
                .header("authorization", "Bearer gateway-only-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::FORBIDDEN);
    // The service's own browser token cannot authenticate this adapter.
    let reply = app
        .oneshot(
            Request::builder()
                .uri("/api/mux/status")
                .header("host", ADDR)
                .header("authorization", "Bearer process-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
    assert!(!dir.path().join("auth-token").exists());
    native.stop().await;
}

#[tokio::test]
async fn query_floods_are_rejected_before_body_reads_or_ticket_consumption() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let browser = browser(ADDR);
    let app = gateway::router(
        gateway::Backend::embedded(state.service.clone()),
        browser.clone(),
    );
    let polled = Arc::new(AtomicBool::new(false));
    let body_polled = polled.clone();
    let body = Body::from_stream(futures::stream::poll_fn(move |_| {
        body_polled.store(true, Ordering::SeqCst);
        std::task::Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
    }));
    let path = format!("/api/jobs?{}", "arg=x&".repeat(7888));
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("host", ADDR)
                .header("authorization", "Bearer gateway-only-token")
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        ),
    )
    .await
    .expect("parameter rejection must not wait for the body")
    .unwrap();
    assert_eq!(reply.status(), StatusCode::BAD_REQUEST);
    assert!(!polled.load(Ordering::SeqCst));
    let ticket = browser.sessions.issue_ticket();
    let path = format!("/ws?ticket={ticket}&{}", "extra=x&".repeat(7000));
    let mut upgrade = request(&path);
    upgrade
        .headers_mut()
        .insert("origin", format!("http://{ADDR}").parse().unwrap());
    assert_eq!(
        app.clone().oneshot(upgrade).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert!(
        browser.sessions.redeem_ticket(&ticket),
        "bad queries must not consume one-use tickets"
    );
    assert_eq!(
        app.oneshot(request("/api/mux/status"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn gateway_rejects_json_amplification_before_backend_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let native = Native::start(&state, &dir.path().join("client.sock")).await;
    let app = gateway::router(gateway::Backend::daemon(native.info.clone()), browser(ADDR));
    let input = format!("[{}0]", "0,".repeat(262_144));
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/home/settings")
                .header("host", ADDR)
                .header("authorization", "Bearer gateway-only-token")
                .header("content-type", "application/json")
                .body(Body::from(input))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(value(reply).await["error"]
        .as_str()
        .unwrap()
        .contains("budget"));
    assert_eq!(
        app.oneshot(request("/api/mux/status"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    native.stop().await;
}

#[tokio::test]
async fn gateway_credentials_survive_backend_replacement_and_are_isolated_from_other_gateways() {
    let dir = tempfile::tempdir().unwrap();
    let first = state(ADDR, dir.path());
    let native = Native::start(&first, &dir.path().join("first.sock")).await;
    let backend = gateway::Backend::daemon(native.info.clone());
    let browser = browser(ADDR);
    let other = super::gateway::browser(ADDR);
    assert_ne!(browser.sessions.cookie_name(), other.sessions.cookie_name());
    let code = browser.sessions.issue_bootstrap();
    let ticket = browser.sessions.issue_ticket();
    assert!(other.sessions.redeem_bootstrap(&code).is_none());
    assert!(!other.sessions.redeem_ticket(&ticket));
    let app = gateway::router(backend.clone(), browser.clone());
    let exchange = Request::builder()
        .method("POST")
        .uri("/api/session")
        .header("host", ADDR)
        .header("content-type", "application/json")
        .body(Body::from(json!({"bootstrap":code}).to_string()))
        .unwrap();
    let reply = app.clone().oneshot(exchange).await.unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    let cookie = reply.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let authenticated = || {
        Request::builder()
            .uri("/api/mux/status")
            .header("host", ADDR)
            .header("cookie", &cookie)
            .header("sec-fetch-site", "same-origin")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(authenticated()).await.unwrap().status(),
        StatusCode::OK
    );
    backend.set_daemon(None);
    assert_eq!(
        app.clone().oneshot(authenticated()).await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let second = state(ADDR, dir.path());
    let replacement = Native::start(&second, &dir.path().join("second.sock")).await;
    backend.set_daemon(Some(replacement.info.clone()));
    let reply = app.oneshot(authenticated()).await.unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(value(reply).await["instance"], second.instance);
    let other_app = gateway::router(backend, other);
    assert_eq!(
        other_app
            .clone()
            .oneshot(authenticated())
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(other_app.oneshot(request("/api/mux/status")).await.unwrap().status(), StatusCode::OK,
        "shared owner bearer credentials intentionally authorize both gateways; cookies still do not cross");
    native.stop().await;
    replacement.stop().await;
}

#[tokio::test]
async fn gateway_websockets_have_distinct_clients_and_backend_replacement_revokes_both() {
    let _serial = WS_TEST_LOCK.lock().await;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let native = Native::start(&state, &dir.path().join("client.sock")).await;
    let backend = gateway::Backend::daemon(native.info.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let browser = browser(&address.to_string());
    let app = gateway::router(backend.clone(), browser.clone());
    let server = tokio::spawn(async move {
        crate::http::server::serve(listener, app, std::future::pending())
            .await
            .unwrap();
    });
    let open = || {
        let ticket = browser.sessions.issue_ticket();
        let mut request = format!("ws://{address}/ws?ticket={ticket}")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", format!("http://{address}").parse().unwrap());
        request
    };
    let (mut first, _) = tokio_tungstenite::connect_async(open()).await.unwrap();
    let (mut second, _) = tokio_tungstenite::connect_async(open()).await.unwrap();
    let first_welcome = json_of(
        tokio::time::timeout(WAIT, first.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    );
    let second_welcome = json_of(
        tokio::time::timeout(WAIT, second.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    );
    assert_eq!(first_welcome["type"], "mux.welcome");
    assert_eq!(second_welcome["type"], "mux.welcome");
    assert_ne!(first_welcome["clientId"], second_welcome["clientId"]);
    let (mut third, _) = tokio_tungstenite::connect_async(open()).await.unwrap();
    let welcome = json_of(
        tokio::time::timeout(WAIT, third.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    );
    assert_ne!(welcome["clientId"], second_welcome["clientId"]);
    assert_eq!(
        state.client_state.clients.lock().unwrap().len(),
        3,
        "idle streams reserve no maximum-sized message buffers"
    );
    first.close(None).await.unwrap();
    tokio::time::timeout(WAIT, async {
        while state.client_state.clients.lock().unwrap().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    backend.set_daemon(None);
    tokio::time::timeout(WAIT, async {
        while let Some(message) = second.next().await {
            if matches!(message, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(WAIT, async {
        while !state.client_state.clients.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
    native.stop().await;
}

#[tokio::test]
async fn a_silent_browser_expires_even_while_the_native_hop_is_healthy() {
    let _serial = WS_TEST_LOCK.lock().await;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let native = Native::start(&state, &dir.path().join("client.sock")).await;
    let backend = gateway::Backend::daemon(native.info.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let browser = browser(&address.to_string());
    let app = gateway::router(backend, browser.clone());
    let server = tokio::spawn(async move {
        crate::http::server::serve(listener, app, std::future::pending())
            .await
            .unwrap();
    });
    let mut request = format!(
        "ws://{address}/ws?ticket={}",
        browser.sessions.issue_ticket()
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let welcome = socket.next().await.unwrap().unwrap();
    assert_eq!(json_of(welcome)["type"], "mux.welcome");
    let started = std::time::Instant::now();
    // No further reads means this real Tungstenite client cannot receive/answer
    // browser pings. The independent native peers continue their own heartbeat.
    tokio::time::timeout(std::time::Duration::from_secs(35), async {
        while !state.client_state.clients.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_secs(29));
    drop(socket);
    server.abort();
    let _ = server.await;
    native.stop().await;
}

#[tokio::test]
async fn a_partially_received_mutation_cannot_cross_backend_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let first = state(ADDR, dir.path());
    let second = state(ADDR, dir.path());
    let native = Native::start(&first, &dir.path().join("first.sock")).await;
    let replacement = Native::start(&second, &dir.path().join("second.sock")).await;
    let backend = gateway::Backend::daemon(native.info.clone());
    let app = gateway::router(backend.clone(), browser(ADDR));
    let (entered, reading) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let stream = futures::stream::once(async move {
        let _ = entered.send(());
        let _ = released.await;
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(br#"{"theme":"changed"}"#))
    });
    let before = serde_json::to_value(&*second.config.read().unwrap()).unwrap();
    let request = Request::builder()
        .method("PATCH")
        .uri("/api/home/settings")
        .header("host", ADDR)
        .header("authorization", "Bearer gateway-only-token")
        .header("content-type", "application/json")
        .body(Body::from_stream(stream))
        .unwrap();
    let response = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    reading.await.unwrap();
    backend.set_daemon(None);
    backend.set_daemon(Some(replacement.info.clone()));
    let _ = release.send(());
    let reply = response.await.unwrap();
    assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        value(reply).await["uncertain"],
        false,
        "the old request never entered either backend"
    );
    assert_eq!(
        serde_json::to_value(&*second.config.read().unwrap()).unwrap(),
        before
    );
    assert!(first.client_state.clients.lock().unwrap().is_empty());
    assert!(second.client_state.clients.lock().unwrap().is_empty());
    native.stop().await;
    replacement.stop().await;
}

#[tokio::test]
async fn gateway_guards_run_before_consuming_one_use_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let native = Native::start(&state, &dir.path().join("client.sock")).await;
    let browser = browser(ADDR);
    let app = gateway::router(
        gateway::Backend::daemon(native.info.clone()),
        browser.clone(),
    );
    for (header, bad) in [
        ("host", "localhost:1"),
        ("origin", "http://localhost:1"),
        ("sec-fetch-site", "cross-site"),
    ] {
        let code = browser.sessions.issue_bootstrap();
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/session")
            .header("content-type", "application/json");
        if header != "host" {
            request = request.header("host", ADDR);
        }
        let reply = app
            .clone()
            .oneshot(
                request
                    .header(header, bad)
                    .body(Body::from(json!({"bootstrap":code}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::FORBIDDEN, "{header}");
        assert!(
            browser.sessions.redeem_bootstrap(&code).is_some(),
            "a rejected origin must not spend the code"
        );
        let ticket = browser.sessions.issue_ticket();
        let mut request = Request::builder().uri(format!("/ws?ticket={ticket}"));
        if header != "host" {
            request = request.header("host", ADDR);
        }
        let reply = app
            .clone()
            .oneshot(request.header(header, bad).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::FORBIDDEN, "{header}");
        assert!(
            browser.sessions.redeem_ticket(&ticket),
            "a rejected origin must not spend the ticket"
        );
    }
    let code = browser.sessions.issue_bootstrap();
    let body = format!(
        r#"{{"bootstrap":"{code}","padding":"{}"}}"#,
        "a".repeat(4096)
    );
    let reply = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/session")
                .header("host", ADDR)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        browser.sessions.redeem_bootstrap(&code).is_some(),
        "oversized input must fail before redemption"
    );
    native.stop().await;
}

#[tokio::test]
async fn an_ipv6_port_zero_gateway_uses_only_its_actual_origin_and_csp() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let native = Native::start(&state, &dir.path().join("client.sock")).await;
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    assert_ne!(address.port(), 0);
    let (mapping, held) = splitty_remote::tunnel::reserve(splitty_remote::tunnel::PortForward {
        local: address.port(),
        remote: 5173,
    })
    .unwrap();
    assert_ne!(
        mapping.local,
        address.port(),
        "an IPv6 gateway's own port cannot be reused by an IPv4 forward"
    );
    drop(held);
    let browser = BrowserState::new(address, "gateway-only-token".into()).unwrap();
    assert_eq!(
        &*browser.allowed_origins,
        &[
            format!("http://{address}"),
            format!("http://localhost:{}", address.port())
        ]
    );
    let app = gateway::router(gateway::Backend::daemon(native.info.clone()), browser);
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(crate::http::server::serve(listener, app, async {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let reply = client
        .get(format!("http://{address}/api/mux/status"))
        .bearer_auth("gateway-only-token")
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(
        reply.json::<Value>().await.unwrap()["instance"],
        state.instance
    );
    let reply = client
        .get(format!("http://{address}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    let csp = reply.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains(&format!("ws://{address}")), "{csp}");
    assert!(!csp.contains("ws://127.0.0.1:"));
    assert!(!csp.contains("ws://[::1]:*"));
    assert_eq!(reply.headers()["x-content-type-options"], "nosniff");
    assert_eq!(reply.headers()["referrer-policy"], "no-referrer");
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    native.stop().await;
}

#[tokio::test]
async fn an_embedded_gateway_shares_the_supplied_service_and_releases_clients() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let backend = gateway::Backend::embedded(state.service.clone());
    let app = gateway::router(backend.clone(), browser(ADDR));
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(request("/api/mux/status"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(value(response).await["instance"], state.instance);
    }
    assert_eq!(
        state.client_state.clients.lock().unwrap().len(),
        1,
        "stateless HTTP calls reuse one gateway-owned client"
    );
    backend.close();
    tokio::time::timeout(WAIT, async {
        while !state.client_state.clients.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let response = app.oneshot(request("/api/mux/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value(response).await["uncertain"], false);
}

#[tokio::test]
async fn native_host_open_pins_the_local_hub_before_any_ssh_and_keeps_remote_authority() {
    use crate::{
        hub::{testing::ScriptedHost, Hub},
        native::client::Connector,
    };
    use splitty_transport::wire::{self, Kind};
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(ADDR, dir.path());
    let host = ScriptedHost::new(dir.path()).with_keeping_daemon("0.0.1", 1);
    Arc::make_mut(&mut state.service).hub = Arc::new(Hub::new(
        Box::new(host.clone()),
        dir.path().join("hub"),
        None,
    ));
    let native = Native::start(&state, &dir.path().join("client.sock")).await;
    let connector = Connector::default();
    let mut stale = native.info.clone();
    stale.instance = Some("0".repeat(32));
    let denied = connector
        .local_host(
            &stale,
            splitty_remote::ssh::SshTarget::parse("vm1").unwrap(),
            false,
        )
        .await;
    assert_eq!(
        denied.err().unwrap().kind(),
        std::io::ErrorKind::ConnectionAborted
    );
    assert!(
        host.asked().is_empty(),
        "stale discovery cannot start host preparation"
    );
    for embedded in [false, true] {
        let target = if embedded { "vm2" } else { "vm1" };
        let target = splitty_remote::ssh::SshTarget::parse(target).unwrap();
        let expected = wire::Purpose::Host {
            target: target.as_str().into(),
        };
        let mut client = if embedded {
            connector.host(state.service.clone(), target, false).await
        } else {
            connector.local_host(&native.info, target, true).await
        }
        .unwrap();
        assert_eq!(client.ready.purpose, expected);
        loop {
            let message = tokio::time::timeout(WAIT, client.recv())
                .await
                .unwrap()
                .unwrap();
            let body: Value = message.frame.decode().unwrap();
            if body["type"] == "remote.ready" {
                break;
            }
            assert_eq!(body["type"], "remote.progress");
        }
        let welcome = tokio::time::timeout(WAIT, client.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            welcome.frame.decode::<Value>().unwrap()["type"],
            "mux.welcome"
        );
        drop(welcome);
        client
            .sender()
            .json(
                Kind::Request,
                &wire::Request {
                    id: 1,
                    operation: "host.list".into(),
                    args: serde_json::Map::new(),
                },
            )
            .unwrap();
        assert!(
            tokio::time::timeout(WAIT, client.recv())
                .await
                .unwrap()
                .is_none(),
            "host streams cannot gain local hub authority"
        );
        client.shutdown().await;
    }
    assert_eq!(
        host.asked()
            .iter()
            .filter(|call| call.starts_with("prepare "))
            .count(),
        2
    );
    assert_eq!(
        host.asked()
            .iter()
            .filter(|call| call.as_str() == "stop")
            .count(),
        1,
        "only the explicitly requested keeper-safe update replaces a daemon"
    );
    native.stop().await;
}

#[tokio::test]
async fn transport_worker_exhaustion_is_busy_in_both_browser_and_native_reply_shapes() {
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
        assert_eq!(value(response).await["code"], "busy");
    }
}

#[tokio::test]
async fn gateway_host_upgrade_reports_progress_before_ready_and_admits_early_input_once() {
    use crate::hub::{
        testing::{ForwardGate, ScriptedHost},
        Hub,
    };
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let _serial = WS_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new(ForwardGate::default());
    let host = ScriptedHost::new(dir.path()).with_preparation_gate(gate.clone());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(Hub::new(
        Box::new(host.clone()),
        dir.path().to_owned(),
        None,
    ));
    let backend = gateway::Backend::embedded(state.service.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let browser = browser(&address.to_string());
    let app = gateway::router(backend.clone(), browser.clone());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(crate::http::server::serve(listener, app, async {
        let _ = stopped.await;
    }));
    let mut request = format!(
        "ws://{address}/ws/remote?target=vm1&ticket={}",
        browser.sessions.issue_ticket()
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    let (mut socket, response) =
        tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    gate.entered().await;
    let progress = json_of(
        tokio::time::timeout(WAIT, socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    );
    assert_eq!(progress["type"], "remote.progress");
    assert!(
        !host.asked().iter().any(|call| call == "handshake"),
        "progress must precede preparation completion"
    );
    let early = json!({"type":"mux.resync"});
    socket
        .send(Message::Text(early.to_string().into()))
        .await
        .unwrap();
    gate.release();
    let mut ready = false;
    let mut welcome = false;
    loop {
        let next = json_of(
            tokio::time::timeout(WAIT, socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
        match next["type"].as_str().unwrap() {
            "remote.progress" => assert!(!ready),
            "remote.ready" => {
                assert!(!ready);
                assert_eq!(next["target"], "vm1");
                ready = true;
            }
            "mux.welcome" => {
                assert!(ready);
                assert!(!welcome);
                welcome = true;
            }
            "mux.resync" => {
                assert!(ready && welcome);
                assert_eq!(next, early);
                break;
            }
            other => panic!("unexpected {other}: {next}"),
        }
    }
    backend.close();
    tokio::time::timeout(WAIT, async {
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert_eq!(
        host.asked()
            .iter()
            .filter(|call| call.starts_with("prepare "))
            .count(),
        1
    );
}

#[tokio::test]
async fn gateway_host_setup_cancellation_never_replays_input_or_reaches_the_next_phase() {
    use crate::hub::{
        testing::{ForwardGate, ScriptedHost},
        Hub,
    };
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let _serial = WS_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new(ForwardGate::default());
    let cleanup = Arc::new(ForwardGate::default());
    let host = ScriptedHost::new(dir.path())
        .with_preparation_gate(gate.clone())
        .with_preparation_cleanup(cleanup.clone());
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).hub = Arc::new(Hub::new(
        Box::new(host.clone()),
        dir.path().to_owned(),
        None,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let browser = browser(&address.to_string());
    let app = gateway::router(
        gateway::Backend::embedded(state.service.clone()),
        browser.clone(),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(crate::http::server::serve(listener, app, async {
        let _ = stopped.await;
    }));
    let mut request = format!(
        "ws://{address}/ws/remote?target=vm1&ticket={}",
        browser.sessions.issue_ticket()
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    gate.entered().await;
    socket
        .send(Message::Text(
            json!({"type":"mux.resync"}).to_string().into(),
        ))
        .await
        .unwrap();
    socket.close(None).await.unwrap();
    drop(socket);
    // Graceful listener shutdown observes upgraded IO cancellation and waits
    // for its admitted tasks; no release of the host's setup barrier is needed.
    stop.send(()).unwrap();
    cleanup.entered().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut server)
            .await
            .is_err(),
        "the upgraded HTTP task must retain its lifetime through setup cleanup"
    );
    cleanup.release();
    tokio::time::timeout(WAIT, server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    gate.release();
    tokio::task::yield_now().await;
    assert_eq!(
        host.asked(),
        vec!["prepare vm1"],
        "closed input must not advance or reconnect setup"
    );
}

#[tokio::test]
async fn gateway_refuses_wrong_reply_ids_and_reports_mutation_uncertainty_without_replay() {
    use splitty_transport::{
        negotiation,
        peer::Peer,
        queue,
        wire::{self, Kind},
    };
    for mutation in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let state = state(ADDR, dir.path());
        let path = dir.path().join("wrong-reply.sock");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let info = discovery(&state, &path);
        let ready = wire::Ready {
            generation: 1,
            instance: state.instance.clone(),
            mux_protocol: 1,
            purpose: wire::Purpose::Direct,
            limits: wire::Limits::default(),
        };
        let server = tokio::spawn(async move {
            let (io, _) = listener.accept().await.unwrap();
            let (mut read, mut write) = io.into_split();
            let context = queue::Context::default();
            negotiation::server(&mut read, &mut write, &context.budget)
                .await
                .unwrap();
            negotiation::offer(&mut read, &mut write, &context.budget, &ready)
                .await
                .unwrap();
            let mut peer = Peer::start(read, write, &ready, context);
            let message = peer.recv().await.unwrap();
            assert_eq!(message.frame.kind, Kind::Request);
            let request: wire::Request = message.frame.decode().unwrap();
            assert_eq!(
                request.operation,
                if mutation {
                    "home.update"
                } else {
                    "mux.status"
                }
            );
            drop(message);
            peer.sender
                .json(
                    Kind::Reply,
                    &wire::Reply {
                        id: request.id + 1,
                        status: 200,
                        content_type: wire::ContentType::Json,
                        body: "{}".into(),
                        uncertain: false,
                    },
                )
                .unwrap();
            peer.sender.flush().unwrap().await.unwrap();
            while let Some(message) = peer.recv().await {
                assert_eq!(
                    message.frame.kind,
                    Kind::Cancel,
                    "a failed request cannot be replayed"
                );
            }
            let _ = peer.finish().await;
        });
        let app = gateway::router(gateway::Backend::daemon(info), browser(ADDR));
        let request = if mutation {
            Request::builder()
                .method("PATCH")
                .uri("/api/home/settings")
                .header("host", ADDR)
                .header("authorization", "Bearer gateway-only-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"theme":"dark"}"#))
                .unwrap()
        } else {
            request("/api/mux/status")
        };
        let response = tokio::time::timeout(WAIT, app.oneshot(request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let error = value(response).await;
        assert!(error["error"]
            .as_str()
            .unwrap()
            .contains("unexpected native API reply"));
        assert_eq!(error["uncertain"], mutation);
        tokio::time::timeout(WAIT, server).await.unwrap().unwrap();
    }
}

async fn fake_api_peer(
    listener: &tokio::net::UnixListener,
    instance: String,
) -> splitty_transport::peer::Peer {
    use splitty_transport::{negotiation, peer::Peer, queue, wire};
    let (io, _) = listener.accept().await.unwrap();
    let (mut read, mut write) = io.into_split();
    let context = queue::Context::default();
    let ready = wire::Ready {
        generation: 1,
        instance,
        mux_protocol: 1,
        purpose: wire::Purpose::Direct,
        limits: wire::Limits::default(),
    };
    negotiation::server(&mut read, &mut write, &context.budget)
        .await
        .unwrap();
    negotiation::offer(&mut read, &mut write, &context.budget, &ready)
        .await
        .unwrap();
    Peer::start(read, write, &ready, context)
}
fn fake_api_listener(state: &AppState, path: &Path) -> (tokio::net::UnixListener, DaemonInfo) {
    std::fs::set_permissions(
        path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    (listener, discovery(state, path))
}
fn api_reply(sender: &splitty_transport::peer::Sender, id: u32) {
    use splitty_transport::wire::{self, Kind};
    sender
        .json(
            Kind::Reply,
            &wire::Reply {
                id,
                status: 200,
                content_type: wire::ContentType::Json,
                body: format!("{{\"id\":{id}}}"),
                uncertain: false,
            },
        )
        .unwrap();
}

#[tokio::test]
async fn api_calls_share_one_carrier_with_bounded_concurrency_and_monotonic_ids() {
    use splitty_transport::wire::{self, Kind};
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (listener, info) = fake_api_listener(&state, &dir.path().join("api.sock"));
    let backend = gateway::Backend::daemon(info);
    let app = gateway::router(backend.clone(), browser(ADDR));
    let (full, filled) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let instance = state.instance.clone();
    let server = tokio::spawn(async move {
        let mut peer = fake_api_peer(&listener, instance).await;
        for id in 1..=64 {
            let message = peer.recv().await.unwrap();
            assert_eq!(message.frame.kind, Kind::Request);
            assert_eq!(message.frame.decode::<wire::Request>().unwrap().id, id);
        }
        full.send(()).unwrap();
        released.await.unwrap();
        // Replies may arrive out of order; route them to the matching waiter.
        for id in (1..=64).rev() {
            api_reply(&peer.sender, id);
        }
        for id in 65..=66 {
            let message = peer.recv().await.unwrap();
            assert_eq!(message.frame.kind, Kind::Request);
            assert_eq!(message.frame.decode::<wire::Request>().unwrap().id, id);
            drop(message);
            api_reply(&peer.sender, id);
        }
        assert!(peer.recv().await.is_none());
        peer.finish().await.unwrap();
        use futures::FutureExt;
        assert!(
            listener.accept().now_or_never().is_none(),
            "only one native dial"
        );
    });
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..64 {
        let app = app.clone();
        requests.spawn(async move { app.oneshot(request("/api/mux/status")).await.unwrap() });
    }
    tokio::time::timeout(WAIT, filled).await.unwrap().unwrap();
    let excess = app
        .clone()
        .oneshot(request("/api/mux/status"))
        .await
        .unwrap();
    assert_eq!(excess.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value(excess).await["uncertain"], false);
    release.send(()).unwrap();
    let mut ids = std::collections::BTreeSet::new();
    while let Some(response) = tokio::time::timeout(WAIT, requests.join_next())
        .await
        .unwrap()
    {
        let response = response.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(ids.insert(value(response).await["id"].as_u64().unwrap()));
    }
    assert_eq!(ids, (1..=64).collect());
    for id in 65..=66 {
        let response = app
            .clone()
            .oneshot(request("/api/mux/status"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(value(response).await["id"], id);
    }
    backend.close();
    tokio::time::timeout(WAIT, server).await.unwrap().unwrap();
}

#[tokio::test]
async fn cancelling_one_api_wait_keeps_other_requests_and_the_carrier_alive() {
    use splitty_transport::wire::{self, Kind};
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (listener, info) = fake_api_listener(&state, &dir.path().join("api.sock"));
    let backend = gateway::Backend::daemon(info);
    let app = gateway::router(backend.clone(), browser(ADDR));
    let (entered, admitted) = tokio::sync::oneshot::channel();
    let (cancelled, cancellation) = tokio::sync::oneshot::channel();
    let instance = state.instance.clone();
    let server = tokio::spawn(async move {
        let mut peer = fake_api_peer(&listener, instance).await;
        let first = peer.recv().await.unwrap();
        assert_eq!(first.frame.decode::<wire::Request>().unwrap().id, 1);
        drop(first);
        entered.send(()).unwrap();
        let message = peer.recv().await.unwrap();
        assert_eq!(message.frame.kind, Kind::Cancel);
        assert_eq!(message.frame.decode::<wire::Cancel>().unwrap().id, 1);
        drop(message);
        api_reply(&peer.sender, 1);
        cancelled.send(()).unwrap();
        let second = peer.recv().await.unwrap();
        assert_eq!(second.frame.kind, Kind::Request);
        assert_eq!(second.frame.decode::<wire::Request>().unwrap().id, 2);
        drop(second);
        api_reply(&peer.sender, 2);
        assert!(peer.recv().await.is_none());
        peer.finish().await.unwrap();
    });
    let pending = tokio::spawn(app.clone().oneshot(request("/api/mux/status")));
    tokio::time::timeout(WAIT, admitted).await.unwrap().unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    tokio::time::timeout(WAIT, cancellation)
        .await
        .unwrap()
        .unwrap();
    let response = app.oneshot(request("/api/mux/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(value(response).await["id"], 2);
    backend.close();
    tokio::time::timeout(WAIT, server).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn unanswered_api_cancellation_closes_the_carrier_before_capacity_is_reused() {
    use splitty_transport::wire::{self, Kind};
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (listener, info) = fake_api_listener(&state, &dir.path().join("api.sock"));
    let backend = gateway::Backend::daemon(info);
    let app = gateway::router(backend.clone(), browser(ADDR));
    let (entered, admitted) = tokio::sync::oneshot::channel();
    let (cancelled, cancellation) = tokio::sync::oneshot::channel();
    let instance = state.instance.clone();
    let server = tokio::spawn(async move {
        let mut peer = fake_api_peer(&listener, instance.clone()).await;
        let first = peer.recv().await.unwrap();
        assert_eq!(first.frame.decode::<wire::Request>().unwrap().id, 1);
        drop(first);
        entered.send(()).unwrap();
        let cancel = peer.recv().await.unwrap();
        assert_eq!(cancel.frame.kind, Kind::Cancel);
        drop(cancel);
        cancelled.send(()).unwrap();
        // The peer remains healthy and answers transport pings, but never
        // confirms request cancellation. The gateway must close this carrier.
        assert!(peer.recv().await.is_none());
        peer.finish().await.unwrap();
        let mut replacement = fake_api_peer(&listener, instance).await;
        let next = replacement.recv().await.unwrap();
        assert_eq!(next.frame.decode::<wire::Request>().unwrap().id, 1);
        drop(next);
        api_reply(&replacement.sender, 1);
        assert!(replacement.recv().await.is_none());
        replacement.finish().await.unwrap();
    });
    let pending = tokio::spawn(app.clone().oneshot(request("/api/mux/status")));
    admitted.await.unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    cancellation.await.unwrap();
    tokio::time::advance(wire::HANDSHAKE_TIMEOUT).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    let response = app.oneshot(request("/api/mux/status")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(value(response).await["id"], 1);
    backend.close();
    server.await.unwrap();
}

#[tokio::test]
async fn api_backend_replacement_never_replays_an_admitted_mutation() {
    use splitty_transport::wire::{self, Kind};
    let dir = tempfile::tempdir().unwrap();
    let first = state(ADDR, dir.path());
    let second = state(ADDR, dir.path());
    let (listener, info) = fake_api_listener(&first, &dir.path().join("old.sock"));
    let replacement = Native::start(&second, &dir.path().join("new.sock")).await;
    let backend = gateway::Backend::daemon(info);
    let app = gateway::router(backend.clone(), browser(ADDR));
    let (entered, admitted) = tokio::sync::oneshot::channel();
    let instance = first.instance.clone();
    let server = tokio::spawn(async move {
        let mut peer = fake_api_peer(&listener, instance).await;
        let request = peer.recv().await.unwrap();
        assert_eq!(request.frame.kind, Kind::Request);
        assert_eq!(
            request.frame.decode::<wire::Request>().unwrap().operation,
            "home.update"
        );
        drop(request);
        entered.send(()).unwrap();
        while let Some(message) = peer.recv().await {
            assert_eq!(message.frame.kind, Kind::Cancel, "no replay on old carrier");
        }
        peer.finish().await.unwrap();
    });
    let before = serde_json::to_value(&*second.config.read().unwrap()).unwrap();
    let request = Request::builder()
        .method("PATCH")
        .uri("/api/home/settings")
        .header("host", ADDR)
        .header("authorization", "Bearer gateway-only-token")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"theme":"changed"}"#))
        .unwrap();
    let response = tokio::spawn(app.clone().oneshot(request));
    tokio::time::timeout(WAIT, admitted).await.unwrap().unwrap();
    backend.set_daemon(Some(replacement.info.clone()));
    let response = tokio::time::timeout(WAIT, response)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value(response).await["uncertain"], true);
    let fresh = app
        .oneshot(super::gateway::request("/api/mux/status"))
        .await
        .unwrap();
    assert_eq!(fresh.status(), StatusCode::OK);
    assert_eq!(value(fresh).await["instance"], second.instance);
    assert_eq!(
        serde_json::to_value(&*second.config.read().unwrap()).unwrap(),
        before,
        "the failed mutation never reached the replacement backend"
    );
    backend.close();
    tokio::time::timeout(WAIT, server).await.unwrap().unwrap();
    replacement.stop().await;
}
