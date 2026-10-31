//! In-process tests of the router: the remote screen hub's routes through the
//! real middleware and handlers. The host is scripted (`hub::testing`), its
//! bridge a local `sh`, so the relay runs over real pipes; the WebSocket tests
//! serve the router on a loopback port of their own.

use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use serde_json::json;
use splitty_core::protocol::PROTOCOL_VERSION;
use splitty_core::pty::ports::ListeningPort;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tower::ServiceExt;

use super::{allowed_origins, apply_middleware, routes, session::SessionStore, AppState};
use crate::hub::testing::{Prepare, ScriptedHost, WELCOME};
use crate::hub::{Hub, HOST_LOST, VERSION};
use crate::ws::ports::{PortWatch, Scan};

const WAIT: Duration = Duration::from_secs(10);

mod editor;
mod files;
mod managed_jobs;
mod move_pane;
mod mux_tabs;
mod pty_size;
mod scratchpad;

/// The state `serve` builds, for a server on `listen_addr` with its files in
/// `dir`.
fn state(listen_addr: &str, dir: &Path) -> AppState {
    state_with(listen_addr, dir, None)
}

/// [`state`], with the listening ports found by `ports` when given.
fn state_with(listen_addr: &str, dir: &Path, ports: Option<PortWatch>) -> AppState {
    use splitty_core::mux::effect_runner::{EffectRunner, EnvContext};

    let shell_profiles = Arc::new(RwLock::new(
        splitty_core::shell_profiles::ShellProfilesState::default(),
    ));
    let pty = Arc::new(splitty_core::pty::manager::Manager::with_shell_profiles(
        shell_profiles.clone(),
    ));
    let dispatcher = Arc::new(splitty_core::protocol::dispatcher::Dispatcher::new(
        pty.clone(),
    ));
    let mux_store = Arc::new(splitty_core::mux::Store::new());
    let (notify_tx, _) = tokio::sync::broadcast::channel(16);
    let mut ws_state = crate::ws::WsState::new(
        mux_store.clone(),
        dispatcher,
        shell_profiles.clone(),
        Arc::new(EffectRunner::new(pty)),
        EnvContext::default(),
        notify_tx,
        "embedded".to_owned(),
    );
    if let Some(ports) = ports {
        ws_state = ws_state.with_ports(ports);
    }
    let ws_state = Arc::new(ws_state);
    AppState {
        token: "process-token".to_owned(),
        sessions: Arc::new(SessionStore::new()),
        allowed_origins: Arc::new(allowed_origins(listen_addr)),
        mux_store,
        ws_state,
        config: Arc::new(RwLock::new(splitty_core::config::HomeConfig::default())),
        shell_profiles,
        server_config: Arc::new(RwLock::new(splitty_core::config::ServerConfig::default())),
        template_store: Arc::new(splitty_core::template::FileStore::new(
            dir.join("templates"),
        )),
        themes_dir: dir.join("themes"),
        version: "test".to_owned(),
        commit: "test".to_owned(),
        build_time: "test".to_owned(),
        listen_addr: listen_addr.to_owned(),
        server_mode: "embedded".to_owned(),
    }
}

/// `build_router` with the hub given.
fn router(state: &AppState, hub: Hub) -> axum::Router {
    apply_middleware(routes::api_routes_with(Arc::new(hub)), state).with_state(state.clone())
}

/// A hub with `host`, recording its recent hosts in `dir/config`.
fn hub(host: &ScriptedHost, dir: &Path) -> Hub {
    Hub::new(Box::new(host.clone()), dir.join("config"), None)
}

// ── Through `oneshot` ───────────────────────────────────────────────────────

/// The address the oneshot tests' state believes it listens on.
const ADDR: &str = "127.0.0.1:45299";
const ORIGIN: &str = "http://127.0.0.1:45299";

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

async fn status(router: &axum::Router, request: Request<Body>) -> StatusCode {
    router
        .clone()
        .oneshot(request)
        .await
        .expect("response")
        .status()
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

/// access-control, Remote Screen Hub Access and Hub Port Forwards (Control
/// socket).
#[tokio::test]
async fn the_hubs_routes_are_not_on_the_control_socket() {
    let dir = tempfile::tempdir().expect("dir");
    let control = super::build_control_router(state(ADDR, dir.path()));
    for (method, path) in [
        ("GET", "/ws/remote?target=vm1"),
        ("GET", "/ws/remote?target=vm1&update=1"),
        ("GET", "/api/remote/hosts"),
        ("DELETE", "/api/remote/hosts"),
        ("GET", HOST_PREFERENCES),
        ("PUT", HOST_PREFERENCES),
        ("GET", HOST_ORDER),
        ("PUT", HOST_ORDER),
        ("GET", SCREENS),
        ("PUT", SCREENS),
        ("GET", CATALOG),
        ("PUT", CATALOG),
        ("GET", PREFERENCES),
        ("PUT", PREFERENCES),
        ("DELETE", PREFERENCES),
        ("GET", FORWARDS),
        ("POST", FORWARDS),
        ("DELETE", FORWARDS),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"version":1,"screens":[],"target":"vm1","port":3000}"#,
            ))
            .expect("request");
        assert_eq!(
            status(&control, request).await,
            StatusCode::NOT_FOUND,
            "{method} {path}"
        );
    }
}

const HOST_PREFERENCES: &str = "/api/remote/host-preferences";
const HOST_ORDER: &str = "/api/remote/host-order";

const SCREENS: &str = "/api/remote/screens";
const CATALOG: &str = "/api/remote/catalog";

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

/// The status and the JSON body of the answer to `request`.
async fn exchange(
    router: &axum::Router,
    request: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    let body =
        serde_json::from_slice(&body).unwrap_or_else(|_| json!(String::from_utf8_lossy(&body)));
    (status, body)
}

/// remote-screens, Remembered Remote Screens: `PUT` replaces the list in
/// `remote-screens.json` of the server's configuration directory, owner-only,
/// and `GET` returns it; a list without the shape is refused and changes
/// nothing; a missing or corrupted file is an empty list.
#[tokio::test]
async fn remembered_screens_are_kept_in_the_configuration_directory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, hub(&host, dir.path()));
    let file = dir.path().join("config/remote-screens.json");
    let empty = json!({"version": 1, "screens": []});

    assert_eq!(
        exchange(&router, api("GET", SCREENS, None)).await,
        (StatusCode::OK, empty.clone()),
        "nothing remembered yet"
    );

    let list = json!({"version": 1, "screens": [
        {"target": "user@vm1", "screenId": "screen-a", "name": "api", "position": 1},
        {"target": "api.devpod", "screenId": "screen-a"},
        {"target": "user@vm1", "screenId": "screen-b", "position": 4},
    ]});
    assert_eq!(
        exchange(&router, api("PUT", SCREENS, Some(&list))).await,
        (StatusCode::OK, list.clone())
    );
    assert_eq!(
        exchange(&router, api("GET", SCREENS, None)).await,
        (StatusCode::OK, list.clone())
    );
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).expect("the file")).expect("JSON");
    assert_eq!(stored, list);
    let mode = |path: &Path| std::fs::metadata(path).expect("mode").permissions().mode() & 0o777;
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(file.parent().expect("directory")), 0o700);

    // Refused lists leave the remembered one as it is.
    for refused in [
        json!({"version": 1, "screens": [
            {"target": "vm1", "screenId": "s1"},
            {"target": "vm1", "screenId": "s1", "name": "again"},
        ]}),
        json!({"version": 1, "screens": [{"target": "-oProxyCommand=touch /tmp/x", "screenId": "s1"}]}),
        json!({"version": 1, "screens": [{"target": "vm1", "screenId": ""}]}),
        json!({"version": 1, "screens": [{"target": "vm1", "screenId": "s1", "position": -1}]}),
        json!({"version": 2, "screens": []}),
        json!({"version": 1}),
        json!([]),
    ] {
        let (status, body) = exchange(&router, api("PUT", SCREENS, Some(&refused))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
        assert!(body["error"].is_string(), "{body}");
    }
    let mut not_json = api("PUT", SCREENS, None);
    *not_json.body_mut() = Body::from("{");
    not_json
        .headers_mut()
        .insert("content-type", "application/json".parse().expect("value"));
    assert_eq!(status(&router, not_json).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        exchange(&router, api("GET", SCREENS, None)).await,
        (StatusCode::OK, list.clone()),
        "unchanged"
    );

    // An empty list is a list too.
    assert_eq!(
        exchange(&router, api("PUT", SCREENS, Some(&empty))).await,
        (StatusCode::OK, empty.clone())
    );

    std::fs::write(&file, "not JSON").expect("corrupt the file");
    assert_eq!(
        exchange(&router, api("GET", SCREENS, None)).await,
        (StatusCode::OK, empty)
    );
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

/// remote-screens, Host Screen Directory Cache: per-host merges, empty
/// replacements, fresh-hub persistence and private files, without any SSH.
#[tokio::test]
async fn remote_catalog_merges_hosts_and_survives_a_fresh_hub() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    let file = dir.path().join("config/remote-catalog.json");
    assert_eq!(
        exchange(&app, api("GET", CATALOG, None)).await,
        (StatusCode::OK, json!({"version": 1, "hosts": []}))
    );
    assert!(!file.exists(), "reading does not create the file");
    // The open-screen list and directory are independent.
    let remembered = json!({"version": 1, "screens": [
        {"target": "vm1", "screenId": "s1", "position": 1},
    ]});
    assert_eq!(
        exchange(&app, api("PUT", SCREENS, Some(&remembered)))
            .await
            .0,
        StatusCode::OK
    );
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs()
    };
    let before = now();
    let vm1 = json!({"target": "vm1", "screens": [
        {"id": "s1", "name": "api", "panes": 2},
        {"id": "s2", "panes": 0},
    ]});
    let (status, first) = exchange(&app, api("PUT", CATALOG, Some(&vm1))).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let timestamp = first["lastSeen"].as_u64().expect("Unix seconds");
    assert!((before..=now()).contains(&timestamp));
    assert_eq!(
        first,
        json!({"target": "vm1", "lastSeen": timestamp, "screens": vm1["screens"]})
    );

    let vm2 = json!({"target": "user@vm2:2222", "screens": [
        {"id": "s1", "name": "same id, other host", "panes": 1},
    ]});
    let (status, second) = exchange(&app, api("PUT", CATALOG, Some(&vm2))).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        exchange(&app, api("GET", CATALOG, None)).await,
        (
            StatusCode::OK,
            json!({"version": 1, "hosts": [second, first]})
        )
    );
    let empty = json!({"target": "vm1", "screens": []});
    let (status, updated) = exchange(&app, api("PUT", CATALOG, Some(&empty))).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["screens"], json!([]));
    let expected = json!({"version": 1, "hosts": [updated, second]});
    drop(app);
    let fresh = router(&state, hub(&host, dir.path()));
    assert_eq!(
        exchange(&fresh, api("GET", CATALOG, None)).await,
        (StatusCode::OK, expected.clone())
    );
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).expect("file")).expect("JSON");
    assert_eq!(stored, expected);
    let mode = |path: &Path| std::fs::metadata(path).expect("mode").permissions().mode() & 0o777;
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(file.parent().expect("directory")), 0o700);
    assert_eq!(
        exchange(&fresh, api("GET", SCREENS, None)).await,
        (StatusCode::OK, remembered)
    );
    assert!(host.asked().is_empty(), "catalog APIs never reach a host");
}

