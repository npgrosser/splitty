//! All HTTP and WebSocket route definitions for the Splitty API.
//!
//! Registers endpoints for health checks, WebSocket upgrade, the remote screen
//! hub, home/server settings, theme resolution, templates CRUD, build info,
//! command broadcast, mux state/operations, stats, and the embedded frontend
//! SPA fallback.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{rejection::JsonRejection, Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, RequestExt, Router,
};
use serde_json::{json, Value};

use super::AppState;
use crate::embed::FrontendAssets;
use crate::hub::Hub;
use crate::service::operations::{self, MuxOpRequest, SessionSendRequest};
use crate::service::review;
use crate::ws;

/// Build the API router with all endpoints.
///
/// GET handlers must stay free of side effects. The Fetch Metadata check in
/// `require_origin` stops cross-port GETs in current engines, but an engine that
/// sends no `Sec-Fetch-Site` falls back to the `Origin` rules, and a no-cors GET
/// carries no `Origin`. For such an engine, a state-changing GET would be
/// reachable with the user's cookie from any page on another loopback port.
/// The service owns the hub; tests can assign a scripted host to it.
pub(crate) fn api_routes_with(hub: Arc<Hub>) -> Router<AppState> {
    let upgrade_hub = hub.clone();
    let hosts_hub = hub.clone();
    let directory_hub = hub.clone();
    let forgetting_host_hub = hub.clone();
    let host_preferences_hub = hub.clone();
    let updating_host_preferences_hub = hub.clone();
    let host_order_hub = hub.clone();
    let updating_host_order_hub = hub.clone();
    let remembering_hub = hub.clone();
    let catalog_hub = hub.clone();
    let updating_catalog_hub = hub.clone();
    let listing_hub = hub.clone();
    let forwarding_hub = hub.clone();
    let stopping_hub = hub.clone();
    let preferences_hub = hub.clone();
    let saving_forward_hub = hub.clone();
    let forgetting_forward_hub = hub.clone();
    let listing_reverse_hub = hub.clone();
    let reversing_hub = hub.clone();
    let stopping_reverse_hub = hub.clone();
    let reverse_preferences_hub = hub.clone();
    let saving_reverse_hub = hub.clone();
    let forgetting_reverse_hub = hub.clone();
    Router::new()
        // Health (no auth required — middleware skips /health)
        .route("/health", get(health))
        // Session bootstrap and WebSocket tickets
        .route("/api/session", post(super::session::exchange_bootstrap))
        .route("/api/ws-ticket", post(super::session::issue_ws_ticket))
        // Deliberately not in `control_routes`: see the handler's documentation.
        .route("/api/bootstrap", post(super::session::issue_bootstrap))
        // WebSocket
        .route("/ws", get(ws_upgrade))
        // The remote screen hub. Not in `control_routes`: a pane process must
        // not open SSH connections or forwards, stop a host's daemon, or read
        // the user's hosts, cached directories or remembered screens through it.
        .route(
            "/ws/remote",
            get(
                move |State(state): State<AppState>,
                      Query(params): Query<HashMap<String, String>>,
                      request: axum::extract::Request| {
                    remote_upgrade(upgrade_hub, state, params, request)
                },
            ),
        )
        .route(
            "/api/remote/directory",
            get(move || async move { Json(directory_hub.directory()) }),
        )
        .route(
            "/api/remote/hosts",
            get(move || async move { Json(hosts_hub.known_hosts()) }).delete(
                move |Json(document): Json<Value>| async move {
                    host_preference_response(
                        forgetting_host_hub.forget_host(document).map(|()| None),
                    )
                },
            ),
        )
        .route(
            "/api/remote/host-preferences",
            get(move || async move { Json(host_preferences_hub.host_preferences()) }).put(
                move |Json(document): Json<Value>| async move {
                    host_preference_response(
                        updating_host_preferences_hub
                            .update_host_preference(document)
                            .map(Some),
                    )
                },
            ),
        )
        .route(
            "/api/remote/host-order",
            get(move || async move { Json(host_order_hub.host_order()) }).put(
                move |Json(document): Json<Value>| async move {
                    match updating_host_order_hub.set_host_order(document) {
                        Ok(order) => Json(order).into_response(),
                        Err(error) => host_preference_response(Err(error)),
                    }
                },
            ),
        )
        .route(
            "/api/remote/screens",
            get(move || async move { Json(hub.remembered_screens()) }).put(
                move |Json(document): Json<Value>| async move {
                    remember_screens(&remembering_hub, document)
                },
            ),
        )
        .route(
            "/api/remote/catalog",
            get(move || async move { Json(catalog_hub.catalog()) }).put(
                move |Json(document): Json<Value>| async move {
                    update_catalog(&updating_catalog_hub, document)
                },
            ),
        )
        .route(
            "/api/remote/forwards",
            get(move || async move { Json(listing_hub.forwards()) })
                .post(move |Json(body): Json<Value>| async move {
                    open_forward(&forwarding_hub, body).await
                })
                .delete(move |Json(body): Json<Value>| async move {
                    stop_forward(&stopping_hub, body).await
                }),
        )
        .route(
            "/api/remote/forward-preferences",
            get(move || async move { Json(preferences_hub.forward_preferences()) })
                .put(move |body: Result<Json<Value>, JsonRejection>| async move {
                    match body {
                        Ok(Json(body)) => save_forward(&saving_forward_hub, body).await,
                        Err(error) => preference_rejected(error),
                    }
                })
                .delete(move |body: Result<Json<Value>, JsonRejection>| async move {
                    match body {
                        Ok(Json(body)) => forget_forward(&forgetting_forward_hub, body).await,
                        Err(error) => preference_rejected(error),
                    }
                }),
        )
        // Reverse forwards: ports of this machine shared with hosts. The
        // local server's own port is never shared.
        .route(
            "/api/remote/reverse-forwards",
            get(move || async move { Json(listing_reverse_hub.reverses()) })
                .post(move |body: Result<Json<Value>, JsonRejection>| async move {
                    match body {
                        Ok(Json(body)) => open_reverse(&reversing_hub, body).await,
                        Err(error) => preference_rejected(error),
                    }
                })
                .delete(move |body: Result<Json<Value>, JsonRejection>| async move {
                    match body {
                        Ok(Json(body)) => stop_reverse(&stopping_reverse_hub, body).await,
                        Err(error) => preference_rejected(error),
                    }
                }),
        )
        .route(
            "/api/remote/reverse-forward-preferences",
            get(move || async move { Json(reverse_preferences_hub.reverse_preferences()) })
                .put(move |body: Result<Json<Value>, JsonRejection>| async move {
                    match body {
                        Ok(Json(body)) => save_reverse(&saving_reverse_hub, body).await,
                        Err(error) => preference_rejected(error),
                    }
                })
                .delete(move |body: Result<Json<Value>, JsonRejection>| async move {
                    match body {
                        Ok(Json(body)) => forget_reverse(&forgetting_reverse_hub, body).await,
                        Err(error) => preference_rejected(error),
                    }
                }),
        )
        // Home (personal preferences)
        .route(
            "/api/home/settings",
            get(get_home_settings).patch(patch_home_settings),
        )
        .route("/api/home/theme", get(get_resolved_theme))
        .route("/api/home/themes", get(list_available_themes))
        .route(
            "/api/shell-profiles",
            get(get_shell_profiles).put(put_shell_profiles),
        )
        // Server (placeholder)
        .route(
            "/api/server/settings",
            get(get_server_settings).patch(patch_server_settings),
        )
        // Templates
        .route("/api/templates", get(list_templates))
        .route("/api/templates/extract/screen", post(extract_template))
        .route(
            "/api/templates/{slug}",
            get(get_template)
                .post(save_template)
                .delete(delete_template),
        )
        // Build info
        .route("/api/build", get(get_build))
        // Command broadcast
        .route("/api/command", post(post_command))
        // Mux API
        .route("/api/mux/state", get(get_mux_state))
        .route("/api/mux/screens", get(get_mux_screens))
        .route("/api/mux/screen/{id}", get(get_mux_screen))
        .route("/api/mux/op", post(post_mux_op))
        .route("/api/mux/neighbor", get(get_mux_neighbor))
        .route("/api/mux/status", get(get_mux_status))
        // Session API
        .route("/api/session/{id}/send", post(post_session_send))
        .route("/api/editor/open", post(post_editor_open))
        .route("/api/file/offer", post(post_file_offer))
        // The agent channel of viewer panes
        .route("/api/viewer/comments", get(get_viewer_comments))
        .route("/api/viewer/comments/reply", post(post_comment_reply))
        .route("/api/viewer/review/wait", post(post_review_wait))
        // Stats
        .route("/api/stats", get(get_stats))

    // REMOVED (security review SR-01/SR-02/SR-08, see docs/security/findings.md):
    // the `/proxy/{port}` reverse proxy, `/api/favicon`, `/api/frame-check` and
    // `/api/webapps`. They existed only to render foreign web content inside the
    // application, which let a web context with no local privileges reach the
    // parent DOM, the auth cookie and the terminal API; the two fetchers also
    // reached arbitrary loopback and internal services. Their paths stay in
    // `SERVER_PATH_PREFIXES` so they answer 404 instead of the app shell. Web
    // content must not come back without its own origin and process.
}

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
/// These handlers are the same functions the HTTP router uses; only the set of
/// registered paths differs.
pub fn control_routes() -> Router<AppState> {
    Router::new()
        .route("/api/daemon/stop", post(stop_daemon_instance))
        .route("/api/mux/state", get(get_mux_state))
        .route("/api/mux/screens", get(get_mux_screens))
        .route("/api/mux/screen/{id}", get(get_mux_screen))
        .route("/api/mux/op", post(post_mux_op))
        .route("/api/mux/neighbor", get(get_mux_neighbor))
        .route("/api/mux/status", get(get_mux_status))
        .route("/api/session/{id}/send", post(post_session_send))
        .route("/api/editor/open", post(post_editor_open))
        .route("/api/file/offer", post(post_file_offer))
        // What the user handed over on the viewer panes of the caller's
        // screen, agents' replies to it, and waits for the user's review.
        .route("/api/viewer/comments", get(get_viewer_comments))
        .route("/api/viewer/comments/reply", post(post_comment_reply))
        .route("/api/viewer/review/wait", post(post_review_wait))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StopDaemonInstance {
    instance: String,
}

/// Owner socket only. Address the service that accepted this connection,
/// never a PID read earlier from a discovery file.
async fn stop_daemon_instance(
    State(state): State<AppState>,
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

// ── Health ──────────────────────────────────────────────────────────────────

async fn health() -> &'static str {
    "ok"
}

// ── WebSocket ───────────────────────────────────────────────────────────────

/// Why a WebSocket upgrade is refused, before anything else happens for it:
/// a foreign or missing `Origin`, a `Sec-Fetch-Site` other than same-origin,
/// or no valid single-use ticket. `/ws` and `/ws/remote` both pass it.
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

async fn ws_upgrade(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    request: axum::extract::Request,
) -> Response {
    if let Some(refused) =
        refuse_upgrade(&axum::extract::FromRef::from_ref(&state), &params, &request)
    {
        return refused;
    }

    let ws = match request.extract::<WebSocketUpgrade, _>().await {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };

    ws.on_upgrade(move |socket| ws::handle_socket(socket, state.client_state.clone()))
}

/// `/ws/remote?ticket=…&target=…[&update=1]`: the checks of `/ws`, then the
/// hub serves the socket. The target is validated after the upgrade, so the
/// client reads why it is refused; no `ssh` starts for an invalid one.
async fn remote_upgrade(
    hub: Arc<Hub>,
    state: AppState,
    params: HashMap<String, String>,
    request: axum::extract::Request,
) -> Response {
    if let Some(refused) =
        refuse_upgrade(&axum::extract::FromRef::from_ref(&state), &params, &request)
    {
        return refused;
    }
    // Saved reverse forwards are restored on this socket's readiness.

    let ws = match request.extract::<WebSocketUpgrade, _>().await {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };

    let target = params.get("target").cloned().unwrap_or_default();
    // Stopping a daemon ends its sessions: only the exact value asks for it.
    let update = params.get("update").is_some_and(|value| value == "1");
    // Larger messages are refused by the WebSocket itself.
    ws.max_message_size(crate::hub::MAX_MESSAGE)
        .max_frame_size(crate::hub::MAX_MESSAGE)
        .on_upgrade(move |socket| async move { hub.serve(socket, target, update).await })
}

/// `PUT /api/remote/screens`: 200 with the stored list, 400 for a list
/// without the shape (nothing is written), 500 when the file cannot be
/// written. The body is taken as any JSON first, so a wrong shape is a 400
/// with its reason like a syntax error, not the extractor's 422.
fn remember_screens(hub: &Hub, document: Value) -> Response {
    operations::remember_screens(hub, document).into_response()
}

fn update_catalog(hub: &Hub, document: Value) -> Response {
    operations::update_catalog(hub, document).into_response()
}

fn host_preference_response(
    result: Result<Option<crate::hub::HostPreference>, crate::hub::PreferenceError>,
) -> Response {
    operations::host_preference_response(result).into_response()
}

async fn save_forward(hub: &Hub, body: Value) -> Response {
    operations::save_forward(hub, body).await.into_response()
}

async fn forget_forward(hub: &Hub, body: Value) -> Response {
    operations::forget_forward(hub, body).await.into_response()
}

async fn open_forward(hub: &Hub, body: Value) -> Response {
    operations::open_forward(hub, body).await.into_response()
}

async fn stop_forward(hub: &Hub, body: Value) -> Response {
    operations::stop_forward(hub, body).await.into_response()
}

async fn open_reverse(hub: &Hub, body: Value) -> Response {
    operations::open_reverse(hub, body).await.into_response()
}

async fn stop_reverse(hub: &Hub, body: Value) -> Response {
    operations::stop_reverse(hub, body).await.into_response()
}

async fn save_reverse(hub: &Hub, body: Value) -> Response {
    operations::save_reverse(hub, body).await.into_response()
}

async fn forget_reverse(hub: &Hub, body: Value) -> Response {
    operations::forget_reverse(hub, body).await.into_response()
}

async fn get_resolved_theme(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    Json(
        operations::get_resolved_theme(&state.service, params)
            .await
            .0,
    )
}

async fn list_available_themes(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(operations::list_available_themes(&state.service).await.0)
}

async fn get_home_settings(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_home_settings(&state.service).await.0)
}

