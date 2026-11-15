//! Adapter integration with real Unix sockets; no SSH host or daemon process
//! is needed to verify per-request identity, credential isolation and epochs.
use super::conformance::{discovery, native, Native};
use super::*;
use crate::http::{gateway, session::SessionStore, BrowserState};
use crate::service::catalog::Operation;
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use splitty_core::daemon::DaemonInfo;
use splitty_core::{
    ids::{PaneId, ScreenId},
    mux::Op,
};
use std::os::unix::fs::PermissionsExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

static WS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    // Not routes, and no operations either (control-api, Build and Runtime
    // Information: Removed runtime routes).
    for path in ["/api/agents", "/api/stats", "/api/server/settings"] {
        let reply = app.clone().oneshot(request(path)).await.unwrap();
        assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{path}");
    }
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

/// remote-screens, Hub of the previous release: such a hub repeats its
/// preparation progress as `remote.progress` text after accepting the host
/// stream. The browser gets each step once, from the transport reports, and
/// no text the gateway did not translate.
#[tokio::test]
async fn an_older_hubs_repeated_progress_text_is_not_forwarded_to_the_browser() {
    use splitty_transport::{
        codec::{self, Frame},
        negotiation, queue,
        wire::{self, Kind},
    };
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let _serial = WS_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let state = state(ADDR, dir.path());
    let (listener, info) = fake_api_listener(&state, &dir.path().join("hub.sock"));
    let instance = state.instance.clone();
    let hub = tokio::spawn(async move {
        let (io, _) = listener.accept().await.unwrap();
        let (mut read, mut write) = io.into_split();
        let context = queue::Context::default();
        let open = negotiation::server_main(&mut read, &mut write, &context.budget)
            .await
            .unwrap();
        let target = open.as_ref().as_ref().unwrap().target.clone();
        for phase in ["connecting", "install", "install", "daemon"] {
            let report = wire::Progress {
                phase: phase.into(),
                message: format!("{phase}..."),
            };
            codec::write(&mut write, &Frame::json(Kind::Progress, &report).unwrap())
                .await
                .unwrap();
        }
        let ready = wire::Ready {
            generation: 1,
            instance,
            mux_protocol: splitty_core::protocol::PROTOCOL_VERSION,
            purpose: wire::Purpose::Host {
                target: target.clone(),
            },
            limits: wire::Limits::default(),
        };
        negotiation::offer(&mut read, &mut write, &context.budget, &ready)
            .await
            .unwrap();
        let mut peer = splitty_transport::peer::Peer::start(read, write, &ready, context);
        for step in ["install", "daemon", "bridge"] {
            peer.sender
                .json(
                    Kind::Text,
                    &json!({"type":"remote.progress","step":step,"message":"again"}),
                )
                .unwrap();
        }
        let ready = json!({"type":"remote.ready","target":target,
            "daemon":{"version":"0.0.1","protocol":1,"keepsSessions":true,"started":false},
            "update":{"available":false,"keepsSessions":true},"screens":[]});
        peer.sender.json(Kind::Text, &ready).unwrap();
        peer.sender
            .json(Kind::Text, &json!({"type":"mux.welcome","screens":[]}))
            .unwrap();
        while peer.recv().await.is_some() {}
        peer.finish().await.ok();
    });
    let backend = gateway::Backend::daemon(info);
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
    let (mut socket, _) = tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
        .await
        .unwrap()
        .unwrap();
    let mut seen = Vec::new();
    loop {
        let next = json_of(
            tokio::time::timeout(WAIT, socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
        let kind = next["type"].as_str().unwrap().to_owned();
        if kind == "remote.progress" {
            assert_ne!(
                next["message"], "again",
                "no repeated text reaches the browser"
            );
            seen.push(next["step"].as_str().unwrap().to_owned());
            continue;
        }
        seen.push(kind.clone());
        if kind == "mux.welcome" {
            break;
        }
    }
    assert_eq!(seen, ["install", "daemon", "remote.ready", "mux.welcome"]);
    socket.close(None).await.unwrap();
    backend.close();
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    tokio::time::timeout(WAIT, hub).await.unwrap().unwrap();
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
fn fake_api_listener(state: &TestState, path: &Path) -> (tokio::net::UnixListener, DaemonInfo) {
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

/// access-control, Remote Screen Hub Access (Missing ticket): the checks of
/// `/ws` come first, and nothing reaches the host before they pass.
#[tokio::test]
async fn the_hub_refuses_an_upgrade_before_it_reaches_a_host() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));

    for query in [
        "?target=vm1",
        "?target=vm1&ticket=",
        "?target=vm1&ticket=guess",
    ] {
        assert_eq!(
            status(&router, upgrade(query, Some(ORIGIN), None)).await,
            StatusCode::UNAUTHORIZED,
            "{query}"
        );
    }

    // The guards come before the ticket, which they leave unspent.
    let ticket = state.sessions.issue_ticket();
    let query = format!("?target=vm1&ticket={ticket}");
    for (origin, fetch_site) in [
        (Some("http://127.0.0.1:1"), None),
        (Some("http://localhost:1420"), None),
        (Some("http://127.0.0.1:1420"), None),
        (None, None),
        (Some(ORIGIN), Some("same-site")),
        (Some(ORIGIN), Some("none")),
    ] {
        assert_eq!(
            status(&router, upgrade(&query, origin, fetch_site)).await,
            StatusCode::FORBIDDEN,
            "{origin:?} {fetch_site:?}"
        );
    }
    assert!(
        state.sessions.redeem_ticket(&ticket),
        "the ticket is unspent"
    );
    assert_eq!(host.asked(), Vec::<String>::new(), "no ssh");
}

/// remote-screens, Remembered Remote Screens: the authentication of the other
/// `/api` routes, and the origin and Fetch Metadata guards before it.
#[tokio::test]
async fn remembered_screens_need_the_api_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let list = json!({"version": 1, "screens": [{"target": "vm1", "screenId": "s1"}]});

    for method in ["GET", "PUT"] {
        let mut request = api(method, SCREENS, Some(&list));
        request.headers_mut().remove("authorization");
        assert_eq!(
            status(&router, request).await,
            StatusCode::UNAUTHORIZED,
            "{method}"
        );
        for (name, value) in [
            ("origin", "http://127.0.0.1:1"),
            ("sec-fetch-site", "same-site"),
        ] {
            let mut request = api(method, SCREENS, Some(&list));
            request
                .headers_mut()
                .insert(name, value.parse().expect("value"));
            assert_eq!(
                status(&router, request).await,
                StatusCode::FORBIDDEN,
                "{method} {name}"
            );
        }
    }
    assert!(!dir.path().join("config").exists(), "nothing written");
}