/// Invalid bodies, including unknown fields, must not alter a saved snapshot.
#[tokio::test]
async fn remote_catalog_rejects_invalid_updates_without_writing() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    let initial = json!({"target": "vm1", "screens": [{"id": "s1", "name": "api", "panes": 1}]});
    assert_eq!(
        exchange(&app, api("PUT", CATALOG, Some(&initial))).await.0,
        StatusCode::OK
    );
    let file = dir.path().join("config/remote-catalog.json");
    let saved = std::fs::read(&file).expect("file");
    let mut refused = vec![
        json!([]),
        json!(null),
        json!({}),
        json!({"target": "vm1"}),
        json!({"screens": []}),
        json!({"target": "vm1", "screens": null}),
        json!({"target": "vm1", "screens": [], "version": 1}),
        json!({"target": "vm1", "screens": [], "lastSeen": 0}),
        json!({"target": "vm1", "screens": [], "extra": true}),
    ];
    for target in [
        "",
        " vm1",
        "vm1\t",
        "a b",
        "vm1\n",
        "-oProxyCommand=x",
        "user@",
        "vm1:0",
    ] {
        refused.push(json!({"target": target, "screens": []}));
    }
    for screen in [
        json!({"id": "s1", "panes": 1, "extra": true}),
        json!({"id": "s1"}),
        json!({"panes": 1}),
        json!({"id": 3, "panes": 1}),
        json!({"id": "", "panes": 1}),
        json!({"id": "界".repeat(257), "panes": 1}),
        json!({"id": "s1", "name": "界".repeat(201), "panes": 1}),
        json!({"id": "s1", "name": 4, "panes": 1}),
        json!({"id": "s1", "panes": -1}),
        json!({"id": "s1", "panes": 1.5}),
        json!({"id": "s1", "panes": 1.0}),
        json!({"id": "s1", "panes": "1"}),
        json!({"id": "s1", "panes": null}),
        json!({"id": "s1", "panes": true}),
    ] {
        refused.push(json!({"target": "vm1", "screens": [screen]}));
    }
    refused.push(json!({"target": "vm1", "screens": [
        {"id": "s1", "panes": 1}, {"id": "s1", "name": "again", "panes": 0},
    ]}));
    let too_many: Vec<_> = (0..201)
        .map(|n| json!({"id": format!("s{n}"), "panes": 0}))
        .collect();
    refused.push(json!({"target": "vm1", "screens": too_many}));
    for document in refused {
        let (code, body) = exchange(&app, api("PUT", CATALOG, Some(&document))).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{document}: {body}");
        assert!(body["error"].is_string(), "{body}");
        assert_eq!(
            std::fs::read(&file).expect("file"),
            saved,
            "invalid writes change nothing"
        );
    }
    let mut broken = api("PUT", CATALOG, None);
    *broken.body_mut() = Body::from("{");
    broken
        .headers_mut()
        .insert("content-type", "application/json".parse().expect("header"));
    assert_eq!(status(&app, broken).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        status(&app, api("PUT", CATALOG, None)).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(std::fs::read(&file).expect("file"), saved);
    assert!(host.asked().is_empty());
}

/// The caps accept their boundaries, count Unicode characters, and retain the
/// 20 most recently updated hosts even when timestamps have the same second.
#[tokio::test]
async fn remote_catalog_enforces_limits_and_moves_updated_hosts_to_the_front() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    let mut screens: Vec<_> = (0..199)
        .map(|n| json!({"id": format!("s{n}"), "name": "", "panes": 0}))
        .collect();
    screens.push(json!({"id": "界".repeat(256), "name": "名".repeat(200), "panes": 0}));
    let boundary = json!({"target": "vm0", "screens": screens});
    let (code, body) = exchange(&app, api("PUT", CATALOG, Some(&boundary))).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["screens"], boundary["screens"]);
    for n in 1..21 {
        let body = json!({"target": format!("vm{n}"), "screens": []});
        assert_eq!(
            exchange(&app, api("PUT", CATALOG, Some(&body))).await.0,
            StatusCode::OK
        );
    }
    let (code, catalog) = exchange(&app, api("GET", CATALOG, None)).await;
    assert_eq!(code, StatusCode::OK);
    let targets = |catalog: &serde_json::Value| {
        catalog["hosts"]
            .as_array()
            .expect("hosts")
            .iter()
            .map(|host| host["target"].as_str().expect("target").to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        targets(&catalog),
        (1..21).rev().map(|n| format!("vm{n}")).collect::<Vec<_>>()
    );
    let body = json!({"target": "vm1", "screens": []});
    assert_eq!(
        exchange(&app, api("PUT", CATALOG, Some(&body))).await.0,
        StatusCode::OK
    );
    let (_, updated) = exchange(&app, api("GET", CATALOG, None)).await;
    let expected: Vec<_> = std::iter::once("vm1".to_owned())
        .chain((2..21).rev().map(|n| format!("vm{n}")))
        .collect();
    assert_eq!(targets(&updated), expected);
    assert!(host.asked().is_empty());
}

/// Independent window writes must merge under one hub lock, not race a
/// read/modify/write of the same file. Tasks execute on multiple OS threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_catalog_concurrent_updates_keep_other_hosts() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    for round in 0..3 {
        let barrier = Arc::new(tokio::sync::Barrier::new(20));
        let mut tasks = Vec::new();
        for n in 0..20 {
            let app = app.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                let body = json!({"target": format!("vm{n}"), "screens": [
                    {"id": "s1", "name": format!("round-{round}"), "panes": n},
                ]});
                barrier.wait().await;
                let (code, stored) = exchange(&app, api("PUT", CATALOG, Some(&body))).await;
                assert_eq!(code, StatusCode::OK, "{stored}");
                assert_eq!(stored["target"], body["target"]);
                assert_eq!(stored["screens"], body["screens"]);
            }));
        }
        for task in tasks {
            task.await.expect("request task");
        }
        let (code, catalog) = exchange(&app, api("GET", CATALOG, None)).await;
        assert_eq!(code, StatusCode::OK);
        let hosts = catalog["hosts"].as_array().expect("hosts");
        assert_eq!(hosts.len(), 20);
        for n in 0..20 {
            let entry = hosts
                .iter()
                .find(|entry| entry["target"] == format!("vm{n}"))
                .expect("host was not lost");
            assert_eq!(
                entry["screens"],
                json!([{"id": "s1", "name": format!("round-{round}"), "panes": n}])
            );
        }
    }
    assert!(host.asked().is_empty());
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
                        format!("splitty-session={session}")
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
        format!("splitty-session={session}")
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

