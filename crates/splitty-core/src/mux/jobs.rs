//! Managed-job data and validation. No process handles, clocks or I/O live here.

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, JobId, PaneId, ScreenId, SessionId};

use super::Direction;

/// Transport-independent input after the coordinator resolves the principal and anchor.
#[derive(Clone, Debug)]
pub struct JobAdmission {
    pub job_id: JobId,
    pub launch: JobLaunch,
    pub placement: JobPlacement,
    pub owner: Option<AgentId>,
    pub created_at: u64,
}

#[derive(Clone, Debug)]
pub enum JobUpdate {
    /// Server shutdown interrupted an admitted job (never a client-supplied fact).
    Interrupted {
        at: u64,
    },
    Started {
        at: u64,
    },
    Failed {
        at: u64,
        error: String,
    },
    Exited {
        at: u64,
        result: JobResult,
    },
    Stop,
    Owner {
        owner: Option<AgentId>,
    },
    Forget,
}

/// The public launch intent, shared by the control and mux protocols.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobLaunch {
    pub request_id: String,
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub placement: JobPlacement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_screen_id: Option<ScreenId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_pane_id: Option<PaneId>,
    #[serde(default = "background")]
    pub background: bool,
}

fn background() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobPlacement {
    Screen {},
    #[serde(rename_all = "camelCase")]
    Tab {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen_id: Option<ScreenId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<PaneId>,
    },
    #[serde(rename_all = "camelCase")]
    Split {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen_id: Option<ScreenId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<PaneId>,
        direction: Direction,
    },
}

impl JobPlacement {
    pub fn anchor(&self) -> Option<(&ScreenId, &PaneId)> {
        match self {
            Self::Screen {} => None,
            Self::Tab { screen_id, pane_id }
            | Self::Split {
                screen_id, pane_id, ..
            } => screen_id.as_ref().zip(pane_id.as_ref()),
        }
    }
}

