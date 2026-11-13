//! Routes of the local control socket, the WebSocket upgrade checks the
//! browser gateway applies, and the embedded frontend with its security
//! headers.
//!
//! Browser API routes are not defined here: the gateway (`super::gateway`)
//! derives them from the native operation catalog.

use std::collections::HashMap;

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{on, post, MethodFilter},
    Json, Router,
};
use serde_json::json;
use splitty_transport::budget::ConnectionBudget;

use super::operation;
use super::ControlState;
use crate::service::catalog::{self, Operation, Role, Scope};

/// Routes reachable over the local control socket.
///
/// A deliberately small subset of the HTTP API (security review
/// SR-05). Callers here are processes running inside a pane — build scripts,
/// package hooks, editor helpers — which previously received the master token
/// and could therefore reach *every* endpoint.
///
/// Multiplexer operations are what the in-shell `splitty` CLI legitimately
/// needs. Everything else is omitted on purpose, and the omissions matter:
///
/// - **Configuration and shell profiles** are executable configuration. Writing
///   them lets a process change what future panes run, turning a one-off script
///   into persistence.
/// - **Templates** are the same problem in another shape (SR-12).
/// - **Themes** read files from disk (SR-16).
/// - **Session and ticket minting** would hand out browser credentials.
///
/// This narrows what the socket offers, not what a process of the same user can
/// do: such a process can edit the configuration files directly, or read the
/// 0600 token file and call the HTTP API. What the socket guarantees is that no
/// credential is handed to panes implicitly, so nothing portable leaks through
/// `env` output, crash reports or CI logs.
///
/// The set is the catalog's `Control` scope: those operations go through the
/// same request translation as the browser gateway's and into the service's
/// native entry point, so a request is answered alike on both transports.
pub fn control_routes() -> Router<ControlState> {
    let mut router = Router::new().route("/api/daemon/stop", post(stop_daemon_instance));
    for entry in catalog::OPERATIONS
        .iter()
        .filter(|entry| entry.scope == Scope::Control)
    {
        let operation = entry.operation;
        let method = MethodFilter::try_from(
            Method::from_bytes(entry.method.as_bytes()).expect("catalog method"),
        )
        .expect("catalog filter");
        router = router.route(
            entry.path,
            on(
                method,
                move |State(state): State<ControlState>, request: Request| {
                    control_operation(state, operation, request)
                },
            ),
        );
    }
    router
}

/// One catalog operation over the control socket, with the owner's local
/// main role. Dropping the request (a closed connection) drops the call,
/// which ends a held wait such as a review.
async fn control_operation(
    state: ControlState,
    operation: Operation,
    request: Request,
) -> Response {
    let description = operation.description();
    if let Some(refused) = operation::refuse_content_type(description, request.headers()) {
        return refused;
    }
    let budget = ConnectionBudget::default();
    let (query, _parameter_bytes) =
        match operation::query_arguments(description, request.uri(), &budget) {
            Ok(query) => query.into_parts(),
            Err(response) => return *response,
        };
    let (mut parts, body) = request.into_parts();
    let path = match operation::path_arguments(&mut parts).await {
        Ok(path) => path,
        Err(response) => return *response,
    };
    let bytes = match operation::read_body(body, operation::body_limit(description)).await {
        Ok(bytes) => bytes,
        Err(response) => return *response,
    };
    match operation::native_request(operation, path, query, &bytes, Role::Main) {
        Ok(request) => state
            .service
            .execute(request, Role::Main)
            .await
            .into_response(),
        Err(response) => *response,
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StopDaemonInstance {
    instance: String,
}

/// Owner socket only. Address the service that accepted this connection,
/// never a PID read earlier from a discovery file.
async fn stop_daemon_instance(
    State(state): State<ControlState>,
    Json(request): Json<StopDaemonInstance>,
) -> Response {
    if state.server_mode != "daemon" || request.instance != state.instance {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"daemon instance changed; refresh discovery"})),
        )
            .into_response();
    }
    state.owner_shutdown.notify_one();
    Json(json!({"stopping":state.instance})).into_response()
}

// ── WebSocket ───────────────────────────────────────────────────────────────

