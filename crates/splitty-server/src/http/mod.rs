//! HTTP layer for the Splitty server built on Axum.
//!
//! Two routers: the explicit browser gateway (`gateway::router`), which turns
//! every `/api` request into a native request to its assigned backend and owns
//! the browser credentials (`BrowserState`), and the control-socket router
//! (`build_control_router`) over the service alone (`ControlState`).

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

/// What the control-socket router serves: the terminal service, and no
/// browser credentials, origins or listening address.
#[derive(Clone)]
pub struct ControlState {
    pub service: Arc<crate::service::State>,
}

impl std::ops::Deref for ControlState {
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
pub fn build_control_router(service: Arc<crate::service::State>) -> Router {
    let owned = Arc::downgrade(&service.client_state.jobs);
    routes::control_routes()
        .merge(jobs::routes(owned, crate::jobs::Actor::User))
        .merge(jobs::admin_routes())
        .with_state(ControlState { service })
}