#[tokio::test]
async fn remote_catalog_corrupt_files_and_write_failures_use_the_api_contract() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    let file = dir.path().join("config/remote-catalog.json");
    std::fs::create_dir_all(file.parent().expect("directory")).expect("directory");
    for text in ["not JSON", r#"{"version":2,"hosts":[]}"#] {
        std::fs::write(&file, text).expect("file");
        assert_eq!(
            exchange(&app, api("GET", CATALOG, None)).await,
            (StatusCode::OK, json!({"version": 1, "hosts": []}))
        );
        assert_eq!(std::fs::read_to_string(&file).expect("file"), text);
    }
    // Make just the catalog unwritable, without relying on permission tests
    // that would also pass for root or overwrite other configuration files.
    std::fs::remove_file(&file).expect("remove file");
    std::fs::create_dir(&file).expect("block replacement");
    let body = json!({"target": "vm1", "screens": []});
    let (code, error) = exchange(&app, api("PUT", CATALOG, Some(&body))).await;
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(error["error"]
        .as_str()
        .expect("error")
        .contains("remote-catalog.json"));
    assert!(file.is_dir());
    assert!(host.asked().is_empty());
}

/// remote-screens, Known Hosts API (Hosts for the picker): the recent hosts
/// of the server's configuration directory first, then `~/.ssh/config`, with
/// the authentication of the other `/api` routes.
#[tokio::test]
async fn known_hosts_come_from_the_servers_user() {
    let dir = tempfile::tempdir().expect("dir");
    let config = dir.path().join("config");
    let ssh = dir.path().join("ssh");
    std::fs::create_dir_all(&config).expect("config");
    std::fs::create_dir_all(&ssh).expect("ssh");
    std::fs::write(
        config.join("recent-hosts.json"),
        r#"{"version":1,"hosts":[{"target":"vm1","lastConnected":1760000000}]}"#,
    )
    .expect("recent hosts");
    std::fs::write(ssh.join("config"), "Host api.devpod vm1 *.internal\n").expect("ssh config");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let router = router(&state, Hub::new(Box::new(host), config, Some(ssh)));

    let request = |authorization: Option<&str>| {
        let mut request = Request::get("/api/remote/hosts").header("host", ADDR);
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        request.body(Body::empty()).expect("request")
    };
    assert_eq!(
        status(&router, request(None)).await,
        StatusCode::UNAUTHORIZED
    );

    let response = router
        .clone()
        .oneshot(request(Some("Bearer process-token")))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    let hosts: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(
        hosts,
        json!([
            {"target": "vm1", "source": "recent", "lastConnected": 1760000000},
            {"target": "api.devpod", "source": "ssh-config"},
        ])
    );
}

/// remote-screens, Host Group Order API: `PUT` replaces the order and keeps a
/// repeated target once, an invalid order changes nothing, `GET` returns what
/// is stored, also to a fresh hub; nothing reaches SSH.
#[tokio::test]
async fn the_host_group_order_is_replaced_and_kept() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    assert_eq!(
        exchange(&app, api("GET", HOST_ORDER, None)).await,
        (StatusCode::OK, json!({"version":1,"order":[]}))
    );
    let order = json!({"order":["vm2","vm1","vm2"]});
    let stored = json!({"version":1,"order":["vm2","vm1"]});
    assert_eq!(
        exchange(&app, api("PUT", HOST_ORDER, Some(&order))).await,
        (StatusCode::OK, stored.clone())
    );
    for invalid in [
        json!({"order":[" vm1"]}),
        json!({"order":"vm1"}),
        json!({"order":["vm1"],"extra":true}),
    ] {
        assert_eq!(
            status(&app, api("PUT", HOST_ORDER, Some(&invalid))).await,
            StatusCode::BAD_REQUEST,
            "{invalid}"
        );
    }
    let app = router(&state, hub(&host, dir.path()));
    assert_eq!(
        exchange(&app, api("GET", HOST_ORDER, None)).await,
        (StatusCode::OK, stored)
    );
    assert_eq!(host.asked(), Vec::<String>::new(), "no ssh");
}

/// Display preferences survive fresh hubs; removal never reaches SSH and a
/// live connection's stale directory update cannot bring a host back.
#[tokio::test]
async fn remote_host_preferences_forget_and_explicit_readd() {
    let dir = tempfile::tempdir().expect("dir");
    let config = dir.path().join("config");
    let ssh = dir.path().join("ssh");
    std::fs::create_dir(&ssh).expect("ssh");
    std::fs::write(ssh.join("config"), "Host vm1 vm2\n").expect("ssh config");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let make_hub = || Hub::new(Box::new(host.clone()), config.clone(), Some(ssh.clone()));
    let app = router(&state, make_hub());
    assert_eq!(
        exchange(&app, api("GET", HOST_PREFERENCES, None)).await,
        (StatusCode::OK, json!({"version":1,"hosts":[]}))
    );
    let alias = json!({"target":"vm1", "alias":" Home Lab "});
    assert_eq!(
        exchange(&app, api("PUT", HOST_PREFERENCES, Some(&alias))).await,
        (StatusCode::OK, json!({"target":"vm1","alias":"Home Lab"}))
    );
    std::fs::write(
        config.join("recent-hosts.json"),
        json!({"version":1,"hosts":[{"target":"vm1","lastConnected":100}]}).to_string(),
    )
    .expect("recent fixture");
    let catalog = json!({"target":"vm1","screens":[{"id":"s1","panes":1}]});
    assert_eq!(
        status(&app, api("PUT", CATALOG, Some(&catalog))).await,
        StatusCode::OK
    );
    let target = json!({"target":"vm1"});
    assert_eq!(
        status(&app, api("DELETE", "/api/remote/hosts", Some(&target))).await,
        StatusCode::NO_CONTENT
    );
    let app = router(&state, make_hub());
    assert_eq!(
        exchange(&app, api("GET", "/api/remote/hosts", None)).await,
        (
            StatusCode::OK,
            json!([{"target":"vm2","source":"ssh-config"}])
        )
    );
    assert_eq!(
        exchange(&app, api("GET", HOST_PREFERENCES, None)).await,
        (
            StatusCode::OK,
            json!({"version":1,"hosts":[{"target":"vm1","hidden":true}]})
        )
    );
    assert_eq!(
        exchange(&app, api("GET", CATALOG, None)).await,
        (StatusCode::OK, json!({"version":1,"hosts":[]}))
    );
    assert_eq!(
        status(&app, api("PUT", CATALOG, Some(&catalog))).await,
        StatusCode::CONFLICT
    );
    let recent: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.join("recent-hosts.json")).expect("recent file"),
    )
    .expect("recent JSON");
    assert_eq!(recent["hosts"], json!([]));
    let reveal = json!({"target":"vm1", "hidden":false});
    assert_eq!(
        status(&app, api("PUT", HOST_PREFERENCES, Some(&reveal))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, api("PUT", CATALOG, Some(&catalog))).await,
        StatusCode::OK
    );
    assert_eq!(
        std::fs::read_to_string(ssh.join("config")).expect("config"),
        "Host vm1 vm2\n"
    );
    assert!(
        host.asked().is_empty(),
        "metadata actions do not contact hosts"
    );
}

#[test]
fn remote_host_removal_wins_racing_stale_catalog_writes_across_hubs() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let first = hub(&host, dir.path());
    let second = hub(&host, dir.path());
    let target = json!({"target":"vm1"});
    for _ in 0..20 {
        first
            .update_host_preference(json!({"target":"vm1","hidden":false}))
            .expect("reveal");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                let result = first.update_catalog(json!({"target":"vm1","screens":[]}));
                assert!(result.is_ok() || matches!(result, Err(crate::hub::CatalogError::Hidden)));
            });
            barrier.wait();
            second.forget_host(target.clone()).expect("forget");
        });
        assert!(first.catalog().hosts.is_empty());
        assert!(first.host_preferences().hosts[0].hidden);
    }
    assert!(host.asked().is_empty());
}