impl JobLaunch {
    /// UUID spelling is not part of an attempt's identity.
    pub fn normalize(&mut self) -> Result<(), String> {
        self.request_id = uuid::Uuid::parse_str(&self.request_id)
            .map_err(|_| "requestId must be a UUID".to_owned())?
            .to_string();
        self.validate()
    }
    /// Validate the intent's shape. The coordinator checks filesystem and authority.
    pub fn validate(&self) -> Result<(), String> {
        if uuid::Uuid::parse_str(&self.request_id).is_err() {
            return Err("requestId must be a UUID".into());
        }
        if self.argv.is_empty()
            || self.argv[0].is_empty()
            || self.argv.len() > 256
            || self.argv.iter().any(|arg| arg.contains('\0'))
            || self.argv.iter().map(String::len).sum::<usize>() > 64 * 1024
        {
            return Err(
                "argv must name a program and contain at most 256 arguments / 64 KiB, without NULs"
                    .into(),
            );
        }
        if !self.cwd.starts_with('/') || self.cwd.len() > 4096 || self.cwd.contains('\0') {
            return Err("cwd must be an absolute server path of at most 4096 bytes".into());
        }
        if self
            .name
            .as_ref()
            .is_some_and(|name| name.chars().count() > 200 || name.contains('\0'))
        {
            return Err("name must contain at most 200 characters without NULs".into());
        }
        if !self.background {
            return Err("managed jobs preserve focus; background must be true".into());
        }
        for id in self
            .new_screen_id
            .as_ref()
            .map(ScreenId::as_str)
            .into_iter()
            .chain(self.new_pane_id.as_ref().map(PaneId::as_str))
        {
            validate_id(id)?;
        }
        match &self.placement {
            JobPlacement::Screen {} => {}
            JobPlacement::Tab { screen_id, pane_id }
            | JobPlacement::Split {
                screen_id, pane_id, ..
            } => {
                if self.new_screen_id.is_some() {
                    return Err("newScreenId is only valid for screen placement".into());
                }
                match (screen_id, pane_id) {
                    (None, None) => {} // A grant may supply its configured anchor.
                    (Some(screen), Some(pane)) => {
                        validate_id(screen.as_str())?;
                        validate_id(pane.as_str())?;
                    }
                    _ => return Err("screenId and paneId must be supplied together".into()),
                }
            }
        }
        Ok(())
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 256 || id.contains('\0') {
        Err("identifiers must contain 1 to 256 bytes without NULs".into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Starting,
    Running,
    Stopping,
    Exited,
    Failed,
    Interrupted,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Failed | Self::Interrupted)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResult {
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub job_id: JobId,
    pub launch: JobLaunch,
    pub screen_id: ScreenId,
    pub pane_id: PaneId,
    pub session_id: SessionId,
    /// None denotes the trusted user, never an anonymous principal.
    pub created_by: Option<AgentId>,
    pub owner: Option<AgentId>,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
    pub state: JobState,
    pub result: JobResult,
    pub error: Option<String>,
    #[serde(default)]
    pub forgotten: bool,
}

/// The UI needs lifecycle metadata, not every retained argv/request in a screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub job_id: JobId,
    pub state: JobState,
    pub error: Option<String>,
    pub result: JobResult,
}
impl From<&JobRecord> for JobSnapshot {
    fn from(job: &JobRecord) -> Self {
        Self {
            job_id: job.job_id.clone(),
            state: job.state,
            error: job.error.clone(),
            result: job.result.clone(),
        }
    }
}

/// Explicitly marks persisted managed sessions even if their job record is damaged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    #[default]
    Shell,
    Never,
}

impl RestartPolicy {
    pub fn is_shell(&self) -> bool {
        *self == Self::Shell
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::{Op, Store, WorkspaceState};

    pub(super) fn launch() -> JobLaunch {
        JobLaunch {
            request_id: uuid::Uuid::new_v4().to_string(),
            argv: vec!["printf".into(), "two words".into()],
            cwd: "/work".into(),
            name: None,
            placement: JobPlacement::Screen {},
            new_screen_id: None,
            new_pane_id: None,
            background: true,
        }
    }

    #[test]
    fn legacy_workspace_defaults_to_no_jobs_and_keeps_order() {
        let store = Store::new();
        for id in ["second", "first"] {
            store
                .dispatch(Op::CreateScreen {
                    name: None,
                    screen_id: Some(ScreenId::from(id)),
                    new_pane_id: None,
                    shell_profile_id: None,
                })
                .expect("screen");
        }
        let mut wire = serde_json::to_value(store.get_workspace_state()).expect("serialize");
        wire.as_object_mut().expect("workspace").remove("jobs");
        let restored: WorkspaceState = serde_json::from_value(wire).expect("legacy state");
        assert!(restored.jobs.is_empty());
        assert_eq!(
            restored.screen_order,
            vec![ScreenId::from("second"), ScreenId::from("first")]
        );
        for screen in restored.screens.values() {
            assert!(screen.panes.values().all(|pane| pane.job_id.is_none()));
            assert!(screen
                .sessions
                .values()
                .all(|session| session.job_id.is_none()
                    && session.restart_policy == RestartPolicy::Shell));
        }
        assert!(WorkspaceState::default().jobs.is_empty());
    }

    #[test]
    fn launch_rejects_forged_authority_and_contradictory_fields() {
        let intent = launch();
        assert!(intent.validate().is_ok());
        let mut wire = serde_json::to_value(intent).expect("serialize");
        wire["owner"] = serde_json::json!("another-agent");
        assert!(serde_json::from_value::<JobLaunch>(wire).is_err());
        let bad = serde_json::json!({"kind":"screen", "direction":"right"});
        assert!(serde_json::from_value::<JobPlacement>(bad).is_err());
        let mut invalid = launch();
        invalid.argv.push("nul\0argument".into());
        assert!(invalid.validate().is_err());
        invalid = launch();
        invalid.placement = JobPlacement::Tab {
            screen_id: Some(ScreenId::from("s")),
            pane_id: None,
        };
        assert!(invalid.validate().is_err());
    }
}
