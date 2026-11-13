//! Thread-safe workspace state store.
//!
//! `Store` wraps the workspace state behind a `RwLock` and dispatches operations
//! through the reducer. Persistence behavior (whether screens survive disconnects)
//! is determined by server mode, not per-screen ownership.

use std::{
    collections::{HashMap, HashSet},
    sync::RwLock,
};

use super::effects::Effect;
use super::ops::{Op, ScreenOp};
use super::reducer::reduce_workspace;
use super::state::*;
use super::MuxError;
use crate::ids::*;

/// Callback run with the new state after every successful dispatch.
type OnChange = Box<dyn Fn(&WorkspaceState) + Send + Sync>;

/// Thread-safe store for workspace state.
/// Store's only job: lock → reduce → store → unlock. Zero business logic.
pub struct Store {
    state: RwLock<WorkspaceState>,
    on_change: Option<OnChange>,
    /// The number of applied changes, for whoever waits for the next one.
    changes: tokio::sync::watch::Sender<u64>,
}

impl Store {
    pub fn new() -> Self {
        Self::with_state(WorkspaceState::default())
    }

    /// Create a store initialized with existing workspace state (for daemon restart).
    pub fn with_state(state: WorkspaceState) -> Self {
        Self {
            state: RwLock::new(state),
            on_change: None,
            changes: tokio::sync::watch::Sender::new(0),
        }
    }

    /// A receiver that sees every successful dispatch after it was taken,
    /// as a rising count; what changed is read from the store.
    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Set a callback invoked after every successful dispatch with the new state.
    /// Used for persisting state to disk in daemon mode.
    pub fn set_on_change(&mut self, cb: impl Fn(&WorkspaceState) + Send + Sync + 'static) {
        self.on_change = Some(Box::new(cb));
    }

    // ── Dispatch ───────────────────────────────────────────────────────────

    /// Dispatch a top-level operation through the workspace reducer.
    ///
    /// A rejected op leaves the workspace exactly as it was. The reducer consumes
    /// the state by value, so without the rollback below a single failed op —
    /// `move_pane` for a pane dockview already moved, a split off a pane the
    /// server never learned about — would drop every screen on the floor and
    /// every following op would fail too.
    pub fn dispatch(&self, op: Op) -> Result<Vec<Effect>, MuxError> {
        self.dispatch_inspect(op, |_, _| ())
            .map(|(effects, ())| effects)
    }

    /// Dispatch like [`Self::dispatch`], and hand `inspect` the workspace
    /// from before and after the op while the lock is still held, so that
    /// what it reads belongs to this op alone. A rejected op is not inspected.
    pub fn dispatch_inspect<R>(
        &self,
        op: Op,
        inspect: impl FnOnce(&WorkspaceState, &WorkspaceState) -> R,
    ) -> Result<(Vec<Effect>, R), MuxError> {
        let mut state = self.state.write().unwrap();
        let rollback = state.clone();
        let (new_state, effects) = match reduce_workspace(std::mem::take(&mut *state), op) {
            Ok(result) => result,
            Err(e) => {
                *state = rollback;
                return Err(e);
            }
        };
        *state = new_state;
        let inspected = inspect(&rollback, &state);
        if let Some(ref cb) = self.on_change {
            cb(&state);
        }
        self.changes
            .send_modify(|count| *count = count.wrapping_add(1));
        Ok((effects, inspected))
    }

    /// Dispatch a screen-level operation (convenience for ModifyScreen).
    pub fn dispatch_screen(
        &self,
        screen_id: &ScreenId,
        op: ScreenOp,
    ) -> Result<Vec<Effect>, MuxError> {
        self.dispatch(Op::ModifyScreen {
            screen_id: screen_id.clone(),
            op,
        })
    }

    // ── Queries ────────────────────────────────────────────────────────────

    /// Get a clone of a screen by ID.
    pub fn get_screen(&self, screen_id: &ScreenId) -> Option<ScreenState> {
        let state = self.state.read().unwrap();
        let mut screen = state.screens.get(screen_id)?.clone();
        screen.jobs = state
            .jobs
            .iter()
            .filter(|(_, job)| !job.forgotten && screen.panes.contains_key(&job.pane_id))
            .map(|(id, job)| (id.clone(), super::jobs::JobSnapshot::from(job)))
            .collect();
        Some(screen)
    }