#[tokio::test]
async fn remote_host_metadata_validates_bodies_and_requires_api_auth() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let state = state(ADDR, dir.path());
    let app = router(&state, hub(&host, dir.path()));
    for (method, path, body) in [
        ("PUT", HOST_PREFERENCES, json!({"target":"vm1","alias":3})),
        (
            "PUT",
            HOST_PREFERENCES,
            json!({"target":"vm1","hidden":true}),
        ),
        ("DELETE", "/api/remote/hosts", json!({"target":" vm1"})),
        (
            "DELETE",
            "/api/remote/hosts",
            json!({"target":"vm1","extra":true}),
        ),
    ] {
        assert_eq!(
            status(&app, api(method, path, Some(&body))).await,
            StatusCode::BAD_REQUEST
        );
    }
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
    assert!(
        !dir.path().join("config").exists(),
        "refused requests write nothing"
    );
    assert!(host.asked().is_empty());
}

// ── Over a real socket ──────────────────────────────────────────────────────

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The router served on a loopback port of its own.
struct Served {
    hub: Arc<Hub>,
    port: u16,
    state: AppState,
    task: tokio::task::JoinHandle<()>,
}

impl Served {
    async fn start(hub: Hub, dir: &Path) -> Self {
        Self::start_with(hub, |addr| state(addr, dir)).await
    }

    /// Serve the state `state_of` builds for the address bound.
    async fn start_with(hub: Hub, state_of: impl FnOnce(&str) -> AppState) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("address");
        let state = state_of(&addr.to_string());
        let hub = Arc::new(hub);
        let app = apply_middleware(routes::api_routes_with(hub.clone()), &state)
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            hub,
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

    /// Open the server's own `/ws`.
    async fn open_local(&self) -> Socket {
        self.connect("/ws?").await
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

/// Percent-encode everything but unreserved characters.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn next(socket: &mut Socket) -> Message {
    tokio::time::timeout(WAIT, socket.next())
        .await
        .expect("a message in time")
        .expect("the socket is open")
        .expect("a message")
}

fn json_of(message: Message) -> serde_json::Value {
    match message {
        Message::Text(text) => serde_json::from_str(text.as_str()).expect("JSON"),
        other => panic!("not a text message: {other:?}"),
    }
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

fn steps(progress: &[serde_json::Value]) -> Vec<&str> {
    progress
        .iter()
        .map(|message| message["step"].as_str().expect("a step"))
        .collect()
}

/// The close code the hub ended the socket with.
async fn close_code(socket: &mut Socket) -> u16 {
    match next(socket).await {
        Message::Close(Some(frame)) => u16::from(frame.code),
        other => panic!("not a close frame: {other:?}"),
    }
}

/// Whether process `pid` is gone (and reaped) within [`WAIT`].
async fn gone(pid: u32) -> bool {
    let pid = Pid::from_raw(i32::try_from(pid).expect("pid"));
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if kill(pid, None).is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// remote-screens, Remote Screen Hub Endpoint (Connect a host without
/// Splitty, Client closes the socket): progress, `remote.ready`, then the
/// daemon's messages verbatim both ways, and a closed socket ends the bridge.
#[tokio::test]
async fn a_host_is_installed_started_and_relayed_both_ways() {
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
            "daemon": {"version": VERSION, "protocol": PROTOCOL_VERSION, "keepsSessions": true, "started": true},
            "screens": [{"id": "s1", "name": "api", "panes": 3}],
        })
    );
    assert_eq!(next(&mut socket).await, Message::text(WELCOME));
    assert_eq!(next(&mut socket).await, Message::text(early));

    socket
        .send(Message::binary(vec![0, 1, 2, 255]))
        .await
        .expect("send");
    assert_eq!(next(&mut socket).await, Message::binary(vec![0, 1, 2, 255]));
    let text = r#"{"type":"mux.op","screenId":"s1","op":{"type":"close_pane"}}"#;
    socket.send(Message::text(text)).await.expect("send");
    assert_eq!(next(&mut socket).await, Message::text(text));

    let asked = host.asked();
    assert_eq!(asked[0], "prepare user@vm1");
    let hint: u16 = asked[1]
        .strip_prefix("handshake ")
        .and_then(|port| port.parse().ok())
        .expect("a port hint");
    assert_ne!(hint, 0);
    assert_eq!(asked[2..], ["bridge"]);

    let recent: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("config/recent-hosts.json")).expect("recent hosts"),
    )
    .expect("JSON");
    assert_eq!(recent["hosts"][0]["target"], "user@vm1", "{recent}");

    let bridge = host.bridge_pid().expect("a bridge");
    socket.close(None).await.expect("close");
    assert!(gone(bridge).await, "the bridge ends with the socket");
}

/// remote-screens, Remote Screen Hub Endpoint (Host connection lost).
#[tokio::test]
async fn a_host_side_that_ends_closes_the_socket_with_4001() {
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

/// remote-screens, Remote Screen Protocol Check (Newer host).
#[tokio::test]
async fn an_incompatible_daemon_is_reported_and_no_bridge_is_opened() {
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
async fn a_failed_host_key_check_is_reported_with_its_hint() {
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

/// A bridge that ends before the daemon's first message is the bridge's
/// failure, with what it printed.
#[tokio::test]
async fn a_bridge_that_ends_before_the_daemon_answers_is_reported() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path())
        .with_bridge("echo 'No Splitty daemon is running on this host.' >&2; exit 1");
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open("vm1").await;
    let (progress, error) = until_outcome(&mut socket).await;
    assert_eq!(steps(&progress).last(), Some(&"bridge"), "{progress:?}");
    assert_eq!(error["code"], "bridge", "{error}");
    let message = error["message"].as_str().expect("message");
    assert!(
        message.contains("No Splitty daemon is running"),
        "{message}"
    );
}

/// The calls the host got, without their arguments.
fn calls(host: &ScriptedHost) -> Vec<String> {
    host.asked()
        .iter()
        .map(|asked| asked.split(' ').next().unwrap_or_default().to_owned())
        .collect()
}

/// remote-screens, Host Update (Update an incompatible host): without
/// `update=1` the incompatible daemon keeps running; with it, the daemon of
/// the other version is stopped, the handshake starts this version, and the
/// socket relays it.
#[tokio::test]
async fn update_replaces_a_daemon_of_another_version() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path())
        .preparing(Prepare::Installed)
        .with_daemon("0.0.9", PROTOCOL_VERSION + 1);
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;

    let mut socket = served.open("vm1").await;
    let (progress, error) = until_outcome(&mut socket).await;
    assert_eq!(steps(&progress), ["daemon"], "{progress:?}");
    assert_eq!(error["code"], "incompatible", "{error}");
    assert_eq!(error["version"], "0.0.9", "{error}");
    assert_eq!(
        error["keepsSessions"], false,
        "an older daemon's sessions end: {error}"
    );
    assert_eq!(calls(&host), ["prepare", "handshake"], "nothing stopped");

    let mut socket = served.open_with("vm1", "&update=1").await;
    let (progress, ready) = until_outcome(&mut socket).await;
    assert_eq!(
        steps(&progress),
        ["daemon", "update", "started", "bridge"],
        "{progress:?}"
    );
    assert_eq!(progress[1]["message"], "Stopping Splitty 0.0.9 on vm1...");
    assert_eq!(ready["type"], "remote.ready", "{ready}");
    assert_eq!(
        ready["daemon"],
        json!({"version": VERSION, "protocol": PROTOCOL_VERSION, "keepsSessions": true, "started": true})
    );
    assert_eq!(next(&mut socket).await, Message::text(WELCOME));
    assert_eq!(
        calls(&host)[2..],
        ["prepare", "handshake", "stop", "handshake", "bridge"]
    );
    // The second handshake keeps the port hint the first one was given.
    let asked = host.asked();
    assert_eq!(asked[3], asked[5], "{asked:?}");
}

/// remote-screens, Host Update (Update a host whose daemon keeps its
/// sessions): the `incompatible` error says that the daemon keeps them, which
/// the client's warning follows, and the update replaces it as any other.
#[tokio::test]
async fn an_incompatible_daemon_that_keeps_its_sessions_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path())
        .preparing(Prepare::Installed)
        .with_keeping_daemon("0.0.9", PROTOCOL_VERSION + 1);
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;

    let mut socket = served.open("vm1").await;
    let (_, error) = until_outcome(&mut socket).await;
    assert_eq!(error["code"], "incompatible", "{error}");
    assert_eq!(error["keepsSessions"], true, "{error}");

    let mut socket = served.open_with("vm1", "&update=1").await;
    let (progress, ready) = until_outcome(&mut socket).await;
    assert_eq!(
        steps(&progress),
        ["daemon", "update", "started", "bridge"],
        "{progress:?}"
    );
    assert_eq!(ready["daemon"]["keepsSessions"], true, "{ready}");
    assert_eq!(
        calls(&host)[2..],
        ["prepare", "handshake", "stop", "handshake", "bridge"]
    );
}