/// The catalog uses the API's token/cookie authentication, origin and Fetch
/// Metadata checks. Failed requests do not change the file or call SSH.
#[tokio::test]
async fn remote_catalog_requires_api_authentication_and_origin_guards() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let session = state.sessions.create_session();
    let app = router(&state, hub(&host, dir.path()));
    let body = json!({"target": "vm1", "screens": []});
    for method in ["GET", "PUT"] {
        for authorization in [None, Some("Bearer incorrect")] {
            let mut request = api(method, CATALOG, Some(&body));
            request.headers_mut().remove("authorization");
            if let Some(value) = authorization {
                request
                    .headers_mut()
                    .insert("authorization", value.parse().expect("header"));
            }
            assert_eq!(
                status(&app, request).await,
                StatusCode::UNAUTHORIZED,
                "{method}"
            );
        }
        for (name, value) in [
            ("origin", "http://127.0.0.1:1"),
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-site", "cross-site"),
        ] {
            for cookie in [false, true] {
                let mut request = api(method, CATALOG, Some(&body));
                if cookie {
                    request.headers_mut().remove("authorization");
                    request.headers_mut().insert(
                        "cookie",
                        format!("{}={session}", state.sessions.cookie_name())
                            .parse()
                            .expect("cookie"),
                    );
                }
                request
                    .headers_mut()
                    .insert(name, value.parse().expect("header"));
                assert_eq!(
                    status(&app, request).await,
                    StatusCode::FORBIDDEN,
                    "{method} {name}, cookie={cookie}"
                );
            }
        }
    }
    assert!(!dir.path().join("config/remote-catalog.json").exists());
    // A browser's same-origin authenticated request succeeds too.
    let mut request = api("PUT", CATALOG, Some(&body));
    request.headers_mut().remove("authorization");
    request.headers_mut().insert(
        "cookie",
        format!("{}={session}", state.sessions.cookie_name())
            .parse()
            .expect("cookie"),
    );
    request
        .headers_mut()
        .insert("origin", ORIGIN.parse().expect("origin"));
    request
        .headers_mut()
        .insert("sec-fetch-site", "same-origin".parse().expect("header"));
    assert_eq!(status(&app, request).await, StatusCode::OK);
    assert!(host.asked().is_empty());
}

