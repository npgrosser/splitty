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
use serde::Deserialize;
use serde_json::{json, Value};

use splitty_core::ids::*;
use splitty_core::mux::{self, Direction, ScreenOp};
use splitty_core::template::TemplateStore;

use super::AppState;
use crate::embed::FrontendAssets;
use crate::hub::Hub;
use crate::ws;

/// Build the API router with all endpoints.
///
/// GET handlers must stay free of side effects. The Fetch Metadata check in
/// `require_origin` stops cross-port GETs in current engines, but an engine that
/// sends no `Sec-Fetch-Site` falls back to the `Origin` rules, and a no-cors GET
/// carries no `Origin`. For such an engine, a state-changing GET would be
/// reachable with the user's cookie from any page on another loopback port.
pub fn api_routes() -> Router<AppState> {
    api_routes_with(Arc::new(Hub::system()))
}

/// [`api_routes`] with the remote screen hub given, so tests can point it at a
/// scripted host and their own configuration directory.
pub(crate) fn api_routes_with(hub: Arc<Hub>) -> Router<AppState> {
    let upgrade_hub = hub.clone();
    let hosts_hub = hub.clone();
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
                .post(
                    move |State(state): State<AppState>,
                          body: Result<Json<Value>, JsonRejection>| async move {
                        reserve_own_port(&reversing_hub, &state);
                        match body {
                            Ok(Json(body)) => open_reverse(&reversing_hub, body).await,
                            Err(error) => preference_rejected(error),
                        }
                    },
                )
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
                .put(
                    move |State(state): State<AppState>,
                          body: Result<Json<Value>, JsonRejection>| async move {
                        reserve_own_port(&saving_reverse_hub, &state);
                        match body {
                            Ok(Json(body)) => save_reverse(&saving_reverse_hub, body).await,
                            Err(error) => preference_rejected(error),
                        }
                    },
                )
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
        .route("/api/mux/state", get(get_mux_state))
        .route("/api/mux/screens", get(get_mux_screens))
        .route("/api/mux/screen/{id}", get(get_mux_screen))
        .route("/api/mux/op", post(post_mux_op))
        .route("/api/mux/neighbor", get(get_mux_neighbor))
        .route("/api/mux/status", get(get_mux_status))
        .route("/api/session/{id}/send", post(post_session_send))
        .route("/api/editor/open", post(post_editor_open))
        .route("/api/file/offer", post(post_file_offer))
}

// ── Health ──────────────────────────────────────────────────────────────────

async fn health() -> &'static str {
    "ok"
}

// ── WebSocket ───────────────────────────────────────────────────────────────

/// Why a WebSocket upgrade is refused, before anything else happens for it:
/// a foreign or missing `Origin`, a `Sec-Fetch-Site` other than same-origin,
/// or no valid single-use ticket. `/ws` and `/ws/remote` both pass it.
fn refuse_upgrade(
    state: &AppState,
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
    if let Some(refused) = refuse_upgrade(&state, &params, &request) {
        return refused;
    }

    let ws = match request.extract::<WebSocketUpgrade, _>().await {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };

    ws.on_upgrade(move |socket| ws::handle_socket(socket, state.ws_state.clone()))
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
    if let Some(refused) = refuse_upgrade(&state, &params, &request) {
        return refused;
    }
    // Saved reverse forwards are restored on this socket's readiness.
    reserve_own_port(&hub, &state);

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
    use crate::hub::RememberError;

    match hub.remember_screens(document) {
        Ok(remembered) => Json(remembered).into_response(),
        Err(error @ RememberError::Invalid(_)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
        Err(error @ RememberError::Write(_)) => {
            tracing::warn!("remote screens: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
                .into_response()
        }
    }
}

/// A catalog update has the same error contract as remembered screens.
fn update_catalog(hub: &Hub, document: Value) -> Response {
    use crate::hub::CatalogError;

    match hub.update_catalog(document) {
        Ok(host) => Json(host).into_response(),
        Err(error @ CatalogError::Invalid(_)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
        Err(error @ CatalogError::Hidden) => (
            StatusCode::CONFLICT,
            Json(json!({"error": error.to_string()})),
        )
            .into_response(),
        Err(error @ CatalogError::Write(_)) => {
            tracing::warn!("remote catalog: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": error.to_string()})),
            )
                .into_response()
        }
    }
}