/// `update=1` changes nothing when the daemon runs this version, or when the
/// handshake just started one.
#[tokio::test]
async fn update_leaves_a_daemon_of_this_version_running() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path())
        .preparing(Prepare::Installed)
        .with_daemon(VERSION, PROTOCOL_VERSION);
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open_with("vm1", "&update=1").await;
    let (progress, ready) = until_outcome(&mut socket).await;
    assert_eq!(steps(&progress), ["daemon", "bridge"], "{progress:?}");
    assert_eq!(ready["daemon"]["started"], false, "{ready}");
    assert_eq!(calls(&host), ["prepare", "handshake", "bridge"]);

    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path()).preparing(Prepare::Installed);
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open_with("vm1", "&update=1").await;
    let (progress, ready) = until_outcome(&mut socket).await;
    assert_eq!(steps(&progress), ["daemon", "started", "bridge"]);
    assert_eq!(ready["daemon"]["started"], true, "{ready}");
    assert_eq!(calls(&host), ["prepare", "handshake", "bridge"]);
}

/// A host whose only server is another version would start that version
/// again: the update stops nothing and needs the user.
#[tokio::test]
async fn an_update_without_this_version_on_the_host_stops_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path())
        .preparing(Prepare::Installed)
        .with_server_version("0.0.9")
        .with_daemon("0.0.9", PROTOCOL_VERSION + 1);
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open_with("vm1", "&update=1").await;
    let (progress, error) = until_outcome(&mut socket).await;
    assert_eq!(steps(&progress), ["daemon"], "{progress:?}");
    assert_eq!(error["code"], "unsupported_host", "{error}");
    let hint = error["hint"].as_str().expect("a hint");
    assert!(hint.contains("SPLITTY_REMOTE_PAYLOAD"), "{hint}");
    assert_eq!(close_code(&mut socket).await, 1000);
    assert_eq!(calls(&host), ["prepare", "handshake"]);
}

/// A daemon that still runs after `daemon stop` is reported, and no bridge
/// opens to it.
#[tokio::test]
async fn a_daemon_that_does_not_stop_is_reported() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path())
        .preparing(Prepare::Installed)
        .with_daemon("0.0.9", PROTOCOL_VERSION)
        .not_stopping();
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let mut socket = served.open_with("vm1", "&update=1").await;
    let (progress, error) = until_outcome(&mut socket).await;
    assert_eq!(steps(&progress), ["daemon", "update"], "{progress:?}");
    assert_eq!(error["code"], "daemon", "{error}");
    let message = error["message"].as_str().expect("message");
    assert!(message.contains("did not stop"), "{message}");
    assert_eq!(calls(&host), ["prepare", "handshake", "stop", "handshake"]);
}

// ── Listening ports ─────────────────────────────────────────────────────────

/// The `ports` of the next `ports.changed` on `socket`, skipping other
/// messages.
async fn next_ports(socket: &mut Socket) -> serde_json::Value {
    loop {
        let message = json_of(next(socket).await);
        if message["type"] == "ports.changed" {
            return message["ports"].clone();
        }
    }
}

/// mux-protocol, Listening Ports: right after its `mux.welcome` a client gets
/// the list, also when it is empty; every client gets the whole list again
/// when it changes; a client that connects later gets the list the others
/// hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clients_get_the_listening_ports_after_their_welcome_and_on_change() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let listed: Arc<Mutex<Vec<ListeningPort>>> = Arc::default();
    let scan: Scan = {
        let listed = listed.clone();
        Arc::new(move || listed.lock().expect("listed").clone())
    };
    let served = Served::start_with(hub(&host, dir.path()), |addr| {
        state_with(addr, dir.path(), Some(PortWatch::with_scan(true, scan)))
    })
    .await;

    let mut first = served.open_local().await;
    assert_eq!(json_of(next(&mut first).await)["type"], "mux.welcome");
    let greeting = json_of(next(&mut first).await);
    assert_eq!(greeting["type"], "ports.changed", "{greeting}");
    assert_eq!(greeting["ports"], json!([]));
    assert!(greeting["id"].is_string() && greeting["timestamp"].is_i64());

    *listed.lock().expect("listed") = vec![ListeningPort {
        port: 5173,
        session_id: "s1".to_owned(),
        process: "vite".to_owned(),
    }];
    let vite = json!([{"port": 5173, "sessionId": "s1", "process": "vite"}]);
    assert_eq!(next_ports(&mut first).await, vite);

    let mut second = served.open_local().await;
    assert_eq!(json_of(next(&mut second).await)["type"], "mux.welcome");
    let greeting = json_of(next(&mut second).await);
    assert_eq!(greeting["type"], "ports.changed", "{greeting}");
    assert_eq!(greeting["ports"], vite);

    listed.lock().expect("listed").clear();
    assert_eq!(next_ports(&mut first).await, json!([]));
    assert_eq!(next_ports(&mut second).await, json!([]));
}

/// mux-protocol, Listening Ports: where the scan does not exist, nothing is
/// sent. A ping answered right after the welcome shows that nothing was
/// queued before it.
#[tokio::test]
async fn without_a_port_scan_no_ports_are_sent() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let scan: Scan = Arc::new(|| panic!("no scan on this system"));
    let served = Served::start_with(hub(&host, dir.path()), |addr| {
        state_with(addr, dir.path(), Some(PortWatch::with_scan(false, scan)))
    })
    .await;
    let mut socket = served.open_local().await;
    assert_eq!(json_of(next(&mut socket).await)["type"], "mux.welcome");
    socket
        .send(Message::Ping(vec![7].into()))
        .await
        .expect("ping");
    assert_eq!(next(&mut socket).await, Message::Pong(vec![7].into()));
}

/// mux-protocol, Listening Ports (Dev server started in a pane, Server
/// stopped), with the real scan and a real PTY session of the server's
/// backend: a listener a shell's child starts on loopback reaches the
/// client with its session and process, and its end does too. In-process,
/// since the router, the WebSocket and the scan are the server's own; the
/// binary adds nothing here.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_listener_started_in_a_session_reaches_the_clients_and_so_does_its_end() {
    use splitty_core::pty::CreateSessionOpts;

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: no python3 to listen with");
        return;
    }
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start(hub(&host, dir.path()), dir.path()).await;
    let backend = served.state.ws_state.dispatcher.backend().clone();
    let opts = CreateSessionOpts {
        id: "listening".to_owned(),
        shell_profile_id: String::new(),
        shell: "/bin/sh".to_owned(),
        shell_args: Vec::new(),
        cols: 80,
        rows: 24,
        dir: dir.path().display().to_string(),
        command: String::new(),
        restore_mode: String::new(),
        extra_env: vec![format!("HOME={}", dir.path().display())],
    };
    let session = match backend.create(opts).await {
        Ok(session) => session,
        Err(error) => {
            eprintln!("skipping: no PTY available ({error})");
            return;
        }
    };
    let mut socket = served.open_local().await;
    assert_eq!(json_of(next(&mut socket).await)["type"], "mux.welcome");
    assert_eq!(next_ports(&mut socket).await, json!([]));

    // A child of the session's shell, until a line arrives on its input.
    session
        .write(
            b"python3 -c \"import socket; s = socket.socket(); s.bind(('127.0.0.1', 0)); \
              s.listen(); input()\"; echo done\n",
        )
        .await
        .expect("type the command");
    let deadline = Instant::now() + WAIT;
    let listed = loop {
        let ports = next_ports(&mut socket).await;
        if let Some(listed) = ports.as_array().and_then(|ports| ports.first()) {
            break listed.clone();
        }
        assert!(Instant::now() < deadline, "no port reported");
    };
    assert_eq!(listed["sessionId"], "listening", "{listed}");
    let process = listed["process"].as_str().expect("process");
    assert!(process.starts_with("python"), "{listed}");
    let port = listed["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .expect("port");
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "port {port} listens"
    );

    session.write(b"\n").await.expect("end the listener");
    assert_eq!(next_ports(&mut socket).await, json!([]));
    backend.close("listening").expect("close the session");
}

// ── Port forwards through the hub ───────────────────────────────────────────

const FORWARDS: &str = "/api/remote/forwards";

/// A port nothing listens on here, on `127.0.0.1` or `::1`, which a forward
/// therefore takes as asked.
fn free_port() -> u16 {
    loop {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("address").port();
        let ipv6 = std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port));
        if !matches!(ipv6, Err(error) if error.kind() == std::io::ErrorKind::AddrInUse) {
            return port;
        }
    }
}

/// What the forward at local `port` answers: the scripted host's listener
/// names the host's port it forwards.
fn through(port: u16) -> String {
    use std::io::Read;

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(WAIT)).expect("timeout");
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer.trim().to_owned()
}

fn forward(target: &str, port: impl serde::Serialize) -> serde_json::Value {
    json!({"target": target, "port": port})
}

