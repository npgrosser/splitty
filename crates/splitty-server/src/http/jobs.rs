//! Both local agent sockets and the trusted API use these same handlers.
use super::AppState;
use crate::jobs::{Access, Actor, Anchor, JobError, WeakJobs};
use crate::service::jobs::{Context, Input, Logs};
use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        DefaultBodyLimit, Path, Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;
use splitty_core::ids::{AgentId, JobId};
use splitty_core::mux::jobs::JobLaunch;

pub fn routes<S: Clone + Send + Sync + 'static>(jobs: WeakJobs, actor: Actor) -> Router<S> {
    Router::new()
        .route("/api/jobs", get(list).post(launch))
        .route("/api/jobs/{id}", get(inspect))
        .route("/api/jobs/{id}/logs", get(logs))
        .route("/api/jobs/{id}/input", post(input))
        .route("/api/jobs/{id}/stop", post(stop))
        .route("/api/jobs/{id}/forget", post(forget))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .with_state(Context { jobs, actor })
}
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/agents", get(agents).post(create_agent))
        .route("/api/agents/{id}", get(agent))
        .route("/api/agents/{id}/revoke", post(revoke))
        .route("/api/jobs/{id}/owner", post(owner))
        .layer(DefaultBodyLimit::max(128 * 1024))
}
impl IntoResponse for JobError {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(self),
        )
            .into_response()
    }
}
pub fn respond(access: Access, status: StatusCode) -> Response {
    crate::service::reply::job(access, status).into_response()
}
fn body<T: DeserializeOwned>(value: Result<Json<T>, JsonRejection>) -> Result<T, JobError> {
    value.map(|v| v.0).map_err(|e| {
        JobError::new(
            if e.status() == StatusCode::UNPROCESSABLE_ENTITY {
                400
            } else {
                e.status().as_u16()
            },
            "invalid_request",
            e.body_text(),
        )
    })
}
async fn launch(
    State(ctx): State<Context>,
    payload: Result<Json<JobLaunch>, JsonRejection>,
) -> Result<Response, JobError> {
    ctx.launch(body(payload)?)
        .await
        .map(IntoResponse::into_response)
}
async fn list(State(ctx): State<Context>) -> Result<Response, JobError> {
    ctx.list().await.map(IntoResponse::into_response)
}
async fn inspect(State(ctx): State<Context>, Path(id): Path<String>) -> Result<Response, JobError> {
    ctx.inspect(id).await.map(IntoResponse::into_response)
}
async fn stop(State(ctx): State<Context>, Path(id): Path<String>) -> Result<Response, JobError> {
    ctx.stop(id).await.map(IntoResponse::into_response)
}
async fn forget(State(ctx): State<Context>, Path(id): Path<String>) -> Result<Response, JobError> {
    ctx.forget(id).await.map(IntoResponse::into_response)
}
async fn input(
    State(ctx): State<Context>,
    Path(id): Path<String>,
    payload: Result<Json<Input>, JsonRejection>,
) -> Result<Response, JobError> {
    ctx.input(id, body(payload)?)
        .await
        .map(IntoResponse::into_response)
}
async fn logs(
    State(ctx): State<Context>,
    Path(id): Path<String>,
    query: Result<Query<Logs>, QueryRejection>,
) -> Result<Response, JobError> {
    ctx.logs(
        id,
        query
            .map_err(|e| JobError::new(400, "invalid_request", e.body_text()))?
            .0,
    )
    .await
    .map(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgent {
    name: String,
    anchor: Option<Anchor>,
}
async fn create_agent(
    State(state): State<AppState>,
    payload: Result<Json<CreateAgent>, JsonRejection>,
) -> Result<Json<Value>, JobError> {
    let args = body(payload)?;
    #[cfg(unix)]
    {
        crate::control::spawn_grant(&state.client_state.jobs, args.name, args.anchor).map(Json)
    }
    #[cfg(not(unix))]
    {
        let _ = (state, args);
        Err(JobError::new(
            501,
            "unsupported",
            "local grants require Unix/WSL",
        ))
    }
}
async fn agents(State(state): State<AppState>) -> Json<Value> {
    Json(state.client_state.jobs.grants())
}
async fn agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, JobError> {
    state
        .client_state
        .jobs
        .grant_info(&AgentId::from(id))
        .map(Json)
}
async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, JobError> {
    state
        .client_state
        .jobs
        .revoke(&AgentId::from(id))
        .await
        .map(Json)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Owner {
    agent_id: Value,
}
async fn owner(
    State(state): State<AppState>,
    Path(id): Path<String>,
    payload: Result<Json<Owner>, JsonRejection>,
) -> Result<Response, JobError> {
    let owner = match body(payload)?.agent_id {
        Value::Null => None,
        Value::String(id) => Some(AgentId::from(id)),
        _ => {
            return Err(JobError::new(
                400,
                "invalid_request",
                "agentId must be a grant ID or null",
            ))
        }
    };
    Ok(respond(
        state
            .client_state
            .jobs
            .transfer(&JobId::from(id), owner)
            .await?,
        StatusCode::OK,
    ))
}