fn host_preference_response(
    result: Result<Option<crate::hub::HostPreference>, crate::hub::PreferenceError>,
) -> Response {
    use crate::hub::PreferenceError;
    match result {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            let status = match &error {
                PreferenceError::Invalid(_) => StatusCode::BAD_REQUEST,
                PreferenceError::Write(_) => {
                    tracing::warn!("remote host metadata: {error}");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            (status, Json(json!({"error": error.to_string()}))).into_response()
        }
    }
}

/// The body of `POST` and `DELETE /api/remote/forwards`.
#[derive(Deserialize)]
struct ForwardRequest {
    target: String,
    port: i64,
}

#[derive(Deserialize)]
struct OpenForwardRequest {
    target: String,
    port: i64,
    #[serde(default, deserialize_with = "supplied")]
    local: Option<i64>,
    #[serde(default)]
    automatic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveForwardRequest {
    target: String,
    port: i64,
    #[serde(default, deserialize_with = "supplied")]
    local: Option<i64>,
    #[serde(default, deserialize_with = "supplied")]
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgetForwardRequest {
    target: String,
    port: i64,
}

/// Optional means omitted, not null. Supplied fields retain their types.
fn supplied<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// Syntax errors use the preference API's JSON error shape. Content-type and
/// body-limit rejections retain Axum's status and response behavior.
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

async fn save_forward(hub: &Hub, body: Value) -> Response {
    let request: SaveForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub
        .save_forward(&request.target, request.port, request.local, request.label)
        .await
    {
        Ok(row) => Json(row).into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

async fn forget_forward(hub: &Hub, body: Value) -> Response {
    let request: ForgetForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub.forget_forward(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// The 400 for a body that is no [`ForwardRequest`]. The body is taken as any
/// JSON first, so a wrong shape is a 400 with its reason, not the extractor's
/// 422.
fn refuse_forward_request(error: &serde_json::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": format!("expected {{\"target\", \"port\"}}: {error}")})),
    )
        .into_response()
}

/// `POST /api/remote/forwards`: 200 with the forward once it is verified, or
/// the one that exists.
async fn open_forward(hub: &Hub, body: Value) -> Response {
    let request: OpenForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub
        .open_forward_with(
            &request.target,
            request.port,
            request.local,
            request.automatic,
        )
        .await
    {
        Ok(forward) => Json(forward).into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// `DELETE /api/remote/forwards`: 204 once the forward's `ssh` has ended.
async fn stop_forward(hub: &Hub, body: Value) -> Response {
    let request: ForwardRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_forward_request(&error),
    };
    match hub.stop_forward(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// Tell the hub which local port this server listens on, so that it is
/// never shared with a host.
fn reserve_own_port(hub: &Hub, state: &AppState) {
    if let Some(port) = state
        .listen_addr
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
    {
        hub.reserve_local_port(port);
    }
}

/// The body of `POST /api/remote/reverse-forwards`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenReverseRequest {
    target: String,
    port: i64,
    local: i64,
}

/// The body of `DELETE` on both reverse-forward APIs.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReverseKey {
    target: String,
    port: i64,
}

/// The body of `PUT /api/remote/reverse-forward-preferences`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveReverseRequest {
    target: String,
    port: i64,
    #[serde(default, deserialize_with = "supplied")]
    local: Option<i64>,
}

/// The 400 for a reverse-forward body without the expected fields.
fn refuse_reverse_request(expected: &str, error: &serde_json::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": format!("expected {expected}: {error}")})),
    )
        .into_response()
}

/// `POST /api/remote/reverse-forwards`: 200 with the reverse forward once the
/// host's relay listens, or the one that exists.
async fn open_reverse(hub: &Hub, body: Value) -> Response {
    let request: OpenReverseRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port", "local"}"#, &error),
    };
    match hub
        .open_reverse(&request.target, request.port, request.local)
        .await
    {
        Ok(reverse) => Json(reverse).into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// `DELETE /api/remote/reverse-forwards`: 204 once the reverse forward's
/// `ssh` has ended.
async fn stop_reverse(hub: &Hub, body: Value) -> Response {
    let request: ReverseKey = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port"}"#, &error),
    };
    match hub.stop_reverse(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

async fn save_reverse(hub: &Hub, body: Value) -> Response {
    let request: SaveReverseRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port", "local"?}"#, &error),
    };
    match hub
        .save_reverse(&request.target, request.port, request.local)
        .await
    {
        Ok(row) => Json(row).into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

async fn forget_reverse(hub: &Hub, body: Value) -> Response {
    let request: ReverseKey = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => return refuse_reverse_request(r#"{"target", "port"}"#, &error),
    };
    match hub.forget_reverse(&request.target, request.port).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => forward_failed(&request.target, error),
    }
}

/// A failed forward request: 400 for an invalid target or port, 404 for a
/// forward that does not exist, 409 for a host at its limit, and for a
/// forward that could not be opened 502 (500 for a local problem) with the
/// code and hint of the hub's errors.
fn forward_failed(target: &str, error: crate::hub::ForwardError) -> Response {
    use crate::hub::{Code, ForwardError};

    let status = match &error {
        ForwardError::Target(_) | ForwardError::Port(_) | ForwardError::Invalid(_) => {
            StatusCode::BAD_REQUEST
        }
        ForwardError::NotFound { .. } => StatusCode::NOT_FOUND,
        ForwardError::Limit { .. }
        | ForwardError::PreferenceLimit
        | ForwardError::LocalConflict { .. }
        | ForwardError::ActiveConflict { .. }
        | ForwardError::Paused
        | ForwardError::HostPortConflict { .. }
        | ForwardError::LocalPortReserved { .. } => StatusCode::CONFLICT,
        ForwardError::Storage(_) | ForwardError::OperationEnded => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        ForwardError::Failed(_) => StatusCode::BAD_GATEWAY,
    };
    let ForwardError::Failed(failed) = &error else {
        let mut body = json!({"error": error.to_string()});
        if let Some(code) = error.code() {
            body["code"] = json!(code);
        }
        return (status, Json(body)).into_response();
    };
    let failure = failed.failure(target);
    tracing::info!(
        "remote screen hub: a forward to {target:?} failed ({}): {}",
        failure.code.as_str(),
        failure.message
    );
    let status = if failure.code == Code::Internal {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        status
    };
    let mut body = json!({"error": failure.message, "code": failure.code});
    if let Some(hint) = failure.hint {
        body["hint"] = json!(hint);
    }
    (status, Json(body)).into_response()
}

// ── Theme ───────────────────────────────────────────────────────────────────

async fn get_resolved_theme(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let name = params
        .get("name")
        .cloned()
        .unwrap_or_else(|| state.config.read().unwrap().appearance.theme.clone());
    let theme = splitty_core::theme::resolve_theme(&name, &state.themes_dir);
    Json(serde_json::to_value(&theme).unwrap_or(json!({})))
}

async fn list_available_themes(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(splitty_core::theme::list_themes(&state.themes_dir))
}

// ── Home Settings ────────────────────────────────────────────────────────────

async fn get_home_settings(State(state): State<AppState>) -> Json<Value> {
    let cfg = state.config.read().unwrap();
    Json(serde_json::to_value(&*cfg).unwrap_or(json!({})))
}

async fn patch_home_settings(
    State(state): State<AppState>,
    Json(partial): Json<Value>,
) -> Response {
    match splitty_core::config::update(partial) {
        Ok(updated) => {
            apply_home_settings(&state, updated.clone());
            Json(serde_json::to_value(&updated).unwrap_or(json!({}))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Settings that were saved take effect in the running server: the shell
/// profiles, the settings themselves and the PTY size policy.
pub(super) fn apply_home_settings(state: &AppState, updated: splitty_core::config::HomeConfig) {
    let shell_state = splitty_core::shell_profiles::state_from_settings(&updated.shells);
    *state.shell_profiles.write().unwrap() = shell_state;
    state.ws_state.dispatcher.configure_sizes(
        splitty_core::protocol::sizing::SizeConfig::from_settings(
            &updated.terminal.size_policy,
            updated.terminal.size_idle_seconds,
        ),
    );
    *state.config.write().unwrap() = updated;
}

// ── Shell Profiles ─────────────────────────────────────────────────────────

async fn get_shell_profiles(State(state): State<AppState>) -> Json<Value> {
    let profiles = state.shell_profiles.read().unwrap();
    Json(serde_json::to_value(&*profiles).unwrap_or(json!({})))
}

async fn put_shell_profiles(
    State(state): State<AppState>,
    Json(update): Json<splitty_core::shell_profiles::ShellProfilesState>,
) -> Response {
    let detection = splitty_core::shell_profiles::detect_shell_profiles(
        &splitty_core::shell_profiles::DetectionEnvironment::current(),
    );
    let mut candidate = state.config.read().unwrap().clone();
    let shell_state = match splitty_core::shell_profiles::apply_shell_profiles_update(
        &mut candidate.shells,
        update,
        &detection,
    ) {
        Ok(profiles) => profiles,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    if let Err(e) = splitty_core::config::save(&candidate) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }

    *state.config.write().unwrap() = candidate;
    *state.shell_profiles.write().unwrap() = shell_state.clone();
    Json(serde_json::to_value(shell_state).unwrap_or(json!({}))).into_response()
}

// ── Server Settings ──────────────────────────────────────────────────────────

async fn get_server_settings(State(state): State<AppState>) -> Json<Value> {
    let cfg = state.server_config.read().unwrap();
    Json(serde_json::to_value(&*cfg).unwrap_or(json!({})))
}

async fn patch_server_settings(
    State(_state): State<AppState>,
    Json(_partial): Json<Value>,
) -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "server settings are not yet configurable"})),
    )
        .into_response()
}

// ── Templates ───────────────────────────────────────────────────────────────

async fn list_templates(State(state): State<AppState>) -> Response {
    match state.template_store.list() {
        Ok(summaries) => {
            Json(serde_json::to_value(&summaries).unwrap_or(json!([]))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn get_template(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    match state.template_store.get(&slug) {
        Ok(template) => Json(serde_json::to_value(&template).unwrap_or(json!({}))).into_response(),
        Err(splitty_core::template::TemplateError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "template not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn save_template(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let template: splitty_core::template::Template = match serde_json::from_value(body) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };

    match state.template_store.save(&slug, &template) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn delete_template(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    match state.template_store.delete(&slug) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(splitty_core::template::TemplateError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "template not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Build info ──────────────────────────────────────────────────────────────

async fn get_build(State(state): State<AppState>) -> Json<Value> {
    let distro = std::env::var("WSL_DISTRO_NAME").ok();
    Json(build_body(
        &state.version,
        &state.commit,
        &state.build_time,
        distro.as_deref(),
    ))
}

/// `GET /api/build`. Inside WSL it names the distribution, through which the
/// UI opens a local folder in VS Code (`wsl+<distro>`).
pub(super) fn build_body(
    version: &str,
    commit: &str,
    build_time: &str,
    wsl_distro: Option<&str>,
) -> Value {
    let mut build = json!({
        "version": version,
        "commit": commit,
        "buildTime": build_time,
    });
    if let Some(distro) = wsl_distro.filter(|d| !d.is_empty()) {
        build["wslDistro"] = Value::String(distro.to_owned());
    }
    build
}

// ── Command broadcast ───────────────────────────────────────────────────────

async fn post_command(
    State(state): State<AppState>,
    Json(cmd): Json<splitty_core::protocol::ClientCommand>,
) -> Json<Value> {
    let sent = state.ws_state.broadcast_command(cmd).await;
    Json(json!({ "ok": true, "sent": sent }))
}

// ── Mux API ─────────────────────────────────────────────────────────────────

/// Scratchpads are a window's alone (`session-scratchpad`): neither this API
/// nor the control socket, which every pane reaches, hands their text out.
fn without_scratchpads(screen: &mut mux::ScreenState) {
    for session in screen.sessions.values_mut() {
        session.scratchpad.clear();
    }
}

async fn get_mux_state(State(state): State<AppState>) -> Json<Value> {
    let mut ws = state.mux_store.get_workspace_state();
    ws.screens.values_mut().for_each(without_scratchpads);
    Json(serde_json::to_value(&ws).unwrap_or(json!({})))
}

async fn get_mux_screen(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let screen_id = ScreenId::from(id.as_str());
    match state.mux_store.get_screen(&screen_id) {
        Some(mut screen) => {
            without_scratchpads(&mut screen);
            Json(serde_json::to_value(&screen).unwrap_or(json!({}))).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "screen not found"})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MuxOpRequest {
    screen_id: String,
    op: Value,
}

async fn post_mux_op(State(state): State<AppState>, Json(req): Json<MuxOpRequest>) -> Response {
    if req.op.get("type").and_then(|t| t.as_str()) == Some("set_scratchpad") {
        // Only a window sets a scratchpad (`session-scratchpad`).
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": "set_scratchpad is only accepted from a window"})),
        )
            .into_response();
    }
    if let Some(result) = state
        .ws_state
        .jobs
        .mux(&req.screen_id, req.op.clone())
        .await
    {
        return match result {
            Ok(access) => super::jobs::respond(access, StatusCode::OK),
            Err(error) => error.into_response(),
        };
    }
    let mut screen_id = ScreenId::from(req.screen_id.as_str());
    let op_type = req.op.get("type").and_then(|t| t.as_str()).unwrap_or("");

    // The target of a pane moved to another screen, nudged with its source.
    let mut moved_to = None;
    // Workspace-level ops vs screen-level ops
    let result = match op_type {
        "move_pane_to_screen" => {
            let field = |name: &str| req.op.get(name).and_then(|v| v.as_str());
            let (Some(pane_id), Some(target)) = (field("paneId"), field("targetScreenId")) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"ok": false, "error": "invalid op: paneId and targetScreenId are required"})),
                )
                    .into_response();
            };
            let target = ScreenId::from(target);
            moved_to = Some(target.clone());
            state.mux_store.dispatch(mux::Op::MovePaneToScreen {
                screen_id: screen_id.clone(),
                pane_id: splitty_core::ids::PaneId::from(pane_id),
                target_screen_id: target,
                create_screen: req.op.get("createScreen").and_then(|v| v.as_bool()) == Some(true),
                screen_name: field("screenName").map(String::from),
            })
        }
        "create_screen" => {
            let name = req
                .op
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from);
            let sid = req
                .op
                .get("screenId")
                .and_then(|v| v.as_str())
                .map(ScreenId::from)
                .unwrap_or_else(|| ScreenId::with_prefix("screen"));
            screen_id = sid.clone();
            let pid = req
                .op
                .get("newPaneId")
                .and_then(|v| v.as_str())
                .map(PaneId::from);
            let shell_profile_id = req
                .op
                .get("shellProfileId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| {
                    state
                        .shell_profiles
                        .read()
                        .unwrap()
                        .default_profile()
                        .map(|p| p.id.clone())
                });
            state.mux_store.dispatch(mux::Op::CreateScreen {
                name,
                screen_id: Some(sid),
                new_pane_id: pid,
                shell_profile_id,
            })
        }
        "close_screen" => state.mux_store.dispatch(mux::Op::CloseScreen {
            screen_id: screen_id.clone(),
        }),
        "rename_screen" => {
            let name = req
                .op
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            state.mux_store.dispatch(mux::Op::RenameScreen {
                screen_id: screen_id.clone(),
                name,
            })
        }
        "set_active_screen" => state.mux_store.dispatch(mux::Op::SetActiveScreen {
            screen_id: screen_id.clone(),
        }),
        "reorder_screens" => state.mux_store.dispatch(mux::Op::ReorderScreens {
            screen_order: req.op["screenOrder"]
                .as_array()
                .map(|order| {
                    order
                        .iter()
                        .filter_map(|id| id.as_str().map(ScreenId::from))
                        .collect()
                })
                .unwrap_or_default(),
        }),
        _ => {
            // A shell whose pane moved still names its old screen
            // ($SPLITTY_SCREEN_ID): the op follows the pane.
            if let Some(pane) = req.op.get("paneId").and_then(|v| v.as_str()) {
                let pane = splitty_core::ids::PaneId::from(pane);
                let holds = state
                    .mux_store
                    .get_screen(&screen_id)
                    .is_some_and(|screen| screen.panes.contains_key(&pane));
                if !holds {
                    if let Some(actual) = state.mux_store.screen_of_movable_pane(&pane) {
                        screen_id = actual;
                    }
                }
            }
            let screen_op: ScreenOp = match serde_json::from_value(req.op) {
                Ok(op) => op,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"ok": false, "error": format!("invalid op: {}", e)})),
                    )
                        .into_response();
                }
            };
            state.mux_store.dispatch_screen(&screen_id, screen_op)
        }
    };

    match result {
        Ok(effects) => {
            if let Err(error) = state.ws_state.jobs.stop_effects(&effects).await {
                return error.into_response();
            }
            let outcomes = state
                .ws_state
                .effect_runner
                .execute(effects, &state.ws_state.env)
                .await;
            let spawned: Vec<_> = outcomes.iter().filter_map(|o| o.spawned()).collect();
            let mut batch = Vec::new();
            for (_, pane, session) in &spawned {
                if let Ok(text) =
                    serde_json::to_string(&splitty_core::protocol::ServerEvent::mux_pane_spawned(
                        pane.to_string(),
                        session.to_string(),
                    ))
                {
                    batch.push(text);
                }
            }
            if let Some(target) = &moved_to {
                if let Ok(text) =
                    serde_json::to_string(&splitty_core::protocol::ServerEvent::mux_nudge(
                        "cli_op".into(),
                        Some(target.to_string()),
                    ))
                {
                    batch.push(text);
                }
            }
            if let Ok(text) =
                serde_json::to_string(&splitty_core::protocol::ServerEvent::mux_nudge(
                    "cli_op".into(),
                    Some(screen_id.to_string()),
                ))
            {
                batch.push(text);
            }
            state.ws_state.broadcast_events(batch, None).await;
            if let Some(failed) = outcomes.iter().find(|o| o.result.is_err()) {
                let mut value = json!({"ok":false,"code":"spawn_failed","error":failed.result.as_ref().err().map(ToString::to_string),"screenId":screen_id});
                if let splitty_core::mux::Effect::SpawnSession {
                    pane_id,
                    session_id,
                    ..
                } = &failed.effect
                {
                    value["paneId"] = json!(pane_id);
                    value["sessionId"] = json!(session_id);
                }
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(value)).into_response();
            }

            let screen = state.mux_store.get_screen(&screen_id);
            let revision = screen.map(|s| s.revision).unwrap_or(0);

            Json(json!({
                "ok": true,
                "revision": revision,
                "screenId": screen_id,
                "created": spawned.iter().map(|(_,pane,session)|json!({"paneId":pane,"sessionId":session})).collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn get_mux_neighbor(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let screen_id = params.get("screen").cloned().unwrap_or_default();
    let pane_id = params.get("pane").cloned().unwrap_or_default();
    let direction = params.get("dir").cloned().unwrap_or_default();

    if screen_id.is_empty() || pane_id.is_empty() || direction.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "screen, pane, and dir params required"})),
        )
            .into_response();
    }

    let screen_id = ScreenId::from(screen_id.as_str());
    let screen = match state.mux_store.get_screen(&screen_id) {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "screen not found"})),
            )
                .into_response()
        }
    };

    let dir = match direction.as_str() {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "above" | "up" => Direction::Above,
        "below" | "down" => Direction::Below,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid direction"})),
            )
                .into_response()
        }
    };

    let pane = PaneId::from(pane_id.as_str());
    let neighbor = mux::find_neighbor(&screen.root, &pane, &dir);
    let neighbor_str = neighbor.map(|p| p.to_string()).unwrap_or_default();

    Json(json!({"paneId": neighbor_str})).into_response()
}

