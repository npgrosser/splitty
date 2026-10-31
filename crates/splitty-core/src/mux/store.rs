//! Thread-safe workspace state store.
//!
//! `Store` wraps the workspace state behind a `RwLock` and dispatches operations
//! through the reducer. Persistence behavior (whether screens survive disconnects)
//! is determined by server mode, not per-screen ownership.

use std::sync::RwLock;

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
}

impl Store {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(WorkspaceState::default()),
            on_change: None,
        }
    }

    /// Create a store initialized with existing workspace state (for daemon restart).
    pub fn with_state(state: WorkspaceState) -> Self {
        Self {
            state: RwLock::new(state),
            on_change: None,
        }
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
        if let Some(ref cb) = self.on_change {
            cb(&state);
        }
        Ok(effects)
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
}
