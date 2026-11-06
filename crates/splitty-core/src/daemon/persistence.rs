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

    /// server-modes, Workspace Persistence In Daemon Mode (Viewer panes
    /// persisted), and viewer-panes, Viewer Pane Lifetime (Daemon restart):
    /// both viewer records and the selection's text are in `state.json`, the
    /// file's content is not, and the restore starts only the terminal.
    #[test]
    fn viewer_panes_survive_state_json_and_the_restore_without_a_process() {
        use crate::ids::{PaneId, ScreenId};
        use crate::mux::{Effect, ScreenOp, ViewerSource};

        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("state.json");
        let shown = dir.path().join("PLAN.md");
        fs::write(&shown, "the plan's own content").expect("file");
        let shown = shown.to_str().expect("UTF-8 path").to_owned();
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
        for op in [
            serde_json::json!({
                "type": "open_viewer", "paneId": "p1", "newPaneId": "v1",
                "source": {"kind": "file", "path": shown},
            }),
            serde_json::json!({
                "type": "open_viewer", "paneId": "p1", "newPaneId": "v2", "cwd": "/home/me/app",
                "source": {"kind": "text", "title": "Selection from claude", "text": "the agent's answer"},
            }),
        ] {
            let op: ScreenOp = serde_json::from_value(op).expect("wire op");
            assert!(store
                .dispatch_screen(&screen_id, op)
                .expect("open")
                .is_empty());
        }

        let bytes = fs::read(&path).expect("state.json");
        let written: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        let panes = &written["screens"]["s"]["panes"];
        assert_eq!(panes["v1"]["viewer"]["source"]["path"], shown.as_str());
        assert_eq!(
            panes["v2"]["viewer"]["source"]["text"],
            "the agent's answer"
        );
        assert!(!String::from_utf8_lossy(&bytes).contains("the plan's own content"));

        let mut restored: WorkspaceState = serde_json::from_value(written).expect("restore");
        super::super::restore_jobs(&mut restored, &std::collections::HashSet::new());
        let effects = super::super::restore_effects(&restored, |_| false);
        assert!(
            matches!(effects.as_slice(), [Effect::SpawnSession { pane_id, .. }] if pane_id == &PaneId::from("p1")),
            "{effects:?}"
        );
        assert_eq!(restored.screens[&screen_id].sessions.len(), 1);
        let restored = Store::with_state(restored);
        assert_eq!(
            restored.viewer_source(&PaneId::from("v1")),
            Some(ViewerSource::File { path: shown })
        );
        assert_eq!(
            restored.viewer_source(&PaneId::from("v2")),
            Some(ViewerSource::Text {
                title: "Selection from claude".into(),
                text: "the agent's answer".into(),
            })
        );
    }

    /// server-modes, Workspace Persistence In Daemon Mode (Annotations
    /// persisted), and viewer-annotations, Annotation Lifetime (Daemon
    /// restart): `state.json` holds the annotations in the viewer record once
    /// the op is applied, and the restored pane holds the same ones.
    #[test]
    fn annotations_survive_state_json_and_the_restore() {
        use crate::ids::{PaneId, ScreenId};
        use crate::mux::ScreenOp;

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
        let annotations = serde_json::json!([
            {
                "id": "ann-00000000000000a1", "kind": "question", "body": "Why not five times?",
                "target": {
                    "view": "source", "quote": "retry three times", "prefix": "On failure, ",
                    "suffix": ".", "startLine": 30, "endLine": 30,
                },
            },
            {"id": "ann-00000000000000a2", "kind": "comment", "body": "Split step 4."},
        ]);
        for op in [
            serde_json::json!({
                "type": "open_viewer", "paneId": "p1", "newPaneId": "v1",
                "source": {"kind": "file", "path": "/home/me/app/PLAN.md"},
            }),
            serde_json::json!({"type": "put_annotations", "paneId": "v1", "annotations": annotations}),
        ] {
            let op: ScreenOp = serde_json::from_value(op).expect("wire op");
            assert!(store
                .dispatch_screen(&screen_id, op)
                .expect("op")
                .is_empty());
        }

        let written: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("state.json")).expect("JSON");
        assert_eq!(
            written["screens"]["s"]["panes"]["v1"]["viewer"]["annotations"],
            annotations
        );
        let mut restored: WorkspaceState = serde_json::from_value(written).expect("restore");
        super::super::restore_jobs(&mut restored, &std::collections::HashSet::new());
        let effects = super::super::restore_effects(&restored, |_| false);
        assert_eq!(effects.len(), 1, "only the terminal starts");
        let restored = Store::with_state(restored);
        let screen = restored.get_screen(&screen_id).expect("screen");
        assert_eq!(
            screen.panes[&PaneId::from("v1")].viewer,
            store.get_screen(&screen_id).expect("screen").panes[&PaneId::from("v1")].viewer
        );
    }
}