// ── Mux status & screens summary ────────────────────────────────────────

async fn get_mux_screens(State(state): State<AppState>) -> Json<Value> {
    let screens = state.mux_store.list_screen_infos();
    let ws = state.mux_store.get_workspace_state();
    let result: Vec<Value> = screens
        .iter()
        .map(|s| {
            let pane_count = ws.screens.get(&s.id).map(|sc| sc.panes.len()).unwrap_or(0);
            json!({
                "id": s.id.to_string(),
                "name": s.name,
                "paneCount": pane_count,
                "active": ws.active_screen_id.as_ref() == Some(&s.id),
            })
        })
        .collect();
    Json(json!(result))
}

async fn get_mux_status(State(state): State<AppState>) -> Json<Value> {
    let ws = state.mux_store.get_workspace_state();
    let total_sessions: usize = ws.screens.values().map(|s| s.sessions.len()).sum();
    Json(json!({
        "serverMode": state.server_mode,
        "capabilities": [crate::jobs::CAPABILITY],
        "version": state.version,
        "screens": ws.screens.len(),
        "sessions": total_sessions,
        "activeScreen": ws.active_screen_id.as_ref().map(|id| id.to_string()),
        "listenAddr": state.listen_addr,
    }))
}

// ── Session send ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SessionSendRequest {
    data: String,
}

