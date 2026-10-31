//! Daemon lifecycle helpers: info file, PID checks, and state persistence.
//!
//! The daemon info file (`daemon.json`) in the config directory acts as the
//! discovery mechanism for CLI tools and the Tauri desktop app. It records
//! the PID, port, and token of a running daemon so clients can find it.

mod persistence;

pub use persistence::{load_state, save_state, state_path};

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;

/// Runtime metadata written by a running daemon.
///
/// The `token` field is gone (security review SR-21). It held the
/// server's master credential in a file whose path is documented as a discovery
/// mechanism, so removing the same token from the PTY environment (SR-05) while
/// leaving it here would have accomplished nothing — a process would simply have
/// read the file instead of its own environment.
///
/// Clients reach the daemon over `socket` and are authorised by the filesystem,
/// not by a bearer value. `#[serde(default)]` lets a `daemon.json` written by an
/// older version still deserialise; any `token` key it contains is ignored.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    pub pid: u32,
    pub port: u16,
    /// Path to the daemon's local control socket.
    #[serde(default)]
    pub socket: String,
    pub started_at: String,
    pub version: String,
    /// The daemon's protocol version (`protocol::PROTOCOL_VERSION`). A file
    /// written before the field existed is protocol 1.
    #[serde(default = "first_protocol")]
    pub protocol: u32,
    /// The daemon's sessions outlive it in session keepers. A file written
    /// before the field existed is a daemon whose sessions end with it.
    #[serde(default)]
    pub keeps_sessions: bool,
}

fn first_protocol() -> u32 {
    1
}

/// Path to the daemon info file.
pub fn info_path() -> PathBuf {
    config_dir().join("daemon.json")
}

/// Write daemon info atomically, readable only by the owner.
///
/// This file holds the server token, and it used to be written with a plain
/// `fs::write` into a directory created by a plain `create_dir_all`
/// (security review SR-21): mode 0644 inside a 0755 directory
/// under a typical umask. The auth-token module's 0700 handling did not cover
/// it, because in the desktop flow that module is never reached.
pub fn write_info(info: &DaemonInfo) -> io::Result<()> {
    let json = serde_json::to_string_pretty(info).map_err(io::Error::other)?;
    crate::config::write_private_file(&info_path(), json.as_bytes())
}

/// Read daemon info. Returns `None` if the file is missing or corrupt.
pub fn read_info() -> Option<DaemonInfo> {
    let data = fs::read_to_string(info_path()).ok()?;
    serde_json::from_str(&data).ok()
}

/// What `splitty daemon status --json` prints, and what `splitty connect`
/// reads back from a remote host: the info file of a running daemon, or only
/// `"running": false`. Holds no credential, like the info file itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonInfo>,
}

impl DaemonStatus {
    pub fn of(daemon: Option<DaemonInfo>) -> Self {
        Self {
            running: daemon.is_some(),
            daemon,
        }
    }
}

/// Remove the daemon info file. Ignores errors (file may already be gone).
pub fn remove_info() {
    let _ = fs::remove_file(info_path());
}