    /// What closing `scope` would end, read under the lock without a copy of
    /// the workspace (`mux-protocol`, Close Assessment).
    pub fn assess_close(
        &self,
        scope: &super::CloseScope,
    ) -> Result<super::CloseAssessment, MuxError> {
        super::assess_close(&self.state.read().unwrap(), scope)
    }

    /// Get a clone of the full workspace state.
    pub fn get_workspace_state(&self) -> WorkspaceState {
        self.state.read().unwrap().clone()
    }

    pub fn job(&self, id: &JobId) -> Option<super::jobs::JobRecord> {
        self.state.read().expect("workspace").jobs.get(id).cloned()
    }

    pub fn jobs(&self) -> Vec<super::jobs::JobRecord> {
        self.state
            .read()
            .expect("workspace")
            .jobs
            .values()
            .cloned()
            .collect()
    }

    /// Includes removed job panes: old clients must not resurrect them via create.
    pub fn job_for_pane(&self, pane_id: &str) -> Option<(SessionId, super::jobs::JobState)> {
        let state = self.state.read().expect("workspace lock");
        if let Some(job) = state
            .jobs
            .values()
            .find(|job| job.pane_id.as_str() == pane_id)
        {
            return Some((job.session_id.clone(), job.state));
        }
        for screen in state.screens.values() {
            if let Some(pane) = screen.panes.get(&PaneId::from(pane_id)) {
                let session = pane
                    .session_id
                    .as_ref()
                    .and_then(|id| screen.sessions.get(id));
                if pane.job_id.is_some()
                    || session.is_some_and(|s| {
                        s.job_id.is_some() || s.restart_policy == super::jobs::RestartPolicy::Never
                    })
                {
                    return Some((
                        pane.session_id.clone().unwrap_or_else(|| {
                            SessionId::from(format!("unavailable-job:{pane_id}"))
                        }),
                        super::jobs::JobState::Interrupted,
                    ));
                }
            }
        }
        None
    }

    /// Get screen summaries for the welcome message, in screen_order.
    pub fn list_screen_infos(&self) -> Vec<ScreenInfo> {
        let state = self.state.read().unwrap();
        state
            .screen_order
            .iter()
            .filter_map(|id| {
                state.screens.get(id).map(|screen| ScreenInfo {
                    id: screen.id.clone(),
                    name: screen.name.clone(),
                })
            })
            .collect()
    }

    /// Project a catalog and usable active hint from one workspace read.
    /// Keep the first occurrence of each ordered ID; partial orders remain partial.
    pub fn workspace_catalog(&self) -> (Vec<ScreenInfo>, Option<ScreenId>) {
        let state = self.state.read().unwrap();
        let mut seen = HashSet::new();
        let screens = state
            .screen_order
            .iter()
            .filter_map(|id| {
                if !seen.insert(id.clone()) {
                    return None;
                }
                state.screens.get(id).map(|screen| ScreenInfo {
                    id: screen.id.clone(),
                    name: screen.name.clone(),
                })
            })
            .collect();
        let active = state
            .active_screen_id
            .as_ref()
            .filter(|id| seen.contains(*id) && state.screens.contains_key(*id))
            .cloned();
        (screens, active)
    }

    /// The document of the viewer pane `pane_id`, on whichever screen holds
    /// it; none when no pane of the role `viewer` has that ID.
    pub fn viewer_source(&self, pane_id: &PaneId) -> Option<ViewerSource> {
        let state = self.state.read().expect("workspace lock");
        state.screens.values().find_map(|screen| {
            let pane = screen.panes.get(pane_id)?;
            if pane.role != Some(PaneRole::Viewer) {
                return None;
            }
            Some(pane.viewer.as_ref()?.source.clone())
        })
    }

    /// The record of the viewer pane `pane_id`, on whichever screen holds
    /// it: its document and annotations as the next operation would see them.
    pub fn viewer(&self, pane_id: &PaneId) -> Option<ViewerState> {
        let state = self.state.read().expect("workspace lock");
        state.screens.values().find_map(|screen| {
            let pane = screen.panes.get(pane_id)?;
            if pane.role != Some(PaneRole::Viewer) {
                return None;
            }
            pane.viewer.clone()
        })
    }