async fn post_session_send(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SessionSendRequest>,
) -> Response {
    let backend = state.ws_state.dispatcher.backend();
    match backend.get(&id) {
        Some(session) => match session.write(req.data.as_bytes()).await {
            Ok(()) => Json(json!({"ok": true})).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("write failed: {e}")})),
            )
                .into_response(),
        },
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "session not found"})),
        )
            .into_response(),
    }
}

// ── Stats ───────────────────────────────────────────────────────────────────

async fn get_stats(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "screens": state.mux_store.screen_count(),
        "runtime": {
            "backend": "rust",
        },
    }))
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
         connect-src 'self' ws://127.0.0.1:{port} ws://localhost:{port}; \
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

pub async fn serve_frontend(State(state): State<AppState>, uri: axum::http::Uri) -> Response {
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

/// `splitty download`: offer a file of a pane's session to the windows that
/// show it, and answer once one saved it or why none did (`control-api`, File
/// Offer Requests).
async fn post_file_offer(
    State(state): State<AppState>,
    Json(input): Json<crate::ws::files::OfferRequest>,
) -> Response {
    use crate::ws::editor::{validate_path, Failure};
    fn error((status, code, message): Failure) -> Response {
        (
            StatusCode::from_u16(status).expect("fixed HTTP status"),
            Json(json!({"code":code,"error":message})),
        )
            .into_response()
    }
    if input.session_id.is_empty()
        || input.session_id.len() > 256
        || state
            .ws_state
            .dispatcher
            .backend()
            .get(&input.session_id)
            .is_none()
    {
        return error((
            404,
            "session_not_found",
            "The originating terminal session is no longer available",
        ));
    }
    if let Err(e) = validate_path(&input.path) {
        return error(e);
    }
    let path = std::path::PathBuf::from(&input.path);
    let file = match tokio::task::spawn_blocking(move || crate::ws::files::open(&path)).await {
        Ok(Ok(file)) => file,
        Ok(Err(e)) => return error(e),
        Err(_) => return error((500, "read_failed", "The file could not be opened")),
    };
    let offers = state.ws_state.file_offers.clone();
    match offers.offer(&input, file).await {
        Ok(()) => Json(json!({"outcome":"saved"})).into_response(),
        Err(e) => error(e),
    }
}

async fn post_editor_open(
    State(state): State<AppState>,
    Json(input): Json<crate::ws::editor::OpenRequest>,
) -> Response {
    use crate::ws::editor::{validate_path, Failure};
    fn error((status, code, message): Failure) -> Response {
        (
            StatusCode::from_u16(status).expect("fixed HTTP status"),
            Json(json!({"code":code,"error":message})),
        )
            .into_response()
    }
    if input.session_id.is_empty()
        || input.session_id.len() > 256
        || state
            .ws_state
            .dispatcher
            .backend()
            .get(&input.session_id)
            .is_none()
    {
        return error((
            404,
            "session_not_found",
            "The originating terminal session is no longer available",
        ));
    }
    if let Err(e) = validate_path(&input.path) {
        return error(e);
    }
    let metadata = match tokio::fs::metadata(&input.path).await {
        Ok(m) => m,
        Err(_) => {
            return error((
                400,
                "invalid_path",
                "The requested path does not exist or is inaccessible",
            ))
        }
    };
    let kind = if metadata.is_dir() {
        "folder"
    } else if metadata.is_file() {
        "file"
    } else {
        return error((
            400,
            "invalid_path",
            "Only regular files and folders can be opened",
        ));
    };
    let requests = state.ws_state.editor_requests.clone();
    match requests.request(input, kind).await {
        Ok(()) => Json(json!({"outcome":"dispatched"})).into_response(),
        Err(e) => error(e),
    }
}
