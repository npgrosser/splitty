//! The same managed-job operations for UI and scoped control adapters.
use super::reply::{job, Reply};
use crate::jobs::{Actor, JobError, Jobs, WeakJobs};
use http::StatusCode;
use serde::Deserialize;
use splitty_core::ids::JobId;
use splitty_core::mux::jobs::JobLaunch;
use splitty_core::pty::output::{LogFormat, OutputQuery};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Context {
    pub jobs: WeakJobs,
    pub actor: Actor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub data: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Logs {
    pub cursor: Option<String>,
    pub tail: Option<usize>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub wait_ms: u64,
    #[serde(default)]
    pub format: LogFormat,
}

impl Context {
    fn service(&self) -> Result<Arc<Jobs>, JobError> {
        self.jobs
            .upgrade()
            .ok_or_else(|| JobError::new(503, "shutting_down", "server has stopped"))
    }

    pub async fn launch(&self, request: JobLaunch) -> Result<Reply, JobError> {
        let (access, replay) = self.service()?.launch(self.actor.clone(), request).await?;
        Ok(job(
            access,
            if replay {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            },
        ))
    }

    pub async fn list(&self) -> Result<Reply, JobError> {
        Ok(job(
            self.service()?.list(&self.actor).await?,
            StatusCode::OK,
        ))
    }

    pub async fn inspect(&self, id: String) -> Result<Reply, JobError> {
        Ok(job(
            self.service()?
                .inspect(&self.actor, &JobId::from(id))
                .await?,
            StatusCode::OK,
        ))
    }

    pub async fn stop(&self, id: String) -> Result<Reply, JobError> {
        Ok(job(
            self.service()?.stop(&self.actor, &JobId::from(id)).await?,
            StatusCode::OK,
        ))
    }

    pub async fn forget(&self, id: String) -> Result<Reply, JobError> {
        Ok(job(
            self.service()?
                .forget(&self.actor, &JobId::from(id))
                .await?,
            StatusCode::OK,
        ))
    }

    pub async fn input(&self, id: String, input: Input) -> Result<Reply, JobError> {
        Ok(job(
            self.service()?
                .input(&self.actor, &JobId::from(id), input.data)
                .await?,
            StatusCode::OK,
        ))
    }

    pub async fn logs(&self, id: String, args: Logs) -> Result<Reply, JobError> {
        let query = OutputQuery {
            cursor: args.cursor,
            tail: args.tail,
            limit: args.limit.unwrap_or(65536),
            format: args.format,
        };
        Ok(job(
            self.service()?
                .logs(
                    &self.actor,
                    &JobId::from(id),
                    query,
                    Duration::from_millis(args.wait_ms),
                )
                .await?,
            StatusCode::OK,
        ))
    }
}