    /// The wait for a review that the viewer pane `pane_id` names, on
    /// whichever screen holds it (`viewer-annotations`, Waiting For A Review).
    pub fn review_wait(&self, pane_id: &PaneId) -> Option<ReviewWait> {
        let state = self.state.read().expect("workspace lock");
        state.screens.values().find_map(|screen| {
            screen
                .panes
                .get(pane_id)?
                .viewer
                .as_ref()?
                .review_wait
                .clone()
        })
    }

    /// The number of changes applied so far; it rises with every change.
    pub fn change_count(&self) -> u64 {
        *self.changes.borrow()
    }

    /// The pane and screen of each session a pane holds, read under one lock
    /// without copying the workspace.
    pub fn session_locations(&self) -> HashMap<SessionId, (PaneId, ScreenId)> {
        let state = self.state.read().expect("workspace lock");
        let mut locations = HashMap::new();
        for screen in state.screens.values() {
            for pane in screen.panes.values() {
                if let Some(session) = &pane.session_id {
                    locations
                        .entry(session.clone())
                        .or_insert_with(|| (pane.id.clone(), screen.id.clone()));
                }
            }
        }
        locations
    }

    /// Get the session ID for a pane (searches all screens).
    pub fn session_id_for_pane(&self, pane_id: &PaneId) -> Option<SessionId> {
        let state = self.state.read().unwrap();
        for screen in state.screens.values() {
            if let Some(pane) = screen.panes.get(pane_id) {
                return pane.session_id.clone();
            }
        }
        None
    }

    /// Record the profile actually chosen by the PTY backend after fallback.
    /// Future splits inherit this value rather than a stale requested ID.
    pub fn set_resolved_shell_profile(&self, session_id: &SessionId, profile_id: Option<&str>) {
        let mut state = self.state.write().expect("workspace lock");
        for screen in state.screens.values_mut() {
            if let Some(session) = screen.sessions.get_mut(session_id) {
                session.shell_profile_id = profile_id.map(str::to_owned);
                if let Some(ref cb) = self.on_change {
                    cb(&state);
                }
                self.changes
                    .send_modify(|count| *count = count.wrapping_add(1));
                break;
            }
        }
    }

    /// The screen that holds `pane_id` in its pane table, where a pane that
    /// may have moved is looked for; none for a managed job's pane, which
    /// never moves (`move_pane_to_screen`), so a mismatch stays a conflict.
    pub fn screen_of_movable_pane(&self, pane_id: &PaneId) -> Option<ScreenId> {
        self.state
            .read()
            .unwrap()
            .screens
            .values()
            .find(|screen| {
                screen
                    .panes
                    .get(pane_id)
                    .is_some_and(|pane| pane.job_id.is_none())
            })
            .map(|screen| screen.id.clone())
    }

    /// Number of screens.
    pub fn screen_count(&self) -> usize {
        self.state.read().unwrap().screens.len()
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::state::Axis;

    #[test]
    fn workspace_catalog_keeps_order_and_names_but_filters_unlisted_active_screen() {
        let mut state = WorkspaceState::default();
        for (id, name) in [
            ("first", Some("First")),
            ("second", None),
            ("unlisted", Some("Hidden")),
        ] {
            let id = ScreenId::from(id);
            state
                .screens
                .insert(id.clone(), ScreenState::new(id, name.map(str::to_owned)));
        }
        state.screen_order = vec![
            ScreenId::from("second"),
            ScreenId::from("second"),
            ScreenId::from("missing"),
            ScreenId::from("first"),
        ];
        state.active_screen_id = Some(ScreenId::from("unlisted"));
        let store = Store::with_state(state);

        let (screens, active) = store.workspace_catalog();
        assert_eq!(
            screens
                .iter()
                .map(|screen| screen.id.as_str())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(screens[0].name, None);
        assert_eq!(screens[1].name.as_deref(), Some("First"));
        assert_eq!(active, None);

        store
            .dispatch(Op::SetActiveScreen {
                screen_id: ScreenId::from("first"),
            })
            .unwrap();
        assert_eq!(store.workspace_catalog().1, Some(ScreenId::from("first")));
        assert_eq!(Store::new().workspace_catalog().0.len(), 0);
        assert_eq!(Store::new().workspace_catalog().1, None);
    }

    #[test]
    fn dispatch_create_screen() {
        let store = Store::new();
        let effects = store
            .dispatch(Op::CreateScreen {
                name: Some("Test".to_string()),
                screen_id: Some(ScreenId::from("scr1")),
                new_pane_id: None,
                shell_profile_id: None,
            })
            .unwrap();

        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::SpawnSession { .. }));
        assert_eq!(store.screen_count(), 1);
        assert!(store.get_screen(&ScreenId::from("scr1")).is_some());
    }