async fn patch_home_settings(
    State(state): State<AppState>,
    Json(partial): Json<Value>,
) -> Response {
    operations::patch_home_settings(&state.service, partial)
        .await
        .into_response()
}

#[cfg(test)]
pub(super) fn apply_home_settings(state: &AppState, updated: splitty_core::config::HomeConfig) {
    operations::apply_home_settings(&state.service, updated);
}

async fn get_shell_profiles(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_shell_profiles(&state.service).await.0)
}

async fn put_shell_profiles(
    State(state): State<AppState>,
    Json(update): Json<splitty_core::shell_profiles::ShellProfilesState>,
) -> Response {
    operations::put_shell_profiles(&state.service, update)
        .await
        .into_response()
}

async fn get_server_settings(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_server_settings(&state.service).await.0)
}

async fn patch_server_settings(
    State(_state): State<AppState>,
    Json(_partial): Json<Value>,
) -> Response {
    operations::patch_server_settings(&_state.service, _partial)
        .await
        .into_response()
}

async fn extract_template(
    State(state): State<AppState>,
    Json(body): Json<operations::ExtractTemplateRequest>,
) -> Response {
    operations::extract_template(&state.service, body)
        .await
        .into_response()
}

async fn list_templates(State(state): State<AppState>) -> Response {
    operations::list_templates(&state.service)
        .await
        .into_response()
}

