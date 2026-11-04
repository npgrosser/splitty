//! Daemon lifecycle helpers: info file, PID checks, and state persistence.
//!
//! The daemon info file (`daemon.json`) in the config directory acts as the
//! discovery mechanism for explicit daemon clients. Embedded desktop backends
//! do not use it. Format 2 identifies a private native socket and incarnation;
//! old TCP records are read only for diagnostics and deliberate migration.

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
/// UI clients use `clientSocket`; owner CLI requests use the optional `socket`.
/// Both are authorised by peer user and private filesystem permissions,
/// not by a bearer value. `#[serde(default)]` lets a `daemon.json` written by an
/// older version still deserialise; any `token` key it contains is ignored.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_version: Option<u32>,
    /// Diagnostic only: native clients never connect to a legacy TCP port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
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

impl DaemonInfo {
    pub fn legacy(&self) -> bool {
        self.format.is_none()
    }

    /// Validate discovery before it can select an endpoint. An unsupported
    /// transport version is retained for the caller's compatibility error.
    pub fn validate(&self) -> io::Result<()> {
        let valid = self.pid > 0
            && self.pid <= i32::MAX as u32
            && match self.format {
                None => {
                    self.port.is_some_and(|port| port > 0)
                        && self.client_socket.is_none()
                        && self.instance.is_none()
                        && self.transport_version.is_none()
                }
                Some(2) => {
                    self.port.is_none()
                        && self.transport_version.is_some_and(|v| v > 0)
                        && self
                            .client_socket
                            .as_ref()
                            .is_some_and(|p| std::path::Path::new(p).is_absolute())
                        && self.instance.as_ref().is_some_and(|s| {
                            s.len() == 32
                                && s.bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        })
                        && self.keeps_sessions
                }
                _ => false,
            };
        if valid {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid or unsupported daemon discovery record",
            ))
        }
    }
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
/// This file used to hold the server token and be written with a plain
/// `fs::write` into a directory created by a plain `create_dir_all`
/// (security review SR-21): mode 0644 inside a 0755 directory
/// under a typical umask. The auth-token module's 0700 handling did not cover
/// it, because in the desktop flow that module is never reached.
pub fn write_info(info: &DaemonInfo) -> io::Result<()> {
    info.validate()?;
    let json = serde_json::to_string_pretty(info).map_err(io::Error::other)?;
    write_record(&info_path(), json.as_bytes())
}

/// Unique O_EXCL temporary file and atomic rename. Never truncate an existing
/// path or follow a symlink from a previous interrupted publication.
fn write_record(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing discovery directory"))?;
    let temp = format!(".daemon-{}.tmp", uuid::Uuid::new_v4());
    #[cfg(unix)]
    {
        use nix::{fcntl::OFlag, sys::stat::Mode};
        use std::ffi::OsStr;
        let directory = crate::config::PrivateDir::prepare_config(parent)?;
        let result = (|| {
            let mut file = directory.open_file(
                OsStr::new(&temp),
                OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL,
                Mode::S_IRUSR | Mode::S_IWUSR,
            )?;
            file.write_all(bytes)?;
            file.sync_all()?;
            directory.verify()?;
            directory.rename(
                OsStr::new(&temp),
                path.file_name()
                    .ok_or_else(|| io::Error::other("missing discovery file name"))?,
            )?;
            directory.verify()
        })();
        if result.is_err() {
            let _ = directory.remove_file(OsStr::new(&temp));
        }
        result
    }
    #[cfg(not(unix))]
    {
        crate::config::ensure_private_dir(parent)?;
        let temp = parent.join(temp);
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

/// Read daemon info. Returns `None` if the file is missing or corrupt.
pub fn read_info() -> Option<DaemonInfo> {
    read_info_checked().ok().flatten()
}

/// Startup must distinguish absent metadata from damaged live discovery. The
/// native instance lock remains authoritative even when this record is lost.
pub fn read_info_checked() -> io::Result<Option<DaemonInfo>> {
    let path = info_path();
    #[cfg(unix)]
    let directory =
        match crate::config::PrivateDir::open(path.parent().expect("discovery parent"), false) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
    #[cfg(unix)]
    let opened = directory.open_file(
        path.file_name().expect("discovery name"),
        nix::fcntl::OFlag::O_RDONLY | nix::fcntl::OFlag::O_NONBLOCK,
        nix::sys::stat::Mode::empty(),
    );
    #[cfg(not(unix))]
    let opened = fs::File::open(path);
    let file = match opened {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let info = read_record(file)?;
    #[cfg(unix)]
    directory.verify()?;
    Ok(Some(info))
}

fn read_record(file: fs::File) -> io::Result<DaemonInfo> {
    use std::io::Read;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > 16 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid daemon discovery file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != nix::unistd::geteuid().as_raw() || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsafe daemon discovery ownership or permissions",
            ));
        }
    }
    let mut data = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut data)?;
    if data.len() > 16 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon discovery too large",
        ));
    }
    let info: DaemonInfo = serde_json::from_slice(&data)?;
    info.validate()?;
    Ok(info)
}

/// What `splitty daemon status --json` prints: the info file of a running daemon, or only
/// `"running": false`. Holds no credential, like the info file itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub legacy: bool,
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonInfo>,
}

impl DaemonStatus {
    pub fn of(daemon: Option<DaemonInfo>) -> Self {
        Self {
            running: daemon.is_some(),
            legacy: daemon.as_ref().is_some_and(DaemonInfo::legacy),
            daemon,
        }
    }
}