    #[test]
    fn dispatch_screen_op() {
        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("scr1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .unwrap();

        let effects = store
            .dispatch_screen(
                &ScreenId::from("scr1"),
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
            .unwrap();

        assert_eq!(effects.len(), 1);
        let screen = store.get_screen(&ScreenId::from("scr1")).unwrap();
        assert_eq!(screen.panes.len(), 2);
    }

    #[test]
    fn invalid_split_sizes_leave_workspace_revision_and_notifications_untouched() {
        let store = Store::new();
        let screen_id = ScreenId::from("scr1");
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(screen_id.clone()),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .unwrap();
        let split = |pane: &str, new_pane: &str, sizes: Vec<f64>| ScreenOp::SplitPane {
            pane_id: PaneId::from(pane),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: Some(PaneId::from(new_pane)),
            resulting_sizes: Some(sizes),
            cwd: None,
            shell_profile_id: None,
        };
        let effects = store
            .dispatch_screen(&screen_id, split("p1", "p2", vec![0.4, 0.6]))
            .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::SpawnSession { .. }]));

        let before = serde_json::to_value(store.get_workspace_state()).unwrap();
        let changes = store.changes();
        for (sizes, expected_error) in [
            (vec![0.5, 0.5], "invalid sizes: expected 3 got 2"),
            (vec![0.2, 0.2, 0.2], "sizes must sum to ~1.0"),
            (vec![-0.2, 0.7, 0.5], "sizes must be non-negative"),
            (vec![f64::NAN, 0.5, 0.5], "sizes must sum to ~1.0"),
        ] {
            let error = store
                .dispatch_screen(&screen_id, split("p2", "p3", sizes))
                .unwrap_err();
            assert!(error.to_string().starts_with(expected_error));
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).unwrap(),
                before
            );
            assert!(!changes.has_changed().unwrap());
        }

        let effects = store
            .dispatch_screen(&screen_id, split("p2", "p3", vec![0.2, 0.3, 0.5]))
            .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::SpawnSession { .. }]));
        assert_eq!(store.get_screen(&screen_id).unwrap().revision, 3);
    }

    #[test]
    fn failed_dispatch_leaves_state_untouched() {
        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: Some("Work".to_string()),
                screen_id: Some(ScreenId::from("scr1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .unwrap();

        // Split off a pane the store never saw — must be rejected without
        // taking the screen (or the rest of the workspace) with it.
        let err = store.dispatch_screen(
            &ScreenId::from("scr1"),
            ScreenOp::SplitPane {
                pane_id: PaneId::from("ghost"),
                axis: Axis::Horizontal,
                new_pane_index: None,
                new_pane_id: Some(PaneId::from("p2")),
                resulting_sizes: None,
                cwd: None,
                shell_profile_id: None,
            },
        );
        assert!(err.is_err());

        assert_eq!(store.screen_count(), 1);
        let screen = store
            .get_screen(&ScreenId::from("scr1"))
            .expect("screen survives");
        assert_eq!(screen.name.as_deref(), Some("Work"));
        assert!(screen.panes.contains_key(&PaneId::from("p1")));
        assert_eq!(store.list_screen_infos().len(), 1);

        // And the store still works afterwards.
        store
            .dispatch_screen(
                &ScreenId::from("scr1"),
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
            .expect("split after a rejected op");
        assert_eq!(
            store
                .get_screen(&ScreenId::from("scr1"))
                .unwrap()
                .panes
                .len(),
            2
        );
    }

    #[test]
    fn an_inspected_dispatch_sees_its_own_op_and_a_rejected_one_is_not_inspected() {
        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("scr1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .unwrap();
        let open = |pane: &str| Op::ModifyScreen {
            screen_id: ScreenId::from("scr1"),
            op: serde_json::from_value(serde_json::json!({
                "type": "open_viewer", "paneId": pane,
                "source": {"kind": "file", "path": "/home/me/app/PLAN.md"},
            }))
            .expect("wire op"),
        };
        let shown = |before: &WorkspaceState, after: &WorkspaceState| {
            let id = ScreenId::from("scr1");
            crate::mux::shown_viewer(before.screens.get(&id)?, after.screens.get(&id)?)
        };
        let (effects, first) = store.dispatch_inspect(open("p1"), shown).expect("opened");
        assert!(effects.is_empty());
        let (pane, reused) = first.expect("a viewer pane");
        assert!(!reused);
        assert_eq!(
            store.viewer_source(&pane),
            Some(ViewerSource::File {
                path: "/home/me/app/PLAN.md".into()
            })
        );
        assert_eq!(store.viewer_source(&PaneId::from("p1")), None);
        let (_, again) = store.dispatch_inspect(open("p1"), shown).expect("again");
        assert_eq!(again, Some((pane, true)));

        let mut inspected = false;
        assert!(store
            .dispatch_inspect(open("absent"), |_, _| inspected = true)
            .is_err());
        assert!(!inspected);
    }

    #[test]
    fn session_id_for_pane() {
        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("scr1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .unwrap();

        let sid = store.session_id_for_pane(&PaneId::from("p1"));
        assert!(sid.is_some());
        assert!(store
            .session_id_for_pane(&PaneId::from("nonexistent"))
            .is_none());
    }

    #[test]
    fn list_screen_infos_respects_order() {
        let store = Store::new();
        for name in ["C", "A", "B"] {
            store
                .dispatch(Op::CreateScreen {
                    name: Some(name.to_string()),
                    screen_id: Some(ScreenId::from(name)),
                    new_pane_id: None,
                    shell_profile_id: None,
                })
                .unwrap();
        }

        let infos = store.list_screen_infos();
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].name, Some("C".to_string()));
        assert_eq!(infos[1].name, Some("A".to_string()));
        assert_eq!(infos[2].name, Some("B".to_string()));
    }

    #[test]
    fn reorder_screens() {
        let store = Store::new();
        for name in ["A", "B", "C"] {
            store
                .dispatch(Op::CreateScreen {
                    name: Some(name.to_string()),
                    screen_id: Some(ScreenId::from(name)),
                    new_pane_id: None,
                    shell_profile_id: None,
                })
                .unwrap();
        }

        store
            .dispatch(Op::ReorderScreens {
                screen_order: vec![
                    ScreenId::from("C"),
                    ScreenId::from("A"),
                    ScreenId::from("B"),
                ],
            })
            .unwrap();

        let infos = store.list_screen_infos();
        assert_eq!(infos[0].id, ScreenId::from("C"));
        assert_eq!(infos[1].id, ScreenId::from("A"));
        assert_eq!(infos[2].id, ScreenId::from("B"));
    }

    #[test]
    fn move_screen_uses_current_order_and_preserves_screen_content_and_sessions() {
        let store = Store::new();
        for name in ["A", "B", "C", "D"] {
            store
                .dispatch(Op::CreateScreen {
                    name: Some(name.into()),
                    screen_id: Some(ScreenId::from(name)),
                    new_pane_id: Some(PaneId::from(format!("pane-{name}").as_str())),
                    shell_profile_id: None,
                })
                .expect("create screen");
        }
        store
            .dispatch(Op::SetActiveScreen {
                screen_id: ScreenId::from("B"),
            })
            .expect("active B");
        let unchanged = store.get_workspace_state();
        let place = |moved: &str, before: Option<&str>| {
            let effects = store
                .dispatch(Op::MoveScreen {
                    screen_id: ScreenId::from(moved),
                    before_screen_id: before.map(ScreenId::from),
                })
                .expect("relative placement");
            assert!(effects.is_empty());
            let current = store.get_workspace_state();
            let mut expected = unchanged.clone();
            expected.screen_order = current.screen_order.clone();
            assert_eq!(
                serde_json::to_value(current).unwrap(),
                serde_json::to_value(expected).unwrap(),
                "placement changes only order, not active Screen, revision, pane or session"
            );
        };
        // A stale client observed A,B,C; D was appended before its move.
        place("C", Some("A"));
        assert_eq!(
            store.get_workspace_state().screen_order,
            ids(&["C", "A", "B", "D"])
        );
        place("C", Some("B"));
        assert_eq!(
            store.get_workspace_state().screen_order,
            ids(&["A", "C", "B", "D"])
        );
        place("A", None);
        assert_eq!(
            store.get_workspace_state().screen_order,
            ids(&["C", "B", "D", "A"])
        );
        place("D", Some("A"));
        assert_eq!(
            store.get_workspace_state().screen_order,
            ids(&["C", "B", "D", "A"])
        );
        place("B", Some("B"));
        assert_eq!(
            store.get_workspace_state().screen_order,
            ids(&["C", "B", "D", "A"])
        );
    }

    #[test]
    fn move_screen_rejects_missing_or_unlisted_identity_atomically() {
        let source = Store::new();
        for name in ["A", "B", "C", "X"] {
            source
                .dispatch(Op::CreateScreen {
                    name: None,
                    screen_id: Some(ScreenId::from(name)),
                    new_pane_id: None,
                    shell_profile_id: None,
                })
                .expect("create screen");
        }
        let mut state = source.get_workspace_state();
        state.screen_order = ids(&["B", "B", "A", "A", "C"]);
        let store = Store::with_state(state);
        let before = serde_json::to_value(store.get_workspace_state()).unwrap();
        let changes = store.changes();
        for (moved, anchor, missing) in [
            ("gone", None, true),
            ("X", None, false),
            ("A", Some("gone"), true),
            ("A", Some("X"), false),
        ] {
            let error = store
                .dispatch(Op::MoveScreen {
                    screen_id: ScreenId::from(moved),
                    before_screen_id: anchor.map(ScreenId::from),
                })
                .expect_err("invalid placement");
            assert_eq!(
                matches!(error, MuxError::ScreenNotFound(_)),
                missing,
                "missing and unordered identities have distinct errors"
            );
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).unwrap(),
                before
            );
            assert!(!changes.has_changed().unwrap());
        }
        let effects = store
            .dispatch(Op::MoveScreen {
                screen_id: ScreenId::from("A"),
                before_screen_id: Some(ScreenId::from("B")),
            })
            .expect("valid placement");
        assert!(effects.is_empty());
        assert_eq!(
            store.get_workspace_state().screen_order,
            ids(&["A", "B", "B", "C"])
        );
        assert!(store
            .get_workspace_state()
            .screens
            .contains_key(&ScreenId::from("X")));
    }

    #[test]
    fn move_screen_preserves_legacy_partial_order_after_an_unrelated_reorder() {
        let store = Store::new();
        for name in ["A", "B", "C", "D"] {
            store
                .dispatch(Op::CreateScreen {
                    name: None,
                    screen_id: Some(ScreenId::from(name)),
                    new_pane_id: None,
                    shell_profile_id: None,
                })
                .expect("create Screen");
        }
        store
            .dispatch(Op::ReorderScreens {
                screen_order: ids(&["C", "absent", "A"]),
            })
            .expect("legacy replacement order");
        assert_eq!(store.get_workspace_state().screen_order, ids(&["C", "A"]));
        let before = store.get_workspace_state();
        store
            .dispatch(Op::MoveScreen {
                screen_id: ScreenId::from("A"),
                before_screen_id: Some(ScreenId::from("C")),
            })
            .expect("relative placement after unrelated reorder");
        let mut expected = before;
        expected.screen_order = ids(&["A", "C"]);
        assert_eq!(
            serde_json::to_value(store.get_workspace_state()).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "legacy omitted Screens and all records remain unchanged"
        );
    }

    fn ids(values: &[&str]) -> Vec<ScreenId> {
        values.iter().map(|value| ScreenId::from(*value)).collect()
    }
}
