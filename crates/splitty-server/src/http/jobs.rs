//! Both local agent sockets and the trusted API use these same handlers.
use super::AppState;
use crate::jobs::{Access, Actor, Anchor, JobError, Jobs, WeakJobs};
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
use splitty_core::pty::output::{LogFormat, OutputQuery};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Context {
    pub jobs: WeakJobs,
    pub actor: Actor,
}
impl Context {
    fn service(&self) -> Result<Arc<Jobs>, JobError> {
        self.jobs
            .upgrade()
            .ok_or_else(|| JobError::new(503, "shutting_down", "server has stopped"))
    }
}

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
    let response = (status, Json(&access.value)).into_response();
    drop(access);
    response
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
    let (access, replay) = ctx.service()?.launch(ctx.actor, body(payload)?).await?;
    Ok(respond(
        access,
        if replay {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
    ))
}
async fn list(State(ctx): State<Context>) -> Result<Response, JobError> {
    Ok(respond(
        ctx.service()?.list(&ctx.actor).await?,
        StatusCode::OK,
    ))
}
async fn inspect(State(ctx): State<Context>, Path(id): Path<String>) -> Result<Response, JobError> {
    Ok(respond(
        ctx.service()?.inspect(&ctx.actor, &JobId::from(id)).await?,
        StatusCode::OK,
    ))
}
async fn stop(State(ctx): State<Context>, Path(id): Path<String>) -> Result<Response, JobError> {
    Ok(respond(
        ctx.service()?.stop(&ctx.actor, &JobId::from(id)).await?,
        StatusCode::OK,
    ))
}
async fn forget(State(ctx): State<Context>, Path(id): Path<String>) -> Result<Response, JobError> {
    Ok(respond(
        ctx.service()?.forget(&ctx.actor, &JobId::from(id)).await?,
        StatusCode::OK,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    data: String,
}
async fn input(
    State(ctx): State<Context>,
    Path(id): Path<String>,
    payload: Result<Json<Input>, JsonRejection>,
) -> Result<Response, JobError> {
    Ok(respond(
        ctx.service()?
            .input(&ctx.actor, &JobId::from(id), body(payload)?.data)
            .await?,
        StatusCode::OK,
    ))
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Logs {
    cursor: Option<String>,
    tail: Option<usize>,
    limit: Option<usize>,
    #[serde(default)]
    wait_ms: u64,
    #[serde(default)]
    format: LogFormat,
}
async fn logs(
    State(ctx): State<Context>,
    Path(id): Path<String>,
    query: Result<Query<Logs>, QueryRejection>,
) -> Result<Response, JobError> {
    let args = query
        .map_err(|e| JobError::new(400, "invalid_request", e.body_text()))?
        .0;
    let query = OutputQuery {
        cursor: args.cursor,
        tail: args.tail,
        limit: args.limit.unwrap_or(65536),
        format: args.format,
    };
    Ok(respond(
        ctx.service()?
            .logs(
                &ctx.actor,
                &JobId::from(id),
                query,
                Duration::from_millis(args.wait_ms),
            )
            .await?,
        StatusCode::OK,
    ))
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
        crate::control::spawn_grant(&state.ws_state.jobs, args.name, args.anchor).map(Json)
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
    Json(state.ws_state.jobs.grants())
}
async fn agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, JobError> {
    state.ws_state.jobs.grant_info(&AgentId::from(id)).map(Json)
}
async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, JobError> {
    state
        .ws_state
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
            .ws_state
            .jobs
            .transfer(&JobId::from(id), owner)
            .await?,
        StatusCode::OK,
    ))
}