/// Check if a process with the given PID is alive (signal-0 check).
#[cfg(unix)]
pub fn is_running(pid: u32) -> bool {
    use nix::sys::signal;
    use nix::unistd::Pid;
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

#[cfg(not(unix))]
pub fn is_running(_pid: u32) -> bool {
    false
}

/// Build the list of [`Effect::SpawnSession`] effects needed to restore
/// PTY sessions from persisted workspace state.
///
/// A session whose keeper the starting daemon adopted (`held`) still runs;
/// every other session that was not already exited gets a spawn effect, with
/// the same session ID so that pane mappings remain valid.
pub fn restore_effects(
    state: &crate::mux::WorkspaceState,
    held: impl Fn(&str) -> bool,
) -> Vec<crate::mux::Effect> {
    use crate::mux::{Effect, SessionStatus};

    let mut effects = Vec::new();

    for screen in state.screens.values() {
        for (pane_id, pane) in &screen.panes {
            let session_id = match &pane.session_id {
                Some(sid) => sid.clone(),
                None => continue,
            };

            let session = match screen.sessions.get(&session_id) {
                Some(s) => s,
                None => continue,
            };

            if pane.job_id.is_some()
                || session.job_id.is_some()
                || session.restart_policy == crate::mux::jobs::RestartPolicy::Never
                || state.jobs.values().any(|job| job.session_id == session_id)
            {
                continue;
            }

            if session.status == SessionStatus::Exited || held(session_id.as_str()) {
                continue;
            }

            effects.push(Effect::SpawnSession {
                session_id,
                pane_id: pane_id.clone(),
                screen_id: screen.id.clone(),
                command: session.command.clone(),
                cwd: session.cwd.clone().or_else(|| pane.cwd.clone()),
                shell_profile_id: session.shell_profile_id.clone(),
                shell: session.shell.clone(),
            });
        }
    }

    effects
}

/// A new daemon incarnation has no live agent authority, and job processes
/// only in the keepers it adopted (`adopted` holds their session ids): those
/// jobs keep their state, under trusted administration. Never infer that a
/// stored PID/owner label represents something we still own.
pub fn restore_jobs(
    state: &mut crate::mux::WorkspaceState,
    adopted: &std::collections::HashSet<String>,
) {
    use crate::mux::jobs::{JobResult, JobState, RestartPolicy};

    state.jobs.retain(|_, job| !job.forgotten);
    for job in state.jobs.values_mut() {
        if !job.state.is_terminal() && !adopted.contains(job.session_id.as_str()) {
            job.state = JobState::Interrupted;
            job.result = JobResult::default();
            job.ended_at = None;
            job.error = Some("Splitty server restarted; this job was not replayed".into());
        }
    }
    for screen in state.screens.values_mut() {
        for session in screen.sessions.values_mut() {
            if session.job_id.is_some()
                || session.restart_policy == RestartPolicy::Never
                || state.jobs.values().any(|job| job.session_id == session.id)
                || screen.panes.values().any(|pane| {
                    pane.job_id.is_some() && pane.session_id.as_ref() == Some(&session.id)
                })
            {
                session.restart_policy = RestartPolicy::Never;
                if adopted.contains(session.id.as_str()) {
                    continue;
                }
                session.status = crate::mux::SessionStatus::Exited;
                session.exit_code = state
                    .jobs
                    .values()
                    .find(|job| job.session_id == session.id)
                    .and_then(|job| job.result.exit_code)
                    .and_then(|code| i32::try_from(code).ok());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_never_replays_jobs_even_with_stale_or_damaged_markers() {
        use crate::mux::jobs::{
            JobAdmission, JobLaunch, JobPlacement, JobResult, JobState, JobUpdate, RestartPolicy,
        };
        use crate::mux::{Op, Store};

        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: None,
                new_pane_id: None,
                shell_profile_id: None,
            })
            .expect("ordinary shell");
        for id in ["running", "finished", "failed"] {
            store
                .dispatch(Op::CreateJob(Box::new(JobAdmission {
                    job_id: id.into(),
                    owner: None,
                    created_at: 1,
                    placement: JobPlacement::Screen {},
                    launch: JobLaunch {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        argv: vec!["do-not-replay".into()],
                        cwd: "/work".into(),
                        name: None,
                        placement: JobPlacement::Screen {},
                        new_screen_id: None,
                        new_pane_id: None,
                        background: true,
                    },
                })))
                .expect("job");
        }
        store
            .dispatch(Op::UpdateJob {
                job_id: "finished".into(),
                update: JobUpdate::Exited {
                    at: 2,
                    result: JobResult {
                        exit_code: Some(7),
                        signal: None,
                    },
                },
            })
            .expect("exit");
        store
            .dispatch(Op::UpdateJob {
                job_id: "failed".into(),
                update: JobUpdate::Failed {
                    at: 2,
                    error: "missing executable".into(),
                },
            })
            .expect("failure");
        let mut state = store.get_workspace_state();
        assert_eq!(restore_effects(&state, |_| false).len(), 1);
        restore_jobs(&mut state, &std::collections::HashSet::new());
        let running = state
            .jobs
            .values()
            .find(|j| j.job_id.as_str() == "running")
            .expect("running metadata");
        assert_eq!(running.state, JobState::Interrupted);
        assert_eq!(running.result, JobResult::default());
        assert_eq!(
            state
                .jobs
                .values()
                .find(|j| j.job_id.as_str() == "finished")
                .expect("finished")
                .result
                .exit_code,
            Some(7)
        );
        assert_eq!(
            state
                .jobs
                .values()
                .find(|j| j.job_id.as_str() == "failed")
                .expect("failed")
                .state,
            JobState::Failed
        );
        assert_eq!(restore_effects(&state, |_| false).len(), 1);
        let wire = serde_json::to_value(&state).expect("serialize");
        let restored: crate::mux::WorkspaceState = serde_json::from_value(wire).expect("roundtrip");
        assert_eq!(restore_effects(&restored, |_| false).len(), 1);
        // Any surviving marker prevents a malformed record from becoming a shell.
        state.jobs.clear();
        for screen in state.screens.values_mut() {
            for session in screen.sessions.values_mut() {
                if session.job_id.is_some() {
                    session.job_id = None;
                    session.status = crate::mux::SessionStatus::Starting;
                    assert_eq!(session.restart_policy, RestartPolicy::Never);
                }
            }
        }
        assert_eq!(restore_effects(&state, |_| false).len(), 1);
    }

    #[test]
    fn adopted_sessions_are_not_respawned_and_adopted_jobs_keep_running() {
        use crate::mux::jobs::{JobAdmission, JobLaunch, JobPlacement, JobState};
        use crate::mux::{Op, Store};

        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: None,
                new_pane_id: None,
                shell_profile_id: None,
            })
            .expect("ordinary shell");
        store
            .dispatch(Op::CreateJob(Box::new(JobAdmission {
                job_id: "dev".into(),
                owner: Some("agent-1".into()),
                created_at: 1,
                placement: JobPlacement::Screen {},
                launch: JobLaunch {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    argv: vec!["npm".into(), "run".into(), "dev".into()],
                    cwd: "/work".into(),
                    name: None,
                    placement: JobPlacement::Screen {},
                    new_screen_id: None,
                    new_pane_id: None,
                    background: true,
                },
            })))
            .expect("job");
        store
            .dispatch(Op::UpdateJob {
                job_id: "dev".into(),
                update: crate::mux::jobs::JobUpdate::Started { at: 2 },
            })
            .expect("started");
        let mut state = store.get_workspace_state();
        let shell = restore_effects(&state, |_| false);
        assert_eq!(shell.len(), 1, "the ordinary shell would be respawned");
        let crate::mux::Effect::SpawnSession { session_id, .. } = &shell[0] else {
            panic!("a spawn effect");
        };
        let shell_session = session_id.to_string();
        assert!(
            restore_effects(&state, |id| id == shell_session).is_empty(),
            "Adopted shell is not respawned"
        );

        let job_session = state
            .jobs
            .values()
            .next()
            .expect("job")
            .session_id
            .to_string();
        restore_jobs(
            &mut state,
            &[job_session.clone(), shell_session].into_iter().collect(),
        );
        let job = state.jobs.values().next().expect("job");
        assert_eq!(
            job.state,
            JobState::Running,
            "the adopted job keeps running"
        );
        assert_eq!(job.owner.as_ref().map(|o| o.as_str()), Some("agent-1"));
        let session = state
            .screens
            .values()
            .find_map(|screen| {
                screen
                    .sessions
                    .values()
                    .find(|session| session.id.as_str() == job_session)
            })
            .expect("job session");
        assert_ne!(session.status, crate::mux::SessionStatus::Exited);
        assert!(
            restore_effects(&state, |_| false).len() == 1,
            "jobs are never respawned"
        );
    }

    fn info() -> DaemonInfo {
        DaemonInfo {
            pid: 42,
            port: 9800,
            socket: "/run/user/1000/splitty/control.sock".into(),
            started_at: "1760000000".into(),
            version: "0.1.0".into(),
            protocol: 1,
            keeps_sessions: true,
        }
    }

    #[test]
    fn status_json_is_the_info_file_plus_running() {
        let running = serde_json::to_value(DaemonStatus::of(Some(info()))).expect("serialize");
        assert_eq!(
            running,
            serde_json::json!({
                "running": true,
                "pid": 42,
                "port": 9800,
                "socket": "/run/user/1000/splitty/control.sock",
                "startedAt": "1760000000",
                "version": "0.1.0",
                "protocol": 1,
                "keepsSessions": true,
            })
        );
        let stopped = serde_json::to_string(&DaemonStatus::of(None)).expect("serialize");
        assert_eq!(stopped, r#"{"running":false}"#);
    }

    #[test]
    fn status_json_reads_back() {
        let text = serde_json::to_string(&DaemonStatus::of(Some(info()))).expect("serialize");
        let status: DaemonStatus = serde_json::from_str(&text).expect("parse");
        assert!(status.running);
        assert_eq!(status.daemon.map(|d| d.pid), Some(42));

        let status: DaemonStatus = serde_json::from_str(r#"{"running":false}"#).expect("parse");
        assert!(!status.running);
        assert!(status.daemon.is_none());
    }

    #[test]
    fn an_info_file_without_protocol_is_protocol_1() {
        let older = r#"{"pid":1,"port":9800,"socket":"","startedAt":"1","version":"0.0.9"}"#;
        let info: DaemonInfo = serde_json::from_str(older).expect("older daemon.json");
        assert_eq!(info.protocol, 1);
        assert!(
            !info.keeps_sessions,
            "an older daemon's sessions end with it"
        );
    }
}