async fn get_template(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    operations::get_template(&state.service, slug)
        .await
        .into_response()
}

async fn save_template(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    operations::save_template(&state.service, slug, body)
        .await
        .into_response()
}

async fn delete_template(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    operations::delete_template(&state.service, slug)
        .await
        .into_response()
}

async fn get_build(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_build(&state.service).await.0)
}

#[cfg(test)]
pub(super) use crate::service::operations::build_body;

async fn post_command(
    State(state): State<AppState>,
    Json(cmd): Json<splitty_core::protocol::ClientCommand>,
) -> Json<Value> {
    Json(operations::post_command(&state.service, cmd).await.0)
}

async fn get_mux_state(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_mux_state(&state.service).await.0)
}

async fn get_mux_screen(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    operations::get_mux_screen(&state.service, id)
        .await
        .into_response()
}

async fn post_mux_op(State(state): State<AppState>, Json(req): Json<MuxOpRequest>) -> Response {
    operations::post_mux_op(&state.service, req)
        .await
        .into_response()
}

async fn get_mux_neighbor(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    operations::get_mux_neighbor(&state.service, params)
        .await
        .into_response()
}

async fn get_mux_screens(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_mux_screens(&state.service).await.0)
}

async fn get_mux_status(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_mux_status(&state.service).await.0)
}