/// Whether the local `port` is held, by a forward or a reservation.
fn held(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

/// The forwards `GET` lists, until `done` holds for them or [`WAIT`] passed.
async fn listed_until(
    router: &axum::Router,
    mut done: impl FnMut(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let (status, list) = exchange(router, api("GET", FORWARDS, None)).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        if done(&list) || Instant::now() >= deadline {
            return list;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// remote-screens, Port Forwards Through The Hub (Forward a dev server):
/// the host's port here when it is free, any free one when it is not,
/// announced as `localhost`; a repeated request gets the forward that
/// exists; `GET` lists them; `DELETE` ends one and frees its port.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwards_are_opened_listed_and_stopped() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let free = free_port();
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a busy port");
    let busy = busy.local_addr().expect("address").port();

    let opened = json!({
        "target": "vm1", "remote": free, "local": free,
        "url": format!("http://localhost:{free}"), "state": "up",
    });
    assert_eq!(
        exchange(&router, api("POST", FORWARDS, Some(&forward("vm1", free)))).await,
        (StatusCode::OK, opened.clone())
    );
    assert_eq!(through(free), format!("forwarded {free}"));
    assert_eq!(
        exchange(
            &router,
            api("POST", FORWARDS, Some(&forward(" vm1 ", free)))
        )
        .await,
        (StatusCode::OK, opened.clone()),
        "the forward that exists"
    );

    let (status, moved) =
        exchange(&router, api("POST", FORWARDS, Some(&forward("vm1", busy)))).await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let local = moved["local"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .expect("a local port");
    assert_ne!(local, busy, "the busy port is left alone");
    assert_eq!(moved["remote"], busy);
    assert_eq!(moved["url"], format!("http://localhost:{local}"));
    assert_eq!(through(local), format!("forwarded {busy}"));

    let mut both = vec![opened.clone(), moved.clone()];
    both.sort_by_key(|forward| forward["remote"].as_u64());
    assert_eq!(
        exchange(&router, api("GET", FORWARDS, None)).await,
        (StatusCode::OK, json!(both))
    );
    assert_eq!(
        host.asked(),
        [
            format!("forward vm1 {free} at {free}"),
            format!("forward vm1 {busy} at {local}")
        ],
        "one ssh per forward"
    );
    assert_eq!(
        preferences(&router).await,
        json!({"version": 1, "forwards": []}),
        "opening a forward remembers nothing"
    );

    let (status, _) = exchange(
        &router,
        api("DELETE", FORWARDS, Some(&forward("vm1", free))),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!held(free), "the port is free again");
    assert_eq!(
        exchange(&router, api("GET", FORWARDS, None)).await,
        (StatusCode::OK, json!([moved]))
    );
    let (status, body) = exchange(
        &router,
        api("DELETE", FORWARDS, Some(&forward("vm1", free))),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].is_string(), "{body}");
}

/// Two requests for the same port at once open one forward.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_for_the_same_forward_at_once_open_it_once() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let port = free_port();
    let body = forward("vm1", port);
    let (first, second) = tokio::join!(
        exchange(&router, api("POST", FORWARDS, Some(&body))),
        exchange(&router, api("POST", FORWARDS, Some(&body))),
    );
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    assert_eq!(first, second);
    assert_eq!(host.asked(), [format!("forward vm1 {port} at {port}")]);
}

/// remote-screens, Port Forwards Through The Hub (Forward rebuilt): when a
/// forward's `ssh` exits, its port stays held while it is `down`, and it
/// is rebuilt on the same port after an attempt that failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forward_whose_ssh_ends_is_rebuilt_on_its_port() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let port = free_port();
    let (status, opened) =
        exchange(&router, api("POST", FORWARDS, Some(&forward("vm1", port)))).await;
    assert_eq!(status, StatusCode::OK, "{opened}");

    host.fail_next_forwards(1);
    host.end_forward(port);
    let down = listed_until(&router, |list| list[0]["state"] == "down").await;
    assert_eq!(down[0]["state"], "down", "{down}");
    assert_eq!(down[0]["local"], port, "{down}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(held(port), "the port is held while the forward is down");

    let up = listed_until(&router, |list| list[0]["state"] == "up").await;
    assert_eq!(up, json!([opened]), "the same forward, up again");
    assert_eq!(through(port), format!("forwarded {port}"));
    assert_eq!(
        host.asked(),
        [
            format!("forward vm1 {port} at {port}"),
            format!("forward vm1 {port} at {port}"),
            format!("forward vm1 {port} at {port}"),
        ],
        "opened, failed once, rebuilt"
    );

    let (status, _) = exchange(
        &router,
        api("DELETE", FORWARDS, Some(&forward("vm1", port))),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!held(port));
}

/// access-control, Hub Port Forwards: a host has at most 32 forwards; the
/// ones it has are still returned, and other hosts are not affected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_has_at_most_32_forwards() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    // Ports that need not be free here: a busy one moves.
    for port in 41001..=41032 {
        let (status, body) =
            exchange(&router, api("POST", FORWARDS, Some(&forward("vm1", port)))).await;
        assert_eq!(status, StatusCode::OK, "{port}: {body}");
    }
    let (status, body) =
        exchange(&router, api("POST", FORWARDS, Some(&forward("vm1", 41033)))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "forward_limit", "{body}");
    assert!(
        body["error"].as_str().expect("error").contains("32"),
        "{body}"
    );
    let (status, _) = exchange(&router, api("POST", FORWARDS, Some(&forward("vm1", 41032)))).await;
    assert_eq!(status, StatusCode::OK, "an existing forward");
    let (status, _) = exchange(&router, api("POST", FORWARDS, Some(&forward("vm2", 41033)))).await;
    assert_eq!(status, StatusCode::OK, "another host");
    assert_eq!(host.asked().len(), 33);
}

/// access-control, Hub Port Forwards: a target that fails the target
/// validation, a port outside 1 to 65535 and a body without the shape are
/// refused before any `ssh` runs.
#[tokio::test]
async fn invalid_forward_requests_are_refused_before_ssh_runs() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    for method in ["POST", "DELETE"] {
        for body in [
            forward("-oProxyCommand=touch /tmp/pwned", 3000),
            forward("vm1 vm2", 3000),
            forward("", 3000),
            forward("vm1", 0),
            forward("vm1", 65536),
            forward("vm1", -1),
            forward("vm1", "3000"),
            forward("vm1", 30.5),
            json!({"target": "vm1"}),
            json!([]),
        ] {
            let (status, answer) = exchange(&router, api(method, FORWARDS, Some(&body))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {body}: {answer}");
            assert!(answer["error"].is_string(), "{answer}");
        }
    }
    assert_eq!(host.asked(), Vec::<String>::new(), "no ssh");
}

/// A forward whose `ssh` fails is answered with the hub's code and hint.
#[tokio::test]
async fn a_failed_forward_is_answered_with_its_code_and_hint() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path()).forwards_failing("Host key verification failed.");
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let port = free_port();
    let (status, body) =
        exchange(&router, api("POST", FORWARDS, Some(&forward("vm1", port)))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "host_key", "{body}");
    assert!(
        body["hint"].as_str().expect("hint").contains("`ssh vm1`"),
        "{body}"
    );
    assert!(!held(port), "nothing holds the port");
    assert_eq!(
        exchange(&router, api("GET", FORWARDS, None)).await,
        (StatusCode::OK, json!([]))
    );
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

const PREFERENCES: &str = "/api/remote/forward-preferences";

