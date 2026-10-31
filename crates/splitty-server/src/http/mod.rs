//! HTTP layer for the Splitty server built on Axum.
//!
//! Defines `AppState` (shared across all handlers) and exposes the router
//! constructors: `build_router` (API-only), `build_router_with_frontend`
//! (API + embedded SPA fallback) and `build_control_router` (control socket).
//! Middleware is applied in order: localhost guard, origin check, then
//! authentication — see `apply_middleware`.

pub mod jobs;
pub mod middleware;
pub mod routes;
pub mod session;
#[cfg(all(test, unix))]
mod tests;

use std::sync::{Arc, RwLock};

use axum::Router;

use crate::ws;

/// Shared application state, passed to all handlers via Axum's State extractor.
#[derive(Clone)]
pub struct AppState {
    /// Process token. Used only by non-browser callers (CLI, native launchers)
    /// over `Authorization: Bearer`. The browser UI authenticates with a session
    /// cookie instead and never sees this value.
    pub token: String,
    /// Bootstrap codes, browser sessions and WebSocket tickets.
    pub sessions: Arc<session::SessionStore>,
    /// Origins the application itself is served from, e.g.
    /// `http://127.0.0.1:45299`. Any browser request carrying an `Origin`
    /// outside this list is by definition not the Splitty UI. Built by
    /// [`allowed_origins`] from the address actually bound.
    pub allowed_origins: Arc<Vec<String>>,
    pub mux_store: Arc<splitty_core::mux::Store>,
    pub ws_state: Arc<ws::WsState>,
    pub config: Arc<RwLock<splitty_core::config::HomeConfig>>,
    pub shell_profiles: Arc<RwLock<splitty_core::shell_profiles::ShellProfilesState>>,
    pub server_config: Arc<RwLock<splitty_core::config::ServerConfig>>,
    pub template_store: Arc<splitty_core::template::FileStore>,
    pub themes_dir: std::path::PathBuf,
    /// Build info
    pub version: String,
    pub commit: String,
    pub build_time: String,
    /// The address the server is listening on. Feeds the CSP `connect-src` and
    /// the stats endpoint.
    pub listen_addr: String,
    /// Server mode: "embedded" (dies with window) or "daemon" (persistent).
    pub server_mode: String,
}

/// The origins the UI is served from, derived from the address actually bound.
///
/// Both spellings of loopback are included because a user may reach the app
/// through either, and both are the same origin from the server's point of view.
///
/// In debug builds the Vite dev server's origin is added as well: `make
/// dev-tauri` serves the UI from `http://localhost:1420` and proxies `/api` and
/// `/ws` through to this server, so the requests arrive carrying that origin.
/// This is compiled out of release builds rather than being a runtime switch, so
/// a shipped binary has no way to widen its own origin policy.
pub fn allowed_origins(listen_addr: &str) -> Vec<String> {
    let port = listen_addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok());

    let mut origins = Vec::new();
    if let Some(port) = port {
        origins.push(format!("http://127.0.0.1:{port}"));
        origins.push(format!("http://localhost:{port}"));
    }

    #[cfg(debug_assertions)]
    {
        origins.push("http://localhost:1420".to_string());
        origins.push("http://127.0.0.1:1420".to_string());
    }

    origins
}

/// Apply the shared middleware stack.
///
/// Order matters and is asserted by the integration tests. Layers run in reverse
/// registration order, so the effective sequence per request is:
///
/// 1. `require_localhost` — Host header must be loopback (DNS-rebinding guard).
/// 2. `require_origin` — cross-origin browser requests are rejected.
/// 3. `require_token` — authentication.
///
/// The cheapest and broadest checks run first so an unauthenticated cross-origin
/// probe never reaches the auth path.
///
/// There is deliberately no CORS layer (security review SR-07):
/// it previously emitted `Access-Control-Allow-Origin: *` whenever remote mode
/// was on. The UI is same-origin with the API, so it needs no CORS headers at
/// all, and emitting none is the strictest correct answer.
fn apply_middleware(app: Router<AppState>, state: &AppState) -> Router<AppState> {
    app.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        middleware::require_token,
    ))
    .layer(axum::middleware::from_fn_with_state(
        state.clone(),
        middleware::require_origin,
    ))
    .layer(axum::middleware::from_fn(middleware::require_localhost))
}

/// Build the full Axum router with all routes and middleware.
pub fn build_router(state: AppState) -> Router {
    let app = routes::api_routes().merge(jobs::routes(
        Arc::downgrade(&state.ws_state.jobs),
        crate::jobs::Actor::User,
    ));
    apply_middleware(app, &state).with_state(state)
}

/// Build the router with API routes + embedded frontend (SPA fallback).
pub fn build_router_with_frontend(state: AppState) -> Router {
    let app = routes::api_routes()
        .merge(jobs::routes(
            Arc::downgrade(&state.ws_state.jobs),
            crate::jobs::Actor::User,
        ))
        .fallback(routes::serve_frontend);
    apply_middleware(app, &state).with_state(state)
}

/// Build the router served over the local control socket.
///
/// No authentication middleware, and that is the design rather than an omission
/// (security review SR-05): authorisation for this channel is the
/// filesystem — a socket mode 0600 inside a directory mode 0700 — plus a peer
/// credential check on every accepted connection (see `control.rs`). A bearer
/// token here would reintroduce exactly the copyable secret this channel exists
/// to remove.
///
/// The origin and Host guards are likewise absent because neither concept
/// applies: there is no browser and no DNS on a Unix socket.
///
/// The narrowing that *does* apply is the route set — see `control_routes`.
pub fn build_control_router(state: AppState) -> Router {
    routes::control_routes()
        .merge(jobs::routes(
            Arc::downgrade(&state.ws_state.jobs),
            crate::jobs::Actor::User,
        ))
        .merge(jobs::admin_routes())
        .with_state(state)
}