async fn post_session_send(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SessionSendRequest>,
) -> Response {
    operations::post_session_send(&state.service, id, req)
        .await
        .into_response()
}

async fn get_stats(State(state): State<AppState>) -> Json<Value> {
    Json(operations::get_stats(&state.service).await.0)
}

async fn post_file_offer(
    State(state): State<AppState>,
    Json(input): Json<crate::client_session::files::OfferRequest>,
) -> Response {
    operations::post_file_offer(&state.service, input)
        .await
        .into_response()
}

async fn post_editor_open(
    State(state): State<AppState>,
    Json(input): Json<crate::client_session::editor::OpenRequest>,
) -> Response {
    operations::post_editor_open(&state.service, input)
        .await
        .into_response()
}

async fn get_viewer_comments(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    review::get_viewer_comments(&state.service, params)
        .await
        .into_response()
}

async fn post_comment_reply(
    State(state): State<AppState>,
    body: Result<Json<Value>, JsonRejection>,
) -> Response {
    match body {
        Ok(Json(body)) => review::post_comment_reply(&state.service, body).await,
        Err(error) => review::invalid_body(error.body_text()),
    }
    .into_response()
}

/// Held until the user answers or the viewer pane closes; a closed
/// connection drops the handler, which ends the wait.
async fn post_review_wait(
    State(state): State<AppState>,
    body: Result<Json<Value>, JsonRejection>,
) -> Response {
    match body {
        Ok(Json(body)) => review::post_review_wait(&state.service, body).await,
        Err(error) => review::invalid_body(error.body_text()),
    }
    .into_response()
}

fn preference_rejected(error: JsonRejection) -> Response {
    if error.status() == StatusCode::BAD_REQUEST
        || error.status() == StatusCode::UNPROCESSABLE_ENTITY
    {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.body_text()})),
        )
            .into_response()
    } else {
        error.into_response()
    }
}

// ── Frontend static files ───────────────────────────────────────────────────

/// Prefixes that belong to the server's own surface rather than to the SPA.
///
/// Without this guard the SPA fallback answers *every* unmatched path with
/// `index.html` and HTTP 200, so a removed or misspelled endpoint looks like it
/// exists. That matters beyond tidiness: the routes disabled in SR-01
/// (`/proxy/...`, `/api/favicon`, ...) must be observably absent, and an API
/// client must be able to tell "no such endpoint" from "here is the app shell".
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
        if let Some(file) = FrontendAssets::get(path) {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let mut response = (StatusCode::OK, file.data.into_owned()).into_response();
            apply_headers(&mut response, mime.as_ref(), &headers);
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
