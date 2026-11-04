//! HTTP layer for the Splitty server built on Axum.
//!
//! Defines `AppState` (shared across all handlers) and exposes the router
//! constructors: `build_router` (API-only), `build_router_with_frontend`
//! (API + embedded SPA fallback) and `build_control_router` (control socket).
//! Middleware is applied in order: localhost guard, origin check, then
//! authentication — see `apply_middleware`.

mod browser_wire;
pub mod gateway;
pub mod jobs;
pub mod middleware;
pub mod routes;
pub mod server;
pub mod session;
#[cfg(all(test, unix))]
mod tests;

use std::sync::Arc;

use axum::Router;

/// Authentication and asset policy of one explicit browser adapter. Contains
/// no terminal service, host connector or upstream credential.
#[derive(Clone)]
pub struct BrowserState {
    pub token: String,
    pub sessions: Arc<session::SessionStore>,
    pub allowed_origins: Arc<Vec<String>>,
    pub listen_addr: String,
}
impl BrowserState {
    pub fn new(address: std::net::SocketAddr, token: String) -> std::io::Result<Self> {
        if address.ip() != std::net::Ipv4Addr::LOCALHOST
            && address.ip() != std::net::Ipv6Addr::LOCALHOST
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "browser gateways require 127.0.0.1 or ::1",
            ));
        }
        Ok(Self {
            token,
            sessions: Arc::new(session::SessionStore::isolated()),
            allowed_origins: Arc::new(canonical_origins(address)),
            listen_addr: address.to_string(),
        })
    }
}

fn canonical_origins(address: std::net::SocketAddr) -> Vec<String> {
    vec![
        format!("http://{address}"),
        format!("http://localhost:{}", address.port()),
    ]
}

impl axum::extract::FromRef<AppState> for BrowserState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            token: state.token.clone(),
            sessions: state.sessions.clone(),
            allowed_origins: state.allowed_origins.clone(),
            listen_addr: state.listen_addr.clone(),
        }
    }
}

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
    pub service: Arc<crate::service::State>,
    /// The browser adapter's address, used for CSP and origin checks.
    pub listen_addr: String,
}

impl std::ops::Deref for AppState {
    type Target = crate::service::State;

    fn deref(&self) -> &Self::Target {
        &self.service
    }
}

impl axum::response::IntoResponse for crate::service::reply::Reply {
    fn into_response(self) -> axum::response::Response {
        use axum::http::{header::CONTENT_TYPE, HeaderValue, StatusCode};
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let content_type = self.content_type;
        let body = axum::body::Bytes::from_owner(self.into_body());
        let mut response = (status, body).into_response();
        if let Some(content_type) = content_type {
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        }
        response
    }
}

/// The origins the UI is served from, derived from the address actually bound.
///
/// Both spellings of loopback are included because a user may reach the app
/// through either, and both are the same origin from the server's point of view.
///
/// Desktop clients load packaged assets and do not add browser origins.
pub fn allowed_origins(listen_addr: &str) -> Vec<String> {
    listen_addr
        .parse::<std::net::SocketAddr>()
        .map(canonical_origins)
        .unwrap_or_default()
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
    use axum::extract::FromRef;
    // Register the already-bound browser listener before any adapter can ask
    // the shared hub to open or remember a reverse forward. Native operations
    // must have the same exclusion even when no HTTP handler ran first.
    if let Ok(address) = state.listen_addr.parse::<std::net::SocketAddr>() {
        state.service.hub.reserve_local_port(address.port());
    }
    app.layer(axum::middleware::from_fn_with_state(
        BrowserState::from_ref(state),
        middleware::require_token,
    ))
    .layer(axum::middleware::from_fn_with_state(
        BrowserState::from_ref(state),
        middleware::require_origin,
    ))
    .layer(axum::middleware::from_fn(middleware::require_localhost))
}

/// Build the full Axum router with all routes and middleware.
pub fn build_router(state: AppState) -> Router {
    let app = routes::api_routes_with(state.service.hub.clone()).merge(jobs::routes(
        Arc::downgrade(&state.client_state.jobs),
        crate::jobs::Actor::User,
    ));
    apply_middleware(app, &state).with_state(state)
}

/// Build the router with API routes + embedded frontend (SPA fallback).
pub fn build_router_with_frontend(state: AppState) -> Router {
    let app = routes::api_routes_with(state.service.hub.clone())
        .merge(jobs::routes(
            Arc::downgrade(&state.client_state.jobs),
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
            Arc::downgrade(&state.client_state.jobs),
            crate::jobs::Actor::User,
        ))
        .merge(jobs::admin_routes())
        .with_state(state)
}