/// remote-screens, Port Forwards Through The Hub: the authentication of the
/// other `/api` routes, with the origin and Fetch Metadata guards first.
#[tokio::test]
async fn forwards_need_the_api_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let body = forward("vm1", 3000);
    for method in ["GET", "POST", "DELETE"] {
        let mut request = api(method, FORWARDS, Some(&body));
        request.headers_mut().remove("authorization");
        assert_eq!(
            status(&router, request).await,
            StatusCode::UNAUTHORIZED,
            "{method}"
        );
        for (name, value) in [
            ("origin", "http://127.0.0.1:1"),
            ("sec-fetch-site", "same-site"),
        ] {
            let mut request = api(method, FORWARDS, Some(&body));
            request
                .headers_mut()
                .insert(name, value.parse().expect("value"));
            assert_eq!(
                status(&router, request).await,
                StatusCode::FORBIDDEN,
                "{method} {name}"
            );
        }
    }
    assert_eq!(host.asked(), Vec::<String>::new(), "no ssh");
}

#[tokio::test]
async fn forward_preferences_return_json_for_malformed_json() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    for method in ["PUT", "DELETE"] {
        let mut request = api(method, PREFERENCES, Some(&json!({})));
        *request.body_mut() = Body::from("{");
        let (code, body) = exchange(&router, request).await;
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].is_string());
        assert_eq!(body["uncertain"], false);
    }
    assert!(host.asked().is_empty());
    assert!(!dir.path().join("config").exists());
}

/// An invalid target is answered on the socket, and no host is asked.
#[tokio::test]
async fn an_invalid_target_is_refused_without_reaching_a_host() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    for target in ["-oProxyCommand=touch /tmp/pwned", "", "vm1 vm2"] {
        let mut socket = served.open(target).await;
        let (progress, error) = until_outcome(&mut socket).await;
        assert!(progress.is_empty(), "{progress:?}");
        assert_eq!(error["type"], "remote.error", "{error}");
        assert_eq!(error["code"], "invalid_target", "{error}");
        assert!(error["message"].is_string(), "{error}");
        assert_eq!(close_code(&mut socket).await, 1000);
    }
    assert_eq!(host.asked(), Vec::<String>::new(), "no ssh");
}

/// The browser gateway `serve` builds, with the hub given: every `/api`
/// route becomes a native request to the in-process service of `state`.
fn router(state: &TestState, hub: Hub) -> axum::Router {
    let mut state = state.clone();
    Arc::make_mut(&mut state.service).hub = Arc::new(hub);
    browser_router(&state)
}

/// The browser gateway `serve` builds over the service of `state`, with the
/// browser credentials of `state`.
fn browser_router(state: &TestState) -> axum::Router {
    crate::http::gateway::router(
        crate::http::gateway::Backend::embedded(state.service.clone()),
        axum::extract::FromRef::from_ref(state),
    )
}

/// A WebSocket handshake for `/ws/remote` as a browser sends it.
fn upgrade(query: &str, origin: Option<&str>, fetch_site: Option<&str>) -> Request<Body> {
    let mut request = Request::get(format!("/ws/remote{query}"))
        .header("host", ADDR)
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13");
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    if let Some(fetch_site) = fetch_site {
        request = request.header("sec-fetch-site", fetch_site);
    }
    request.body(Body::empty()).expect("request")
}

/// `request` as a client of the gateway on `listen_addr` sends it.
fn at(listen_addr: &str, mut request: Request<Body>) -> Request<Body> {
    request
        .headers_mut()
        .insert("host", listen_addr.parse().expect("host"));
    request
}

/// An `/api` request as the CLI sends it: the process token, no `Origin`.
fn api(method: &str, path: &str, body: Option<&serde_json::Value>) -> Request<Body> {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", ADDR)
        .header("authorization", "Bearer process-token");
    match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .expect("request")
}

