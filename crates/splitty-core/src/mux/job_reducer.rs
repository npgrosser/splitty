//! Pure placement and lifecycle updates for managed jobs.

use super::jobs::{
    JobAdmission, JobPlacement, JobRecord, JobResult, JobState, JobUpdate, RestartPolicy,
};
use super::reducer::{reduce_workspace, reject_pane_in_use};
use super::{
    Axis, Direction, Effect, LayoutNode, MuxError, Op, PaneRole, PaneState, ScreenOp, SessionEntry,
    SessionStatus, WorkspaceState,
};
use crate::ids::{JobId, PaneId, ScreenId, SessionId};

pub(super) fn reduce_create_job(
    state: WorkspaceState,
    admission: JobAdmission,
) -> Result<(WorkspaceState, Vec<Effect>), MuxError> {
    let JobAdmission {
        job_id,
        launch,
        placement,
        owner,
        created_at,
    } = admission;
    launch.validate().map_err(MuxError::Internal)?;
    if state.jobs.contains_key(&job_id) {
        return Err(MuxError::Internal("job already exists".into()));
    }
    let pane_id = launch
        .new_pane_id
        .clone()
        .unwrap_or_else(|| PaneId::with_prefix("pane"));
    reject_pane_in_use(&state, &pane_id)?;
    let (mut state, screen_id) = match &placement {
        JobPlacement::Screen {} => {
            let screen_id = launch
                .new_screen_id
                .clone()
                .unwrap_or_else(|| ScreenId::with_prefix("screen"));
            if state.screens.contains_key(&screen_id) {
                return Err(MuxError::ScreenExists(screen_id));
            }
            let (state, _) = reduce_workspace(
                state,
                Op::CreateScreen {
                    name: launch.name.clone(),
                    screen_id: Some(screen_id.clone()),
                    new_pane_id: Some(pane_id.clone()),
                    shell_profile_id: None,
                },
            )?;
            (state, screen_id)
        }
        JobPlacement::Tab { .. } | JobPlacement::Split { .. } => {
            let (screen_id, anchor) = placement.anchor().ok_or(MuxError::ScreenIdRequired)?;
            let screen = state
                .screens
                .get(screen_id)
                .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;
            if !screen.panes.contains_key(anchor) {
                return Err(MuxError::PaneNotFound(anchor.clone()));
            }
            let slot_id = screen
                .root
                .find_slot(anchor)
                .ok_or_else(|| MuxError::PaneNotInLayout(anchor.clone()))?
                .id()
                .clone();
            if let JobPlacement::Split { direction, .. } = &placement {
                let axis = match direction {
                    Direction::Left | Direction::Right => Axis::Horizontal,
                    Direction::Above | Direction::Below => Axis::Vertical,
                };
                let before = matches!(direction, Direction::Left | Direction::Above);
                let (state, _) = reduce_workspace(
                    state,
                    Op::ModifyScreen {
                        screen_id: screen_id.clone(),
                        op: ScreenOp::SplitPane {
                            pane_id: anchor.clone(),
                            axis,
                            new_pane_index: Some(if before { 0 } else { 1 }),
                            new_pane_id: Some(pane_id.clone()),
                            resulting_sizes: None,
                            cwd: Some(launch.cwd.clone()),
                            shell_profile_id: None,
                        },
                    },
                )?;
                (state, screen_id.clone())
            } else {
                let mut state = state;
                let screen = state
                    .screens
                    .get_mut(screen_id)
                    .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;
                let session_id = SessionId::with_prefix("sess");
                let slot = screen
                    .root
                    .find_node_mut(&slot_id)
                    .ok_or_else(|| MuxError::NodeNotFound(slot_id.clone()))?;
                if let LayoutNode::Slot {
                    pane_ids,
                    active_pane_id,
                    ..
                } = slot
                {
                    pane_ids.push(pane_id.clone());
                    if active_pane_id.is_none() {
                        *active_pane_id = pane_ids.first().cloned();
                    }
                }
                screen.panes.insert(
                    pane_id.clone(),
                    PaneState {
                        id: pane_id.clone(),
                        job_id: None,
                        session_id: Some(session_id.clone()),
                        title: None,
                        cwd: None,
                        role: Some(PaneRole::Terminal),
                    },
                );
                screen.sessions.insert(
                    session_id.clone(),
                    SessionEntry {
                        id: session_id,
                        job_id: None,
                        restart_policy: RestartPolicy::Shell,
                        shell_profile_id: None,
                        command: None,
                        cwd: None,
                        shell: None,
                        status: SessionStatus::Starting,
                        exit_code: None,
                        scratchpad: String::new(),
                    },
                );
                screen.revision += 1;
                (state, screen_id.clone())
            }
        }
    };
    let screen = state
        .screens
        .get_mut(&screen_id)
        .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;
    let pane = screen
        .panes
        .get_mut(&pane_id)
        .ok_or_else(|| MuxError::PaneNotFound(pane_id.clone()))?;
    let old_session_id = pane
        .session_id
        .clone()
        .ok_or_else(|| MuxError::Internal("new job pane has no session".into()))?;
    let session_id = SessionId::new();
    pane.session_id = Some(session_id.clone());
    pane.job_id = Some(job_id.clone());
    pane.title = launch.name.clone();
    pane.cwd = Some(launch.cwd.clone());
    let mut session = screen
        .sessions
        .remove(&old_session_id)
        .ok_or_else(|| MuxError::Internal("new job session missing".into()))?;
    session.id = session_id.clone();
    session.job_id = Some(job_id.clone());
    session.restart_policy = RestartPolicy::Never;
    session.shell_profile_id = None;
    session.command = None;
    session.shell = None;
    session.cwd = Some(launch.cwd.clone());
    screen.sessions.insert(session_id.clone(), session);
    let job = JobRecord {
        job_id: job_id.clone(),
        launch,
        screen_id,
        pane_id,
        session_id,
        created_by: owner.clone(),
        owner,
        created_at,
        started_at: None,
        ended_at: None,
        state: JobState::Starting,
        result: JobResult::default(),
        error: None,
        forgotten: false,
    };
    state.jobs.insert(job_id, job.clone());
    Ok((state, vec![Effect::SpawnJob { job: Box::new(job) }]))
}