/// Remove only the expected publication. Reading and unlinking share the
/// retained private directory, so an ancestor swap cannot redirect cleanup.
pub fn remove_info_if(expected: &DaemonInfo) -> io::Result<bool> {
    #[cfg(unix)]
    {
        let directory = crate::config::PrivateDir::open(&config_dir(), false)?;
        remove_record_if(&directory, expected)
    }
    #[cfg(not(unix))]
    {
        if read_info_checked()?.as_ref() != Some(expected) {
            return Ok(false);
        }
        fs::remove_file(info_path())?;
        Ok(true)
    }
}

#[cfg(unix)]
fn remove_record_if(
    directory: &crate::config::PrivateDir,
    expected: &DaemonInfo,
) -> io::Result<bool> {
    use nix::{fcntl::OFlag, sys::stat::Mode};
    use std::ffi::OsStr;
    let name = OsStr::new("daemon.json");
    let file = directory.open_file(name, OFlag::O_RDONLY | OFlag::O_NONBLOCK, Mode::empty())?;
    use std::os::fd::AsRawFd;
    let opened = nix::sys::stat::fstat(file.as_raw_fd())?;
    if &read_record(file)? != expected {
        return Ok(false);
    }
    directory.verify()?;
    let current = directory.metadata(name)?;
    if current.st_dev != opened.st_dev || current.st_ino != opened.st_ino {
        return Ok(false);
    }
    directory.remove_file(name)?;
    Ok(true)
}

/// Check if a process with the given PID is alive (signal-0 check).
#[cfg(unix)]
pub fn is_running(pid: u32) -> bool {
    use nix::sys::signal;
    use nix::unistd::Pid;
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    pid > 0
        && matches!(
            signal::kill(Pid::from_raw(pid), None),
            Ok(()) | Err(nix::errno::Errno::EPERM)
        )
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
            format: Some(2),
            port: None,
            transport_version: Some(1),
            client_socket: Some("/run/user/1000/splitty/client.sock".into()),
            instance: Some("a".repeat(32)),
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
                "format": 2, "clientSocket": "/run/user/1000/splitty/client.sock", "instance": "a".repeat(32), "transportVersion": 1,
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
        info.validate().unwrap();
        let status = serde_json::to_value(DaemonStatus::of(Some(info))).unwrap();
        assert_eq!(status["legacy"], true);
        for key in [
            "format",
            "clientSocket",
            "instance",
            "transportVersion",
            "token",
        ] {
            assert!(status.get(key).is_none());
        }
    }

    #[test]
    fn native_discovery_requires_identity_and_never_retains_a_legacy_port_or_token() {
        let native = info();
        native.validate().unwrap();
        let mut value = serde_json::to_value(native).unwrap();
        value["token"] = serde_json::json!("old-secret");
        let decoded: DaemonInfo = serde_json::from_value(value.clone()).unwrap();
        decoded.validate().unwrap();
        assert!(!serde_json::to_string(&decoded).unwrap().contains("secret"));
        for (key, bad) in [
            ("pid", serde_json::json!(0)),
            ("format", serde_json::json!(3)),
            ("port", serde_json::json!(9800)),
            ("clientSocket", serde_json::json!("relative.sock")),
            ("instance", serde_json::json!("A".repeat(32))),
            ("transportVersion", serde_json::json!(0)),
            ("keepsSessions", serde_json::json!(false)),
        ] {
            let mut bad_value = value.clone();
            bad_value[key] = bad;
            assert!(
                serde_json::from_value::<DaemonInfo>(bad_value)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{key}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn conditional_discovery_cleanup_keeps_replacements_and_rejects_directory_swaps() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("private");
        let pinned = crate::config::PrivateDir::open(&path, true).unwrap();
        let original = info();
        write_record(
            &path.join("daemon.json"),
            &serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        let mut replacement = original.clone();
        replacement.instance = Some("b".repeat(32));
        write_record(
            &path.join("daemon.json"),
            &serde_json::to_vec(&replacement).unwrap(),
        )
        .unwrap();
        assert!(!remove_record_if(&pinned, &original).unwrap());
        assert!(remove_record_if(&pinned, &replacement).unwrap());
        write_record(
            &path.join("daemon.json"),
            &serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        fs::rename(&path, root.join("moved")).unwrap();
        crate::config::PrivateDir::open(&path, true).unwrap();
        let bytes = serde_json::to_vec(&replacement).unwrap();
        write_record(&path.join("daemon.json"), &bytes).unwrap();
        assert!(remove_record_if(&pinned, &original).is_err());
        assert_eq!(fs::read(path.join("daemon.json")).unwrap(), bytes);
        assert!(root.join("moved/daemon.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn metadata_publication_is_atomic_private_and_never_follows_a_link() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let shared = root.join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o775)).unwrap();
        let root = shared.join("private");
        crate::config::PrivateDir::open(&root, true).unwrap();
        let target = root.join("daemon.json");
        let unrelated = root.join("unrelated");
        fs::write(&unrelated, "preserve").unwrap();
        symlink(&unrelated, &target).unwrap();
        write_record(&target, b"complete").unwrap();
        assert_eq!(fs::read(&unrelated).unwrap(), b"preserve");
        assert_eq!(fs::read(&target).unwrap(), b"complete");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&shared).unwrap().mode() & 0o777, 0o775);
        let linked = root.join("linked");
        symlink(&root, &linked).unwrap();
        assert!(write_record(&linked.join("no-write.json"), b"bad").is_err());
        assert!(!root.join("no-write.json").exists());
    }
}