#[tokio::test]
async fn a_host_write_requires_api_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let mut request = api(
        "PUT",
        SCREENS_HOST,
        Some(&json!({"target": "vm2", "screens": []})),
    );
    request.headers_mut().remove("authorization");
    assert_eq!(status(&router, request).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn remote_host_metadata_requires_api_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    let body = json!({"target":"vm1","alias":"Home"});
    for (method, path) in [
        ("GET", HOST_PREFERENCES),
        ("PUT", HOST_PREFERENCES),
        ("DELETE", "/api/remote/hosts"),
    ] {
        let mut request = api(method, path, Some(&body));
        request.headers_mut().remove("authorization");
        assert_eq!(status(&app, request).await, StatusCode::UNAUTHORIZED);
        for (name, value) in [
            ("origin", "http://127.0.0.1:1"),
            ("sec-fetch-site", "cross-site"),
        ] {
            let mut request = api(method, path, Some(&body));
            request
                .headers_mut()
                .insert(name, value.parse().expect("header"));
            assert_eq!(status(&app, request).await, StatusCode::FORBIDDEN);
        }
    }
    assert!(!dir.path().join("config").exists());
    assert!(host.asked().is_empty());
}

#[tokio::test]
async fn forward_preference_guards_have_no_side_effects() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let key = forward("vm1", 3000);
    for method in ["GET", "PUT", "DELETE"] {
        let mut request = api(method, PREFERENCES, Some(&key));
        request.headers_mut().remove("authorization");
        assert_eq!(status(&router, request).await, StatusCode::UNAUTHORIZED);
        for (name, value) in [
            ("origin", "http://127.0.0.1:3000"),
            ("sec-fetch-site", "cross-site"),
            ("host", "hostile.example"),
        ] {
            let mut request = api(method, PREFERENCES, Some(&key));
            request.headers_mut().remove("authorization");
            request
                .headers_mut()
                .insert(name, value.parse().expect("header"));
            assert_eq!(
                status(&router, request).await,
                StatusCode::FORBIDDEN,
                "guards precede authentication"
            );
        }
    }
    assert!(host.asked().is_empty());
    assert!(!dir.path().join("config/remote-forwards.json").exists());
}

#[tokio::test]
async fn known_hosts_require_api_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let mut request = api("GET", "/api/remote/hosts", None);
    request.headers_mut().remove("authorization");
    assert_eq!(status(&router, request).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn authentication_leaves_saved_forwards_passive() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let rule = json!({"target":"vm1","port":5173,"local":15173,"enabled":false});
    assert_eq!(
        exchange(&router, api("PUT", PREFERENCES, Some(&rule)))
            .await
            .0,
        StatusCode::OK
    );
    let expected = preferences(&router).await;
    let login = json!({"token":"process-token"});
    assert_eq!(
        status(&router, api("POST", "/api/session", Some(&login))).await,
        StatusCode::OK
    );
    assert_eq!(preferences(&router).await, expected);
    assert!(host.asked().is_empty());
}

/// The router served on a loopback port of its own.
struct Served {
    port: u16,
    state: TestState,
    task: tokio::task::JoinHandle<()>,
}

impl Served {
    async fn start(hub: Hub, dir: &Path) -> Self {
        Self::start_with(hub, |addr| state(addr, dir)).await
    }