pub(super) fn mark_closing_jobs(state: &mut WorkspaceState, effects: &[Effect]) {
    for effect in effects {
        if let Effect::KillSession { session_id } = effect {
            for job in state.jobs.values_mut() {
                if &job.session_id == session_id && !job.state.is_terminal() {
                    job.state = JobState::Stopping;
                }
            }
        }
    }
}

pub(super) fn reduce_update_job(
    mut state: WorkspaceState,
    job_id: JobId,
    update: JobUpdate,
) -> Result<(WorkspaceState, Vec<Effect>), MuxError> {
    let job = state
        .jobs
        .get_mut(&job_id)
        .ok_or_else(|| MuxError::Internal("job not found".into()))?;
    if job.forgotten {
        return Err(MuxError::Internal("job forgotten".into()));
    }
    match update {
        JobUpdate::Interrupted { at } => {
            job.state = JobState::Interrupted;
            job.result = JobResult::default();
            job.ended_at = Some(at);
            job.error = Some("Splitty server stopped; this job was interrupted".into());
        }
        JobUpdate::Started { at } => {
            if !job.state.is_terminal() {
                job.started_at.get_or_insert(at);
                if job.state == JobState::Starting {
                    job.state = JobState::Running;
                }
            }
        }
        JobUpdate::Failed { at, error } => {
            if !job.state.is_terminal() {
                job.state = JobState::Failed;
                job.ended_at = Some(at);
                job.error = Some(error);
            }
        }
        JobUpdate::Exited { at, result } => {
            if !job.state.is_terminal() {
                job.state = JobState::Exited;
                job.ended_at = Some(at);
                job.result = result;
            }
        }
        JobUpdate::Stop => {
            if !job.state.is_terminal() {
                job.state = JobState::Stopping;
            }
        }
        JobUpdate::Owner { owner } => job.owner = owner,
        JobUpdate::Forget => {
            if !job.state.is_terminal() {
                return Err(MuxError::Internal("cannot forget a live job".into()));
            }
            job.forgotten = true;
            let screen_id = job.screen_id.clone();
            let pane_id = job.pane_id.clone();
            if state
                .screens
                .get(&screen_id)
                .is_some_and(|s| s.panes.contains_key(&pane_id))
            {
                return reduce_workspace(
                    state,
                    Op::ModifyScreen {
                        screen_id,
                        op: ScreenOp::ClosePane {
                            pane_id,
                            resulting_sizes: None,
                        },
                    },
                );
            }
            return Ok((state, vec![]));
        }
    }
    if let Some(screen) = state.screens.get_mut(&job.screen_id) {
        if let Some(session) = screen.sessions.get_mut(&job.session_id) {
            session.status = if job.state.is_terminal() {
                SessionStatus::Exited
            } else if job.state == JobState::Starting {
                SessionStatus::Starting
            } else {
                SessionStatus::Running
            };
            session.exit_code = job
                .result
                .exit_code
                .and_then(|code| i32::try_from(code).ok());
        }
        screen.revision += 1;
    }
    Ok((state, vec![]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentId;
    use crate::mux::jobs::JobLaunch;
    use crate::mux::Store;

    fn workspace() -> Store {
        let store = Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: Some("User".into()),
                screen_id: Some(ScreenId::from("user")),
                new_pane_id: Some(PaneId::from("anchor")),
                shell_profile_id: Some("privileged-profile".into()),
            })
            .expect("user screen");
        store
    }

    fn admission(id: &str, placement: JobPlacement) -> JobAdmission {
        JobAdmission {
            job_id: JobId::from(id),
            owner: Some(AgentId::from("a")),
            created_at: 1,
            placement: placement.clone(),
            launch: JobLaunch {
                request_id: uuid::Uuid::new_v4().to_string(),
                argv: vec!["backend".into()],
                cwd: "/project".into(),
                name: Some("Backend".into()),
                placement,
                new_screen_id: None,
                new_pane_id: Some(PaneId::from(id)),
                background: true,
            },
        }
    }

    fn tab() -> JobPlacement {
        JobPlacement::Tab {
            screen_id: Some(ScreenId::from("user")),
            pane_id: Some(PaneId::from("anchor")),
        }
    }

    #[test]
    fn direct_tab_is_one_effect_without_inheritance_or_focus_change() {
        let store = workspace();
        let effects = store
            .dispatch(Op::CreateJob(Box::new(admission("job", tab()))))
            .expect("job");
        assert!(matches!(effects.as_slice(), [Effect::SpawnJob { .. }]));
        let state = store.get_workspace_state();
        assert_eq!(state.active_screen_id, Some(ScreenId::from("user")));
        let screen = &state.screens[&ScreenId::from("user")];
        match &screen.root {
            LayoutNode::Slot {
                pane_ids,
                active_pane_id,
                ..
            } => {
                assert_eq!(pane_ids, &[PaneId::from("anchor"), PaneId::from("job")]);
                assert_eq!(active_pane_id.as_ref(), Some(&PaneId::from("anchor")));
            }
            _ => panic!("tab creation must not introduce a split"),
        }
        let job = &state.jobs[&JobId::from("job")];
        let session = &screen.sessions[&job.session_id];
        assert_eq!(session.job_id, Some(job.job_id.clone()));
        assert_eq!(session.restart_policy, RestartPolicy::Never);
        assert_eq!(session.cwd.as_deref(), Some("/project"));
        assert!(
            session.shell.is_none()
                && session.shell_profile_id.is_none()
                && session.command.is_none()
        );
        let roundtrip: WorkspaceState =
            serde_json::from_value(serde_json::to_value(&state).expect("json")).expect("restore");
        assert_eq!(roundtrip.jobs[&job.job_id].launch, job.launch);
        assert_eq!(roundtrip.jobs[&job.job_id].owner, job.owner);
    }

    #[test]
    fn screen_creation_preserves_selection_and_client_ids() {
        let store = workspace();
        let mut request = admission("job", JobPlacement::Screen {});
        request.launch.new_screen_id = Some(ScreenId::from("requested-screen"));
        store
            .dispatch(Op::CreateJob(Box::new(request)))
            .expect("new screen");
        let state = store.get_workspace_state();
        assert_eq!(
            state.screen_order,
            vec![ScreenId::from("user"), ScreenId::from("requested-screen")]
        );
        assert_eq!(state.active_screen_id, Some(ScreenId::from("user")));
        assert_eq!(state.jobs[&JobId::from("job")].pane_id, PaneId::from("job"));
        let empty = Store::new();
        empty
            .dispatch(Op::CreateJob(Box::new(admission(
                "first",
                JobPlacement::Screen {},
            ))))
            .expect("first");
        let state = empty.get_workspace_state();
        assert_eq!(
            state.active_screen_id.as_ref(),
            Some(&state.jobs[&JobId::from("first")].screen_id)
        );
    }

    #[test]
    fn all_split_directions_and_same_axis_siblings() {
        for (direction, axis, before) in [
            (Direction::Left, Axis::Horizontal, true),
            (Direction::Right, Axis::Horizontal, false),
            (Direction::Above, Axis::Vertical, true),
            (Direction::Below, Axis::Vertical, false),
        ] {
            let store = workspace();
            for id in ["one", "two"] {
                let placement = JobPlacement::Split {
                    screen_id: Some(ScreenId::from("user")),
                    pane_id: Some(PaneId::from("anchor")),
                    direction: direction.clone(),
                };
                store
                    .dispatch(Op::CreateJob(Box::new(admission(id, placement))))
                    .expect("split");
            }
            let screen = store.get_screen(&ScreenId::from("user")).expect("screen");
            match screen.root {
                LayoutNode::Split {
                    axis: actual,
                    children,
                    sizes,
                    ..
                } => {
                    assert_eq!(actual, axis);
                    assert_eq!(children.len(), 3);
                    assert_eq!(sizes, vec![1.0 / 3.0; 3]);
                    let anchor_index = if before { 2 } else { 0 };
                    assert!(children[anchor_index]
                        .find_slot(&PaneId::from("anchor"))
                        .is_some());
                }
                _ => panic!("expected split"),
            }
        }
    }

    #[test]
    fn invalid_placement_leaves_the_entire_workspace_unchanged() {
        let store = workspace();
        let check = |request| {
            let before = serde_json::to_value(store.get_workspace_state()).expect("state");
            assert!(store.dispatch(Op::CreateJob(Box::new(request))).is_err());
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("state"),
                before
            );
        };
        check(admission("anchor", tab()));
        check(admission(
            "missing",
            JobPlacement::Tab {
                screen_id: Some(ScreenId::from("user")),
                pane_id: Some(PaneId::from("gone")),
            },
        ));
        let mut duplicate_screen = admission("new", JobPlacement::Screen {});
        duplicate_screen.launch.new_screen_id = Some(ScreenId::from("user"));
        check(duplicate_screen);
        store
            .dispatch(Op::CreateJob(Box::new(admission("floater", tab()))))
            .expect("tab");
        store
            .dispatch_screen(
                &ScreenId::from("user"),
                ScreenOp::FloatPane {
                    pane_id: PaneId::from("floater"),
                    bounds: None,
                },
            )
            .expect("float");
        check(admission(
            "new",
            JobPlacement::Tab {
                screen_id: Some(ScreenId::from("user")),
                pane_id: Some(PaneId::from("floater")),
            },
        ));
    }

    #[test]
    fn lifecycle_is_monotonic_and_transfer_preserves_creator() {
        let store = workspace();
        store
            .dispatch(Op::CreateJob(Box::new(admission("job", tab()))))
            .expect("job");
        let update = |update| {
            store.dispatch(Op::UpdateJob {
                job_id: JobId::from("job"),
                update,
            })
        };
        assert!(update(JobUpdate::Forget).is_err());
        update(JobUpdate::Stop).expect("stop");
        update(JobUpdate::Started { at: 2 }).expect("late start");
        assert_eq!(
            store.get_workspace_state().jobs[&JobId::from("job")].state,
            JobState::Stopping
        );
        update(JobUpdate::Exited {
            at: 3,
            result: JobResult {
                exit_code: Some(23),
                signal: None,
            },
        })
        .expect("exit");
        update(JobUpdate::Started { at: 4 }).expect("late start ignored");
        update(JobUpdate::Failed {
            at: 5,
            error: "late".into(),
        })
        .expect("late error ignored");
        update(JobUpdate::Owner {
            owner: Some(AgentId::from("b")),
        })
        .expect("transfer");
        let state = store.get_workspace_state();
        let job = &state.jobs[&JobId::from("job")];
        assert_eq!(job.state, JobState::Exited);
        assert_eq!(job.result.exit_code, Some(23));
        assert_eq!(job.ended_at, Some(3));
        assert_eq!(job.created_by, Some(AgentId::from("a")));
        assert_eq!(job.owner, Some(AgentId::from("b")));
        update(JobUpdate::Forget).expect("forget");
        assert!(store.get_workspace_state().jobs[&JobId::from("job")].forgotten);
        assert!(!store
            .get_screen(&ScreenId::from("user"))
            .expect("user")
            .panes
            .contains_key(&PaneId::from("job")));
    }

    #[test]
    fn close_pending_job_cannot_be_undone_by_a_late_start() {
        let store = workspace();
        store
            .dispatch(Op::CreateJob(Box::new(admission("job", tab()))))
            .expect("job");
        store
            .dispatch_screen(
                &ScreenId::from("user"),
                ScreenOp::ClosePane {
                    pane_id: PaneId::from("job"),
                    resulting_sizes: None,
                },
            )
            .expect("close");
        store
            .dispatch(Op::UpdateJob {
                job_id: JobId::from("job"),
                update: JobUpdate::Started { at: 2 },
            })
            .expect("late completion");
        let state = store.get_workspace_state();
        assert_eq!(state.jobs[&JobId::from("job")].state, JobState::Stopping);
        assert!(!state.screens[&ScreenId::from("user")]
            .panes
            .contains_key(&PaneId::from("job")));
    }

    #[test]
    fn moving_a_job_preserves_identity_and_failed_jobs_keep_their_outcome() {
        let store = workspace();
        let split = JobPlacement::Split {
            screen_id: Some(ScreenId::from("user")),
            pane_id: Some(PaneId::from("anchor")),
            direction: Direction::Right,
        };
        store
            .dispatch(Op::CreateJob(Box::new(admission("job", split))))
            .expect("job");
        let before = store.get_workspace_state().jobs[&JobId::from("job")].clone();
        let screen = store.get_screen(&ScreenId::from("user")).expect("screen");
        let target = screen
            .root
            .find_slot(&PaneId::from("anchor"))
            .expect("anchor")
            .id()
            .clone();
        store
            .dispatch_screen(
                &screen.id,
                ScreenOp::MovePane {
                    pane_id: before.pane_id.clone(),
                    target_slot_id: Some(target),
                    direction: None,
                    target_split_id: None,
                    source_resulting_sizes: None,
                    resulting_sizes: None,
                },
            )
            .expect("move");
        let after = store.get_workspace_state().jobs[&before.job_id].clone();
        assert_eq!(after.session_id, before.session_id);
        assert_eq!(after.owner, before.owner);
        assert_eq!(after.launch, before.launch);
        store
            .dispatch(Op::UpdateJob {
                job_id: before.job_id.clone(),
                update: JobUpdate::Failed {
                    at: 2,
                    error: "no executable".into(),
                },
            })
            .expect("fail");
        store
            .dispatch(Op::UpdateJob {
                job_id: before.job_id.clone(),
                update: JobUpdate::Started { at: 3 },
            })
            .expect("late start ignored");
        let failed = store.get_workspace_state().jobs[&before.job_id].clone();
        assert_eq!(failed.state, JobState::Failed);
        assert_eq!(failed.error.as_deref(), Some("no executable"));
    }
}
