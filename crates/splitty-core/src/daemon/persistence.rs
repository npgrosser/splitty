//! Workspace state persistence: save/load `WorkspaceState` as JSON.

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::config::config_dir;
use crate::mux::WorkspaceState;

/// Path to the persisted workspace state file.
pub fn state_path() -> PathBuf {
    config_dir().join("state.json")
}

/// Save workspace state atomically with owner-only permissions.
pub fn save_state(state: &WorkspaceState) -> io::Result<()> {
    let path = state_path();
    save_at(&path, state)
}

fn save_at(path: &std::path::Path, state: &WorkspaceState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        // 0700: this file records the workspace layout, and it lives beside the
        // credential files (SR-15).
        crate::config::ensure_private_dir(parent)?;
    }
    let json = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
    crate::config::write_private_file(path, json.as_bytes())
}

/// Load workspace state from disk as it was written. Returns `None` if the
/// file is missing or corrupt. A starting daemon applies
/// [`super::restore_jobs`] once it knows which keepers it adopted.
pub fn load_state() -> Option<WorkspaceState> {
    let data = fs::read_to_string(state_path()).ok()?;
    serde_json::from_str(&data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ids::JobId,
        mux::{
            jobs::{JobAdmission, JobLaunch, JobPlacement, JobResult, JobState, JobUpdate},
            Op, Store,
        },
    };
    #[test]
    fn committed_lifecycle_is_durable_rejections_do_not_write_and_restore_never_replays() {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("state.json");
        let mut store = Store::new();
        let writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = writes.clone();
        let file = path.clone();
        store.set_on_change(move |state| {
            save_at(&file, state).expect("persist");
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let id = JobId::new();
        store
            .dispatch(Op::CreateJob(Box::new(JobAdmission {
                job_id: id.clone(),
                launch: JobLaunch {
                    request_id: JobId::new().to_string(),
                    argv: vec!["program".into()],
                    cwd: "/work".into(),
                    name: None,
                    placement: JobPlacement::Screen {},
                    new_screen_id: None,
                    new_pane_id: None,
                    background: true,
                },
                placement: JobPlacement::Screen {},
                owner: None,
                created_at: 1,
            })))
            .expect("admit");
        let bytes = fs::read(&path).expect("committed");
        assert!(store
            .dispatch(Op::CloseScreen {
                screen_id: "absent".into()
            })
            .is_err());
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(fs::read(&path).expect("unchanged"), bytes);
        let mut abrupt: WorkspaceState =
            serde_json::from_slice(&bytes).expect("last committed state");
        super::super::restore_jobs(&mut abrupt, &std::collections::HashSet::new());
        assert_eq!(abrupt.jobs[&id].state, JobState::Interrupted);
        assert!(super::super::restore_effects(&abrupt, |_| false).is_empty());
        store
            .dispatch(Op::UpdateJob {
                job_id: id.clone(),
                update: JobUpdate::Exited {
                    at: 2,
                    result: JobResult {
                        exit_code: Some(23),
                        signal: None,
                    },
                },
            })
            .expect("unwatched exit");
        drop(store);
        let bytes = fs::read(&path).expect("durable exit");
        let wire: serde_json::Value = serde_json::from_slice(&bytes).expect("wire");
        for forbidden in [
            "output",
            "scrollback",
            "socket",
            "pid",
            "token",
            "connections",
        ] {
            assert!(wire["jobs"][id.as_str()].get(forbidden).is_none());
        }
        let mut restored: WorkspaceState = serde_json::from_slice(&bytes).expect("restore");
        super::super::restore_jobs(&mut restored, &std::collections::HashSet::new());
        assert_eq!(restored.jobs[&id].result.exit_code, Some(23));
        assert!(super::super::restore_effects(&restored, |_| false).is_empty());
    }

    /// session-scratchpad, Scratchpad Lifetime (Daemon restart), and
    /// server-modes, Workspace Persistence In Daemon Mode (Scratchpad
    /// persisted): a saved scratchpad is in `state.json` once applied, and the
    /// restored workspace keeps it for a respawned session and an adopted one.
    #[test]
    fn scratchpads_survive_state_json_and_the_restore() {
        use crate::ids::{PaneId, ScreenId, SessionId};
        use crate::mux::{Axis, ScreenOp};

        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("state.json");
        let mut store = Store::new();
        let file = path.clone();
        store.set_on_change(move |state| save_at(&file, state).expect("persist"));
        let screen_id = ScreenId::from("s");
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(screen_id.clone()),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .expect("screen");
        store
            .dispatch_screen(
                &screen_id,
                ScreenOp::SplitPane {
                    pane_id: PaneId::from("p1"),
                    axis: Axis::Horizontal,
                    new_pane_index: None,
                    new_pane_id: Some(PaneId::from("p2")),
                    resulting_sizes: None,
                    cwd: None,
                    shell_profile_id: None,
                },
            )
            .expect("split");
        let session = |pane: &str| -> SessionId {
            store.get_workspace_state().screens[&screen_id].panes[&PaneId::from(pane)]
                .session_id
                .clone()
                .expect("session")
        };
        let (respawned, adopted) = (session("p1"), session("p2"));
        for (session, text) in [
            (&respawned, "send after the build"),
            (&adopted, "check the diff"),
        ] {
            store
                .dispatch_screen(
                    &screen_id,
                    ScreenOp::SetScratchpad {
                        session_id: session.clone(),
                        text: text.into(),
                    },
                )
                .expect("save");
        }

        let written: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("state.json")).expect("JSON");
        assert_eq!(
            written["screens"]["s"]["sessions"][respawned.as_str()]["scratchpad"],
            "send after the build"
        );
        let mut restored: WorkspaceState = serde_json::from_value(written).expect("restore");
        let kept = std::collections::HashSet::from([adopted.as_str().to_owned()]);
        super::super::restore_jobs(&mut restored, &kept);
        let effects = super::super::restore_effects(&restored, |id| kept.contains(id));
        assert_eq!(effects.len(), 1, "one respawn; the adopted session runs on");
        let sessions = &restored.screens[&screen_id].sessions;
        assert_eq!(sessions[&respawned].scratchpad, "send after the build");
        assert_eq!(sessions[&adopted].scratchpad, "check the diff");
    }
}