    /// Serve the state `state_of` builds for the address bound.
    async fn start_with(hub: Hub, state_of: impl FnOnce(&str) -> TestState) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        Self::start_on(hub, state_of, listener).await
    }

    async fn start_on(
        hub: Hub,
        state_of: impl FnOnce(&str) -> TestState,
        listener: tokio::net::TcpListener,
    ) -> Self {
        let addr = listener.local_addr().expect("address");
        let mut state = state_of(&addr.to_string());
        let hub = Arc::new(hub);
        Arc::make_mut(&mut state.service).hub = hub.clone();
        let mut browser = BrowserState::new(addr, state.token.clone()).expect("browser origin");
        browser.sessions = state.sessions.clone();
        let app = crate::http::gateway::router(
            crate::http::gateway::Backend::embedded(state.service.clone()),
            browser,
        );
        let task = tokio::spawn(async move {
            let _ = crate::http::server::serve(listener, app, std::future::pending()).await;
        });
        Self {
            port: addr.port(),
            state,
            task,
        }
    }

    /// Open `/ws/remote` for `target` as the UI does: a fresh ticket, its own
    /// origin.
    async fn open(&self, target: &str) -> Socket {
        self.open_with(target, "").await
    }

    /// [`Served::open`] with more of the query, such as `&update=1`.
    async fn open_with(&self, target: &str, query: &str) -> Socket {
        self.connect(&format!("/ws/remote?target={}{query}", encode(target)))
            .await
    }

    /// Open the WebSocket at `path_and_query` as the UI does: a fresh ticket,
    /// its own origin.
    async fn connect(&self, path_and_query: &str) -> Socket {
        let ticket = self.state.sessions.issue_ticket();
        let separator = if path_and_query.ends_with('?') {
            ""
        } else {
            "&"
        };
        let mut request = format!(
            "ws://127.0.0.1:{}{path_and_query}{separator}ticket={ticket}",
            self.port,
        )
        .into_client_request()
        .expect("request");
        let origin = format!("http://127.0.0.1:{}", self.port)
            .parse()
            .expect("origin");
        request.headers_mut().insert("origin", origin);
        let (socket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("upgrade");
        socket
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn a_native_reverse_request_cannot_share_a_fresh_browser_gateway() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let browser = browser_router(&served.state);
    // Building another gateway must not replace the first port's exclusion.
    let other_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("second gateway listener");
    let other_address = other_listener.local_addr().expect("address");
    let _other_gateway = browser_router(&TestState {
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
            exchange(
                &browser,
                at(&served.state.listen_addr, api(method, path, Some(&body)))
            )
            .await
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
async fn daemon_shutdown_is_absent_from_the_gateway() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state(ADDR, dir.path());
    Arc::make_mut(&mut state.service).server_mode = "daemon".into();
    let request = |instance: &str| {
        api(
            "POST",
            "/api/daemon/stop",
            Some(&json!({"instance":instance})),
        )
    };
    let browser = browser_router(&state);
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
}

/// control-api, Two Control Transports (Malformed request on both
/// transports), and access-control, JSON Request Bodies: the control socket
/// and the browser gateway answer a malformed request alike, and neither
/// reads a body without a JSON content type.
#[tokio::test]
async fn malformed_requests_are_answered_alike_on_both_transports() {
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
    let browser = browser_router(&state);
    let control = crate::http::build_control_router(state.service.clone());
    let request = |method: &str, path: &str, content_type: Option<&str>, body: &str| {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", ADDR)
            .header("authorization", "Bearer process-token");
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
        let over_gateway = exchange(&browser, request(method, path, content_type, body)).await;
        let over_socket = exchange(&control, request(method, path, content_type, body)).await;
        assert_eq!(
            over_gateway.0, expected,
            "{method} {path}: {over_gateway:?}"
        );
        assert!(over_gateway.1["error"].is_string(), "{over_gateway:?}");
        assert_eq!(over_socket, over_gateway, "{method} {path}");
    }
    assert_eq!(workspace(), before, "no request changed the workspace");
}

#[tokio::test]
async fn managed_jobs_require_gateway_authentication_and_omit_grant_management() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let tcp = router;
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
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn next(socket: &mut Socket) -> Message {
    tokio::time::timeout(WAIT, socket.next())
        .await
        .expect("a message in time")
        .expect("the socket is open")
        .expect("a message")
}

/// The hub's messages up to and including the first that is not progress.
async fn until_outcome(socket: &mut Socket) -> (Vec<serde_json::Value>, serde_json::Value) {
    let mut progress = Vec::new();
    loop {
        let message = json_of(next(socket).await);
        if message["type"] != "remote.progress" {
            return (progress, message);
        }
        progress.push(message);
    }
}

/// The close code the hub ended the socket with.
async fn close_code(socket: &mut Socket) -> u16 {
    match next(socket).await {
        Message::Close(Some(frame)) => u16::from(frame.code),
        other => panic!("not a close frame: {other:?}"),
    }
}

/// remote-screens, Remote Screen Hub Endpoint (Connect a host without
/// Splitty, Client closes the socket): progress, `remote.ready`, then the
/// daemon's messages verbatim both ways, and a closed socket ends the bridge.
#[tokio::test]
async fn gateway_a_host_is_installed_started_and_relayed_both_ways() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open("user@vm1").await;

    // Sent before `remote.ready`: relayed once the bridge is up.
    let early = r#"{"type":"mux.resync","screenId":"s1"}"#;
    socket.send(Message::text(early)).await.expect("send");

    let (progress, ready) = until_outcome(&mut socket).await;
    assert_eq!(
        steps(&progress),
        ["install", "daemon", "started", "bridge"],
        "{progress:?}"
    );
    let install = progress[0]["message"].as_str().expect("message");
    assert!(install.contains("user@vm1 (x86_64)"), "{install}");
    assert_eq!(
        ready,
        json!({
            "type": "remote.ready",
            "target": "user@vm1",
            "update": {"available": false, "keepsSessions": true},
            "daemon": {"version": VERSION, "protocol": PROTOCOL_VERSION, "keepsSessions": true, "started": true},
            "screens": [{"id": "s1", "name": "api", "panes": 3}],
        })
    );
    assert_eq!(next(&mut socket).await, Message::text(WELCOME));
    assert_eq!(next(&mut socket).await, Message::text(early));

    let input = r#"{"type":"input","sessionId":"p1","data":"bytes"}"#;
    socket.send(Message::text(input)).await.expect("send");
    assert_eq!(next(&mut socket).await, Message::text(input));
    assert_eq!(next(&mut socket).await, Message::binary(vec![0, 1, 2, 255]));
    let text = r#"{"type":"mux.op","screenId":"s1","op":{"type":"close_pane"}}"#;
    socket.send(Message::text(text)).await.expect("send");
    assert_eq!(next(&mut socket).await, Message::text(text));

    let bridge = host.bridge_pid().expect("a bridge");
    socket.close(None).await.expect("close");
    assert!(gone(bridge).await, "the bridge ends with the socket");
}

/// remote-screens, Remote Screen Hub Endpoint (Host connection lost).
#[tokio::test]
async fn gateway_a_host_side_that_ends_closes_the_socket_with_4001() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open("vm1").await;
    let (_, ready) = until_outcome(&mut socket).await;
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(next(&mut socket).await, Message::text(WELCOME));

    let bridge = host.bridge_pid().expect("a bridge");
    kill(
        Pid::from_raw(i32::try_from(bridge).expect("pid")),
        Signal::SIGKILL,
    )
    .expect("kill the bridge");
    assert_eq!(close_code(&mut socket).await, HOST_LOST);
}

/// remote-screens, Remote Screen Protocol Check (Newer host).
#[tokio::test]
async fn gateway_an_incompatible_daemon_is_reported_and_no_bridge_is_opened() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path())
        .preparing(Prepare::Installed)
        .with_protocol(PROTOCOL_VERSION + 1);
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open("vm1").await;
    let (progress, error) = until_outcome(&mut socket).await;
    assert_eq!(steps(&progress), ["daemon", "started"], "{progress:?}");
    assert_eq!(error["type"], "remote.error", "{error}");
    assert_eq!(error["code"], "incompatible", "{error}");
    assert_eq!(error["version"], VERSION, "{error}");
    assert_eq!(error["protocol"], PROTOCOL_VERSION + 1, "{error}");
    assert_eq!(close_code(&mut socket).await, 1000);
    assert!(
        !host.asked().iter().any(|asked| asked == "bridge"),
        "{:?}",
        host.asked()
    );
    assert!(
        !dir.path().join("config/recent-hosts.json").exists(),
        "only a ready host is recorded"
    );
}