async fn preferences(router: &axum::Router) -> serde_json::Value {
    let (status, body) = exchange(router, api("GET", PREFERENCES, None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forward_preferences_save_pause_resume_and_forget_independently() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let local = free_port();
    let body = json!({"target":" vm1 ","port":5173,"local":local,"label":"Frontend"});
    let saved =
        json!({"target":"vm1","remote":5173,"local":local,"label":"Frontend","enabled":true});
    assert_eq!(
        exchange(&router, api("PUT", PREFERENCES, Some(&body))).await,
        (StatusCode::OK, saved.clone())
    );
    assert_eq!(
        preferences(&router).await,
        json!({"version":1,"forwards":[saved]})
    );
    assert!(host.asked().is_empty(), "metadata never dials");
    assert!(!held(local), "save never reserves");
    let key = forward("vm1", 5173);
    for _ in 0..2 {
        assert_eq!(
            exchange(&router, api("DELETE", FORWARDS, Some(&key)))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
    }
    let label = json!({"target":"vm1","port":5173,"label":"API"});
    let (_, edited) = exchange(&router, api("PUT", PREFERENCES, Some(&label))).await;
    assert_eq!(edited["enabled"], false);
    assert_eq!(edited["local"], local);
    assert_eq!(
        exchange(&router, api("PUT", PREFERENCES, Some(&key)))
            .await
            .1,
        edited
    );
    let (status, paused) = exchange(
        &router,
        api(
            "POST",
            FORWARDS,
            Some(&json!({"target":"vm1","port":5173,"automatic":true})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(paused["code"], "forward_paused");
    assert!(host.asked().is_empty());
    let (status, live) = exchange(&router, api("POST", FORWARDS, Some(&key))).await;
    assert_eq!(status, StatusCode::OK, "{live}");
    assert_eq!(live["local"], local);
    assert_eq!(through(local), "forwarded 5173");
    assert_eq!(preferences(&router).await["forwards"][0]["enabled"], true);
    for method in ["POST", "PUT"] {
        let path = if method == "POST" {
            FORWARDS
        } else {
            PREFERENCES
        };
        let (status, body) = exchange(
            &router,
            api(
                method,
                path,
                Some(&json!({"target":"vm1","port":5173,"local":if local == 1 {2} else {1}})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "active_mapping_conflict");
    }
    for _ in 0..2 {
        assert_eq!(
            exchange(&router, api("DELETE", PREFERENCES, Some(&key)))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(through(local), "forwarded 5173", "forget keeps active");
    }
    assert_eq!(
        preferences(&router).await,
        json!({"version":1,"forwards":[]})
    );
    host.end_forward(5173);
    host.fail_next_forwards(1);
    assert_eq!(
        listed_until(&router, |rows| rows[0]["state"] == "down").await[0]["state"],
        "down"
    );
    assert_eq!(
        listed_until(&router, |rows| rows[0]["state"] == "up").await[0]["local"],
        local,
        "forget keeps supervision"
    );
    assert_eq!(
        exchange(&router, api("DELETE", FORWARDS, Some(&key)))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert!(!held(local));
    assert_eq!(
        exchange(&router, api("DELETE", FORWARDS, Some(&key)))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forward_preferences_capture_actual_port_and_preserve_unrelated_entries() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let app = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let remote = busy.local_addr().expect("addr").port();
    let key = forward("vm1", remote);
    let (_, live) = exchange(&app, api("POST", FORWARDS, Some(&key))).await;
    assert_ne!(live["local"], remote);
    let (_, saved) = exchange(&app, api("PUT", PREFERENCES, Some(&key))).await;
    assert_eq!(saved["local"], live["local"]);
    assert_eq!(saved["label"], "");
    let api_key = forward("vm1", if remote == 8000 { 8001 } else { 8000 });
    let other = forward("vm2", 5173);
    let (a, b) = tokio::join!(
        exchange(&app, api("PUT", PREFERENCES, Some(&api_key))),
        exchange(&app, api("PUT", PREFERENCES, Some(&other)))
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(a.1["local"], api_key["port"]);
    exchange(&app, api("DELETE", FORWARDS, Some(&key))).await;
    let before = preferences(&app).await;
    let asked = host.asked();
    drop(app);
    let app = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    assert_eq!(preferences(&app).await, before);
    assert_eq!(host.asked(), asked, "reconstruction and reads stay passive");
    assert_eq!(
        exchange(&app, api("GET", FORWARDS, None)).await.1,
        json!([])
    );
}

#[tokio::test]
async fn forward_preference_guards_and_shapes_have_no_side_effects() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
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
    for method in ["PUT", "DELETE"] {
        for body in [
            json!([]),
            forward("-oProxyCommand=x", 3000),
            forward("vm1", 0),
            forward("vm1", 65536),
            forward("vm1", 30.5),
            json!({"target":"vm1","port":3000,"enabled":true}),
            json!({"target":"vm1","port":3000,"extra":1}),
        ] {
            assert_eq!(
                exchange(&router, api(method, PREFERENCES, Some(&body)))
                    .await
                    .0,
                StatusCode::BAD_REQUEST,
                "{method}: {body}"
            );
        }
    }
    for (field, value) in [
        ("local", json!(0)),
        ("local", json!(-1)),
        ("local", json!(null)),
        ("local", json!("3000")),
        ("label", json!(null)),
        ("label", json!(7)),
        ("label", json!("é".repeat(201))),
        ("lastError", json!({})),
    ] {
        let mut body = key.clone();
        body[field] = value;
        assert_eq!(
            exchange(&router, api("PUT", PREFERENCES, Some(&body)))
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "{body}"
        );
    }
    for body in [
        json!({"target":"vm1","port":3000,"automatic":true,"local":3000}),
        json!({"target":"vm1","port":3000,"automatic":null}),
        json!({"target":"vm1","port":3000,"local":null}),
    ] {
        assert_eq!(
            exchange(&router, api("POST", FORWARDS, Some(&body)))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(host.asked().is_empty());
    assert!(!dir.path().join("config/remote-forwards.json").exists());
    let label = json!({"target":"vm1","port":3000,"label":"é".repeat(200)});
    assert_eq!(
        exchange(&router, api("PUT", PREFERENCES, Some(&label)))
            .await
            .0,
        StatusCode::OK
    );
    let (_, saved) = exchange(
        &router,
        api(
            "PUT",
            PREFERENCES,
            Some(&json!({"target":"vm1","port":3000,"label":""})),
        ),
    )
    .await;
    assert_eq!(saved["label"], "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_forward_conflicts_and_explicit_alternates_preserve_durable_state() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let local = busy.local_addr().expect("addr").port();
    let key = forward("vm1", 5173);
    let saved = json!({"target":"vm1","port":5173,"local":local});
    exchange(&router, api("PUT", PREFERENCES, Some(&saved))).await;
    exchange(&router, api("DELETE", FORWARDS, Some(&key))).await;
    let (status, error) = exchange(&router, api("POST", FORWARDS, Some(&key))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "local_port_conflict");
    let (_, edited) = exchange(
        &router,
        api(
            "PUT",
            PREFERENCES,
            Some(&json!({"target":"vm1","port":5173,"label":"API"})),
        ),
    )
    .await;
    assert_eq!(edited["lastError"]["code"], "local_port_conflict");
    assert_eq!(edited["enabled"], false);
    assert_eq!(edited["local"], local);
    let (status, error) = exchange(&router, api("POST", FORWARDS, Some(&saved))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(
        error["code"], "local_port_conflict",
        "an explicit port never moves"
    );
    assert!(host.asked().is_empty());
    let bytes =
        std::fs::read_to_string(dir.path().join("config/remote-forwards.json")).expect("file");
    assert!(!bytes.contains("lastError"));
    let alternate = free_port();
    let alternate_body = json!({"target":"vm1","port":5173,"local":alternate});
    assert_eq!(
        exchange(&router, api("POST", FORWARDS, Some(&alternate_body)))
            .await
            .0,
        StatusCode::OK
    );
    let row = &preferences(&router).await["forwards"][0];
    assert_eq!(row["local"], alternate);
    assert_eq!(row["enabled"], true);
    assert!(row.get("lastError").is_none());
    assert_eq!(through(alternate), "forwarded 5173");
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", local)).is_ok(),
        "busy listener untouched"
    );
}

/// remote-screens, Automatic Port Forwarding (Automatic forwarding uses saved
/// port) and Port Forwards Through The Hub (Explicit local port and live
/// mismatch): an automatic request takes an enabled rule's saved port or
/// reports its conflict, an explicit local port on an unsaved mapping is used
/// exactly or refused, and none of them remembers or moves a mapping.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_and_explicit_requests_keep_saved_ports_and_remember_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let local = free_port();
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let busy = busy.local_addr().expect("addr").port();
    for (port, local) in [(5173, local), (8000, busy)] {
        let body = json!({"target":"vm1","port":port,"local":local});
        assert_eq!(
            exchange(&router, api("PUT", PREFERENCES, Some(&body)))
                .await
                .0,
            StatusCode::OK
        );
    }
    // A failed attempt may record its error on the rule; nothing else changes.
    let durable = |document: serde_json::Value| -> Vec<serde_json::Value> {
        let rows = document["forwards"].as_array().expect("forwards").clone();
        rows.into_iter()
            .map(|mut row| {
                row.as_object_mut().expect("row").remove("lastError");
                row
            })
            .collect()
    };
    let saved = durable(preferences(&router).await);
    let automatic = |port: u16| json!({"target":"vm1","port":port,"automatic":true});

    let (status, opened) = exchange(&router, api("POST", FORWARDS, Some(&automatic(5173)))).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["local"], local, "the saved port");
    let (status, error) = exchange(&router, api("POST", FORWARDS, Some(&automatic(8000)))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(
        error["code"], "local_port_conflict",
        "no other port is chosen"
    );
    let unsaved = free_port();
    let (status, opened) =
        exchange(&router, api("POST", FORWARDS, Some(&automatic(unsaved)))).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["local"], unsaved);
    let explicit = json!({"target":"vm1","port":9100,"local":busy});
    let (status, error) = exchange(&router, api("POST", FORWARDS, Some(&explicit))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"], "local_port_conflict");

    assert_eq!(durable(preferences(&router).await), saved);
    assert_eq!(
        host.asked(),
        [
            format!("forward vm1 5173 at {local}"),
            format!("forward vm1 {unsaved} at {unsaved}")
        ],
        "no ssh for a refused request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forward_preference_storage_failure_does_not_claim_stop_or_leave_uncommitted_opening() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let local = free_port();
    let key = forward("vm1", 5173);
    exchange(
        &router,
        api(
            "PUT",
            PREFERENCES,
            Some(&json!({"target":"vm1","port":5173,"local":local})),
        ),
    )
    .await;
    exchange(&router, api("POST", FORWARDS, Some(&key))).await;
    let config = dir.path().join("config");
    let backup = dir.path().join("backup");
    std::fs::rename(&config, &backup).expect("move config");
    std::fs::write(&config, b"blocked").expect("block writes");
    for (method, path) in [
        ("DELETE", FORWARDS),
        ("DELETE", PREFERENCES),
        ("PUT", PREFERENCES),
    ] {
        let (status, error) = exchange(&router, api(method, path, Some(&key))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error["code"], "storage_failure");
        assert_eq!(through(local), "forwarded 5173");
        assert_eq!(preferences(&router).await["forwards"][0]["enabled"], true);
    }
    std::fs::remove_file(&config).expect("unblock");
    std::fs::rename(&backup, &config).expect("restore");
    exchange(&router, api("DELETE", FORWARDS, Some(&key))).await;
    std::fs::rename(&config, &backup).expect("move");
    std::fs::write(&config, b"blocked").expect("block");
    let (status, error) = exchange(&router, api("POST", FORWARDS, Some(&key))).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error["code"], "storage_failure");
    assert!(
        !held(local),
        "uncommitted new tunnel cleaned before response"
    );
    assert_eq!(
        exchange(&router, api("GET", FORWARDS, None)).await.1,
        json!([])
    );
    assert_eq!(preferences(&router).await["forwards"][0]["enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn successful_ready_restores_only_its_enabled_saved_forwards_without_blocking_relay() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let hub = hub(&host, dir.path());
    let slow = free_port();
    let fast = free_port();
    hub.save_forward("vm1", 5173, Some(slow.into()), Some("Frontend".into()))
        .await
        .expect("save");
    hub.save_forward("vm1", 8000, Some(fast.into()), Some("API".into()))
        .await
        .expect("save");
    hub.save_forward("vm1", 9000, None, None)
        .await
        .expect("save");
    hub.stop_forward("vm1", 9000).await.expect("pause");
    hub.save_forward("vm2", 3000, None, None)
        .await
        .expect("other host");
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    hub.save_forward(
        "vm1",
        7000,
        Some(busy.local_addr().expect("addr").port().into()),
        None,
    )
    .await
    .expect("conflicted");
    assert!(host.asked().is_empty());
    let gate = host.pause_forward(5173);
    let served = Served::start(hub, dir.path()).await;
    assert!(
        host.asked().is_empty(),
        "server construction/authentication/read never dials"
    );
    let mut socket = served.open("vm1").await;
    assert_eq!(until_outcome(&mut socket).await.1["type"], "remote.ready");
    assert_eq!(
        json_of(next(&mut socket).await)["type"],
        "mux.welcome",
        "slow restore does not delay welcome"
    );
    gate.entered().await;
    tokio::time::timeout(WAIT, async {
        while !served.hub.forwards().iter().any(|row| row.remote == 8000) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("independent fast mapping");
    assert_eq!(through(fast), "forwarded 8000");
    let mut other_window = served.open("vm1").await;
    assert_eq!(
        until_outcome(&mut other_window).await.1["type"],
        "remote.ready"
    );
    assert_eq!(
        json_of(next(&mut other_window).await)["type"],
        "mux.welcome"
    );
    gate.release();
    tokio::time::timeout(WAIT, async {
        while served.hub.forwards().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("two forwards");
    assert_eq!(
        host.asked()
            .iter()
            .filter(|call| call.starts_with("forward vm1 5173 "))
            .count(),
        1
    );
    assert_eq!(
        served
            .hub
            .forward_preferences()
            .forwards
            .iter()
            .find(|row| row.remote == 7000)
            .expect("conflict")
            .last_error
            .as_ref()
            .expect("error")
            .code,
        "local_port_conflict"
    );
    assert!(host
        .asked()
        .iter()
        .all(|call| !call.starts_with("forward vm2 ") && !call.starts_with("forward vm1 9000 ")));
    socket
        .send(Message::Text("still relayed".into()))
        .await
        .expect("send");
    assert_eq!(
        next(&mut socket).await,
        Message::Text("still relayed".into()),
        "no hub error frame after ready"
    );
    socket.close(None).await.expect("close");
    other_window.close(None).await.expect("close");
    assert_eq!(
        through(slow),
        "forwarded 5173",
        "screen detachment keeps tunnel"
    );
    assert_eq!(through(fast), "forwarded 8000");
}

#[tokio::test]
async fn failed_host_setup_never_restores_saved_forwards() {
    for host_failure in [true, false] {
        let dir = tempfile::tempdir().expect("dir");
        let mut host = ScriptedHost::new(dir.path());
        if host_failure {
            host = host.preparing(Prepare::SshFails("Host key verification failed."));
        } else {
            host = host.with_protocol(PROTOCOL_VERSION + 1);
        }
        let hub = hub(&host, dir.path());
        hub.save_forward("vm1", 5173, None, None)
            .await
            .expect("save");
        let served = Served::start(hub, dir.path()).await;
        let mut socket = served.open("vm1").await;
        assert_eq!(until_outcome(&mut socket).await.1["type"], "remote.error");
        assert!(host
            .asked()
            .iter()
            .all(|call| !call.starts_with("forward ")));
        assert!(served.hub.forwards().is_empty());
    }
}

/// Metadata and authentication need no socket fixture: saved-but-disconnected
/// state must survive restart without so much as reaching the connector.
#[tokio::test]
async fn forward_preferences_stay_passive_across_authentication_and_reconstruction() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let app = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    let frontend = json!({"target":"vm1","port":5173,"local":15173,"label":"Frontend"});
    let api_rule = json!({"target":"vm1","port":8000,"local":18000,"label":"API"});
    for rule in [&frontend, &api_rule] {
        assert_eq!(
            exchange(&app, api("PUT", PREFERENCES, Some(rule))).await.0,
            StatusCode::OK
        );
    }
    let key = forward("vm1", 5173);
    assert_eq!(
        exchange(&app, api("DELETE", FORWARDS, Some(&key))).await.0,
        StatusCode::NO_CONTENT
    );
    let label = json!({"target":"vm1","port":5173,"label":"Paused frontend"});
    let (code, saved) = exchange(&app, api("PUT", PREFERENCES, Some(&label))).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(saved["enabled"], false);
    assert_eq!(saved["local"], 15173);
    let login = json!({"token":"process-token"});
    assert_eq!(
        status(&app, api("POST", "/api/session", Some(&login))).await,
        StatusCode::OK
    );
    let expected = preferences(&app).await;
    drop(app);
    let app = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    assert_eq!(preferences(&app).await, expected);
    assert_eq!(
        exchange(&app, api("GET", FORWARDS, None)).await.1,
        json!([])
    );
    assert_eq!(
        exchange(&app, api("DELETE", PREFERENCES, Some(&key)))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        preferences(&app).await["forwards"]
            .as_array()
            .expect("rows")
            .len(),
        1
    );
    assert!(
        host.asked().is_empty(),
        "no host setup or forward, including authentication/restart"
    );
}

#[tokio::test]
async fn forward_preferences_return_json_for_malformed_json_and_preserve_415() {
    let dir = tempfile::tempdir().expect("dir");
    let host = ScriptedHost::new(dir.path());
    let router = router(&state(ADDR, dir.path()), hub(&host, dir.path()));
    for method in ["PUT", "DELETE"] {
        let mut request = api(method, PREFERENCES, Some(&json!({})));
        *request.body_mut() = Body::from("{");
        let (code, body) = exchange(&router, request).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(body["error"].is_string());
        let mut request = api(method, PREFERENCES, Some(&json!({})));
        request
            .headers_mut()
            .insert("content-type", "text/plain".parse().expect("type"));
        assert_eq!(
            status(&router, request).await,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
    assert!(host.asked().is_empty());
}

/// control-api, Build Info: the WSL distribution only when the server runs in one.
#[test]
fn build_info_names_the_wsl_distribution_only_inside_wsl() {
    let plain = routes::build_body("0.1.0", "abc", "unknown", None);
    assert_eq!(
        plain,
        json!({"version": "0.1.0", "commit": "abc", "buildTime": "unknown"})
    );
    assert_eq!(
        routes::build_body("0.1.0", "abc", "unknown", Some("")),
        plain
    );
    assert_eq!(
        routes::build_body("0.1.0", "abc", "unknown", Some("Ubuntu"))["wslDistro"],
        "Ubuntu"
    );
}
