//! HTTP framing and catalog translation for the owner control socket.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{on, post, MethodFilter},
    Json, Router,
};
use serde_json::json;
use splitty_transport::budget::ConnectionBudget;

use crate::service::catalog::{self, Operation, Role, Scope};

use super::{jobs, operation};

/// Terminal service shared by the control routes.
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
/// The router has no authentication layer: the channel is authorised by the
/// file system (a socket of mode 0600 inside a directory of mode 0700) and by
/// a peer credential check on every accepted connection (the listener in
/// `control/mod.rs`). No request carries a secret that could be copied
/// elsewhere.
///
/// What narrows the channel is its route set, see [`control_routes`].
pub fn build_control_router(service: Arc<crate::service::State>) -> Router {
    let owned = Arc::downgrade(&service.client_state.jobs);
    control_routes()
        .merge(jobs::routes(owned, crate::jobs::Actor::User))
        .merge(jobs::admin_routes())
        .with_state(ControlState { service })
}

/// The catalog's Control scope, authorized by filesystem permissions and peer UID.
/// Configuration, templates and hub operations are available only to native clients.
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