/// remote-screens, Remote Screen Hub Endpoint (Unknown host key).
#[tokio::test]
async fn gateway_a_failed_host_key_check_is_reported_with_its_hint() {
    let dir = tempfile::tempdir().expect("dir");
    let host =
        ScriptedHost::new(dir.path()).preparing(Prepare::SshFails("Host key verification failed."));
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open("vm1").await;
    let (progress, error) = until_outcome(&mut socket).await;
    assert!(progress.is_empty(), "{progress:?}");
    assert_eq!(error["code"], "host_key", "{error}");
    let hint = error["hint"].as_str().expect("a hint");
    assert!(hint.contains("Run `ssh vm1` once"), "{hint}");
    assert_eq!(close_code(&mut socket).await, 1000);
    assert_eq!(host.asked(), ["prepare vm1"]);
}

const HOST_LOST: u16 = 4001;

#[derive(Clone)]
struct TestState {
    service_state: super::TestState,
    token: String,
    sessions: Arc<crate::http::session::SessionStore>,
    allowed_origins: Arc<Vec<String>>,
    listen_addr: String,
}

impl std::ops::Deref for TestState {
    type Target = super::TestState;
    fn deref(&self) -> &Self::Target {
        &self.service_state
    }
}
impl std::ops::DerefMut for TestState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.service_state
    }
}