/// Why a WebSocket upgrade is refused, before anything else happens for it:
/// a foreign or missing `Origin`, a `Sec-Fetch-Site` other than same-origin,
/// or no valid single-use ticket. The gateway's `/ws` and `/ws/remote` both
/// pass it.
pub(crate) fn refuse_upgrade(
    state: &super::BrowserState,
    params: &HashMap<String, String>,
    request: &axum::extract::Request,
) -> Option<Response> {
    // Redundant with `require_origin` in the middleware stack, on purpose. The
    // WebSocket is the highest-value endpoint in the process — it carries full
    // terminal control — and it is the one endpoint CORS does not protect. A
    // future refactor that reorders or drops a layer must not silently expose
    // it, so the check is repeated at the handler itself.
    match super::middleware::origin_allowed(request, &state.allowed_origins) {
        Some(true) => {}
        _ => return Some((StatusCode::FORBIDDEN, "Forbidden origin").into_response()),
    }
    if !super::middleware::fetch_site_allowed(request) {
        return Some((StatusCode::FORBIDDEN, "Cross-origin request").into_response());
    }

    // A single-use ticket, not the session cookie. WebSockets are exempt from
    // CORS, so cookie authentication would let any origin that can reach the
    // port open the socket. A ticket has to be fetched over HTTP first, and that
    // fetch is subject to the origin check *and* the same-origin rule on reading
    // the response — a foreign page can send the request but cannot read the
    // ticket back out. (security review SR-04.)
    let ticket = params.get("ticket").map(String::as_str).unwrap_or_default();
    if !state.sessions.redeem_ticket(ticket) {
        return Some((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
    }
    None
}

// ── Frontend static files ───────────────────────────────────────────────────

/// Prefixes that belong to the server's own surface rather than to the SPA.
///
/// Without this guard the SPA fallback answers *every* unmatched path with
/// `index.html` and HTTP 200, so a removed or misspelled endpoint looks like it
/// exists. That matters beyond tidiness: the routes disabled in SR-01
/// (`/proxy/...`, `/api/favicon`, ...) must be observably absent, and an API
/// client must be able to tell "no such endpoint" from "here is the app shell".
///
/// Removed for good (security review SR-01/SR-02/SR-08, see
/// docs/security/findings.md): the `/proxy/{port}` reverse proxy,
/// `/api/favicon`, `/api/frame-check` and `/api/webapps`. They existed only to
/// render foreign web content inside the application, which let a web context
/// with no local privileges reach the parent DOM, the auth cookie and the
/// terminal API; the two fetchers also reached arbitrary loopback and internal
/// services. Web content must not come back without its own origin and process.
const SERVER_PATH_PREFIXES: [&str; 3] = ["api/", "proxy/", "ws"];

/// Content-Security-Policy for the application document.
///
/// Written as a deny-by-default policy (security review SR-11):
/// `default-src 'none'` and then only what the app demonstrably needs. This
/// would not have prevented SR-01 — same-origin content was trusted by design
/// there — but it bounds the damage of any DOM-XSS class that survives, and it
/// is the machine-checked half of SR-01: `frame-src 'none'` plus
/// `frame-ancestors 'none'` mean that if an iframe path ever returns by
/// accident, the browser refuses it.
///
/// `connect-src` names the actual listening port, so the app can reach its own
/// API and WebSocket and nothing else — an exfiltration attempt to a foreign
/// host is blocked by the browser before it leaves the process.
///
/// `style-src 'unsafe-inline'` is a known and documented exception: the
/// imperative renderers set styles through `element.style`, which counts as
/// inline. Removing it means moving that styling into stylesheets first.
fn content_security_policy(listen_addr: &str) -> String {
    let port = listen_addr.rsplit(':').next().unwrap_or_default();
    let socket = listen_addr.parse::<std::net::SocketAddr>().ok();
    let endpoint = socket
        .map(|address| address.to_string())
        .unwrap_or_else(|| format!("127.0.0.1:{port}"));
    // `'wasm-unsafe-eval'` permits compiling WebAssembly and nothing else: no
    // `eval`, no `new Function`, no inline script. The ghostty renderer is a
    // WebAssembly module and the xterm image addon decodes sixel in one; under
    // plain `'self'` both failed to load.
    format!(
        "default-src 'none'; \
         script-src 'self' 'wasm-unsafe-eval'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; \
         font-src 'self'; \
         connect-src 'self' ws://{endpoint} ws://localhost:{port}; \
         frame-src 'none'; \
         frame-ancestors 'none'; \
         object-src 'none'; \
         base-uri 'none'; \
         form-action 'none'"
    )
}

/// Security headers applied to every frontend response.
fn security_headers(listen_addr: &str) -> [(&'static str, String); 6] {
    [
        (
            "content-security-policy",
            content_security_policy(listen_addr),
        ),
        // Responses must be interpreted as their declared type; sniffing can turn
        // a user-controlled file into script.
        ("x-content-type-options", "nosniff".to_string()),
        // Never leak the app URL — which is where a bootstrap code briefly lives
        // — to anything the user navigates to.
        ("referrer-policy", "no-referrer".to_string()),
        // Clipboard read is denied at the platform level as well as in the
        // terminal's OSC 52 handling (SR-20); the rest are simply unused.
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), clipboard-read=()".to_string(),
        ),
        // A window opened by a foreign page gets no handle on the app window, and
        // the app's own popups to other origins get none either (security review
        // SR-27, see docs/security/findings.md).
        ("cross-origin-opener-policy", "same-origin".to_string()),
        // No other origin may embed or read these responses through a no-cors
        // load, which also keeps them out of a foreign renderer's memory.
        ("cross-origin-resource-policy", "same-origin".to_string()),
    ]
}

pub async fn serve_frontend(
    State(state): State<super::BrowserState>,
    uri: axum::http::Uri,
) -> Response {
    let path = uri.path().trim_start_matches('/');

    if SERVER_PATH_PREFIXES
        .iter()
        .any(|prefix| path == prefix.trim_end_matches('/') || path.starts_with(prefix))
    {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let headers = security_headers(&state.listen_addr);

    // Serve non-index static assets by exact path
    if !path.is_empty() && path != "index.html" {
        if let Some(file) = crate::embed::asset(path) {
            let mut response = (StatusCode::OK, file.data.into_owned()).into_response();
            apply_headers(&mut response, &file.content_type, &headers);
            return response;
        }
    }

    // SPA fallback: themed index.html
    match crate::embed::themed_index_html() {
        Some(html) => {
            let mut response = (StatusCode::OK, html).into_response();
            apply_headers(&mut response, "text/html; charset=utf-8", &headers);
            response
        }
        None => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}

fn apply_headers(response: &mut Response, content_type: &str, headers: &[(&'static str, String)]) {
    use axum::http::HeaderValue;

    let map = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        map.insert("content-type", value);
    }
    for (name, value) in headers {
        if let Ok(value) = HeaderValue::from_str(value) {
            map.insert(*name, value);
        }
    }
}