fn state(listen_addr: &str, dir: &Path) -> TestState {
    TestState {
        service_state: super::state(listen_addr, dir),
        token: "process-token".into(),
        sessions: Arc::new(crate::http::session::SessionStore::isolated()),
        allowed_origins: Arc::new(allowed_origins(listen_addr)),
        listen_addr: listen_addr.into(),
    }
}
impl axum::extract::FromRef<TestState> for BrowserState {
    fn from_ref(state: &TestState) -> Self {
        Self {
            token: state.token.clone(),
            sessions: state.sessions.clone(),
            allowed_origins: state.allowed_origins.clone(),
            listen_addr: state.listen_addr.clone(),
        }
    }
}

/// The origins of a gateway on `listen_addr`, as `BrowserState::new` derives
/// them from the address bound.
fn allowed_origins(listen_addr: &str) -> Vec<String> {
    listen_addr
        .parse::<std::net::SocketAddr>()
        .map(crate::http::canonical_origins)
        .unwrap_or_default()
}

async fn preferences(router: &axum::Router) -> serde_json::Value {
    let (code, body) = exchange(router, api("GET", PREFERENCES, None)).await;
    assert_eq!(code, StatusCode::OK);
    body
}

#[tokio::test]
async fn malformed_remembered_screens_leave_the_saved_list_unchanged() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let list = json!({"version":1,"screens":[{"target":"vm1","screenId":"s1"}]});
    assert_eq!(
        exchange(&router, api("PUT", SCREENS, Some(&list))).await,
        (StatusCode::OK, list.clone())
    );
    let mut not_json = api("PUT", SCREENS, None);
    *not_json.body_mut() = Body::from("{");
    not_json
        .headers_mut()
        .insert("content-type", "application/json".parse().expect("value"));
    // A body that is not JSON never reaches the service (the gateway's 422).
    assert_eq!(
        status(&router, not_json).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        exchange(&router, api("GET", SCREENS, None)).await,
        (StatusCode::OK, list.clone()),
        "unchanged"
    );
}

const ORIGIN: &str = "http://127.0.0.1:45299";

async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    exchange(router, api(method, path, body)).await
}

#[tokio::test]
async fn editor_requests_require_gateway_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let app = browser_router(&state);
    let request = Request::post("/api/editor/open")
        .header("host", ADDR)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"sessionId":"s1","path":dir.path().join("file.txt")}).to_string(),
        ))
        .expect("request");
    assert_eq!(status(&app, request).await.as_u16(), 401);
}

#[tokio::test]
async fn files_requests_require_gateway_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let app = browser_router(&state);
    let request = Request::post("/api/file/offer")
        .header("host", ADDR)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"sessionId":"s1","path":dir.path().join("file.txt")}).to_string(),
        ))
        .expect("request");
    assert_eq!(status(&app, request).await.as_u16(), 401);
}

#[tokio::test]
async fn settings_metadata_requires_gateway_authentication() {
    let dir = tempfile::tempdir().expect("dir");
    let state = state(ADDR, dir.path());
    let browser = browser_router(&state);
    let unauthenticated = axum::http::Request::get("/api/home/settings/metadata")
        .header("host", ADDR)
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        browser
            .clone()
            .oneshot(unauthenticated)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn without_a_port_scan_the_gateway_answers_ping_after_welcome() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let scan: Scan = Arc::new(|| panic!("no scan on this system"));
    let served = Served::start_with(hub(&host, dir.path()), |addr| TestState {
        service_state: super::state_with(addr, dir.path(), Some(PortWatch::with_scan(false, scan))),
        ..state(addr, dir.path())
    })
    .await;
    let mut socket = served.connect("/ws?").await;
    assert_eq!(json_of(next(&mut socket).await)["type"], "mux.welcome");
    socket
        .send(Message::Ping(vec![7].into()))
        .await
        .expect("ping");
    assert_eq!(next(&mut socket).await, Message::Pong(vec![7].into()));
}
