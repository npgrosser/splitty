//! Pure reducers that transform mux state in response to operations.
//!
//! `reduce_workspace` handles workspace-level ops (create/close/rename screen)
//! and the trusted ops (job lifecycle, an agent's reply, a wait for a review).
//! `reduce_screen` handles screen-level ops (split, close, resize, move, float, dock,
//! open a viewer, annotate one, its threads).
//! Both return `(new_state, effects)` without performing any I/O.

use super::effects::Effect;
use super::job_reducer::{mark_closing_jobs, reduce_create_job, reduce_update_job};
use super::ops::{Direction, Op, ScreenOp, ViewerPlacement};
use super::state::*;
use super::validate::{
    arranged_root, validate_agent_reply, validate_review_wait, validate_screen_op,
};
use super::MuxError;
use crate::ids::*;

fn default_screen_name(state: &WorkspaceState) -> String {
    for n in 1.. {
        let name = format!("Screen {n}");
        if !state
            .screens
            .values()
            .any(|screen| screen.name.as_ref() == Some(&name))
        {
            return name;
        }
    }
    unreachable!("finite workspace")
}

/// Apply a workspace-level operation. Returns new state + effects.
pub fn reduce_workspace(
    mut state: WorkspaceState,
    op: Op,
) -> Result<(WorkspaceState, Vec<Effect>), MuxError> {
    match op {
        Op::UpdateSessionMetadata {
            session_id,
            display_title,
            cwd,
            busy,
            exited,
            exit_code,
            close,
        } => {
            let Some(screen) = state
                .screens
                .values_mut()
                .find(|s| s.sessions.contains_key(&session_id))
            else {
                return Ok((state, vec![]));
            };
            let Some(session) = screen.sessions.get_mut(&session_id) else {
                unreachable!("located session")
            };
            let managed = session.job_id.is_some()
                || session.restart_policy == super::jobs::RestartPolicy::Never;
            let status = if exited {
                SessionStatus::Exited
            } else {
                SessionStatus::Running
            };
            let changed = session.display_title.as_deref() != Some(&display_title)
                || session.cwd.as_deref() != Some(&cwd)
                || session.busy != busy
                || (!managed && (session.status != status || session.exit_code != exit_code));
            session.display_title = Some(display_title.clone());
            session.busy = busy && !exited;
            if !cwd.is_empty() {
                session.cwd = Some(cwd.clone());
            }
            if !managed {
                session.status = status;
                session.exit_code = exit_code;
            }
            let mut close_panes = vec![];
            for pane in screen
                .panes
                .values_mut()
                .filter(|p| p.session_id.as_ref() == Some(&session_id))
            {
                if !managed && pane.job_id.is_none() {
                    pane.title = Some(display_title.clone());
                    if !cwd.is_empty() {
                        pane.cwd = Some(cwd.clone());
                    }
                    if exited && close {
                        close_panes.push(pane.id.clone());
                    }
                }
            }
            let id = screen.id.clone();
            if changed {
                screen.revision += 1;
            }
            let mut effects = vec![];
            let closes = !close_panes.is_empty();
            for pane_id in close_panes {
                let screen = state.screens.remove(&id).expect("located screen");
                let (screen, more) = reduce_screen(
                    screen,
                    ScreenOp::ClosePane {
                        pane_id,
                        resulting_sizes: None,
                    },
                )?;
                state.screens.insert(id.clone(), screen);
                effects.extend(more);
            }
            if closes {
                effects.extend(remove_screen_without_panes(&mut state, &id));
            }
            Ok((state, effects))
        }
        Op::ApplyTemplate { screen_id, request } => super::templates::apply_template(
            state,
            screen_id,
            super::templates::assign_template_ids(*request),
        ),
        Op::CreateJob(admission) => reduce_create_job(state, *admission),
        Op::UpdateJob { job_id, update } => reduce_update_job(state, job_id, update),
        Op::ModifyScreen { screen_id, op } => {
            if let ScreenOp::SplitPane {
                new_pane_id: Some(ref new_pane_id),
                ..
            }
            | ScreenOp::NewTab {
                new_pane_id: Some(ref new_pane_id),
                ..
            }
            | ScreenOp::OpenViewer {
                new_pane_id: Some(ref new_pane_id),
                ..
            } = op
            {
                reject_pane_in_use(&state, new_pane_id)?;
            }
            let screen = state
                .screens
                .remove(&screen_id)
                .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;

            let closes = matches!(op, ScreenOp::ClosePane { .. });
            let (new_screen, mut effects) = reduce_screen(screen, op)?;
            state.screens.insert(screen_id.clone(), new_screen);
            if closes {
                effects.extend(remove_screen_without_panes(&mut state, &screen_id));
            }
            mark_closing_jobs(&mut state, &effects);
            Ok((state, effects))
        }

        Op::CreateScreen {
            name,
            screen_id,
            new_pane_id,
            shell_profile_id,
        } => {
            let id = screen_id.unwrap_or_else(|| ScreenId::with_prefix("screen"));

            // Re-creating an existing screen without panes resets it to a
            // single pane (a workspace saved before screens went with their
            // last pane can hold one). Keep its name and its place in the
            // screen order. A screen that still has panes is
            // never re-created: that dropped its sessions from the model while
            // their processes kept running, which is what a reloading client
            // used to do to every screen it had not seen a snapshot for yet.
            let existing = state.screens.get(&id);
            if existing.is_some_and(|screen| !screen.panes.is_empty()) {
                return Err(MuxError::ScreenExists(id));
            }
            if let Some(ref new_pane_id) = new_pane_id {
                reject_pane_in_use(&state, new_pane_id)?;
            }
            let name = name
                .or_else(|| existing.and_then(|s| s.name.clone()))
                .or_else(|| Some(default_screen_name(&state)));
            let is_new = existing.is_none();

            let mut screen = ScreenState::new(id.clone(), name);

            let pane_id = new_pane_id.unwrap_or_else(|| PaneId::with_prefix("pane"));
            let session_id = SessionId::with_prefix("sess");

            screen.panes.insert(
                pane_id.clone(),
                PaneState {
                    id: pane_id.clone(),
                    job_id: None,
                    session_id: Some(session_id.clone()),
                    title: None,
                    cwd: None,
                    role: Some(PaneRole::Terminal),
                    viewer: None,
                },
            );
            screen.sessions.insert(
                session_id.clone(),
                SessionEntry {
                    display_title: None,
                    busy: false,
                    id: session_id.clone(),
                    job_id: None,
                    restart_policy: Default::default(),
                    shell_profile_id: shell_profile_id.clone(),
                    restore_mode: None,
                    command: None,
                    cwd: None,
                    shell: None,
                    status: SessionStatus::Starting,
                    exit_code: None,
                    scratchpad: String::new(),
                },
            );
            screen.root = LayoutNode::new_slot(NodeId::with_prefix("root"), vec![pane_id.clone()]);
            screen.revision = 1;

            if state.active_screen_id.is_none() {
                state.active_screen_id = Some(id.clone());
            }

            let effects = vec![Effect::SpawnSession {
                session_id,
                pane_id,
                screen_id: id.clone(),
                restore_mode: None,
                command: None,
                cwd: None,
                shell_profile_id,
                shell: None,
            }];

            if is_new {
                state.screen_order.push(id.clone());
            }
            state.screens.insert(id, screen);
            Ok((state, effects))
        }

        Op::CloseScreen { screen_id } => {
            if !state.screens.contains_key(&screen_id) {
                return Err(MuxError::ScreenNotFound(screen_id));
            }
            let effects = remove_screen(&mut state, &screen_id);
            mark_closing_jobs(&mut state, &effects);
            Ok((state, effects))
        }

        Op::RenameScreen { screen_id, name } => {
            let screen = state
                .screens
                .get_mut(&screen_id)
                .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;

            screen.name = Some(name);
            screen.revision += 1;
            Ok((state, vec![]))
        }

        Op::SetActiveScreen { screen_id } => {
            if !state.screens.contains_key(&screen_id) {
                return Err(MuxError::ScreenNotFound(screen_id));
            }
            state.active_screen_id = Some(screen_id);
            Ok((state, vec![]))
        }

        Op::ReorderScreens { screen_order } => {
            // Only keep IDs that actually exist
            state.screen_order = screen_order
                .into_iter()
                .filter(|id| state.screens.contains_key(id))
                .collect();
            Ok((state, vec![]))
        }

        Op::MoveScreen {
            screen_id,
            before_screen_id,
        } => {
            for id in std::iter::once(&screen_id).chain(before_screen_id.as_ref()) {
                if !state.screens.contains_key(id) {
                    return Err(MuxError::ScreenNotFound(id.clone()));
                }
                if !state.screen_order.contains(id) {
                    return Err(MuxError::ScreenNotOrdered(id.clone()));
                }
            }
            if before_screen_id.as_ref() == Some(&screen_id) {
                return Ok((state, vec![]));
            }
            state.screen_order.retain(|id| id != &screen_id);
            let index = before_screen_id
                .as_ref()
                .and_then(|before| state.screen_order.iter().position(|id| id == before));
            state
                .screen_order
                .insert(index.unwrap_or(state.screen_order.len()), screen_id);
            Ok((state, vec![]))
        }

        Op::MovePaneToScreen {
            screen_id,
            pane_id,
            target_screen_id,
            create_screen,
            screen_name,
        } => reduce_move_pane_to_screen(
            state,
            screen_id,
            pane_id,
            target_screen_id,
            create_screen,
            screen_name,
        ),

        Op::AgentReply {
            screen_id,
            pane_id,
            number,
            reply,
        } => {
            let screen = state
                .screens
                .get_mut(&screen_id)
                .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;
            validate_agent_reply(screen, &pane_id, number, &reply)?;
            if let Some(annotation) = viewer_mut(screen, &pane_id)?
                .annotations
                .iter_mut()
                .find(|annotation| annotation.number == number)
            {
                annotation.replies.push(reply);
            }
            screen.revision += 1;
            Ok((state, vec![]))
        }

        Op::StartReviewWait {
            screen_id,
            pane_id,
            wait,
            revision,
        } => {
            let screen = state
                .screens
                .get_mut(&screen_id)
                .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;
            validate_review_wait(screen, &pane_id, &wait, revision)?;
            viewer_mut(screen, &pane_id)?.review_wait = Some(wait);
            screen.revision += 1;
            Ok((state, vec![]))
        }

        Op::EndReviewWait { pane_id, wait_id } => {
            let names_wait = |viewer: &ViewerState| {
                viewer
                    .review_wait
                    .as_ref()
                    .is_some_and(|wait| wait.id == wait_id)
            };
            let screen = state
                .screens
                .values_mut()
                .find(|screen| {
                    screen
                        .panes
                        .get(&pane_id)
                        .and_then(|pane| pane.viewer.as_ref())
                        .is_some_and(names_wait)
                })
                .ok_or_else(|| MuxError::ReviewWaitNotFound(wait_id.clone()))?;
            viewer_mut(screen, &pane_id)?.review_wait = None;
            screen.revision += 1;
            Ok((state, vec![]))
        }
    }
}

// ── move_pane_to_screen ────────────────────────────────────────────────────

/// Move a pane and its session record to another screen, or to a new one
/// right after the source. No effect: the session's process is untouched. A
/// source left without panes is removed; every check happens before anything
/// changes, so a rejection leaves the workspace as it was.
fn reduce_move_pane_to_screen(
    mut state: WorkspaceState,
    screen_id: ScreenId,
    pane_id: PaneId,
    target_id: ScreenId,
    create_screen: bool,
    screen_name: Option<String>,
) -> Result<(WorkspaceState, Vec<Effect>), MuxError> {
    if target_id == screen_id {
        return Err(MuxError::SameScreen(pane_id));
    }
    let screen_name = screen_name.or_else(|| create_screen.then(|| default_screen_name(&state)));
    let source = state
        .screens
        .get(&screen_id)
        .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;
    let pane = source
        .panes
        .get(&pane_id)
        .ok_or_else(|| MuxError::PaneNotFound(pane_id.clone()))?;
    if pane.job_id.is_some() {
        return Err(MuxError::JobPaneMove(pane_id));
    }
    let location = source
        .find_slot_location(&pane_id)
        .ok_or_else(|| MuxError::PaneNotInLayout(pane_id.clone()))?;
    match (create_screen, state.screens.contains_key(&target_id)) {
        (true, true) => return Err(MuxError::ScreenExists(target_id)),
        (false, false) => return Err(MuxError::ScreenNotFound(target_id)),
        _ => {}
    }

    // Out of the source, as close_pane takes it out, but nothing is ended.
    let mut source = state
        .screens
        .remove(&screen_id)
        .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;
    let pane = source
        .panes
        .remove(&pane_id)
        .ok_or_else(|| MuxError::PaneNotFound(pane_id.clone()))?;
    let session = pane
        .session_id
        .as_ref()
        .and_then(|id| source.sessions.remove(id));
    match location {
        SlotLocation::Floating(index) => close_pane_in_floating(&mut source, &pane_id, index, None),
        SlotLocation::Root => close_pane_in_root(&mut source, &pane_id, None),
    }
    source.revision += 1;

    let mut target = if create_screen {
        let name = screen_name.or_else(|| Some(default_screen_name(&state)));
        let mut screen = ScreenState::new(target_id.clone(), name);
        screen.root = LayoutNode::new_slot(NodeId::with_prefix("root"), vec![]);
        screen
    } else {
        state
            .screens
            .remove(&target_id)
            .ok_or_else(|| MuxError::ScreenNotFound(target_id.clone()))?
    };
    let slot_id = first_slot_id(&target.root);
    if let Some(LayoutNode::Slot {
        pane_ids,
        active_pane_id,
        ..
    }) = target.root.find_node_mut(&slot_id)
    {
        pane_ids.push(pane_id.clone());
        *active_pane_id = Some(pane_id.clone());
    }
    if let Some(session) = session {
        target.sessions.insert(session.id.clone(), session);
    }
    target.panes.insert(pane_id, pane);
    target.revision += 1;

    if create_screen {
        let at = state
            .screen_order
            .iter()
            .position(|id| id == &screen_id)
            .map_or(state.screen_order.len(), |index| index + 1);
        state.screen_order.insert(at, target_id.clone());
    }
    if source.panes.is_empty() {
        // Nothing runs there any more: the screen goes with the move.
        state.screen_order.retain(|id| id != &screen_id);
        if state.active_screen_id.as_ref() == Some(&screen_id) {
            state.active_screen_id = Some(target_id.clone());
        }
    } else {
        state.screens.insert(screen_id, source);
    }
    state.screens.insert(target_id, target);
    Ok((state, vec![]))
}

/// Take a screen out of the workspace, as `close_screen` does: its sessions
/// end, it leaves the screen order, and the first remaining screen becomes
/// active when it was the active one.
fn remove_screen(state: &mut WorkspaceState, screen_id: &ScreenId) -> Vec<Effect> {
    let Some(screen) = state.screens.remove(screen_id) else {
        return vec![];
    };
    state.screen_order.retain(|id| id != screen_id);
    if state.active_screen_id.as_ref() == Some(screen_id) {
        state.active_screen_id = state.screen_order.first().cloned();
    }
    screen
        .sessions
        .keys()
        .map(|id| Effect::KillSession {
            session_id: id.clone(),
        })
        .collect()
}

/// A screen whose last pane was just closed goes with it. A pane of a
/// floating entry, a viewer pane and a job's pane all count as panes.
fn remove_screen_without_panes(state: &mut WorkspaceState, screen_id: &ScreenId) -> Vec<Effect> {
    if state
        .screens
        .get(screen_id)
        .is_some_and(|screen| screen.panes.is_empty())
    {
        remove_screen(state, screen_id)
    } else {
        vec![]
    }
}

/// The first slot of a layout, depth first.
fn first_slot_id(node: &LayoutNode) -> NodeId {
    match node {
        LayoutNode::Slot { id, .. } => id.clone(),
        LayoutNode::Split { id, children, .. } => {
            children.first().map_or_else(|| id.clone(), first_slot_id)
        }
    }
}

/// Reject a client-supplied pane ID that already names a pane anywhere in the
/// workspace: in a screen's pane table, its layout or one of its floating
/// entries. Inserting it would overwrite that pane, possibly on another screen,
/// and orphan its session.
pub(super) fn reject_pane_in_use(state: &WorkspaceState, pane_id: &PaneId) -> Result<(), MuxError> {
    let in_use = state.screens.values().any(|screen| {
        screen.panes.contains_key(pane_id) || screen.find_slot_location(pane_id).is_some()
    });
    if in_use {
        return Err(MuxError::PaneExists(pane_id.clone()));
    }
    Ok(())
}

/// Apply a screen-level operation. Returns new state + effects.
pub fn reduce_screen(
    state: ScreenState,
    op: ScreenOp,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    validate_screen_op(&state, &op)?;

    let (mut new_state, effects) = match op {
        ScreenOp::SplitPane {
            pane_id,
            axis,
            new_pane_index,
            new_pane_id,
            resulting_sizes,
            cwd,
            shell_profile_id,
        } => reduce_split_pane(
            state,
            pane_id,
            axis,
            new_pane_index,
            new_pane_id,
            resulting_sizes,
            cwd,
            shell_profile_id,
        )?,

        ScreenOp::NewTab {
            pane_id,
            new_pane_id,
            cwd,
            shell_profile_id,
        } => reduce_new_tab(state, pane_id, new_pane_id, cwd, shell_profile_id)?,

        ScreenOp::ClosePane {
            pane_id,
            resulting_sizes,
        } => reduce_close_pane(state, pane_id, resulting_sizes)?,

        ScreenOp::ResizeSplit { node_id, sizes } => reduce_resize_split(state, node_id, sizes)?,

        ScreenOp::MovePane {
            pane_id,
            target_slot_id,
            direction,
            target_split_id,
            source_resulting_sizes,
            resulting_sizes,
        } => reduce_move_pane(
            state,
            pane_id,
            target_slot_id,
            direction,
            target_split_id,
            source_resulting_sizes,
            resulting_sizes,
        )?,

        ScreenOp::MoveTab {
            pane_id,
            source_slot_id,
            target_slot_id,
            before_pane_id,
        } => reduce_move_tab(
            state,
            pane_id,
            source_slot_id,
            target_slot_id,
            before_pane_id,
        )?,

        ScreenOp::ArrangePanes { root } => {
            let mut state = state;
            state.root = arranged_root(&state, &root)?;
            (state, vec![])
        }

        ScreenOp::SelectTab { pane_id } => (reduce_select_tab(state, pane_id)?, vec![]),

        ScreenOp::FloatPane { pane_id, bounds } => reduce_float_pane(state, pane_id, bounds)?,

        ScreenOp::DockPane {
            floating_id,
            pane_id,
        } => reduce_dock_pane(state, floating_id, pane_id)?,

        ScreenOp::MoveFloat {
            floating_id,
            bounds,
        } => reduce_move_float(state, floating_id, bounds)?,

        ScreenOp::SetScratchpad { session_id, text } => {
            let mut state = state;
            state
                .sessions
                .get_mut(&session_id)
                .ok_or(MuxError::SessionNotFound(session_id))?
                .scratchpad = text;
            (state, vec![])
        }

        ScreenOp::OpenViewer {
            pane_id,
            placement,
            new_pane_id,
            source,
            cwd,
            replace,
        } => (
            reduce_open_viewer(
                state,
                pane_id,
                placement.unwrap_or_default(),
                new_pane_id,
                source,
                cwd,
                replace,
            )?,
            vec![],
        ),

        ScreenOp::PutAnnotations {
            pane_id,
            annotations,
        } => (
            reduce_put_annotations(state, &pane_id, annotations)?,
            vec![],
        ),

        ScreenOp::RemoveAnnotations { pane_id, ids } => {
            (reduce_remove_annotations(state, &pane_id, &ids)?, vec![])
        }

        ScreenOp::PutReplies {
            pane_id,
            annotation_id,
            replies,
        } => (
            reduce_put_replies(state, &pane_id, &annotation_id, replies)?,
            vec![],
        ),

        ScreenOp::RemoveReplies {
            pane_id,
            annotation_id,
            ids,
        } => (
            reduce_remove_replies(state, &pane_id, &annotation_id, &ids)?,
            vec![],
        ),

        ScreenOp::MarkSent { pane_id, entries } => {
            (reduce_mark_sent(state, &pane_id, &entries)?, vec![])
        }
    };

    new_state.revision += 1;
    Ok((new_state, effects))
}

/// Register a terminal pane and its session, with the source pane's launch defaults.
fn create_terminal_pane(
    state: &mut ScreenState,
    pane_id: &PaneId,
    new_pane_id: &PaneId,
    cwd: Option<String>,
    shell_profile_id: Option<String>,
) -> Effect {
    let new_session_id = SessionId::with_prefix("sess");

    let source_session = state
        .panes
        .get(pane_id)
        .and_then(|pane| pane.session_id.as_ref())
        .and_then(|session_id| state.sessions.get(session_id));
    let inherited_profile_id = source_session.and_then(|session| session.shell_profile_id.clone());
    let inherited_shell = source_session.and_then(|session| session.shell.clone());
    let inherited_cwd = state
        .panes
        .get(pane_id)
        .and_then(|pane| pane.cwd.clone())
        .or_else(|| source_session.and_then(|session| session.cwd.clone()));
    let explicit_profile = shell_profile_id.is_some();
    let shell_profile_id = shell_profile_id.or(inherited_profile_id);
    let shell = if explicit_profile {
        None
    } else {
        inherited_shell
    };
    let cwd = cwd.or(inherited_cwd);

    // Add new pane and session to state
    state.panes.insert(
        new_pane_id.clone(),
        PaneState {
            id: new_pane_id.clone(),
            job_id: None,
            session_id: Some(new_session_id.clone()),
            title: None,
            cwd: None,
            role: Some(PaneRole::Terminal),
            viewer: None,
        },
    );
    state.sessions.insert(
        new_session_id.clone(),
        SessionEntry {
            display_title: None,
            busy: false,
            id: new_session_id.clone(),
            job_id: None,
            restart_policy: Default::default(),
            shell_profile_id: shell_profile_id.clone(),
            restore_mode: None,
            command: None,
            cwd: cwd.clone(),
            shell: shell.clone(),
            status: SessionStatus::Starting,
            exit_code: None,
            scratchpad: String::new(),
        },
    );

    Effect::SpawnSession {
        session_id: new_session_id,
        pane_id: new_pane_id.clone(),
        screen_id: state.id.clone(),
        restore_mode: None,
        command: None,
        cwd,
        shell_profile_id,
        shell,
    }
}

// ── new_tab ────────────────────────────────────────────────────────────────

fn reduce_new_tab(
    mut state: ScreenState,
    pane_id: PaneId,
    new_pane_id: Option<PaneId>,
    cwd: Option<String>,
    shell_profile_id: Option<String>,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    let new_pane_id = new_pane_id.unwrap_or_else(|| PaneId::with_prefix("pane"));
    let effect = create_terminal_pane(&mut state, &pane_id, &new_pane_id, cwd, shell_profile_id);
    push_tab(&mut state, &pane_id, &new_pane_id)?;
    Ok((state, vec![effect]))
}

/// Add `new_pane` as the last and active tab of the slot holding `anchor`,
/// in the root layout or a floating entry.
fn push_tab(state: &mut ScreenState, anchor: &PaneId, new_pane: &PaneId) -> Result<(), MuxError> {
    let Some(LayoutNode::Slot {
        pane_ids,
        active_pane_id,
        ..
    }) = slot_mut(state, anchor)
    else {
        return Err(MuxError::PaneNotInLayout(anchor.clone()));
    };
    pane_ids.push(new_pane.clone());
    *active_pane_id = Some(new_pane.clone());
    Ok(())
}

/// The slot holding `pane_id`, in the root layout or a floating entry.
fn slot_mut<'a>(state: &'a mut ScreenState, pane_id: &PaneId) -> Option<&'a mut LayoutNode> {
    std::iter::once(&mut state.root)
        .chain(state.floating.iter_mut().map(|entry| &mut entry.root))
        .find_map(|root| root.find_slot_mut(pane_id))
}

// ── select_tab ─────────────────────────────────────────────────────────────

fn reduce_select_tab(mut state: ScreenState, pane_id: PaneId) -> Result<ScreenState, MuxError> {
    select_in_slot(&mut state, &pane_id)?;
    Ok(state)
}

/// Make `pane_id` the active pane of the slot that holds it.
fn select_in_slot(state: &mut ScreenState, pane_id: &PaneId) -> Result<(), MuxError> {
    let Some(LayoutNode::Slot { active_pane_id, .. }) = slot_mut(state, pane_id) else {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    };
    *active_pane_id = Some(pane_id.clone());
    Ok(())
}

// ── split_pane ─────────────────────────────────────────────────────────────

#[expect(
    clippy::too_many_arguments,
    reason = "the parameters are the fields of ScreenOp::SplitPane, destructured in reduce_screen"
)]
fn reduce_split_pane(
    mut state: ScreenState,
    pane_id: PaneId,
    axis: Axis,
    new_pane_index: Option<u8>,
    new_pane_id: Option<PaneId>,
    resulting_sizes: Option<Vec<f64>>,
    cwd: Option<String>,
    shell_profile_id: Option<String>,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    let new_pane_id = new_pane_id.unwrap_or_else(|| PaneId::with_prefix("pane"));
    let effect = create_terminal_pane(&mut state, &pane_id, &new_pane_id, cwd, shell_profile_id);
    insert_beside(
        &mut state.root,
        &pane_id,
        axis,
        new_pane_index == Some(0),
        &new_pane_id,
        resulting_sizes,
    )?;
    Ok((state, vec![effect]))
}

/// Put `new_pane` in a slot of its own beside the slot holding `anchor` in
/// `root`, before it or after it: a sibling in a parent split of the same
/// axis, else in a new split that takes the slot's place.
fn insert_beside(
    root: &mut LayoutNode,
    anchor: &PaneId,
    axis: Axis,
    before: bool,
    new_pane: &PaneId,
    resulting_sizes: Option<Vec<f64>>,
) -> Result<(), MuxError> {
    let slot_id = root
        .find_slot(anchor)
        .ok_or_else(|| MuxError::PaneNotInLayout(anchor.clone()))?
        .id()
        .clone();
    let new_slot = LayoutNode::new_slot(NodeId::with_prefix("slot"), vec![new_pane.clone()]);

    if let Some(info) = root
        .find_parent_info(&slot_id)
        .filter(|info| info.axis == axis)
    {
        // Same axis as the parent: add as a sibling.
        let insert_at = if before { info.index } else { info.index + 1 };
        if let Some(LayoutNode::Split {
            children, sizes, ..
        }) = root.find_node_mut(&info.id)
        {
            children.insert(insert_at, new_slot);
            *sizes = resulting_sizes.unwrap_or_else(|| equal_sizes(children.len()));
        }
        return Ok(());
    }

    // Another axis or the root slot: wrap the slot in a new split.
    let old_slot = root
        .find_node(&slot_id)
        .ok_or_else(|| MuxError::Internal("slot disappeared".to_string()))?
        .clone();
    let children = if before {
        vec![new_slot, old_slot]
    } else {
        vec![old_slot, new_slot]
    };
    let sizes = resulting_sizes.unwrap_or_else(|| vec![0.5, 0.5]);
    let new_split = LayoutNode::new_split(NodeId::with_prefix("split"), axis, children, sizes);
    replace_node(root, &slot_id, new_split);
    Ok(())
}

// ── open_viewer ────────────────────────────────────────────────────────────

/// Add a viewer pane, or show a file again in the viewer pane that shows it,
/// or with `replace` a text in the viewer pane of its title and origin
/// (`viewer-panes`, Opening A Viewer and Reopening A File). No session.
fn reduce_open_viewer(
    mut state: ScreenState,
    anchor: Option<PaneId>,
    placement: ViewerPlacement,
    new_pane_id: Option<PaneId>,
    source: ViewerSource,
    cwd: Option<String>,
    replace: bool,
) -> Result<ScreenState, MuxError> {
    let origin_pane_id = anchor
        .as_ref()
        .and_then(|anchor| viewer_origin(&state, anchor));
    let from_cli = new_pane_id.is_none();
    if from_cli {
        let shown = if replace {
            text_viewer_of(&state, &source, origin_pane_id.as_ref())
        } else {
            viewer_showing(&state, &source)
        };
        if let Some(shown) = shown {
            // A process waits for the user's answer on this text: replacing
            // it would have the answer judge a text that process never
            // showed (`viewer-panes`, Reopening A File).
            if replace
                && state
                    .panes
                    .get(&shown)
                    .and_then(|pane| pane.viewer.as_ref())
                    .is_some_and(|viewer| viewer.review_wait.is_some())
            {
                return Err(MuxError::ReviewWaitBusy(shown));
            }
            select_in_slot(&mut state, &shown)?;
            if let Some(pane) = state.panes.get_mut(&shown) {
                if replace && cwd.is_some() {
                    pane.cwd = cwd;
                }
                if let Some(viewer) = pane.viewer.as_mut() {
                    viewer.revision += 1;
                    // The annotations stay; the window anchors them again in
                    // the new text.
                    if replace {
                        viewer.source = source;
                    }
                    // The terminal that asked last; without one the origin stays.
                    if origin_pane_id.is_some() {
                        viewer.origin_pane_id = origin_pane_id;
                    }
                }
            }
            return Ok(state);
        }
    }

    // A server-chosen ID has the form a window gives its viewer panes, which
    // the CLI tells from a path (`cli`, Comments Command).
    let new_pane_id = new_pane_id.unwrap_or_else(|| PaneId::from(hex_id("viewer")));
    let (title, cwd) = match &source {
        ViewerSource::File { path } => (file_name(path).to_string(), Some(parent_dir(path))),
        ViewerSource::Text { title, .. } => (title.clone(), cwd),
    };
    let split = match placement {
        ViewerPlacement::Right => Some((Axis::Horizontal, false)),
        ViewerPlacement::Left => Some((Axis::Horizontal, true)),
        ViewerPlacement::Below => Some((Axis::Vertical, false)),
        ViewerPlacement::Above => Some((Axis::Vertical, true)),
        ViewerPlacement::Tab => None,
    };
    // A further document the CLI shows for a terminal joins the group of the
    // first viewer opened from that terminal instead of splitting it again.
    let group = match &anchor {
        Some(anchor) if from_cli && split.is_some() => viewer_of_terminal(&state, anchor),
        _ => None,
    };
    match anchor {
        // A split only beside a pane of the root layout: a floating entry
        // holds no splits, so every placement there is a tab.
        Some(anchor) => match (group, split) {
            (Some(viewer), _) => push_tab(&mut state, &viewer, &new_pane_id)?,
            (None, Some((axis, before))) if state.root.find_slot(&anchor).is_some() => {
                insert_beside(&mut state.root, &anchor, axis, before, &new_pane_id, None)?;
            }
            _ => push_tab(&mut state, &anchor, &new_pane_id)?,
        },
        None => place_without_anchor(&mut state, &new_pane_id),
    }
    state.panes.insert(
        new_pane_id.clone(),
        PaneState {
            id: new_pane_id,
            job_id: None,
            session_id: None,
            title: Some(title),
            cwd,
            role: Some(PaneRole::Viewer),
            viewer: Some(ViewerState::new(source, origin_pane_id)),
        },
    );
    Ok(state)
}

/// The panes of the screen in layout order: the root layout depth first,
/// then the floating entries.
/// The first viewer pane in layout order that shows a `text` document with
/// the title of `source` and whose origin is `origin`.
fn text_viewer_of(
    state: &ScreenState,
    source: &ViewerSource,
    origin: Option<&PaneId>,
) -> Option<PaneId> {
    let ViewerSource::Text { title, .. } = source else {
        return None;
    };
    state.panes_in_layout_order().find(|id| {
        state
            .panes
            .get(id)
            .and_then(|pane| pane.viewer.as_ref())
            .is_some_and(|viewer| {
                matches!(&viewer.source, ViewerSource::Text { title: shown, .. } if shown == title)
                    && viewer.origin_pane_id.as_ref() == origin
            })
    })
}

/// The viewer pane of the screen that already shows a `file` source, first
/// in layout order: the root layout depth first, then the floating entries.
fn viewer_showing(state: &ScreenState, source: &ViewerSource) -> Option<PaneId> {
    let ViewerSource::File { .. } = source else {
        return None;
    };
    state.panes_in_layout_order().find(|id| {
        state
            .panes
            .get(id)
            .and_then(|pane| pane.viewer.as_ref())
            .is_some_and(|viewer| &viewer.source == source)
    })
}

/// The first viewer pane in layout order (the root layout depth first, then
/// the floating entries) whose origin is the terminal pane `anchor`.
fn viewer_of_terminal(state: &ScreenState, anchor: &PaneId) -> Option<PaneId> {
    let terminal = state.panes.get(anchor)?;
    if !matches!(
        terminal.role.clone().unwrap_or_default(),
        PaneRole::Terminal
    ) {
        return None;
    }
    std::iter::once(&state.root)
        .chain(state.floating.iter().map(|entry| &entry.root))
        .flat_map(LayoutNode::collect_pane_ids)
        .find(|id| {
            state
                .panes
                .get(id)
                .and_then(|pane| pane.viewer.as_ref())
                .is_some_and(|viewer| viewer.origin_pane_id.as_ref() == Some(anchor))
        })
}

/// The origin of a viewer opened next to `anchor`: the anchor when it is a
/// terminal pane, a viewer anchor's own origin, else none.
fn viewer_origin(state: &ScreenState, anchor: &PaneId) -> Option<PaneId> {
    let pane = state.panes.get(anchor)?;
    match pane.role.clone().unwrap_or_default() {
        PaneRole::Terminal => Some(anchor.clone()),
        PaneRole::Viewer => pane.viewer.as_ref()?.origin_pane_id.clone(),
        PaneRole::Webview => None,
    }
}

/// The name of the file an absolute path names; the path itself for `/`.
fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// The directory that holds the file an absolute path names.
fn parent_dir(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => "/".to_string(),
    }
}

/// A new pane without an anchor: the only pane of a root layout that holds
/// none, else the right half of a new horizontal split around the root.
fn place_without_anchor(state: &mut ScreenState, new_pane: &PaneId) {
    if state.root.count_panes() == 0 {
        let id = match &state.root {
            LayoutNode::Slot { id, .. } => id.clone(),
            LayoutNode::Split { .. } => NodeId::with_prefix("root"),
        };
        state.root = LayoutNode::Slot {
            id,
            pane_ids: vec![new_pane.clone()],
            active_pane_id: Some(new_pane.clone()),
        };
        return;
    }
    let new_slot = LayoutNode::new_slot(NodeId::with_prefix("slot"), vec![new_pane.clone()]);
    state.root = LayoutNode::new_split(
        NodeId::with_prefix("split"),
        Axis::Horizontal,
        vec![state.root.clone(), new_slot],
        vec![0.5, 0.5],
    );
}

/// The viewer pane an applied `open_viewer` showed its document in, from the
/// screen before and after it, and whether that pane existed before.
pub fn shown_viewer(before: &ScreenState, after: &ScreenState) -> Option<(PaneId, bool)> {
    let viewers = after
        .panes
        .values()
        .filter_map(|pane| Some((pane, pane.viewer.as_ref()?)));
    let mut reused = None;
    for (pane, viewer) in viewers {
        match before
            .panes
            .get(&pane.id)
            .and_then(|pane| pane.viewer.as_ref())
        {
            None => return Some((pane.id.clone(), false)),
            Some(old) if old.revision < viewer.revision => reused = Some((pane.id.clone(), true)),
            Some(_) => {}
        }
    }
    reused
}

// ── annotations and their threads ────────────────────────────────────────

/// The record of the viewer pane `pane_id`, to change.
fn viewer_mut<'a>(
    state: &'a mut ScreenState,
    pane_id: &PaneId,
) -> Result<&'a mut ViewerState, MuxError> {
    state
        .panes
        .get_mut(pane_id)
        .and_then(|pane| pane.viewer.as_mut())
        .ok_or_else(|| MuxError::NotAViewerPane(pane_id.clone()))
}

/// Replace each annotation with the same ID in its place, keeping its number
/// and replies, and append the others with the pane's next numbers
/// (`viewer-annotations`, Saving Annotations Through The Window).
fn reduce_put_annotations(
    mut state: ScreenState,
    pane_id: &PaneId,
    annotations: Vec<ViewerAnnotation>,
) -> Result<ScreenState, MuxError> {
    let viewer = viewer_mut(&mut state, pane_id)?;
    for annotation in annotations {
        match viewer
            .annotations
            .iter_mut()
            .find(|held| held.id == annotation.id)
        {
            Some(place) => {
                let number = place.number;
                let replies = std::mem::take(&mut place.replies);
                // Only `mark_sent` hands an entry over: a put keeps the mark
                // of an annotation that reads as before, and drops it
                // otherwise.
                let sent = annotation.sent && place.sent && place.reads_as(&annotation);
                *place = ViewerAnnotation {
                    number,
                    replies,
                    sent,
                    ..annotation
                };
            }
            None => {
                // Validation keeps a number free for every new annotation.
                viewer.last_annotation_number = viewer.last_annotation_number.saturating_add(1);
                viewer.annotations.push(ViewerAnnotation {
                    number: viewer.last_annotation_number,
                    sent: false,
                    ..annotation
                });
            }
        }
    }
    Ok(state)
}

/// Remove the annotations with these IDs and their replies; IDs the pane does
/// not hold are ignored.
fn reduce_remove_annotations(
    mut state: ScreenState,
    pane_id: &PaneId,
    ids: &[String],
) -> Result<ScreenState, MuxError> {
    viewer_mut(&mut state, pane_id)?
        .annotations
        .retain(|annotation| !ids.contains(&annotation.id));
    Ok(state)
}

/// Replace each of the user's replies with the same ID in its place in the
/// thread of `annotation_id`, and append the others (`viewer-annotations`,
/// Threads).
fn reduce_put_replies(
    mut state: ScreenState,
    pane_id: &PaneId,
    annotation_id: &str,
    replies: Vec<AnnotationReply>,
) -> Result<ScreenState, MuxError> {
    let annotation = viewer_mut(&mut state, pane_id)?
        .annotations
        .iter_mut()
        .find(|annotation| annotation.id == annotation_id)
        .ok_or_else(|| MuxError::AnnotationNotFound {
            pane: pane_id.clone(),
            id: annotation_id.to_owned(),
        })?;
    for reply in replies {
        match annotation
            .replies
            .iter_mut()
            .find(|held| held.id == reply.id)
        {
            // As for annotations: only `mark_sent` hands a reply over.
            Some(place) => {
                let sent = reply.sent && place.sent && place.body == reply.body;
                *place = AnnotationReply { sent, ..reply };
            }
            None => annotation.replies.push(AnnotationReply {
                sent: false,
                ..reply
            }),
        }
    }
    Ok(state)
}

/// Remove replies of either author from the thread of `annotation_id`; IDs
/// it does not hold, and an annotation the pane no longer holds, are
/// ignored.
fn reduce_remove_replies(
    mut state: ScreenState,
    pane_id: &PaneId,
    annotation_id: &str,
    ids: &[String],
) -> Result<ScreenState, MuxError> {
    if let Some(annotation) = viewer_mut(&mut state, pane_id)?
        .annotations
        .iter_mut()
        .find(|annotation| annotation.id == annotation_id)
    {
        annotation.replies.retain(|reply| !ids.contains(&reply.id));
    }
    Ok(state)
}

/// Mark the annotations and the user's replies that still read as the
/// review held them as handed over; an entry changed since, by this window or
/// another, stays pending, and agents' replies and IDs the pane does not
/// hold are ignored (`viewer-annotations`, Handing A Review To A Terminal).
fn reduce_mark_sent(
    mut state: ScreenState,
    pane_id: &PaneId,
    entries: &[SentEntry],
) -> Result<ScreenState, MuxError> {
    let entries: std::collections::HashMap<&str, &SentEntry> = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    for annotation in &mut viewer_mut(&mut state, pane_id)?.annotations {
        if entries
            .get(annotation.id.as_str())
            .is_some_and(|entry| entry.holds_annotation(annotation))
        {
            annotation.sent = true;
        }
        for reply in &mut annotation.replies {
            if entries
                .get(reply.id.as_str())
                .is_some_and(|entry| entry.holds_reply(reply))
            {
                reply.sent = true;
            }
        }
    }
    Ok(state)
}

// ── close_pane ─────────────────────────────────────────────────────────────

fn reduce_close_pane(
    mut state: ScreenState,
    pane_id: PaneId,
    resulting_sizes: Option<Vec<f64>>,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    let mut effects = Vec::new();

    // Kill session if exists
    if let Some(pane) = state.panes.get(&pane_id) {
        if let Some(ref session_id) = pane.session_id {
            effects.push(Effect::KillSession {
                session_id: session_id.clone(),
            });
            state.sessions.remove(session_id);
        }
    }
    state.panes.remove(&pane_id);

    let location = state
        .find_slot_location(&pane_id)
        .ok_or_else(|| MuxError::PaneNotInLayout(pane_id.clone()))?;

    match location {
        SlotLocation::Floating(idx) => {
            close_pane_in_floating(&mut state, &pane_id, idx, resulting_sizes);
        }
        SlotLocation::Root => {
            close_pane_in_root(&mut state, &pane_id, resulting_sizes);
        }
    }

    Ok((state, effects))
}

fn close_pane_in_floating(
    state: &mut ScreenState,
    pane_id: &PaneId,
    floating_idx: usize,
    resulting_sizes: Option<Vec<f64>>,
) {
    let root = &mut state.floating[floating_idx].root;
    close_pane_in_tree(root, pane_id, resulting_sizes);
    if root.count_panes() == 0 {
        state.floating.remove(floating_idx);
    }
}

fn close_pane_in_root(
    state: &mut ScreenState,
    pane_id: &PaneId,
    resulting_sizes: Option<Vec<f64>>,
) {
    close_pane_in_tree(&mut state.root, pane_id, resulting_sizes);
}

/// Remove a located pane while preserving sibling subtrees, for either root kind.
fn close_pane_in_tree(root: &mut LayoutNode, pane_id: &PaneId, resulting_sizes: Option<Vec<f64>>) {
    let slot_id = root.find_slot(pane_id).unwrap().id().clone();
    let slot_pane_count = match root.find_node(&slot_id).unwrap() {
        LayoutNode::Slot { pane_ids, .. } => pane_ids.len(),
        _ => 0,
    };

    if slot_pane_count > 1 {
        // Multi-pane slot — just remove from stack
        if let Some(slot) = root.find_node_mut(&slot_id) {
            remove_pane_from_slot(slot, pane_id);
        }
    } else {
        // Single-pane slot — remove from tree
        let parent_info = root.find_parent_info(&slot_id);
        if let Some(info) = parent_info {
            let parent_id = info.id.clone();
            if let Some(parent) = root.find_node_mut(&parent_id) {
                if let LayoutNode::Split {
                    children, sizes, ..
                } = parent
                {
                    children.remove(info.index);
                    *sizes = resulting_sizes.unwrap_or_else(|| equal_sizes(children.len()));

                    if children.len() == 1 {
                        // Collapse: replace parent split with its single child
                        let child = children.remove(0);
                        replace_node(root, &parent_id, child);
                    }
                }
            }
        } else {
            // Root slot — keep a bare empty slot
            *root = LayoutNode::new_slot(slot_id, vec![]);
        }
    }
}

// ── resize_split ───────────────────────────────────────────────────────────

fn reduce_resize_split(
    mut state: ScreenState,
    node_id: NodeId,
    sizes: Vec<f64>,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    if let Some(node) = state.root.find_node_mut(&node_id) {
        if let LayoutNode::Split {
            sizes: ref mut s, ..
        } = node
        {
            *s = sizes;
        }
    }
    Ok((state, vec![]))
}

// ── move_pane ──────────────────────────────────────────────────────────────

fn reduce_move_pane(
    mut state: ScreenState,
    pane_id: PaneId,
    target_slot_id: Option<NodeId>,
    direction: Option<Direction>,
    target_split_id: Option<NodeId>,
    source_resulting_sizes: Option<Vec<f64>>,
    resulting_sizes: Option<Vec<f64>>,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    // Remove pane from source slot
    let src_slot_id = state
        .root
        .find_slot(&pane_id)
        .ok_or_else(|| MuxError::PaneNotInLayout(pane_id.clone()))?
        .id()
        .clone();

    let src_pane_count = match state.root.find_node(&src_slot_id).unwrap() {
        LayoutNode::Slot { pane_ids, .. } => pane_ids.len(),
        _ => 0,
    };

    if src_pane_count <= 1 {
        // Source slot becomes empty — remove from tree
        let parent_info = state.root.find_parent_info(&src_slot_id);
        let Some(info) = parent_info else {
            // The only pane of the grid has nowhere else to go.
            let target = target_slot_id.or(target_split_id).unwrap_or(src_slot_id);
            return Err(MuxError::MoveTargetGone(target));
        };
        let parent_id = info.id.clone();
        if let Some(parent) = state.root.find_node_mut(&parent_id) {
            if let LayoutNode::Split {
                children, sizes, ..
            } = parent
            {
                children.remove(info.index);
                *sizes = source_resulting_sizes.unwrap_or_else(|| equal_sizes(children.len()));
                if children.len() == 1 {
                    let child = children.remove(0);
                    replace_node(&mut state.root, &parent_id, child);
                }
            }
        }
    } else {
        // Remove from source stack
        if let Some(slot) = state.root.find_node_mut(&src_slot_id) {
            remove_pane_from_slot(slot, &pane_id);
        }
    }

    // Place pane at destination. Taking the pane out may have removed the
    // target (its own slot, or a split that collapsed): the move then fails,
    // and the store keeps the state from before it.
    if let Some(target_slot) = target_slot_id {
        // Tab into existing slot
        let Some(LayoutNode::Slot {
            pane_ids,
            active_pane_id,
            ..
        }) = state.root.find_node_mut(&target_slot)
        else {
            return Err(MuxError::MoveTargetGone(target_slot));
        };
        pane_ids.push(pane_id.clone());
        *active_pane_id = Some(pane_id);
    } else if let (Some(dir), Some(target_id)) = (direction, target_split_id) {
        // Create new split
        let Some(target_node_id) = find_target_node_id(&state.root, &target_id) else {
            return Err(MuxError::MoveTargetGone(target_id));
        };
        let Some(old_target) = state.root.find_node(&target_node_id).cloned() else {
            return Err(MuxError::MoveTargetGone(target_id));
        };
        let new_slot_id = NodeId::with_prefix("slot");
        let new_slot = LayoutNode::new_slot(new_slot_id, vec![pane_id]);

        let axis = match dir {
            Direction::Left | Direction::Right => Axis::Horizontal,
            Direction::Above | Direction::Below => Axis::Vertical,
        };

        let children = match dir {
            Direction::Left | Direction::Above => vec![new_slot, old_target],
            Direction::Right | Direction::Below => vec![old_target, new_slot],
        };

        let sizes = resulting_sizes.unwrap_or_else(|| vec![0.5, 0.5]);
        let split_id = NodeId::with_prefix("split");
        let new_split = LayoutNode::new_split(split_id, axis, children, sizes);
        replace_node(&mut state.root, &target_node_id, new_split);
    } else {
        return Err(MuxError::MoveTargetRequired);
    }

    Ok((state, vec![]))
}

/// Exact tab placement reuses the existing cross-slot removal/collapse path,
/// but derives surviving proportions from the current authoritative layout.
/// Same-slot reorder never removes the slot or changes its effective selection.
fn reduce_move_tab(
    mut state: ScreenState,
    pane_id: PaneId,
    source_slot_id: NodeId,
    target_slot_id: NodeId,
    before_pane_id: Option<PaneId>,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    let same_slot = source_slot_id == target_slot_id;
    if !same_slot {
        let source_sizes = surviving_source_sizes(&state.root, &source_slot_id);
        (state, _) = reduce_move_pane(
            state,
            pane_id.clone(),
            Some(target_slot_id.clone()),
            None,
            None,
            source_sizes,
            None,
        )?;
    }
    let Some(LayoutNode::Slot {
        pane_ids,
        active_pane_id,
        ..
    }) = state.root.find_node_mut(&target_slot_id)
    else {
        return Err(MuxError::MoveTargetGone(target_slot_id));
    };
    // None means the first tab is active. Preserve that tab when order changes.
    let selected = active_pane_id.clone().or_else(|| pane_ids.first().cloned());
    pane_ids.retain(|id| id != &pane_id);
    let index = match before_pane_id {
        Some(before) => {
            pane_ids
                .iter()
                .position(|id| id == &before)
                .ok_or(MuxError::PaneOutsideSlot {
                    pane: before,
                    slot: target_slot_id,
                })?
        }
        None => pane_ids.len(),
    };
    pane_ids.insert(index, pane_id);
    if same_slot && active_pane_id.is_none() && pane_ids.first() != selected.as_ref() {
        *active_pane_id = selected;
    }
    Ok((state, vec![]))
}

fn surviving_source_sizes(root: &LayoutNode, source_slot_id: &NodeId) -> Option<Vec<f64>> {
    let info = root.find_parent_info(source_slot_id)?;
    let LayoutNode::Split {
        children, sizes, ..
    } = root.find_node(&info.id)?
    else {
        return None;
    };
    let mut remaining: Vec<_> = sizes
        .iter()
        .enumerate()
        .filter_map(|(index, size)| (index != info.index).then_some(*size))
        .collect();
    let sum: f64 = remaining.iter().sum();
    if remaining.len() != children.len() - 1
        || !sum.is_finite()
        || sum <= 0.0
        || remaining
            .iter()
            .any(|size| !size.is_finite() || *size < 0.0)
    {
        return Some(equal_sizes(children.len() - 1));
    }
    for size in &mut remaining {
        *size /= sum;
    }
    Some(remaining)
}

/// Find the actual node ID for a move target. If the ID is a pane ID, find its slot.
fn find_target_node_id(root: &LayoutNode, id: &NodeId) -> Option<NodeId> {
    if root.find_node(id).is_some() {
        return Some(id.clone());
    }
    // Try as pane ID
    let pane_id = PaneId::from(id.as_str());
    root.find_slot(&pane_id).map(|s| s.id().clone())
}

// ── float_pane ─────────────────────────────────────────────────────────────

fn reduce_float_pane(
    mut state: ScreenState,
    pane_id: PaneId,
    bounds: Option<FloatBounds>,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    let slot_id = state
        .root
        .find_slot(&pane_id)
        .ok_or_else(|| MuxError::PaneNotInLayout(pane_id.clone()))?
        .id()
        .clone();

    let slot_pane_count = match state.root.find_node(&slot_id).unwrap() {
        LayoutNode::Slot { pane_ids, .. } => pane_ids.len(),
        _ => 0,
    };

    if slot_pane_count > 1 {
        // Remove from stack
        if let Some(slot) = state.root.find_node_mut(&slot_id) {
            remove_pane_from_slot(slot, &pane_id);
        }
    } else {
        // Remove slot from tree
        let parent_info = state.root.find_parent_info(&slot_id);
        if let Some(info) = parent_info {
            let parent_id = info.id.clone();
            if let Some(parent) = state.root.find_node_mut(&parent_id) {
                if let LayoutNode::Split {
                    children, sizes, ..
                } = parent
                {
                    children.remove(info.index);
                    *sizes = equal_sizes(children.len());
                    if children.len() == 1 {
                        let child = children.remove(0);
                        replace_node(&mut state.root, &parent_id, child);
                    }
                }
            }
        } else {
            return Err(MuxError::Internal("cannot float the root slot".to_string()));
        }
    }

    let float_id = FloatingId::with_prefix("float");
    let new_slot_id = NodeId::with_prefix("slot");
    let entry = FloatingEntry {
        id: float_id,
        root: LayoutNode::new_slot(new_slot_id, vec![pane_id]),
        bounds: bounds.unwrap_or_default(),
    };
    state.floating.push(entry);

    Ok((state, vec![]))
}

// ── dock_pane ──────────────────────────────────────────────────────────────

fn reduce_dock_pane(
    mut state: ScreenState,
    floating_id: FloatingId,
    target_pane_id: PaneId,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    // Find and remove the floating entry
    let entry_idx = state
        .floating
        .iter()
        .position(|f| f.id == floating_id)
        .ok_or(MuxError::FloatingNotFound(floating_id))?;

    let entry = state.floating.remove(entry_idx);
    let pane_ids = entry.root.collect_pane_ids();

    // Find target slot (by pane), fallback to first slot
    let target_slot_id = state
        .root
        .find_slot(&target_pane_id)
        .map(|s| s.id().clone())
        .or_else(|| {
            let mut first_id = None;
            state.root.walk(&mut |n| {
                if first_id.is_none() {
                    if let LayoutNode::Slot { id, .. } = n {
                        first_id = Some(id.clone());
                        return false;
                    }
                }
                true
            });
            first_id
        });

    if let Some(slot_id) = target_slot_id {
        if let Some(slot) = state.root.find_node_mut(&slot_id) {
            if let LayoutNode::Slot {
                pane_ids: slot_panes,
                active_pane_id,
                ..
            } = slot
            {
                slot_panes.extend(pane_ids.iter().cloned());
                if let Some(first) = pane_ids.first() {
                    *active_pane_id = Some(first.clone());
                }
            }
        }
    }

    Ok((state, vec![]))
}

// ── move_float ─────────────────────────────────────────────────────────────

fn reduce_move_float(
    mut state: ScreenState,
    floating_id: FloatingId,
    bounds: FloatBounds,
) -> Result<(ScreenState, Vec<Effect>), MuxError> {
    let entry = state
        .floating
        .iter_mut()
        .find(|f| f.id == floating_id)
        .ok_or(MuxError::FloatingNotFound(floating_id))?;

    entry.bounds = bounds;
    Ok((state, vec![]))
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Remove a pane from a slot, updating active_pane_id if needed.
fn remove_pane_from_slot(slot: &mut LayoutNode, pane_id: &PaneId) {
    if let LayoutNode::Slot {
        pane_ids,
        active_pane_id,
        ..
    } = slot
    {
        pane_ids.retain(|id| id != pane_id);
        if active_pane_id.as_ref() == Some(pane_id) {
            *active_pane_id = pane_ids.first().cloned();
        }
    }
}

/// Convert a PaneId to a NodeId (for target lookups where IDs might be pane IDs).
impl From<PaneId> for NodeId {
    fn from(p: PaneId) -> Self {
        NodeId::from(p.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn omitted_screen_names_are_unique_and_explicit_names_survive() {
        let mut state = WorkspaceState::default();
        for (id, name, expected) in [
            ("a", Some("Screen 2"), "Screen 2"),
            ("b", None, "Screen 1"),
            ("c", None, "Screen 3"),
            ("d", Some(""), ""),
        ] {
            (state, _) = reduce_workspace(
                state,
                Op::CreateScreen {
                    name: name.map(str::to_owned),
                    screen_id: Some(id.into()),
                    new_pane_id: Some(format!("p-{id}").into()),
                    shell_profile_id: None,
                },
            )
            .expect("create");
            assert_eq!(
                state.screens[&ScreenId::from(id)].name.as_deref(),
                Some(expected)
            );
        }
        (state, _) = reduce_workspace(
            state,
            Op::MovePaneToScreen {
                screen_id: "b".into(),
                pane_id: "p-b".into(),
                target_screen_id: "e".into(),
                create_screen: true,
                screen_name: None,
            },
        )
        .expect("move");
        assert_eq!(
            state.screens[&ScreenId::from("e")].name.as_deref(),
            Some("Screen 4")
        );
    }

    #[test]
    fn metadata_follows_moved_session_and_inherits_for_tabs() {
        let store = super::super::Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some("a".into()),
                new_pane_id: Some("p".into()),
                shell_profile_id: Some("fish".into()),
            })
            .expect("screen");
        let session = store.get_screen(&"a".into()).expect("screen").panes[&PaneId::from("p")]
            .session_id
            .clone()
            .expect("session");
        store
            .dispatch(Op::MovePaneToScreen {
                screen_id: "a".into(),
                pane_id: "p".into(),
                target_screen_id: "b".into(),
                create_screen: true,
                screen_name: None,
            })
            .expect("move");
        store
            .dispatch(Op::UpdateSessionMetadata {
                session_id: session.clone(),
                display_title: "fish: /work".into(),
                cwd: "/work".into(),
                busy: true,
                exited: false,
                exit_code: None,
                close: false,
            })
            .expect("metadata");
        let screen = store.get_screen(&"b".into()).expect("screen");
        assert!(screen.sessions[&session].busy);
        assert_eq!(
            screen.panes[&PaneId::from("p")].title.as_deref(),
            Some("fish: /work")
        );
        let effects = store
            .dispatch_screen(
                &"b".into(),
                ScreenOp::NewTab {
                    pane_id: "p".into(),
                    new_pane_id: Some("tab".into()),
                    cwd: None,
                    shell_profile_id: None,
                },
            )
            .expect("tab");
        assert!(
            matches!(&effects[0], Effect::SpawnSession { cwd: Some(cwd), shell_profile_id: Some(profile), .. } if cwd == "/work" && profile == "fish")
        );
        let effects = store
            .dispatch(Op::UpdateSessionMetadata {
                session_id: session.clone(),
                display_title: "fish".into(),
                cwd: String::new(),
                busy: false,
                exited: true,
                exit_code: Some(0),
                close: true,
            })
            .expect("exit");
        assert!(
            matches!(&effects[0], Effect::KillSession { session_id } if session_id == &session)
        );
        assert!(!store
            .get_screen(&"b".into())
            .expect("screen")
            .panes
            .contains_key(&PaneId::from("p")));
        assert!(store
            .dispatch(Op::UpdateSessionMetadata {
                session_id: session,
                display_title: String::new(),
                cwd: String::new(),
                busy: false,
                exited: true,
                exit_code: Some(0),
                close: true
            })
            .expect("late metadata ignored")
            .is_empty());
    }

    #[test]
    fn managed_session_metadata_never_replaces_job_title_or_closes_it() {
        let mut screen = single_pane_screen();
        screen
            .sessions
            .get_mut(&"s1".into())
            .expect("session")
            .restart_policy = super::super::jobs::RestartPolicy::Never;
        screen.panes.get_mut(&"p1".into()).expect("pane").title = Some("Build".into());
        let mut state = WorkspaceState::default();
        state.screens.insert(screen.id.clone(), screen);
        let (state, effects) = reduce_workspace(
            state,
            Op::UpdateSessionMetadata {
                session_id: "s1".into(),
                display_title: "compiler".into(),
                cwd: "/work".into(),
                busy: false,
                exited: true,
                exit_code: Some(23),
                close: true,
            },
        )
        .expect("metadata");
        assert!(effects.is_empty());
        assert_eq!(
            state.screens[&ScreenId::from("screen1")].panes[&PaneId::from("p1")]
                .title
                .as_deref(),
            Some("Build")
        );
    }

    fn single_pane_screen() -> ScreenState {
        let pane_id = PaneId::from("p1");
        let session_id = SessionId::from("s1");
        let mut panes = HashMap::new();
        panes.insert(
            pane_id.clone(),
            PaneState {
                id: pane_id.clone(),
                job_id: None,
                session_id: Some(session_id.clone()),
                title: None,
                cwd: None,
                role: Some(PaneRole::Terminal),
                viewer: None,
            },
        );
        let mut sessions = HashMap::new();
        sessions.insert(
            session_id.clone(),
            SessionEntry {
                display_title: None,
                busy: false,
                id: session_id,
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
                restore_mode: None,
                command: None,
                cwd: None,
                shell: None,
                status: SessionStatus::Running,
                exit_code: None,
                scratchpad: String::new(),
            },
        );
        ScreenState {
            id: ScreenId::from("screen1"),
            jobs: HashMap::new(),
            name: None,
            revision: 0,
            root: LayoutNode::new_slot(NodeId::from("slot1"), vec![pane_id]),
            floating: vec![],
            panes,
            sessions,
        }
    }

    fn two_pane_screen() -> ScreenState {
        let p1 = PaneId::from("p1");
        let p2 = PaneId::from("p2");
        let s1 = SessionId::from("s1");
        let s2 = SessionId::from("s2");

        let mut panes = HashMap::new();
        panes.insert(
            p1.clone(),
            PaneState {
                id: p1.clone(),
                job_id: None,
                session_id: Some(s1.clone()),
                title: None,
                cwd: None,
                role: Some(PaneRole::Terminal),
                viewer: None,
            },
        );
        panes.insert(
            p2.clone(),
            PaneState {
                id: p2.clone(),
                job_id: None,
                session_id: Some(s2.clone()),
                title: None,
                cwd: None,
                role: Some(PaneRole::Terminal),
                viewer: None,
            },
        );

        let mut sessions = HashMap::new();
        sessions.insert(
            s1.clone(),
            SessionEntry {
                display_title: None,
                busy: false,
                id: s1,
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
                restore_mode: None,
                command: None,
                cwd: None,
                shell: None,
                status: SessionStatus::Running,
                exit_code: None,
                scratchpad: String::new(),
            },
        );
        sessions.insert(
            s2.clone(),
            SessionEntry {
                display_title: None,
                busy: false,
                id: s2,
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
                restore_mode: None,
                command: None,
                cwd: None,
                shell: None,
                status: SessionStatus::Running,
                exit_code: None,
                scratchpad: String::new(),
            },
        );

        ScreenState {
            id: ScreenId::from("screen1"),
            jobs: HashMap::new(),
            name: None,
            revision: 0,
            root: LayoutNode::new_split(
                NodeId::from("split1"),
                Axis::Horizontal,
                vec![
                    LayoutNode::new_slot(NodeId::from("slot1"), vec![p1]),
                    LayoutNode::new_slot(NodeId::from("slot2"), vec![p2]),
                ],
                vec![0.5, 0.5],
            ),
            floating: vec![],
            panes,
            sessions,
        }
    }

    // ── new_tab tests ───

    fn new_tab(pane: &str, new_pane: &str) -> ScreenOp {
        // Exercise the public wire format as well as the reducer.
        serde_json::from_value(serde_json::json!({
            "type": "new_tab", "paneId": pane, "newPaneId": new_pane
        }))
        .unwrap()
    }

    #[test]
    fn new_tab_keeps_the_source_slot_and_creates_one_session() {
        let mut state = single_pane_screen();
        for id in ["p2", "p3"] {
            let revision = state.revision;
            let (next, effects) = reduce_screen(state, new_tab("p1", id)).unwrap();
            let session_id = next.panes[&PaneId::from(id)].session_id.as_ref().unwrap();
            assert!(matches!(effects.as_slice(), [Effect::SpawnSession {
                pane_id, session_id: spawned, screen_id, ..
            }] if pane_id == &PaneId::from(id) && spawned == session_id && screen_id == &next.id));
            assert_eq!(next.revision, revision + 1);
            state = next;
        }
        assert!(
            matches!(&state.root, LayoutNode::Slot { id, pane_ids, active_pane_id }
            if id == &NodeId::from("slot1")
                && pane_ids == &[PaneId::from("p1"), PaneId::from("p2"), PaneId::from("p3")]
                && active_pane_id == &Some(PaneId::from("p3")))
        );
        assert_eq!(state.sessions.len(), 3);
        let (state, effects) = reduce_screen(
            state,
            ScreenOp::ClosePane {
                pane_id: PaneId::from("p3"),
                resulting_sizes: None,
            },
        )
        .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::KillSession { .. }]));
        assert!(matches!(&state.root, LayoutNode::Slot { pane_ids, .. }
            if pane_ids == &[PaneId::from("p1"), PaneId::from("p2")]));
    }

    #[test]
    fn new_tab_preserves_split_sizes_and_other_slots() {
        let mut state = two_pane_screen();
        if let LayoutNode::Split { sizes, .. } = &mut state.root {
            *sizes = vec![0.3, 0.7];
        }
        let (state, _) = reduce_screen(state, new_tab("p2", "p3")).unwrap();
        match &state.root {
            LayoutNode::Split {
                id,
                children,
                sizes,
                ..
            } => {
                assert_eq!(id, &NodeId::from("split1"));
                assert_eq!(sizes, &[0.3, 0.7]);
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], LayoutNode::Slot { pane_ids, .. }
                    if pane_ids == &[PaneId::from("p1")]));
                assert!(
                    matches!(&children[1], LayoutNode::Slot { pane_ids, active_pane_id, .. }
                    if pane_ids == &[PaneId::from("p2"), PaneId::from("p3")]
                        && active_pane_id == &Some(PaneId::from("p3")))
                );
            }
            _ => panic!("existing split must remain unchanged"),
        }
    }

    #[test]
    fn new_tab_stays_in_its_floating_group() {
        let (state, _) = reduce_screen(
            two_pane_screen(),
            ScreenOp::FloatPane {
                pane_id: PaneId::from("p2"),
                bounds: None,
            },
        )
        .unwrap();
        let root = serde_json::to_value(&state.root).unwrap();
        let floating_id = state.floating[0].id.clone();
        let (state, _) = reduce_screen(state, new_tab("p2", "p3")).unwrap();
        assert_eq!(serde_json::to_value(&state.root).unwrap(), root);
        assert_eq!(state.floating.len(), 1);
        assert_eq!(state.floating[0].id, floating_id);
        assert!(
            matches!(&state.floating[0].root, LayoutNode::Slot { pane_ids, .. }
            if pane_ids == &[PaneId::from("p2"), PaneId::from("p3")])
        );
    }

    #[test]
    fn new_tab_inherits_launch_defaults_and_honors_explicit_overrides() {
        for explicit in [false, true] {
            let mut state = single_pane_screen();
            let source = state.sessions.get_mut(&SessionId::from("s1")).unwrap();
            source.shell_profile_id = Some("shell-zsh".into());
            source.shell = Some("/legacy/zsh".into());
            source.cwd = Some("/session-dir".into());
            state.panes.get_mut(&PaneId::from("p1")).unwrap().cwd = Some("/pane-dir".into());
            let op = ScreenOp::NewTab {
                pane_id: PaneId::from("p1"),
                new_pane_id: Some(PaneId::from("p2")),
                cwd: explicit.then(|| "/explicit-dir".into()),
                shell_profile_id: explicit.then(|| "shell-fish".into()),
            };
            let (state, effects) = reduce_screen(state, op).unwrap();
            let session = &state.sessions[state.panes[&PaneId::from("p2")]
                .session_id
                .as_ref()
                .unwrap()];
            assert_eq!(
                session.cwd.as_deref(),
                Some(if explicit {
                    "/explicit-dir"
                } else {
                    "/pane-dir"
                })
            );
            assert_eq!(
                session.shell_profile_id.as_deref(),
                Some(if explicit { "shell-fish" } else { "shell-zsh" })
            );
            assert_eq!(
                session.shell.as_deref(),
                if explicit { None } else { Some("/legacy/zsh") }
            );
            assert!(
                matches!(effects.as_slice(), [Effect::SpawnSession { cwd, shell_profile_id, shell, .. }]
                if cwd == &session.cwd && shell_profile_id == &session.shell_profile_id && shell == &session.shell)
            );
        }
    }

    #[test]
    fn invalid_new_tabs_leave_the_workspace_unchanged() {
        let store = two_screen_store();
        let before = serde_json::to_value(store.get_workspace_state()).unwrap();
        for (anchor, new_id) in [("missing", "new"), ("p1", "p1"), ("p1", "p2")] {
            assert!(store
                .dispatch_screen(&ScreenId::from("scr1"), new_tab(anchor, new_id))
                .is_err());
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).unwrap(),
                before
            );
        }
    }

    // ── split_pane tests ───

    fn sized_split(
        pane: &str,
        axis: Axis,
        before: bool,
        new_pane: &str,
        sizes: Vec<f64>,
    ) -> ScreenOp {
        ScreenOp::SplitPane {
            pane_id: PaneId::from(pane),
            axis,
            new_pane_index: Some(if before { 0 } else { 1 }),
            new_pane_id: Some(PaneId::from(new_pane)),
            resulting_sizes: Some(sizes),
            cwd: None,
            shell_profile_id: None,
        }
    }

    #[test]
    fn split_pane_explicit_sizes_for_root_and_different_axis() {
        let (root, effects) = reduce_screen(
            single_pane_screen(),
            sized_split("p1", Axis::Horizontal, false, "p2", vec![0.2, 0.8]),
        )
        .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::SpawnSession { .. }]));
        assert!(
            matches!(&root.root, LayoutNode::Split { children, sizes, .. }
            if children.len() == 2 && sizes == &[0.2, 0.8])
        );

        let (nested, effects) = reduce_screen(
            two_pane_screen(),
            sized_split("p2", Axis::Vertical, true, "p3", vec![0.6, 0.4]),
        )
        .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::SpawnSession { .. }]));
        let LayoutNode::Split {
            children, sizes, ..
        } = &nested.root
        else {
            panic!("expected outer split");
        };
        assert_eq!(sizes, &[0.5, 0.5]);
        assert!(
            matches!(&children[1], LayoutNode::Split { axis: Axis::Vertical, children, sizes, .. }
            if children.len() == 2
                && children[0].find_slot(&PaneId::from("p3")).is_some()
                && children[1].find_slot(&PaneId::from("p2")).is_some()
                && sizes == &[0.6, 0.4])
        );
    }

    #[test]
    fn split_pane_explicit_sizes_for_same_axis_before_and_after() {
        let (state, effects) = reduce_screen(
            two_pane_screen(),
            sized_split("p2", Axis::Horizontal, true, "p3", vec![0.2, 0.3, 0.5]),
        )
        .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::SpawnSession { .. }]));
        let (state, effects) = reduce_screen(
            state,
            sized_split(
                "p3",
                Axis::Horizontal,
                false,
                "p4",
                vec![0.1, 0.2, 0.3, 0.4],
            ),
        )
        .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::SpawnSession { .. }]));
        assert_eq!(state.revision, 2);
        assert!(
            matches!(&state.root, LayoutNode::Split { children, sizes, .. }
            if sizes == &[0.1, 0.2, 0.3, 0.4]
                && children.iter().map(|child| child.collect_pane_ids())
                    .collect::<Vec<_>>() == vec![
                        vec![PaneId::from("p1")],
                        vec![PaneId::from("p3")],
                        vec![PaneId::from("p4")],
                        vec![PaneId::from("p2")],
                    ])
        );
    }

    #[test]
    fn split_pane_explicit_sizes_use_immediate_nested_parent() {
        let (state, _) = reduce_screen(
            two_pane_screen(),
            sized_split("p2", Axis::Vertical, false, "p3", vec![0.7, 0.3]),
        )
        .unwrap();
        let (state, effects) = reduce_screen(
            state,
            sized_split("p2", Axis::Vertical, true, "p4", vec![0.2, 0.5, 0.3]),
        )
        .unwrap();
        assert!(matches!(effects.as_slice(), [Effect::SpawnSession { .. }]));
        let LayoutNode::Split {
            children, sizes, ..
        } = &state.root
        else {
            panic!("expected outer split");
        };
        assert_eq!(sizes, &[0.5, 0.5]);
        assert!(
            matches!(&children[1], LayoutNode::Split { axis: Axis::Vertical, children, sizes, .. }
            if sizes == &[0.2, 0.5, 0.3]
                && children.iter().map(|child| child.collect_pane_ids())
                    .collect::<Vec<_>>() == vec![
                        vec![PaneId::from("p4")],
                        vec![PaneId::from("p2")],
                        vec![PaneId::from("p3")],
                    ])
        );
    }

    #[test]
    fn split_pane_creates_new_pane_and_session() {
        let state = single_pane_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: Some(PaneId::from("p2")),
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };

        let (new_state, effects) = reduce_screen(state, op).unwrap();
        assert_eq!(new_state.panes.len(), 2);
        assert!(new_state.panes.contains_key(&PaneId::from("p2")));
        assert_eq!(new_state.sessions.len(), 2);
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::SpawnSession { .. }));
    }

    #[test]
    fn split_pane_passes_cwd_to_spawn() {
        let state = single_pane_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: Some(PaneId::from("p2")),
            resulting_sizes: None,
            cwd: Some("/home/me/Code".to_string()),
            shell_profile_id: None,
        };

        let (new_state, effects) = reduce_screen(state, op).unwrap();
        match &effects[0] {
            Effect::SpawnSession { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some("/home/me/Code"));
            }
            other => panic!("expected SpawnSession, got {other:?}"),
        }
        let session = new_state
            .panes
            .get(&PaneId::from("p2"))
            .and_then(|p| p.session_id.as_ref())
            .and_then(|id| new_state.sessions.get(id))
            .expect("new session");
        assert_eq!(session.cwd.as_deref(), Some("/home/me/Code"));
    }

    #[test]
    fn split_pane_inherits_profile_legacy_shell_and_known_cwd() {
        let mut state = single_pane_screen();
        let session = state.sessions.get_mut(&SessionId::from("s1")).unwrap();
        session.shell_profile_id = Some("shell-zsh".to_string());
        session.shell = Some("/legacy/zsh".to_string());
        session.cwd = Some("/work/project".to_string());

        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: Some(PaneId::from("p2")),
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };

        let (new_state, effects) = reduce_screen(state, op).unwrap();
        let new_session = new_state
            .sessions
            .values()
            .find(|session| session.id != SessionId::from("s1"))
            .unwrap();
        assert_eq!(new_session.shell_profile_id.as_deref(), Some("shell-zsh"));
        assert_eq!(new_session.shell.as_deref(), Some("/legacy/zsh"));
        assert_eq!(new_session.cwd.as_deref(), Some("/work/project"));
        assert!(matches!(
            &effects[0],
            Effect::SpawnSession {
                shell_profile_id: Some(id), cwd: Some(cwd), ..
            } if id == "shell-zsh" && cwd == "/work/project"
        ));
    }

    #[test]
    fn explicit_split_profile_overrides_inherited_legacy_shell() {
        let mut state = single_pane_screen();
        let session = state.sessions.get_mut(&SessionId::from("s1")).unwrap();
        session.shell_profile_id = Some("shell-bash".to_string());
        session.shell = Some("/legacy/bash".to_string());

        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: Some(PaneId::from("p2")),
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: Some("shell-fish".to_string()),
        };

        let (new_state, _) = reduce_screen(state, op).unwrap();
        let new_session = new_state
            .sessions
            .values()
            .find(|session| session.id != SessionId::from("s1"))
            .unwrap();
        assert_eq!(new_session.shell_profile_id.as_deref(), Some("shell-fish"));
        assert_eq!(new_session.shell, None);
    }

    #[test]
    fn split_pane_wraps_in_split() {
        let state = single_pane_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: None,
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };

        let (new_state, _) = reduce_screen(state, op).unwrap();
        assert!(matches!(new_state.root, LayoutNode::Split { .. }));
        assert_eq!(new_state.root.count_panes(), 2);
    }

    #[test]
    fn split_pane_same_axis_adds_sibling() {
        let state = two_pane_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: Some(1),
            new_pane_id: Some(PaneId::from("p3")),
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };

        let (new_state, _) = reduce_screen(state, op).unwrap();
        // Should add as sibling to existing split, not create nested split
        if let LayoutNode::Split { children, .. } = &new_state.root {
            assert_eq!(children.len(), 3);
        } else {
            panic!("expected split root");
        }
    }

    // ── close_pane tests ───

    #[test]
    fn close_pane_removes_and_kills_session() {
        let state = two_pane_screen();
        let op = ScreenOp::ClosePane {
            pane_id: PaneId::from("p1"),
            resulting_sizes: None,
        };

        let (new_state, effects) = reduce_screen(state, op).unwrap();
        assert_eq!(new_state.panes.len(), 1);
        assert!(!new_state.panes.contains_key(&PaneId::from("p1")));
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::KillSession { .. }));
    }

    #[test]
    fn close_pane_collapses_parent_split() {
        let state = two_pane_screen();
        let op = ScreenOp::ClosePane {
            pane_id: PaneId::from("p1"),
            resulting_sizes: None,
        };

        let (new_state, _) = reduce_screen(state, op).unwrap();
        // After removing one pane, the split should collapse to just a slot
        assert!(matches!(new_state.root, LayoutNode::Slot { .. }));
    }

    /// workspace-model, Closing a Pane (Close the last pane of a screen).
    #[test]
    fn closing_the_last_pane_of_a_screen_removes_the_screen() {
        let close = |screen: &str, pane: &str| Op::ModifyScreen {
            screen_id: ScreenId::from(screen),
            op: ScreenOp::ClosePane {
                pane_id: PaneId::from(pane),
                resulting_sizes: None,
            },
        };
        let store = two_screen_store();
        store.dispatch(split_op("scr1", "p1", "p3")).unwrap();
        assert_eq!(
            store.get_workspace_state().active_screen_id,
            Some(ScreenId::from("scr1"))
        );

        // A screen that keeps a pane stays.
        store.dispatch(close("scr1", "p3")).unwrap();
        assert!(store.get_screen(&ScreenId::from("scr1")).is_some());

        let untouched = store.get_screen(&ScreenId::from("scr2")).unwrap();
        let effects = store.dispatch(close("scr1", "p1")).unwrap();
        assert!(
            matches!(effects.as_slice(), [Effect::KillSession { .. }]),
            "only the closed pane's session ends"
        );
        let state = store.get_workspace_state();
        assert!(!state.screens.contains_key(&ScreenId::from("scr1")));
        assert_eq!(state.screen_order, [ScreenId::from("scr2")]);
        assert_eq!(state.active_screen_id, Some(ScreenId::from("scr2")));
        let other = store.get_screen(&ScreenId::from("scr2")).unwrap();
        assert_eq!(other.revision, untouched.revision);
        assert_eq!(other.panes.len(), 1);

        // The workspace's last screen goes the same way.
        store.dispatch(close("scr2", "p2")).unwrap();
        let state = store.get_workspace_state();
        assert!(state.screens.is_empty());
        assert!(state.screen_order.is_empty());
        assert_eq!(state.active_screen_id, None);

        // A rejected close removes nothing.
        let store = two_screen_store();
        assert!(store.dispatch(close("scr1", "absent")).is_err());
        assert_eq!(store.get_workspace_state().screens.len(), 2);
    }

    /// workspace-model, Server Close On Exit (Unattended exit of the last pane).
    #[test]
    fn a_pane_closed_on_exit_takes_its_emptied_screen_along() {
        let exit = |session: &SessionId| Op::UpdateSessionMetadata {
            session_id: session.clone(),
            display_title: "sh".into(),
            cwd: "/work".into(),
            busy: false,
            exited: true,
            exit_code: Some(0),
            close: true,
        };
        let store = two_screen_store();
        store.dispatch(split_op("scr1", "p1", "p3")).unwrap();
        let session = |pane: &str| session_of(&store.get_workspace_state(), "scr1", pane).unwrap();
        let (first, second) = (session("p1"), session("p3"));

        store.dispatch(exit(&second)).unwrap();
        let screen = store.get_screen(&ScreenId::from("scr1")).unwrap();
        assert_eq!(screen.panes.len(), 1, "the other pane keeps the screen");

        let effects = store.dispatch(exit(&first)).unwrap();
        assert!(matches!(effects.as_slice(), [Effect::KillSession { .. }]));
        let state = store.get_workspace_state();
        assert!(!state.screens.contains_key(&ScreenId::from("scr1")));
        assert_eq!(state.screen_order, [ScreenId::from("scr2")]);
        assert_eq!(state.active_screen_id, Some(ScreenId::from("scr2")));
    }

    // ── resize_split tests ───

    #[test]
    fn resize_split_updates_sizes() {
        let state = two_pane_screen();
        let op = ScreenOp::ResizeSplit {
            node_id: NodeId::from("split1"),
            sizes: vec![0.3, 0.7],
        };

        let (new_state, effects) = reduce_screen(state, op).unwrap();
        if let LayoutNode::Split { sizes, .. } = &new_state.root {
            assert!((sizes[0] - 0.3).abs() < 0.001);
            assert!((sizes[1] - 0.7).abs() < 0.001);
        }
        assert!(effects.is_empty());
    }

    // ── workspace-level ops ───

    #[test]
    fn create_screen() {
        let state = WorkspaceState::default();
        let op = Op::CreateScreen {
            name: Some("test".to_string()),
            screen_id: Some(ScreenId::from("scr1")),
            new_pane_id: Some(PaneId::from("p1")),
            shell_profile_id: None,
        };

        let (new_state, effects) = reduce_workspace(state, op).unwrap();
        assert_eq!(new_state.screens.len(), 1);
        assert!(new_state.screens.contains_key(&ScreenId::from("scr1")));
        assert_eq!(new_state.active_screen_id, Some(ScreenId::from("scr1")));
        assert_eq!(new_state.screen_order, vec![ScreenId::from("scr1")]);
        assert_eq!(effects.len(), 1);
    }

    #[test]
    fn create_screen_records_explicit_default_profile() {
        let (state, effects) = reduce_workspace(
            WorkspaceState::default(),
            Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("scr-profile")),
                new_pane_id: Some(PaneId::from("p-profile")),
                shell_profile_id: Some("shell-fish".to_string()),
            },
        )
        .unwrap();
        let screen = state.screens.get(&ScreenId::from("scr-profile")).unwrap();
        assert_eq!(
            screen
                .sessions
                .values()
                .next()
                .unwrap()
                .shell_profile_id
                .as_deref(),
            Some("shell-fish")
        );
        assert!(matches!(
            &effects[0],
            Effect::SpawnSession { shell_profile_id: Some(id), .. } if id == "shell-fish"
        ));
    }

    #[test]
    fn create_screen_on_existing_id_keeps_name_and_order() {
        let create = |id: &str, name: Option<&str>, pane: &str| Op::CreateScreen {
            name: name.map(str::to_string),
            screen_id: Some(ScreenId::from(id)),
            new_pane_id: Some(PaneId::from(pane)),
            shell_profile_id: None,
        };
        let (state, _) = reduce_workspace(
            WorkspaceState::default(),
            create("scr1", Some("Work"), "p1"),
        )
        .unwrap();
        let (mut state, _) = reduce_workspace(state, create("scr2", None, "p2")).unwrap();

        // A screen without panes, as a workspace saved by an earlier version
        // can hold it: the screen-level op alone leaves the screen in place.
        let screen = state.screens.remove(&ScreenId::from("scr1")).unwrap();
        let close = ScreenOp::ClosePane {
            pane_id: PaneId::from("p1"),
            resulting_sizes: None,
        };
        let (screen, _) = reduce_screen(screen, close).unwrap();
        assert!(screen.panes.is_empty());
        state.screens.insert(ScreenId::from("scr1"), screen);

        // A client re-creates the empty screen around a fresh pane.
        let (state, effects) = reduce_workspace(state, create("scr1", None, "p9")).unwrap();

        assert_eq!(
            state.screen_order,
            vec![ScreenId::from("scr1"), ScreenId::from("scr2")]
        );
        let screen = state.screens.get(&ScreenId::from("scr1")).unwrap();
        assert_eq!(screen.name.as_deref(), Some("Work"));
        assert_eq!(screen.panes.len(), 1);
        assert!(screen.panes.contains_key(&PaneId::from("p9")));
        assert_eq!(effects.len(), 1);
    }

    #[test]
    fn create_screen_does_not_replace_a_screen_with_panes() {
        let store = crate::mux::Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: Some("Work".to_string()),
                screen_id: Some(ScreenId::from("scr1")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .unwrap();
        let before = store.get_workspace_state();

        // What a reloading client used to send for a screen it had not seen a
        // snapshot for yet.
        let result = store.dispatch(Op::CreateScreen {
            name: Some("Default".to_string()),
            screen_id: Some(ScreenId::from("scr1")),
            new_pane_id: Some(PaneId::from("terminal-1")),
            shell_profile_id: None,
        });

        assert!(
            matches!(result, Err(MuxError::ScreenExists(ref id)) if id == &ScreenId::from("scr1"))
        );
        let after = store.get_workspace_state();
        let (b, a) = (
            &before.screens[&ScreenId::from("scr1")],
            &after.screens[&ScreenId::from("scr1")],
        );
        assert_eq!(a.name, b.name);
        assert_eq!(a.revision, b.revision);
        assert_eq!(
            a.panes.keys().collect::<Vec<_>>(),
            b.panes.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            a.sessions.keys().collect::<Vec<_>>(),
            b.sessions.keys().collect::<Vec<_>>()
        );
        assert_eq!(after.screen_order, before.screen_order);
    }

    fn create_op(screen: &str, pane: &str) -> Op {
        Op::CreateScreen {
            name: None,
            screen_id: Some(ScreenId::from(screen)),
            new_pane_id: Some(PaneId::from(pane)),
            shell_profile_id: None,
        }
    }

    fn split_op(screen: &str, pane: &str, new_pane: &str) -> Op {
        Op::ModifyScreen {
            screen_id: ScreenId::from(screen),
            op: ScreenOp::SplitPane {
                pane_id: PaneId::from(pane),
                axis: Axis::Horizontal,
                new_pane_index: None,
                new_pane_id: Some(PaneId::from(new_pane)),
                resulting_sizes: None,
                cwd: None,
                shell_profile_id: None,
            },
        }
    }

    fn move_op(from: &str, pane: &str, to: &str, create: Option<&str>) -> Op {
        Op::MovePaneToScreen {
            screen_id: ScreenId::from(from),
            pane_id: PaneId::from(pane),
            target_screen_id: ScreenId::from(to),
            create_screen: create.is_some(),
            screen_name: create.map(str::to_string),
        }
    }

    fn session_of(state: &WorkspaceState, screen: &str, pane: &str) -> Option<SessionId> {
        state.screens[&ScreenId::from(screen)].panes[&PaneId::from(pane)]
            .session_id
            .clone()
    }

    #[test]
    fn move_to_an_existing_screen_keeps_the_session_and_ends_nothing() {
        let store = two_screen_store();
        store.dispatch(split_op("scr1", "p1", "p3")).unwrap();
        let before = store.get_workspace_state();
        let session = session_of(&before, "scr1", "p3").unwrap();

        let effects = store.dispatch(move_op("scr1", "p3", "scr2", None)).unwrap();

        assert!(effects.is_empty(), "no session is started or ended");
        let after = store.get_workspace_state();
        let (source, target) = (
            &after.screens[&ScreenId::from("scr1")],
            &after.screens[&ScreenId::from("scr2")],
        );
        assert!(!source.panes.contains_key(&PaneId::from("p3")));
        assert!(!source.sessions.contains_key(&session));
        assert!(source.root.find_slot(&PaneId::from("p3")).is_none());
        assert_eq!(session_of(&after, "scr2", "p3"), Some(session.clone()));
        assert!(target.sessions.contains_key(&session));
        match target.root.find_slot(&PaneId::from("p3")) {
            Some(LayoutNode::Slot {
                pane_ids,
                active_pane_id,
                ..
            }) => {
                assert_eq!(pane_ids, &[PaneId::from("p2"), PaneId::from("p3")]);
                assert_eq!(active_pane_id.as_ref(), Some(&PaneId::from("p3")));
            }
            other => panic!("not in the target's first slot: {other:?}"),
        }
        let revision =
            |state: &WorkspaceState, id: &str| state.screens[&ScreenId::from(id)].revision;
        assert_eq!(revision(&after, "scr1"), revision(&before, "scr1") + 1);
        assert_eq!(revision(&after, "scr2"), revision(&before, "scr2") + 1);
    }

    #[test]
    fn move_to_a_new_screen_places_it_after_the_source() {
        let store = two_screen_store();
        store.dispatch(split_op("scr1", "p1", "p3")).unwrap();
        store
            .dispatch(move_op("scr1", "p3", "scr9", Some("Logs")))
            .unwrap();
        let after = store.get_workspace_state();
        assert_eq!(
            after.screen_order,
            vec![
                ScreenId::from("scr1"),
                ScreenId::from("scr9"),
                ScreenId::from("scr2")
            ]
        );
        let created = &after.screens[&ScreenId::from("scr9")];
        assert_eq!(created.name.as_deref(), Some("Logs"));
        assert_eq!(
            created.panes.keys().collect::<Vec<_>>(),
            vec![&PaneId::from("p3")]
        );
        assert!(created.root.find_slot(&PaneId::from("p3")).is_some());
    }

    #[test]
    fn rejected_moves_leave_the_workspace_unchanged() {
        let store = two_screen_store();
        store.dispatch(split_op("scr1", "p1", "p3")).unwrap();
        let before = workspace_summary(&store.get_workspace_state());
        for (op, expected) in [
            (move_op("nope", "p1", "scr2", None), "screen nope not found"),
            (move_op("scr1", "p1", "nope", None), "screen nope not found"),
            (
                move_op("scr1", "p1", "scr2", Some("X")),
                "screen scr2 already exists",
            ),
            (
                move_op("scr1", "p1", "scr1", None),
                "cannot move to the screen it is on",
            ),
            (move_op("scr1", "p2", "scr2", None), "pane p2 not found"),
        ] {
            let error = store.dispatch(op).expect_err(expected).to_string();
            assert!(error.contains(expected), "{error}");
            assert_eq!(workspace_summary(&store.get_workspace_state()), before);
        }
    }

    #[test]
    fn a_job_pane_does_not_move() {
        let mut state = two_screen_store().get_workspace_state();
        let screen = state.screens.get_mut(&ScreenId::from("scr1")).unwrap();
        screen.panes.get_mut(&PaneId::from("p1")).unwrap().job_id = Some(JobId::new());
        let result = reduce_workspace(state, move_op("scr1", "p1", "scr2", None));
        assert!(matches!(result, Err(MuxError::JobPaneMove(_))));
    }

    #[test]
    fn the_last_pane_moved_away_takes_its_screen_along() {
        let store = two_screen_store();
        store
            .dispatch(Op::SetActiveScreen {
                screen_id: ScreenId::from("scr1"),
            })
            .unwrap();
        let session = session_of(&store.get_workspace_state(), "scr1", "p1").unwrap();

        let effects = store.dispatch(move_op("scr1", "p1", "scr2", None)).unwrap();

        assert!(effects.is_empty(), "no session is ended");
        let after = store.get_workspace_state();
        assert!(!after.screens.contains_key(&ScreenId::from("scr1")));
        assert_eq!(after.screen_order, vec![ScreenId::from("scr2")]);
        assert_eq!(after.active_screen_id, Some(ScreenId::from("scr2")));
        assert_eq!(session_of(&after, "scr2", "p1"), Some(session));
    }

    #[test]
    fn a_floating_pane_moves_and_an_empty_grid_takes_it() {
        let store = two_screen_store();
        store.dispatch(split_op("scr1", "p1", "p3")).unwrap();
        store
            .dispatch(Op::ModifyScreen {
                screen_id: ScreenId::from("scr1"),
                op: ScreenOp::FloatPane {
                    pane_id: PaneId::from("p3"),
                    bounds: Some(FloatBounds {
                        x: 0.0,
                        y: 0.0,
                        w: 100.0,
                        h: 100.0,
                    }),
                },
            })
            .unwrap();
        // scr2's grid runs empty, keeping a bare root slot: its other pane floats.
        store.dispatch(split_op("scr2", "p2", "p4")).unwrap();
        for op in [
            ScreenOp::FloatPane {
                pane_id: PaneId::from("p4"),
                bounds: None,
            },
            ScreenOp::ClosePane {
                pane_id: PaneId::from("p2"),
                resulting_sizes: None,
            },
        ] {
            store
                .dispatch(Op::ModifyScreen {
                    screen_id: ScreenId::from("scr2"),
                    op,
                })
                .unwrap();
        }
        let emptied = store.get_workspace_state();
        let grid = &emptied.screens[&ScreenId::from("scr2")];
        assert_eq!(grid.root.count_panes(), 0);
        assert_eq!(grid.floating.len(), 1, "a floating pane keeps its screen");

        store.dispatch(move_op("scr1", "p3", "scr2", None)).unwrap();

        let after = store.get_workspace_state();
        assert!(after.screens[&ScreenId::from("scr1")].floating.is_empty());
        let target = &after.screens[&ScreenId::from("scr2")];
        assert_eq!(target.floating.len(), 1);
        assert!(matches!(
            &target.root,
            LayoutNode::Slot { pane_ids, .. } if pane_ids == &[PaneId::from("p3")]
        ));
        assert_eq!(
            store.screen_of_movable_pane(&PaneId::from("p3")),
            Some(ScreenId::from("scr2"))
        );
    }

    #[test]
    fn invalid_floating_bounds_reject_atomically_for_float_and_move() {
        let tiled = two_pane_screen();
        let (floated, _) = reduce_screen(
            tiled.clone(),
            ScreenOp::FloatPane {
                pane_id: "p2".into(),
                bounds: None,
            },
        )
        .unwrap();
        for component in 0..4 {
            let mut invalid = vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
            if component >= 2 {
                invalid.extend([0.0, -0.0, -1.0]);
            }
            for value in invalid {
                let mut bounds = FloatBounds::default();
                *match component {
                    0 => &mut bounds.x,
                    1 => &mut bounds.y,
                    2 => &mut bounds.w,
                    _ => &mut bounds.h,
                } = value;
                let float = ScreenOp::FloatPane {
                    pane_id: "p2".into(),
                    bounds: Some(bounds.clone()),
                };
                let movement = ScreenOp::MoveFloat {
                    floating_id: floated.floating[0].id.clone(),
                    bounds,
                };
                for (state, op) in [(&tiled, float), (&floated, movement)] {
                    let store = store_with(state.clone());
                    let changes = store.changes();
                    let before = serde_json::to_value(store.get_workspace_state()).unwrap();
                    assert!(matches!(
                        store.dispatch(Op::ModifyScreen {
                            screen_id: state.id.clone(),
                            op
                        }),
                        Err(MuxError::InvalidFloatBounds)
                    ));
                    assert_eq!(
                        serde_json::to_value(store.get_workspace_state()).unwrap(),
                        before
                    );
                    assert_eq!(*changes.borrow(), 0);
                }
            }
        }
    }

    #[test]
    fn floating_bounds_preserve_finite_offscreen_and_oversized_values() {
        let initial = FloatBounds {
            x: -2.0,
            y: 3.0,
            w: 4.0,
            h: 0.01,
        };
        let (state, effects) = reduce_screen(
            two_pane_screen(),
            ScreenOp::FloatPane {
                pane_id: "p2".into(),
                bounds: Some(initial.clone()),
            },
        )
        .unwrap();
        assert!(effects.is_empty());
        assert_eq!(state.floating[0].bounds, initial);
        let moved = FloatBounds {
            x: f64::MAX,
            y: -f64::MAX,
            w: f64::MIN_POSITIVE,
            h: 2.0,
        };
        let op = ScreenOp::MoveFloat {
            floating_id: state.floating[0].id.clone(),
            bounds: moved.clone(),
        };
        let (state, effects) = reduce_screen(state, op).unwrap();
        assert!(effects.is_empty());
        assert_eq!(state.floating[0].bounds, moved);
    }

    fn nested_float_screen() -> ScreenState {
        let mut state = two_pane_screen();
        for number in 3..=6 {
            let mut pane = state.panes[&PaneId::from("p1")].clone();
            let mut session = state.sessions[&SessionId::from("s1")].clone();
            pane.id = format!("p{number}").into();
            session.id = format!("s{number}").into();
            pane.session_id = Some(session.id.clone());
            state.panes.insert(pane.id.clone(), pane);
            state.sessions.insert(session.id.clone(), session);
        }
        state.root = LayoutNode::new_slot("root".into(), vec!["p1".into()]);
        state.floating = vec![FloatingEntry {
            id: "float".into(),
            bounds: FloatBounds::default(),
            root: serde_json::from_value(serde_json::json!({
                "type":"split", "id":"outer", "axis":"horizontal", "sizes":[0.2,0.5,0.3],
                "children":[
                    {"type":"slot", "id":"stack", "paneIds":["p2","p3"], "activePaneId":"p3"},
                    {"type":"split", "id":"inner", "axis":"vertical", "sizes":[0.4,0.6], "children":[
                        {"type":"slot", "id":"four", "paneIds":["p4"]},
                        {"type":"slot", "id":"five", "paneIds":["p5"]}
                    ]},
                    {"type":"slot", "id":"six", "paneIds":["p6"]}
                ]
            })).unwrap(),
        }];
        state
    }

    #[test]
    fn closing_nested_float_preserves_siblings_and_kills_only_the_requested_session() {
        let mut state = nested_float_screen();
        let root_before = serde_json::to_value(&state.root).unwrap();
        for (index, pane) in ["p3", "p6", "p4", "p2", "p5"].into_iter().enumerate() {
            let pane_id = PaneId::from(pane);
            let session = state.panes[&pane_id].session_id.clone().unwrap();
            let (next, effects) = reduce_screen(
                state,
                ScreenOp::ClosePane {
                    pane_id: pane_id.clone(),
                    resulting_sizes: None,
                },
            )
            .unwrap();
            state = next;
            assert!(
                matches!(effects.as_slice(), [Effect::KillSession { session_id }] if session_id == &session)
            );
            assert!(!state.panes.contains_key(&pane_id));
            assert!(!state.sessions.contains_key(&session));
            assert_eq!(state.panes.len(), 5 - index);
            assert_eq!(state.sessions.len(), 5 - index);
            let located: std::collections::HashSet<_> = state.panes_in_layout_order().collect();
            assert_eq!(located, state.panes.keys().cloned().collect());
            assert_eq!(serde_json::to_value(&state.root).unwrap(), root_before);
            if index == 4 {
                assert!(state.floating.is_empty());
                continue;
            }
            assert_eq!(state.floating.len(), 1);
            assert_eq!(state.floating[0].id, FloatingId::from("float"));
            assert_eq!(state.floating[0].bounds, FloatBounds::default());
            let tree = &state.floating[0].root;
            match index {
                0 => assert!(
                    matches!(tree.find_node(&"stack".into()), Some(LayoutNode::Slot { pane_ids, active_pane_id, .. }) if pane_ids == &[PaneId::from("p2")] && active_pane_id.as_ref() == Some(&PaneId::from("p2")))
                ),
                1 => assert!(
                    matches!(tree, LayoutNode::Split { children, sizes, .. } if children.len() == 2 && sizes == &[0.5, 0.5])
                ),
                2 => {
                    assert!(tree.find_node(&"inner".into()).is_none());
                    assert!(tree.find_node(&"five".into()).is_some());
                }
                3 => assert_eq!(tree.id(), &NodeId::from("five")),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn transfer_from_nested_float_keeps_siblings_and_session_without_effects() {
        let source = nested_float_screen();
        let session_before =
            serde_json::to_value(&source.sessions[&SessionId::from("s4")]).unwrap();
        let store = store_with(source);
        let effects = store
            .dispatch(move_op("screen1", "p4", "target", Some("Target")))
            .unwrap();
        assert!(effects.is_empty());
        let state = store.get_workspace_state();
        let source = &state.screens[&ScreenId::from("screen1")];
        assert_eq!(
            source.floating[0].root.collect_pane_ids(),
            vec![PaneId::from("p2"), "p3".into(), "p5".into(), "p6".into()]
        );
        assert!(source.floating[0].root.find_node(&"inner".into()).is_none());
        assert_eq!(source.panes.len(), 5);
        assert_eq!(source.sessions.len(), 5);
        let target = &state.screens[&ScreenId::from("target")];
        assert_eq!(target.root.collect_pane_ids(), vec![PaneId::from("p4")]);
        assert_eq!(
            target.panes[&PaneId::from("p4")].session_id,
            Some("s4".into())
        );
        assert_eq!(
            serde_json::to_value(&target.sessions[&SessionId::from("s4")]).unwrap(),
            session_before
        );
    }

    /// Two screens, `scr1` with `p1` and `scr2` with `p2`.
    fn two_screen_store() -> crate::mux::Store {
        let store = crate::mux::Store::new();
        store.dispatch(create_op("scr1", "p1")).unwrap();
        store.dispatch(create_op("scr2", "p2")).unwrap();
        store
    }

    /// The workspace's screens with their pane IDs, revisions and session IDs.
    fn workspace_summary(state: &WorkspaceState) -> Vec<(String, u64, Vec<String>, Vec<String>)> {
        let mut summary: Vec<_> = state
            .screens
            .values()
            .map(|screen| {
                let mut panes: Vec<String> = screen.panes.keys().map(ToString::to_string).collect();
                let mut sessions: Vec<String> =
                    screen.sessions.keys().map(ToString::to_string).collect();
                panes.sort();
                sessions.sort();
                (screen.id.to_string(), screen.revision, panes, sessions)
            })
            .collect();
        summary.sort();
        summary
    }

    // workspace-model, Identifier Assignment: Pane ID already in use
    #[test]
    fn split_pane_with_a_pane_id_in_use_on_another_screen_is_rejected() {
        let store = two_screen_store();
        let before = store.get_workspace_state();

        let result = store.dispatch(split_op("scr2", "p2", "p1"));

        assert!(matches!(result, Err(MuxError::PaneExists(ref id)) if id == &PaneId::from("p1")));
        let after = store.get_workspace_state();
        assert_eq!(workspace_summary(&after), workspace_summary(&before));
        assert_eq!(after.screen_order, before.screen_order);
    }

    #[test]
    fn split_pane_with_a_pane_id_in_use_on_the_same_screen_is_rejected() {
        let store = two_screen_store();
        let before = store.get_workspace_state();

        let result = store.dispatch(split_op("scr1", "p1", "p1"));

        assert!(matches!(result, Err(MuxError::PaneExists(_))));
        assert_eq!(
            workspace_summary(&store.get_workspace_state()),
            workspace_summary(&before)
        );
    }

    #[test]
    fn pane_ids_of_floating_entries_are_in_use() {
        let store = two_screen_store();
        store.dispatch(split_op("scr1", "p1", "p3")).unwrap();
        store
            .dispatch(Op::ModifyScreen {
                screen_id: ScreenId::from("scr1"),
                op: ScreenOp::FloatPane {
                    pane_id: PaneId::from("p3"),
                    bounds: None,
                },
            })
            .unwrap();
        let before = store.get_workspace_state();
        assert_eq!(before.screens[&ScreenId::from("scr1")].floating.len(), 1);

        assert!(matches!(
            store.dispatch(split_op("scr2", "p2", "p3")),
            Err(MuxError::PaneExists(_))
        ));
        assert!(matches!(
            store.dispatch(create_op("scr3", "p3")),
            Err(MuxError::PaneExists(_))
        ));
        let after = store.get_workspace_state();
        assert_eq!(workspace_summary(&after), workspace_summary(&before));
        assert_eq!(after.screen_order, before.screen_order);
    }

    /// A pane the layout still names counts as in use even when the pane table
    /// lost it, so the new pane cannot land in a slot that already holds it.
    #[test]
    fn pane_ids_named_only_by_a_layout_are_in_use() {
        let (mut state, _) = reduce_workspace(WorkspaceState::default(), create_op("scr1", "p1"))
            .and_then(|(state, _)| reduce_workspace(state, create_op("scr2", "p2")))
            .unwrap();
        if let Some(screen) = state.screens.get_mut(&ScreenId::from("scr1")) {
            screen.panes.clear();
        }

        let result = reduce_workspace(state, split_op("scr2", "p2", "p1"));

        assert!(matches!(result, Err(MuxError::PaneExists(_))));
    }

    #[test]
    fn create_screen_with_a_pane_id_in_use_is_rejected() {
        let store = two_screen_store();
        let before = store.get_workspace_state();

        let result = store.dispatch(create_op("scr3", "p2"));

        assert!(matches!(result, Err(MuxError::PaneExists(ref id)) if id == &PaneId::from("p2")));
        let after = store.get_workspace_state();
        assert!(!after.screens.contains_key(&ScreenId::from("scr3")));
        assert_eq!(workspace_summary(&after), workspace_summary(&before));
        assert_eq!(after.screen_order, before.screen_order);
    }

    #[test]
    fn close_screen() {
        let mut state = WorkspaceState::default();
        let screen_id = ScreenId::from("scr1");
        let mut screen = ScreenState::new(screen_id.clone(), None);
        screen.sessions.insert(
            SessionId::from("s1"),
            SessionEntry {
                display_title: None,
                busy: false,
                id: SessionId::from("s1"),
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
                restore_mode: None,
                command: None,
                cwd: None,
                shell: None,
                status: SessionStatus::Running,
                exit_code: None,
                scratchpad: String::new(),
            },
        );
        state.screens.insert(screen_id.clone(), screen);
        state.screen_order.push(screen_id.clone());
        state.active_screen_id = Some(screen_id.clone());

        let op = Op::CloseScreen {
            screen_id: screen_id.clone(),
        };
        let (new_state, effects) = reduce_workspace(state, op).unwrap();
        assert!(new_state.screens.is_empty());
        assert!(new_state.screen_order.is_empty());
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::KillSession { .. }));
    }

    #[test]
    fn rename_screen() {
        let mut state = WorkspaceState::default();
        let screen_id = ScreenId::from("scr1");
        let screen = ScreenState::new(screen_id.clone(), None);
        state.screens.insert(screen_id.clone(), screen);

        let op = Op::RenameScreen {
            screen_id: screen_id.clone(),
            name: "New Name".to_string(),
        };
        let (new_state, _) = reduce_workspace(state, op).unwrap();
        assert_eq!(
            new_state.screens[&screen_id].name,
            Some("New Name".to_string())
        );
    }

    // ── revision tracking ───

    #[test]
    fn revision_increments() {
        let state = single_pane_screen();
        assert_eq!(state.revision, 0);

        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: None,
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };

        let (new_state, _) = reduce_screen(state, op).unwrap();
        assert_eq!(new_state.revision, 1);
    }

    fn scratchpad_op(screen: &str, session: &SessionId, text: &str) -> Op {
        Op::ModifyScreen {
            screen_id: ScreenId::from(screen),
            op: ScreenOp::SetScratchpad {
                session_id: session.clone(),
                text: text.into(),
            },
        }
    }

    fn scratchpad(state: &WorkspaceState, screen: &str, session: &SessionId) -> String {
        state.screens[&ScreenId::from(screen)].sessions[session]
            .scratchpad
            .clone()
    }

    /// session-scratchpad, Scratchpad Per Session and Scratchpad Lifetime: set
    /// and cleared, refused over the limit or for an unknown session, empty in
    /// a split's new session, along with its pane to another screen, gone with
    /// its closed pane.
    #[test]
    fn a_scratchpad_belongs_to_its_session() {
        let store = two_screen_store();
        let state = store.get_workspace_state();
        let session = session_of(&state, "scr1", "p1").expect("session");
        let revision = state.screens[&ScreenId::from("scr1")].revision;

        store
            .dispatch(scratchpad_op("scr1", &session, "ask about the tests"))
            .expect("set");
        let state = store.get_workspace_state();
        assert_eq!(scratchpad(&state, "scr1", &session), "ask about the tests");
        assert_eq!(
            state.screens[&ScreenId::from("scr1")].revision,
            revision + 1
        );

        let limit = crate::mux::SCRATCHPAD_LIMIT;
        assert!(matches!(
            store.dispatch(scratchpad_op("scr1", &session, &"x".repeat(limit + 1))),
            Err(MuxError::ScratchpadTooLong { .. })
        ));
        assert!(matches!(
            store.dispatch(scratchpad_op("scr1", &SessionId::from("absent"), "x")),
            Err(MuxError::SessionNotFound(_))
        ));
        assert_eq!(
            scratchpad(&store.get_workspace_state(), "scr1", &session),
            "ask about the tests",
            "a refused text changes nothing"
        );
        store
            .dispatch(scratchpad_op("scr1", &session, &"x".repeat(limit)))
            .expect("the limit itself fits");
        store
            .dispatch(scratchpad_op("scr1", &session, "ask about the tests"))
            .expect("set");

        store.dispatch(split_op("scr1", "p1", "p3")).expect("split");
        let state = store.get_workspace_state();
        let split = session_of(&state, "scr1", "p3").expect("new session");
        assert_eq!(
            scratchpad(&state, "scr1", &split),
            "",
            "nothing is inherited"
        );
        assert_eq!(scratchpad(&state, "scr1", &session), "ask about the tests");

        store
            .dispatch(move_op("scr1", "p1", "scr2", None))
            .expect("move");
        assert_eq!(
            scratchpad(&store.get_workspace_state(), "scr2", &session),
            "ask about the tests"
        );

        store
            .dispatch(scratchpad_op("scr2", &session, ""))
            .expect("clear");
        let record =
            &store.get_workspace_state().screens[&ScreenId::from("scr2")].sessions[&session];
        let wire = serde_json::to_value(record).expect("record");
        assert!(wire.get("scratchpad").is_none(), "{wire}");

        store
            .dispatch(scratchpad_op("scr2", &session, "gone with the pane"))
            .expect("set");
        store
            .dispatch(Op::ModifyScreen {
                screen_id: ScreenId::from("scr2"),
                op: ScreenOp::ClosePane {
                    pane_id: PaneId::from("p1"),
                    resulting_sizes: None,
                },
            })
            .expect("close");
        assert!(!store
            .get_workspace_state()
            .screens
            .values()
            .any(|screen| screen.sessions.contains_key(&session)));
    }

    #[test]
    fn set_scratchpad_has_its_wire_name() {
        let op: ScreenOp = serde_json::from_value(serde_json::json!({
            "type": "set_scratchpad", "sessionId": "s1", "text": "hi"
        }))
        .expect("wire op");
        assert!(matches!(
            op,
            ScreenOp::SetScratchpad { ref session_id, ref text }
                if session_id == &SessionId::from("s1") && text == "hi"
        ));
    }

    // ── arrange_panes and move_pane (workspace-model, Arranging the Panes of a
    // Screen and Moving a Pane) ───

    /// `p1` above a row of `p2` and `p3`, with `p4` floating.
    fn three_pane_screen() -> ScreenState {
        let mut state = two_pane_screen();
        for id in ["p3", "p4"] {
            let mut pane = state.panes[&PaneId::from("p1")].clone();
            pane.id = PaneId::from(id);
            pane.session_id = Some(SessionId::from(format!("s{id}")));
            state.panes.insert(pane.id.clone(), pane);
            let mut session = state.sessions[&SessionId::from("s1")].clone();
            session.id = SessionId::from(format!("s{id}"));
            state.sessions.insert(session.id.clone(), session);
        }
        state.root = LayoutNode::new_split(
            NodeId::from("column"),
            Axis::Vertical,
            vec![
                LayoutNode::new_slot(NodeId::from("slot1"), vec![PaneId::from("p1")]),
                LayoutNode::new_split(
                    NodeId::from("row"),
                    Axis::Horizontal,
                    vec![
                        LayoutNode::new_slot(NodeId::from("slot2"), vec![PaneId::from("p2")]),
                        LayoutNode::new_slot(NodeId::from("slot3"), vec![PaneId::from("p3")]),
                    ],
                    vec![0.5, 0.5],
                ),
            ],
            vec![0.5, 0.5],
        );
        state.floating = vec![FloatingEntry {
            id: FloatingId::from("float1"),
            root: LayoutNode::new_slot(NodeId::from("slot4"), vec![PaneId::from("p4")]),
            bounds: FloatBounds::default(),
        }];
        state
    }

    fn arrange(root: serde_json::Value) -> ScreenOp {
        // Exercise the public wire format as well as the reducer.
        serde_json::from_value(serde_json::json!({"type": "arrange_panes", "root": root}))
            .expect("wire op")
    }

    /// The layout as `(axis or "slot", panes, active pane, sizes)` per node, depth first.
    fn shape(node: &LayoutNode) -> Vec<String> {
        let mut out = Vec::new();
        node.walk(&mut |node| {
            out.push(match node {
                LayoutNode::Slot {
                    pane_ids,
                    active_pane_id,
                    ..
                } => format!("slot {pane_ids:?} {active_pane_id:?}"),
                LayoutNode::Split { axis, sizes, .. } => format!("{axis:?} {sizes:?}"),
            });
            true
        });
        out
    }

    fn node_ids(node: &LayoutNode) -> Vec<NodeId> {
        let mut ids = Vec::new();
        node.walk(&mut |node| {
            ids.push(node.id().clone());
            true
        });
        ids
    }

    #[test]
    fn arranged_panes_replace_the_layout_and_touch_no_session() {
        let state = three_pane_screen();
        let old_ids = node_ids(&state.root);
        let op = arrange(serde_json::json!({
            "type": "split", "axis": "vertical", "sizes": [0.7, 0.3], "children": [
                {"type": "slot", "paneIds": ["p1", "p3"], "activePaneId": "p3"},
                {"type": "slot", "paneIds": ["p2"]},
            ],
        }));
        let (next, effects) = reduce_screen(state.clone(), op).expect("arranged");
        assert!(effects.is_empty());
        assert_eq!(next.revision, state.revision + 1);
        assert_eq!(
            shape(&next.root),
            [
                "Vertical [0.7, 0.3]",
                r#"slot [PaneId("p1"), PaneId("p3")] Some(PaneId("p3"))"#,
                r#"slot [PaneId("p2")] None"#,
            ]
        );
        assert!(node_ids(&next.root).iter().all(|id| !old_ids.contains(id)));
        assert_eq!(
            serde_json::to_value(&next.panes).unwrap(),
            serde_json::to_value(&state.panes).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&next.sessions).unwrap(),
            serde_json::to_value(&state.sessions).unwrap()
        );
        assert_eq!(next.floating.len(), 1);
    }

    #[test]
    fn arrangements_that_lose_repeat_or_float_a_pane_are_rejected() {
        let slot = |panes: &[&str]| serde_json::json!({"type": "slot", "paneIds": panes});
        let row = |children: Vec<serde_json::Value>, sizes: &[f64]| serde_json::json!({"type": "split", "axis": "horizontal", "children": children, "sizes": sizes});
        let cases = [
            (
                row(vec![slot(&["p1"]), slot(&["p2"])], &[0.5, 0.5]),
                "misses pane p3",
            ),
            (
                row(vec![slot(&["p1", "p2"]), slot(&["p3", "p1"])], &[0.5, 0.5]),
                "more than once",
            ),
            (
                row(vec![slot(&["p1", "p2", "p3"]), slot(&["p4"])], &[0.5, 0.5]),
                "not found in layout",
            ),
            (
                row(
                    vec![slot(&["p1"]), slot(&["p2"]), slot(&["p3"])],
                    &[0.5, 0.5],
                ),
                "expected 3 got 2",
            ),
            (
                row(vec![slot(&["p1", "p2", "p3"]), slot(&[])], &[0.5, 0.5]),
                "holds no pane",
            ),
            (
                row(vec![slot(&["p1", "p2", "p3"])], &[1.0]),
                "fewer than two children",
            ),
            (
                serde_json::json!({"type": "slot", "paneIds": ["p1", "p2", "p3"], "activePaneId": "p4"}),
                "not in its slot",
            ),
            (
                serde_json::json!({"type": "slot", "paneIds": []}),
                "holds no pane",
            ),
        ];
        for (root, expected) in cases {
            let error = reduce_screen(three_pane_screen(), arrange(root.clone()))
                .expect_err(&root.to_string())
                .to_string();
            assert!(error.contains(expected), "{root}: {error}");
        }
    }

    #[test]
    fn an_arrangement_leaves_out_panes_the_screen_does_not_hold() {
        let op = arrange(serde_json::json!({
            "type": "split", "axis": "horizontal", "sizes": [0.3, 0.3, 0.4], "children": [
                {"type": "slot", "paneIds": ["p1", "ghost"], "activePaneId": "ghost"},
                {"type": "split", "axis": "vertical", "sizes": [0.5, 0.5], "children": [
                    {"type": "slot", "paneIds": ["p2"]},
                    {"type": "slot", "paneIds": ["p3"]},
                ]},
                {"type": "slot", "paneIds": ["other-ghost"]},
            ],
        }));
        let (next, _) = reduce_screen(three_pane_screen(), op).expect("arranged");
        assert_eq!(
            shape(&next.root),
            [
                "Horizontal [0.5, 0.5]",
                r#"slot [PaneId("p1")] None"#,
                "Vertical [0.5, 0.5]",
                r#"slot [PaneId("p2")] None"#,
                r#"slot [PaneId("p3")] None"#,
            ]
        );
    }

    #[test]
    fn an_empty_grid_is_arranged_only_when_every_pane_floats() {
        let mut state = three_pane_screen();
        state.root = LayoutNode::new_slot(NodeId::from("empty"), vec![]);
        for id in ["p1", "p2", "p3"] {
            state.panes.remove(&PaneId::from(id));
        }
        let (next, _) = reduce_screen(
            state,
            arrange(serde_json::json!({"type": "slot", "paneIds": []})),
        )
        .expect("empty grid");
        assert_eq!(shape(&next.root), ["slot [] None"]);
        assert_eq!(next.floating.len(), 1);
    }

    fn select_tab(pane: &str) -> ScreenOp {
        serde_json::from_value(serde_json::json!({"type": "select_tab", "paneId": pane}))
            .expect("wire op")
    }

    /// `[p1, p2]` in the grid with `p2` active, `[p3, p4]` floating with `p4` active.
    fn tabbed_screen() -> ScreenState {
        let mut state = three_pane_screen();
        let stack = |id: &str, panes: [&str; 2]| match LayoutNode::new_slot(
            NodeId::from(id),
            panes.iter().map(|&p| PaneId::from(p)).collect(),
        ) {
            LayoutNode::Slot { id, pane_ids, .. } => LayoutNode::Slot {
                id,
                active_pane_id: Some(PaneId::from(panes[1])),
                pane_ids,
            },
            split => split,
        };
        state.root = stack("stack", ["p1", "p2"]);
        state.floating[0].root = stack("float-stack", ["p3", "p4"]);
        state.floating[0].bounds = FloatBounds {
            x: 0.1,
            y: 0.2,
            w: 0.3,
            h: 0.4,
        };
        state
    }

    #[test]
    fn a_selected_tab_becomes_its_slots_active_pane_and_nothing_else_changes() {
        let state = tabbed_screen();
        let (next, effects) = reduce_screen(state.clone(), select_tab("p1")).expect("selected");
        assert!(effects.is_empty());
        assert_eq!(next.revision, state.revision + 1);
        assert_eq!(
            shape(&next.root),
            [r#"slot [PaneId("p1"), PaneId("p2")] Some(PaneId("p1"))"#]
        );
        assert_eq!(node_ids(&next.root), node_ids(&state.root));
        assert_eq!(
            serde_json::to_value(&next.floating).unwrap(),
            serde_json::to_value(&state.floating).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&next.sessions).unwrap(),
            serde_json::to_value(&state.sessions).unwrap()
        );

        // A floating group's tab, its bounds kept.
        let (next, _) = reduce_screen(next, select_tab("p3")).expect("selected");
        assert_eq!(
            shape(&next.floating[0].root),
            [r#"slot [PaneId("p3"), PaneId("p4")] Some(PaneId("p3"))"#]
        );
        assert_eq!(
            node_ids(&next.floating[0].root),
            ["float-stack"].map(NodeId::from)
        );
        assert_eq!(
            serde_json::to_value(&next.floating[0].bounds).unwrap(),
            serde_json::to_value(&state.floating[0].bounds).unwrap()
        );
        assert_eq!(
            shape(&next.root),
            [r#"slot [PaneId("p1"), PaneId("p2")] Some(PaneId("p1"))"#]
        );
    }

    #[test]
    fn a_tab_the_screen_does_not_hold_is_not_selected() {
        let state = tabbed_screen();
        let err = reduce_screen(state, select_tab("p9")).expect_err("rejected");
        assert!(matches!(err, MuxError::PaneNotFound(_)), "{err}");

        let store = crate::mux::Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("scr")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .expect("screen");
        let before = serde_json::to_value(store.get_screen(&ScreenId::from("scr"))).unwrap();
        store
            .dispatch_screen(&ScreenId::from("scr"), select_tab("p9"))
            .expect_err("rejected");
        let after = serde_json::to_value(store.get_screen(&ScreenId::from("scr"))).unwrap();
        assert_eq!(after, before);
    }

    fn move_pane(pane: &str, target: serde_json::Value) -> ScreenOp {
        let mut op = serde_json::json!({"type": "move_pane", "paneId": pane});
        op.as_object_mut()
            .unwrap()
            .extend(target.as_object().unwrap().clone());
        serde_json::from_value(op).expect("wire op")
    }

    fn move_tab(pane: &str, source: &str, target: &str, before: Option<&str>) -> ScreenOp {
        let mut wire = serde_json::json!({
            "type":"move_tab", "paneId":pane, "sourceSlotId":source, "targetSlotId":target
        });
        if let Some(before) = before {
            wire["beforePaneId"] = serde_json::json!(before);
        }
        let op: ScreenOp = serde_json::from_value(wire.clone()).expect("exact placement wire");
        assert_eq!(
            serde_json::to_value(&op).expect("serialized placement"),
            wire
        );
        op
    }

    #[test]
    fn move_tab_reorders_exactly_without_changing_selection_or_records() {
        let mut state = three_pane_screen();
        state.root = LayoutNode::new_slot(
            "stack".into(),
            ["p1", "p2", "p3"].map(PaneId::from).to_vec(),
        );
        for active in [None, Some(PaneId::from("p2")), Some(PaneId::from("p3"))] {
            let mut state = state.clone();
            let LayoutNode::Slot { active_pane_id, .. } = &mut state.root else {
                unreachable!()
            };
            *active_pane_id = active.clone();
            let selected = active.unwrap_or_else(|| PaneId::from("p1"));
            for (pane, before, expected) in [
                ("p3", Some("p1"), ["p3", "p1", "p2"]),
                ("p3", Some("p2"), ["p1", "p3", "p2"]),
                ("p1", None, ["p2", "p3", "p1"]),
                ("p1", Some("p2"), ["p1", "p2", "p3"]),
            ] {
                let (next, effects) =
                    reduce_screen(state.clone(), move_tab(pane, "stack", "stack", before))
                        .expect("reordered");
                assert!(effects.is_empty());
                assert_eq!(next.revision, state.revision + 1);
                assert_eq!(next.root.collect_pane_ids(), expected.map(PaneId::from));
                assert_eq!(node_ids(&next.root), node_ids(&state.root));
                let LayoutNode::Slot {
                    pane_ids,
                    active_pane_id,
                    ..
                } = &next.root
                else {
                    unreachable!()
                };
                assert_eq!(
                    active_pane_id.as_ref().or(pane_ids.first()),
                    Some(&selected)
                );
                let mut actual = serde_json::to_value(&next).expect("state");
                let mut original = serde_json::to_value(&state).expect("state");
                actual["root"] = serde_json::Value::Null;
                original["root"] = serde_json::Value::Null;
                actual["revision"] = original["revision"].clone();
                assert_eq!(actual, original, "records and floating groups survive");
            }
        }
        let single = single_pane_screen();
        let slot = single.root.id().to_string();
        let (next, effects) =
            reduce_screen(single.clone(), move_tab("p1", &slot, &slot, None)).expect("single tab");
        assert!(effects.is_empty());
        assert_eq!(shape(&next.root), shape(&single.root));
    }

    #[test]
    fn move_tab_collapses_source_using_current_weights_and_exact_destination() {
        let mut state = three_pane_screen();
        state.floating.clear();
        state.name = Some("Concurrent rename".into());
        state.root = LayoutNode::new_split(
            "row".into(),
            Axis::Horizontal,
            vec![
                LayoutNode::new_slot("slot1".into(), vec!["p1".into()]),
                LayoutNode::new_slot("slot2".into(), vec!["p2".into(), "p4".into()]),
                LayoutNode::new_slot("slot3".into(), vec!["p3".into()]),
            ],
            vec![0.2, 0.3, 0.5],
        );
        for (before, expected) in [
            (Some("p4"), vec!["p2", "p1", "p4"]),
            (None, vec!["p2", "p4", "p1"]),
        ] {
            let (next, effects) =
                reduce_screen(state.clone(), move_tab("p1", "slot1", "slot2", before))
                    .expect("placed");
            assert!(effects.is_empty());
            assert_eq!(next.name, state.name);
            assert_eq!(
                serde_json::to_value(&next.panes).expect("panes"),
                serde_json::to_value(&state.panes).expect("panes")
            );
            assert_eq!(
                serde_json::to_value(&next.sessions).expect("sessions"),
                serde_json::to_value(&state.sessions).expect("sessions")
            );
            assert_eq!(
                node_ids(&next.root),
                ["row", "slot2", "slot3"].map(NodeId::from)
            );
            let LayoutNode::Split { sizes, .. } = &next.root else {
                unreachable!()
            };
            assert!((sizes[0] - 0.375).abs() < 1e-12);
            assert!((sizes[1] - 0.625).abs() < 1e-12);
            let LayoutNode::Slot {
                pane_ids,
                active_pane_id,
                ..
            } = next.root.find_node(&"slot2".into()).expect("slot")
            else {
                unreachable!()
            };
            assert_eq!(
                *pane_ids,
                expected.into_iter().map(PaneId::from).collect::<Vec<_>>()
            );
            assert_eq!(*active_pane_id, Some("p1".into()));
        }
        // Nested two-child source collapse leaves the outer proportions untouched.
        let nested = three_pane_screen();
        let (next, _) = reduce_screen(nested, move_tab("p2", "slot2", "slot1", Some("p1")))
            .expect("nested collapse");
        assert_eq!(
            node_ids(&next.root),
            ["column", "slot1", "slot3"].map(NodeId::from)
        );
        let LayoutNode::Split { sizes, .. } = next.root else {
            unreachable!()
        };
        assert_eq!(sizes, [0.5, 0.5]);
        // Zero-width survivors have no defined relative ratio: equal is valid.
        let LayoutNode::Split { sizes, .. } = &mut state.root else {
            unreachable!()
        };
        *sizes = vec![1.0, 0.0, 0.0];
        let (next, _) =
            reduce_screen(state, move_tab("p1", "slot1", "slot2", None)).expect("zero survivors");
        let LayoutNode::Split { sizes, .. } = next.root else {
            unreachable!()
        };
        assert_eq!(sizes, [0.5, 0.5]);
    }

    #[test]
    fn invalid_move_tab_leaves_all_workspace_state_and_revisions_unchanged() {
        let store = store_with(three_pane_screen());
        let before = serde_json::to_value(store.get_workspace_state()).expect("state");
        let changes = store.changes();
        for (pane, source, target, anchor) in [
            ("missing", "slot1", "slot2", None),
            ("p1", "slot2", "slot3", None),
            ("p1", "slot1", "missing", None),
            ("p1", "slot1", "row", None),
            ("p1", "slot1", "slot2", Some("missing")),
            ("p1", "slot1", "slot2", Some("p3")),
            ("p1", "slot1", "slot1", Some("p1")),
            ("p4", "slot4", "slot1", None),
            ("p1", "slot1", "slot4", Some("p4")),
        ] {
            assert!(store
                .dispatch_screen(&"screen1".into(), move_tab(pane, source, target, anchor))
                .is_err());
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("state"),
                before
            );
            assert!(!changes.has_changed().expect("watch retained"));
        }
    }

    #[test]
    fn move_tab_from_nonempty_source_preserves_slots_and_legacy_move_still_activates() {
        let mut state = three_pane_screen();
        state.floating.clear();
        state.root = LayoutNode::new_split(
            "root".into(),
            Axis::Horizontal,
            vec![
                LayoutNode::Slot {
                    id: "a".into(),
                    pane_ids: vec!["p1".into(), "p2".into()],
                    active_pane_id: Some("p1".into()),
                },
                LayoutNode::new_slot("b".into(), vec!["p3".into(), "p4".into()]),
            ],
            vec![0.7, 0.3],
        );
        let (next, _) =
            reduce_screen(state.clone(), move_tab("p1", "a", "b", Some("p3"))).expect("moved");
        assert_eq!(node_ids(&next.root), node_ids(&state.root));
        let LayoutNode::Split { sizes, .. } = &next.root else {
            unreachable!()
        };
        assert_eq!(*sizes, [0.7, 0.3]);
        let LayoutNode::Slot { active_pane_id, .. } =
            next.root.find_node(&"a".into()).expect("source")
        else {
            unreachable!()
        };
        assert_eq!(*active_pane_id, Some("p2".into()));
        let (legacy, _) = reduce_screen(
            tabbed_screen(),
            move_pane("p1", serde_json::json!({"targetSlotId":"stack"})),
        )
        .expect("legacy");
        let LayoutNode::Slot {
            pane_ids,
            active_pane_id,
            ..
        } = legacy.root
        else {
            unreachable!()
        };
        assert_eq!(pane_ids, ["p2", "p1"].map(PaneId::from));
        assert_eq!(active_pane_id, Some("p1".into()));
    }

    #[test]
    fn a_pane_moves_into_a_stack_or_beside_a_pane() {
        let (next, effects) = reduce_screen(
            three_pane_screen(),
            move_pane("p2", serde_json::json!({"targetSlotId": "slot1"})),
        )
        .expect("into the stack");
        assert!(effects.is_empty());
        assert_eq!(
            shape(&next.root),
            [
                "Vertical [0.5, 0.5]",
                r#"slot [PaneId("p1"), PaneId("p2")] Some(PaneId("p2"))"#,
                r#"slot [PaneId("p3")] None"#,
            ]
        );

        let (next, _) = reduce_screen(
            three_pane_screen(),
            move_pane(
                "p3",
                serde_json::json!({"direction": "left", "targetSplitId": "p1"}),
            ),
        )
        .expect("left of p1");
        assert_eq!(
            shape(&next.root),
            [
                "Vertical [0.5, 0.5]",
                "Horizontal [0.5, 0.5]",
                r#"slot [PaneId("p3")] None"#,
                r#"slot [PaneId("p1")] None"#,
                r#"slot [PaneId("p2")] None"#,
            ]
        );
    }

    #[test]
    fn moves_without_a_reachable_target_are_rejected_instead_of_losing_the_pane() {
        let cases = [
            ("p2", serde_json::json!({}), "is required"),
            (
                "p2",
                serde_json::json!({"targetSlotId": "nowhere"}),
                "node nowhere not found",
            ),
            (
                "p2",
                serde_json::json!({"targetSlotId": "row"}),
                "not a slot",
            ),
            (
                "p2",
                serde_json::json!({"direction": "left"}),
                "is required",
            ),
            (
                "p2",
                serde_json::json!({"direction": "left", "targetSplitId": "nowhere"}),
                "not found",
            ),
            // Its own slot, left empty by the move.
            (
                "p2",
                serde_json::json!({"targetSlotId": "slot2"}),
                "is gone",
            ),
            (
                "p2",
                serde_json::json!({"direction": "below", "targetSplitId": "p2"}),
                "is gone",
            ),
            // The row collapses once p2 leaves it.
            (
                "p2",
                serde_json::json!({"direction": "left", "targetSplitId": "row"}),
                "is gone",
            ),
            (
                "p4",
                serde_json::json!({"targetSlotId": "slot1"}),
                "not found in layout",
            ),
        ];
        for (pane, target, expected) in cases {
            let error = reduce_screen(three_pane_screen(), move_pane(pane, target.clone()))
                .expect_err(&target.to_string())
                .to_string();
            assert!(error.contains(expected), "{pane} {target}: {error}");
        }
        // The only pane of the grid used to be duplicated.
        let error = reduce_screen(
            single_pane_screen(),
            move_pane("p1", serde_json::json!({"targetSlotId": "slot1"})),
        )
        .expect_err("only pane");
        assert!(error.to_string().contains("is gone"), "{error}");
    }

    #[test]
    fn a_rejected_arrangement_or_move_leaves_the_store_unchanged() {
        let store = crate::mux::Store::new();
        store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("scr")),
                new_pane_id: Some(PaneId::from("p1")),
                shell_profile_id: None,
            })
            .expect("screen");
        let before = serde_json::to_value(store.get_screen(&ScreenId::from("scr"))).unwrap();
        for op in [
            arrange(serde_json::json!({"type": "slot", "paneIds": ["p1", "p1"]})),
            move_pane("p1", serde_json::json!({"targetSlotId": "nowhere"})),
        ] {
            store
                .dispatch_screen(&ScreenId::from("scr"), op)
                .expect_err("rejected");
        }
        let after = serde_json::to_value(store.get_screen(&ScreenId::from("scr"))).unwrap();
        assert_eq!(after, before);
    }

    // ── open_viewer (viewer-panes) ───

    const PLAN: &str = "/home/me/app/PLAN.md";

    fn open_viewer(fields: serde_json::Value) -> ScreenOp {
        // Exercise the public wire format as well as the reducer.
        let mut op = serde_json::json!({"type": "open_viewer"});
        if let (Some(op), Some(fields)) = (op.as_object_mut(), fields.as_object()) {
            op.extend(fields.clone());
        }
        serde_json::from_value(op).expect("wire op")
    }

    fn file(path: &str) -> serde_json::Value {
        serde_json::json!({"kind": "file", "path": path})
    }

    fn viewer_of(state: &ScreenState, pane: &str) -> ViewerState {
        state.panes[&PaneId::from(pane)]
            .viewer
            .clone()
            .expect("viewer record")
    }

    /// A workspace of the one screen, as a store.
    fn store_with(screen: ScreenState) -> crate::mux::Store {
        let mut state = WorkspaceState::default();
        state.screen_order.push(screen.id.clone());
        state.screens.insert(screen.id.clone(), screen);
        crate::mux::Store::with_state(state)
    }

    /// viewer-panes, Viewer Panes (A file document) and Opening A Viewer (To
    /// the right of a terminal); workspace-model, Workspace Structure (A
    /// viewer pane next to a terminal).
    #[test]
    fn a_file_opens_to_the_right_of_a_terminal_without_a_session() {
        let state = single_pane_screen();
        let (next, effects) = reduce_screen(
            state.clone(),
            open_viewer(
                serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
            ),
        )
        .expect("opened");
        assert!(effects.is_empty());
        assert_eq!(next.revision, state.revision + 1);
        assert_eq!(
            shape(&next.root),
            [
                "Horizontal [0.5, 0.5]",
                r#"slot [PaneId("p1")] None"#,
                r#"slot [PaneId("v1")] None"#,
            ]
        );
        let pane = &next.panes[&PaneId::from("v1")];
        assert_eq!(pane.role, Some(PaneRole::Viewer));
        assert_eq!(pane.title.as_deref(), Some("PLAN.md"));
        assert_eq!(pane.cwd.as_deref(), Some("/home/me/app"));
        assert_eq!(pane.session_id, None);
        assert_eq!(
            pane.viewer,
            Some(ViewerState::new(
                ViewerSource::File { path: PLAN.into() },
                Some(PaneId::from("p1")),
            ))
        );
        assert_eq!(
            next.sessions.keys().collect::<Vec<_>>(),
            [&SessionId::from("s1")],
            "the terminal's session only"
        );
        let wire = serde_json::to_value(pane).expect("pane");
        assert_eq!(
            wire["viewer"],
            serde_json::json!({
                "source": {"kind": "file", "path": PLAN},
                "originPaneId": "p1",
                "revision": 1,
            })
        );
        assert!(wire.get("sessionId").is_none(), "{wire}");
    }

    /// viewer-panes, Opening A Viewer (A second document of the same
    /// terminal): through the CLI, a further document of a terminal becomes a
    /// tab of the group of that terminal's first viewer; a window's op, or a
    /// document of another terminal, still splits.
    #[test]
    fn a_second_document_of_the_same_terminal_joins_its_viewer_group() {
        let (state, _) = reduce_screen(
            single_pane_screen(),
            open_viewer(serde_json::json!({"paneId": "p1", "source": file(PLAN)})),
        )
        .expect("first");
        let first = state
            .root
            .collect_pane_ids()
            .into_iter()
            .find(|id| id != &PaneId::from("p1"))
            .expect("viewer");
        let (state, _) = reduce_screen(
            state,
            open_viewer(
                serde_json::json!({"paneId": "p1", "source": file("/home/me/app/notes.md")}),
            ),
        )
        .expect("second");
        let ids = state.root.collect_pane_ids();
        assert_eq!(ids.len(), 3, "{ids:?}");
        let second = ids[2].clone();
        assert_eq!(
            shape(&state.root),
            [
                "Horizontal [0.5, 0.5]".to_string(),
                r#"slot [PaneId("p1")] None"#.to_string(),
                format!("slot [{first:?}, {second:?}] Some({second:?})"),
            ]
        );

        // A window places its own viewer, so its op splits as asked.
        let (state, _) = reduce_screen(
            state,
            open_viewer(serde_json::json!({
                "paneId": "p1", "newPaneId": "v9", "source": file("/home/me/app/other.md"),
            })),
        )
        .expect("window");
        assert!(
            matches!(
                state.root.find_slot(&PaneId::from("v9")),
                Some(LayoutNode::Slot { pane_ids, .. }) if pane_ids.len() == 1
            ),
            "a slot of its own"
        );
    }

    /// viewer-panes, Viewer Panes (A text document) and Document Text Outside
    /// Snapshots: the text stays in the record and leaves the projection.
    #[test]
    fn a_text_document_keeps_its_text_out_of_the_projection() {
        let (next, effects) = reduce_screen(
            single_pane_screen(),
            open_viewer(serde_json::json!({
                "paneId": "p1", "newPaneId": "v1", "cwd": "/home/me/app",
                "source": {"kind": "text", "title": "Selection from claude", "text": "## Plan\n1. tests"},
            })),
        )
        .expect("opened");
        assert!(effects.is_empty());
        let pane = &next.panes[&PaneId::from("v1")];
        assert_eq!(pane.title.as_deref(), Some("Selection from claude"));
        assert_eq!(pane.cwd.as_deref(), Some("/home/me/app"));
        assert_eq!(
            viewer_of(&next, "v1").source,
            ViewerSource::Text {
                title: "Selection from claude".into(),
                text: "## Plan\n1. tests".into(),
                format: None,
            }
        );

        let shown = next.clone().without_viewer_texts();
        let wire = serde_json::to_value(&shown.panes[&PaneId::from("v1")]).expect("pane");
        assert_eq!(
            wire["viewer"],
            serde_json::json!({
                "source": {"kind": "text", "title": "Selection from claude"},
                "originPaneId": "p1",
                "revision": 1,
            })
        );
        let mut workspace = WorkspaceState::default();
        workspace.screens.insert(next.id.clone(), next.clone());
        let wire = serde_json::to_string(&workspace.without_viewer_texts()).expect("workspace");
        assert!(!wire.contains("1. tests"), "{wire}");
        assert_eq!(
            serde_json::to_value(&next.panes[&PaneId::from("v1")]).expect("pane")["viewer"]
                ["source"]["text"],
            "## Plan\n1. tests",
            "the record itself keeps the text"
        );
    }

    /// viewer-panes, Opening A Viewer: `left` and `above` before the anchor,
    /// a sibling in a parent split of the same axis, a new split otherwise.
    #[test]
    fn directional_placements_follow_split_pane() {
        let cases = [
            (
                "p2",
                "left",
                vec![
                    "Vertical [0.5, 0.5]",
                    r#"slot [PaneId("p1")] None"#,
                    "Horizontal [0.3333333333333333, 0.3333333333333333, 0.3333333333333333]",
                    r#"slot [PaneId("v1")] None"#,
                    r#"slot [PaneId("p2")] None"#,
                    r#"slot [PaneId("p3")] None"#,
                ],
            ),
            (
                "p1",
                "above",
                vec![
                    "Vertical [0.3333333333333333, 0.3333333333333333, 0.3333333333333333]",
                    r#"slot [PaneId("v1")] None"#,
                    r#"slot [PaneId("p1")] None"#,
                    "Horizontal [0.5, 0.5]",
                    r#"slot [PaneId("p2")] None"#,
                    r#"slot [PaneId("p3")] None"#,
                ],
            ),
            (
                "p3",
                "below",
                vec![
                    "Vertical [0.5, 0.5]",
                    r#"slot [PaneId("p1")] None"#,
                    "Horizontal [0.5, 0.5]",
                    r#"slot [PaneId("p2")] None"#,
                    "Vertical [0.5, 0.5]",
                    r#"slot [PaneId("p3")] None"#,
                    r#"slot [PaneId("v1")] None"#,
                ],
            ),
            (
                "p1",
                "right",
                vec![
                    "Vertical [0.5, 0.5]",
                    "Horizontal [0.5, 0.5]",
                    r#"slot [PaneId("p1")] None"#,
                    r#"slot [PaneId("v1")] None"#,
                    "Horizontal [0.5, 0.5]",
                    r#"slot [PaneId("p2")] None"#,
                    r#"slot [PaneId("p3")] None"#,
                ],
            ),
        ];
        for (anchor, placement, expected) in cases {
            let (next, effects) = reduce_screen(
                three_pane_screen(),
                open_viewer(serde_json::json!({
                    "paneId": anchor, "placement": placement, "newPaneId": "v1", "source": file(PLAN),
                })),
            )
            .expect(placement);
            assert!(effects.is_empty());
            assert_eq!(shape(&next.root), expected, "{placement} of {anchor}");
            assert_eq!(next.floating.len(), 1, "the floating entry stays");
        }
    }

    /// viewer-panes, Opening A Viewer (As a tab of a floating group).
    #[test]
    fn a_tab_of_a_floating_group_becomes_its_active_pane() {
        let state = three_pane_screen();
        let (next, effects) = reduce_screen(
            state.clone(),
            open_viewer(serde_json::json!({
                "paneId": "p4", "placement": "tab", "newPaneId": "v1", "source": file(PLAN),
            })),
        )
        .expect("tab");
        assert!(effects.is_empty());
        assert_eq!(
            shape(&next.floating[0].root),
            [r#"slot [PaneId("p4"), PaneId("v1")] Some(PaneId("v1"))"#]
        );
        assert_eq!(next.floating[0].id, FloatingId::from("float1"));
        assert_eq!(
            serde_json::to_value(&next.floating[0].bounds).expect("bounds"),
            serde_json::to_value(&state.floating[0].bounds).expect("bounds")
        );
        assert_eq!(shape(&next.root), shape(&state.root));
        assert_eq!(
            viewer_of(&next, "v1").origin_pane_id,
            Some(PaneId::from("p4"))
        );

        // A tab of a root slot, after the other tabs.
        let (next, _) = reduce_screen(
            tabbed_screen(),
            open_viewer(serde_json::json!({
                "paneId": "p1", "placement": "tab", "newPaneId": "v1", "source": file(PLAN),
            })),
        )
        .expect("tab");
        assert_eq!(
            shape(&next.root),
            [r#"slot [PaneId("p1"), PaneId("p2"), PaneId("v1")] Some(PaneId("v1"))"#]
        );
    }

    /// viewer-panes, Opening A Viewer (Directional placement in a floating
    /// group): a floating entry holds no splits, so each placement beside its
    /// pane adds a tab, also the default `right` that `splitty open` sends.
    #[test]
    fn a_directional_placement_in_a_floating_group_becomes_a_tab() {
        let mut state = three_pane_screen();
        state.floating[0].bounds = FloatBounds {
            x: 0.1,
            y: 0.2,
            w: 0.3,
            h: 0.4,
        };
        let bounds = serde_json::to_value(&state.floating[0].bounds).expect("bounds");
        for placement in ["below", "above", "left", "right"] {
            let (next, effects) = reduce_screen(
                state.clone(),
                open_viewer(serde_json::json!({
                    "paneId": "p4", "placement": placement, "newPaneId": "v1", "source": file(PLAN),
                })),
            )
            .expect(placement);
            assert!(effects.is_empty());
            assert_eq!(
                shape(&next.floating[0].root),
                [r#"slot [PaneId("p4"), PaneId("v1")] Some(PaneId("v1"))"#],
                "{placement}"
            );
            assert_eq!(next.floating[0].id, FloatingId::from("float1"));
            assert_eq!(
                serde_json::to_value(&next.floating[0].bounds).expect("bounds"),
                bounds
            );
            assert_eq!(shape(&next.root), shape(&state.root));
            assert_eq!(
                viewer_of(&next, "v1").origin_pane_id,
                Some(PaneId::from("p4"))
            );
        }

        // `splitty open` from the floating pane, twice: one tab, shown again.
        let store = store_with(state);
        let screen_id = ScreenId::from("screen1");
        for _ in 0..2 {
            store
                .dispatch_screen(
                    &screen_id,
                    open_viewer(serde_json::json!({
                        "paneId": "p4", "placement": "right", "source": file(PLAN),
                    })),
                )
                .expect("from a floating pane");
        }
        let screen = store.get_screen(&screen_id).expect("screen");
        let LayoutNode::Slot {
            pane_ids,
            active_pane_id,
            ..
        } = &screen.floating[0].root
        else {
            panic!("a floating slot");
        };
        assert_eq!(pane_ids.len(), 2);
        assert_eq!(active_pane_id.as_ref(), pane_ids.last());
        assert_eq!(viewer_of(&screen, pane_ids[1].as_str()).revision, 2);
    }

    /// viewer-panes, Opening A Viewer (Without an anchor).
    #[test]
    fn without_an_anchor_a_viewer_wraps_the_root_or_fills_an_empty_one() {
        let (next, effects) = reduce_screen(
            single_pane_screen(),
            open_viewer(serde_json::json!({"newPaneId": "v1", "source": file(PLAN)})),
        )
        .expect("opened");
        assert!(effects.is_empty());
        assert_eq!(
            shape(&next.root),
            [
                "Horizontal [0.5, 0.5]",
                r#"slot [PaneId("p1")] None"#,
                r#"slot [PaneId("v1")] None"#,
            ]
        );
        assert_eq!(viewer_of(&next, "v1").origin_pane_id, None);

        let (next, _) = reduce_screen(
            three_pane_screen(),
            open_viewer(serde_json::json!({"newPaneId": "v1", "source": file(PLAN)})),
        )
        .expect("opened");
        assert_eq!(
            shape(&next.root)[..2],
            ["Horizontal [0.5, 0.5]", "Vertical [0.5, 0.5]"]
        );
        assert_eq!(
            shape(&next.root).last().map(String::as_str),
            Some(r#"slot [PaneId("v1")] None"#)
        );

        let mut empty = single_pane_screen();
        empty.root = LayoutNode::new_slot(NodeId::from("root"), vec![]);
        empty.panes.clear();
        empty.sessions.clear();
        let (next, _) = reduce_screen(
            empty,
            open_viewer(
                serde_json::json!({"placement": "below", "newPaneId": "v1", "source": file(PLAN)}),
            ),
        )
        .expect("opened");
        assert_eq!(
            shape(&next.root),
            [r#"slot [PaneId("v1")] Some(PaneId("v1"))"#]
        );
        assert_eq!(next.root.id(), &NodeId::from("root"));
    }

    /// viewer-panes, Viewer Panes: the origin is a terminal anchor, a viewer
    /// anchor's own origin, or none.
    #[test]
    fn the_origin_is_the_terminal_a_viewer_was_opened_from() {
        let (state, _) = reduce_screen(
            single_pane_screen(),
            open_viewer(
                serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
            ),
        )
        .expect("first");
        let (state, _) = reduce_screen(
            state,
            open_viewer(serde_json::json!({
                "paneId": "v1", "placement": "tab", "newPaneId": "v2",
                "source": file("/home/me/app/docs/design.md"),
            })),
        )
        .expect("from the viewer");
        assert_eq!(
            viewer_of(&state, "v2").origin_pane_id,
            Some(PaneId::from("p1"))
        );
        let (state, _) = reduce_screen(
            state,
            open_viewer(
                serde_json::json!({"newPaneId": "v3", "source": file("/home/me/notes.txt")}),
            ),
        )
        .expect("without an anchor");
        let (mut state, _) = reduce_screen(
            state,
            open_viewer(serde_json::json!({
                "paneId": "v3", "newPaneId": "v4", "source": file("/home/me/todo.txt"),
            })),
        )
        .expect("from an unanchored viewer");
        assert_eq!(viewer_of(&state, "v4").origin_pane_id, None);

        if let Some(pane) = state.panes.get_mut(&PaneId::from("p1")) {
            pane.role = None;
        }
        let (state, _) = reduce_screen(
            state,
            open_viewer(serde_json::json!({
                "paneId": "p1", "newPaneId": "v5", "source": file("/home/me/a.txt"),
            })),
        )
        .expect("from a pane of an older record");
        assert_eq!(
            viewer_of(&state, "v5").origin_pane_id,
            Some(PaneId::from("p1"))
        );
    }

    /// viewer-panes, Opening A Viewer (A relative path, A text over the limit,
    /// A path with a dot segment) and the other limits; workspace-model,
    /// Identifier Assignment: each rejected atomically.
    #[test]
    fn invalid_viewers_are_rejected_and_leave_the_workspace_unchanged() {
        let long_path = format!("/{}", "a".repeat(crate::mux::VIEWER_PATH_LIMIT));
        let long_text = "x".repeat(crate::mux::VIEWER_TEXT_LIMIT + 1);
        let long_title = "t".repeat(crate::mux::VIEWER_TITLE_LIMIT + 1);
        let text = |title: &str, text: &str| serde_json::json!({"kind": "text", "title": title, "text": text});
        let cases = [
            (
                serde_json::json!({"paneId": "p1", "source": file("docs/plan.md")}),
                "absolute",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": file("/home/me/a\u{7}b")}),
                "absolute",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": file(&long_path)}),
                "absolute",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": file("/home/me/app/../secrets.txt")}),
                "absolute",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": file("//server/share/PLAN.md")}),
                "absolute",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": file("/home/me/app/PLAN\u{202e}dm.txt")}),
                "absolute",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": text("Selection", &long_text)}),
                "at most 262144 bytes",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": text("", "x")}),
                "title",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": text(&long_title, "x")}),
                "title",
            ),
            (
                serde_json::json!({"paneId": "p1", "source": text("a\nb", "x")}),
                "title",
            ),
            (
                serde_json::json!({"paneId": "p1", "cwd": "app", "source": text("Selection", "x")}),
                "cwd",
            ),
            (
                serde_json::json!({"paneId": "p9", "source": file(PLAN)}),
                "pane p9 not found",
            ),
            (
                serde_json::json!({"paneId": "p1", "newPaneId": "p2", "source": file(PLAN)}),
                "already exists",
            ),
        ];
        let store = store_with(three_pane_screen());
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        for (fields, expected) in cases {
            let error = store
                .dispatch_screen(&ScreenId::from("screen1"), open_viewer(fields.clone()))
                .expect_err(&fields.to_string())
                .to_string();
            assert!(error.contains(expected), "{fields}: {error}");
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("workspace"),
                before
            );
        }

        // A pane ID in use on another screen.
        let store = two_screen_store();
        let before = store.get_workspace_state();
        let result = store.dispatch_screen(
            &ScreenId::from("scr2"),
            open_viewer(
                serde_json::json!({"paneId": "p2", "newPaneId": "p1", "source": file(PLAN)}),
            ),
        );
        assert!(matches!(result, Err(MuxError::PaneExists(ref id)) if id == &PaneId::from("p1")));
        assert_eq!(
            workspace_summary(&store.get_workspace_state()),
            workspace_summary(&before)
        );

        // An unknown kind of document is not an op at all.
        assert!(serde_json::from_value::<ScreenOp>(serde_json::json!({
            "type": "open_viewer", "source": {"kind": "url", "url": "https://example.com"},
        }))
        .is_err());
    }

    #[test]
    fn viewer_limits_admit_their_bounds() {
        let store = store_with(three_pane_screen());
        let at_limit = format!("/{}", "a".repeat(crate::mux::VIEWER_PATH_LIMIT - 1));
        let title = "ä".repeat(crate::mux::VIEWER_TITLE_LIMIT);
        for (id, source) in [
            ("v1", file(&at_limit)),
            ("v3", file("/home/me/.config/app/..notes/plan.")),
            (
                "v2",
                serde_json::json!({
                    "kind": "text", "title": title, "text": "x".repeat(crate::mux::VIEWER_TEXT_LIMIT),
                }),
            ),
        ] {
            store
                .dispatch_screen(
                    &ScreenId::from("screen1"),
                    open_viewer(
                        serde_json::json!({"paneId": "p1", "newPaneId": id, "source": source}),
                    ),
                )
                .expect(id);
        }
    }

    /// workspace-model, Identifier Assignment (Client-supplied viewer pane ID)
    /// and Layout Operations (Viewer from both transports): both transports
    /// hand the same wire op to the store, which places the same pane and
    /// spawns nothing.
    #[test]
    fn the_same_viewer_op_gives_the_same_pane_without_a_session() {
        let wire = serde_json::json!({
            "type": "open_viewer", "paneId": "p2", "placement": "below",
            "newPaneId": "viewer-0123456789abcdef", "source": file(PLAN),
        });
        let workspace = two_screen_store().get_workspace_state();
        let results: Vec<_> = (0..2)
            .map(|_| {
                let store = crate::mux::Store::with_state(workspace.clone());
                let op = serde_json::from_value::<ScreenOp>(wire.clone()).expect("wire op");
                let effects = store
                    .dispatch_screen(&ScreenId::from("scr2"), op)
                    .expect("opened");
                assert!(effects.is_empty());
                let screen = store.get_screen(&ScreenId::from("scr2")).expect("screen");
                assert_eq!(screen.sessions.len(), 1);
                (
                    shape(&screen.root),
                    serde_json::to_value(&screen.panes).expect("panes"),
                )
            })
            .collect();
        assert_eq!(results[0], results[1]);
        assert_eq!(
            results[0].1["viewer-0123456789abcdef"]["role"], "viewer",
            "{}",
            results[0].1
        );
        let op: ScreenOp = serde_json::from_value(wire.clone()).expect("wire op");
        assert_eq!(serde_json::to_value(&op).expect("op"), wire);
    }

    /// viewer-panes, Reopening A File (An agent opens its plan again, Another
    /// terminal opens the plan).
    #[test]
    fn a_file_opened_again_selects_its_viewer_raises_its_revision_and_takes_the_new_origin() {
        let store = two_screen_store();
        let scr1 = ScreenId::from("scr1");
        for op in [
            open_viewer(
                serde_json::json!({"paneId": "p1", "placement": "tab", "source": file(PLAN)}),
            ),
            open_viewer(
                serde_json::json!({"paneId": "p1", "placement": "tab", "source": file(PLAN)}),
            ),
            new_tab("p1", "p3"),
        ] {
            store.dispatch_screen(&scr1, op).expect("applied");
        }
        let before = store.get_screen(&scr1).expect("screen");
        let viewers: Vec<_> = before
            .panes
            .values()
            .filter(|pane| pane.viewer.is_some())
            .collect();
        assert_eq!(viewers.len(), 1, "the second open showed the same pane");
        let v1 = viewers[0].id.clone();
        assert_eq!(viewer_of(&before, v1.as_str()).revision, 2);
        // The form a window gives its viewer panes, which the CLI tells from
        // a path.
        assert!(
            v1.as_str()
                .strip_prefix("viewer-")
                .is_some_and(|hex| hex.len() == 16
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            "{v1}"
        );
        assert!(
            matches!(&before.root, LayoutNode::Slot { active_pane_id, .. }
            if active_pane_id == &Some(PaneId::from("p3")))
        );

        let effects = store
            .dispatch_screen(
                &scr1,
                open_viewer(serde_json::json!({"paneId": "p3", "source": file(PLAN)})),
            )
            .expect("again");
        assert!(effects.is_empty());
        let after = store.get_screen(&scr1).expect("screen");
        assert_eq!(after.panes.len(), before.panes.len());
        assert_eq!(viewer_of(&after, v1.as_str()).revision, 3);
        assert_eq!(after.revision, before.revision + 1);
        assert!(
            matches!(&after.root, LayoutNode::Slot { pane_ids, active_pane_id, .. }
            if active_pane_id.as_ref() == Some(&v1) && pane_ids.len() == 3)
        );
        assert_eq!(shown_viewer(&before, &after), Some((v1.clone(), true)));

        // The second terminal asked last: it becomes the origin.
        assert_eq!(
            viewer_of(&before, v1.as_str()).origin_pane_id,
            Some(PaneId::from("p1"))
        );
        assert_eq!(
            viewer_of(&after, v1.as_str()).origin_pane_id,
            Some(PaneId::from("p3"))
        );

        // Without an anchor, or beside a pane that yields no origin, the
        // origin stays.
        store
            .dispatch_screen(
                &scr1,
                open_viewer(serde_json::json!({
                    "newPaneId": "t1", "source": {"kind": "text", "title": "Notes", "text": "x"},
                })),
            )
            .expect("a text without an origin");
        for (fields, case) in [
            (
                serde_json::json!({"source": file(PLAN)}),
                "without an anchor",
            ),
            (
                serde_json::json!({"paneId": "t1", "source": file(PLAN)}),
                "beside a viewer without an origin",
            ),
        ] {
            store
                .dispatch_screen(&scr1, open_viewer(fields))
                .expect(case);
        }
        let last = store.get_screen(&scr1).expect("screen");
        assert_eq!(viewer_of(&last, v1.as_str()).revision, 5);
        assert_eq!(
            viewer_of(&last, v1.as_str()).origin_pane_id,
            Some(PaneId::from("p3"))
        );

        // Another screen does not share its viewer panes.
        store
            .dispatch_screen(
                &ScreenId::from("scr2"),
                open_viewer(serde_json::json!({"paneId": "p2", "source": file(PLAN)})),
            )
            .expect("other screen");
        let scr2 = store.get_screen(&ScreenId::from("scr2")).expect("screen");
        assert_eq!(scr2.panes.len(), 2);
    }

    /// viewer-panes, Reopening A File (A window opens the same file): with
    /// `newPaneId` a pane is added in any case; a text is never reused.
    #[test]
    fn a_window_opens_the_same_file_in_a_new_pane() {
        let (state, _) = reduce_screen(
            single_pane_screen(),
            open_viewer(
                serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
            ),
        )
        .expect("first");
        let (next, effects) = reduce_screen(
            state.clone(),
            open_viewer(serde_json::json!({
                "paneId": "p1", "newPaneId": "viewer-0123456789abcdef", "source": file(PLAN),
            })),
        )
        .expect("second");
        assert!(effects.is_empty());
        assert_eq!(
            viewer_of(&next, "viewer-0123456789abcdef").source,
            ViewerSource::File { path: PLAN.into() }
        );
        assert_eq!(viewer_of(&next, "v1").revision, 1);
        assert_eq!(
            shown_viewer(&state, &next),
            Some((PaneId::from("viewer-0123456789abcdef"), false))
        );

        let text = serde_json::json!({"kind": "text", "title": "Selection", "text": "x"});
        let (state, _) = reduce_screen(
            next,
            open_viewer(serde_json::json!({"paneId": "p1", "source": text})),
        )
        .expect("text");
        let (state, _) = reduce_screen(
            state,
            open_viewer(serde_json::json!({"paneId": "p1", "source": text})),
        )
        .expect("same text");
        assert_eq!(
            state.panes.values().filter(|p| p.viewer.is_some()).count(),
            4
        );
    }

    /// viewer-panes, Viewer Pane Lifetime (Origin closed); workspace-model,
    /// Closing a Pane (Close a viewer pane).
    #[test]
    fn closing_the_origin_keeps_the_viewer_and_closing_the_viewer_ends_nothing() {
        let store = two_screen_store();
        let scr1 = ScreenId::from("scr1");
        store
            .dispatch_screen(
                &scr1,
                open_viewer(
                    serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
                ),
            )
            .expect("opened");
        let record = viewer_of(&store.get_screen(&scr1).expect("screen"), "v1");

        let effects = store
            .dispatch_screen(
                &scr1,
                ScreenOp::ClosePane {
                    pane_id: PaneId::from("p1"),
                    resulting_sizes: None,
                },
            )
            .expect("origin closed");
        assert!(matches!(effects.as_slice(), [Effect::KillSession { .. }]));
        let screen = store.get_screen(&scr1).expect("screen");
        assert_eq!(viewer_of(&screen, "v1"), record);
        assert_eq!(record.origin_pane_id, Some(PaneId::from("p1")));
        assert_eq!(shape(&screen.root), [r#"slot [PaneId("v1")] None"#]);

        let effects = store
            .dispatch_screen(
                &scr1,
                ScreenOp::ClosePane {
                    pane_id: PaneId::from("v1"),
                    resulting_sizes: None,
                },
            )
            .expect("viewer closed");
        assert!(effects.is_empty(), "no session to end");
        // The viewer was the screen's last pane: the screen goes with it.
        assert!(store.get_screen(&scr1).is_none());
        assert_eq!(
            store.get_workspace_state().screen_order,
            [ScreenId::from("scr2")]
        );
    }

    /// viewer-panes, Viewer Pane Lifetime (Moved to another screen),
    /// viewer-annotations, Annotation Lifetime, and the layout ops of every
    /// other pane: the record goes along unchanged, annotations included.
    #[test]
    fn a_viewer_keeps_its_record_through_moves_and_layout_ops() {
        let store = two_screen_store();
        let scr1 = ScreenId::from("scr1");
        store
            .dispatch_screen(
                &scr1,
                open_viewer(serde_json::json!({
                    "paneId": "p1", "newPaneId": "v1", "cwd": "/home/me/app",
                    "source": {"kind": "text", "title": "Selection from claude", "text": "answer"},
                })),
            )
            .expect("opened");
        store
            .dispatch_screen(
                &scr1,
                put(
                    "v1",
                    vec![serde_json::json!({
                        "id": ann(1), "kind": "comment", "body": "Why?",
                        "target": passage(serde_json::json!({"quote": "answer", "startLine": 1, "endLine": 1})),
                    })],
                ),
            )
            .expect("annotated");
        store.dispatch(split_op("scr1", "p1", "p3")).expect("split");
        let pane = |store: &crate::mux::Store, screen: &str| {
            serde_json::to_value(
                &store.get_workspace_state().screens[&ScreenId::from(screen)].panes
                    [&PaneId::from("v1")],
            )
            .expect("pane")
        };
        let record = pane(&store, "scr1");
        assert_eq!(record["viewer"]["annotations"][0]["id"], ann(1));
        let sessions = |store: &crate::mux::Store| {
            workspace_summary(&store.get_workspace_state())
                .into_iter()
                .map(|(_, _, _, sessions)| sessions)
                .collect::<Vec<_>>()
        };
        let before = sessions(&store);

        let floating = || {
            store
                .get_screen(&ScreenId::from("scr1"))
                .expect("screen")
                .floating[0]
                .id
                .clone()
        };
        let ops: Vec<Box<dyn Fn() -> ScreenOp>> = vec![
            Box::new(|| select_tab("v1")),
            Box::new(|| move_pane("v1", serde_json::json!({"targetSlotId": "slot-missing"}))),
            Box::new(|| {
                move_pane(
                    "v1",
                    serde_json::json!({"direction": "below", "targetSplitId": "p1"}),
                )
            }),
            Box::new(|| ScreenOp::FloatPane {
                pane_id: PaneId::from("v1"),
                bounds: None,
            }),
            Box::new(|| ScreenOp::DockPane {
                floating_id: floating(),
                pane_id: PaneId::from("p1"),
            }),
            Box::new(|| {
                arrange(serde_json::json!({
                    "type": "split", "axis": "horizontal", "sizes": [0.5, 0.5], "children": [
                        {"type": "slot", "paneIds": ["v1", "p1"]},
                        {"type": "slot", "paneIds": ["p3"]},
                    ],
                }))
            }),
        ];
        for op in ops {
            let op = op();
            let name = format!("{op:?}");
            match store.dispatch_screen(&scr1, op) {
                Ok(effects) => assert!(effects.is_empty(), "{name}"),
                // The move into a missing slot is the one rejected op.
                Err(error) => assert!(name.contains("slot-missing"), "{name}: {error}"),
            }
            assert_eq!(pane(&store, "scr1"), record, "{name}");
        }

        let effects = store
            .dispatch(move_op("scr1", "v1", "scr2", None))
            .expect("moved");
        assert!(effects.is_empty());
        assert_eq!(pane(&store, "scr2"), record);
        assert_eq!(sessions(&store), before, "no session started or ended");
        assert!(!store.get_workspace_state().screens[&scr1]
            .panes
            .contains_key(&PaneId::from("v1")));
    }

    /// workspace-model, Split Session Inheritance (Split of a viewer pane): a
    /// split or a tab of a viewer starts in its directory with the default
    /// profile, not with the profile of the terminal it came from.
    #[test]
    fn a_split_or_tab_of_a_viewer_starts_in_its_directory_with_the_default_profile() {
        let mut state = single_pane_screen();
        if let Some(session) = state.sessions.get_mut(&SessionId::from("s1")) {
            session.shell_profile_id = Some("shell-zsh".into());
            session.shell = Some("/legacy/zsh".into());
            session.cwd = Some("/elsewhere".into());
        }
        let (state, _) = reduce_screen(
            state,
            open_viewer(
                serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
            ),
        )
        .expect("opened");
        for op in [
            ScreenOp::SplitPane {
                pane_id: PaneId::from("v1"),
                axis: Axis::Vertical,
                new_pane_index: None,
                new_pane_id: Some(PaneId::from("p2")),
                resulting_sizes: None,
                cwd: None,
                shell_profile_id: None,
            },
            new_tab("v1", "p2"),
        ] {
            let (next, effects) = reduce_screen(state.clone(), op).expect("applied");
            let [Effect::SpawnSession {
                pane_id,
                cwd,
                shell_profile_id,
                shell,
                ..
            }] = effects.as_slice()
            else {
                panic!("one spawn: {effects:?}");
            };
            assert_eq!(pane_id, &PaneId::from("p2"));
            assert_eq!(cwd.as_deref(), Some("/home/me/app"));
            assert_eq!(shell_profile_id, &None, "the default profile");
            assert_eq!(shell, &None);
            let pane = &next.panes[&PaneId::from("p2")];
            assert_eq!(pane.role, Some(PaneRole::Terminal));
            assert_eq!(pane.viewer, None);
            let session = &next.sessions[pane.session_id.as_ref().expect("session")];
            assert_eq!(session.cwd.as_deref(), Some("/home/me/app"));
        }
    }

    #[test]
    fn file_names_and_directories_of_absolute_paths() {
        for (path, name, dir) in [
            ("/home/me/app/PLAN.md", "PLAN.md", "/home/me/app"),
            ("/PLAN.md", "PLAN.md", "/"),
            ("/home/me/app/", "app", "/home/me"),
            ("/", "/", "/"),
        ] {
            assert_eq!(file_name(path), name, "{path}");
            assert_eq!(parent_dir(path), dir, "{path}");
        }
    }

    // ── annotations and their threads (viewer-annotations) ───

    /// Screen `screen1` with the terminal `p1` and the viewer `v1` of PLAN.md.
    fn viewer_screen() -> ScreenState {
        reduce_screen(
            single_pane_screen(),
            open_viewer(
                serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
            ),
        )
        .expect("viewer")
        .0
    }

    /// An annotation ID of the form windows give them.
    fn ann(n: u32) -> String {
        format!("ann-{n:016x}")
    }

    /// A reply ID of the form windows and the server give them.
    fn rep(n: u32) -> String {
        format!("rep-{n:016x}")
    }

    /// A comment that asks a question about a passage of line 30.
    fn question(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "kind": "comment", "body": "Why not five times?",
            "target": {
                "view": "source", "quote": "retry three times", "prefix": "On failure, ",
                "suffix": " before giving up", "startLine": 30, "endLine": 30,
            },
        })
    }

    fn comment(id: &str, body: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "kind": "comment", "body": body})
    }

    /// A suggested deletion of a passage: a change with an empty replacement.
    fn deletion(n: u32, target: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"id": ann(n), "kind": "change", "replacement": "", "target": target})
    }

    /// A passage of line 3 with `fields` in place of the defaults.
    fn passage(fields: serde_json::Value) -> serde_json::Value {
        let mut target = serde_json::json!({
            "view": "source", "quote": "retry", "prefix": "", "suffix": "",
            "startLine": 3, "endLine": 3,
        });
        if let (Some(target), Some(fields)) = (target.as_object_mut(), fields.as_object()) {
            target.extend(fields.clone());
        }
        target
    }

    fn put(pane: &str, annotations: Vec<serde_json::Value>) -> ScreenOp {
        serde_json::from_value(serde_json::json!({
            "type": "put_annotations", "paneId": pane, "annotations": annotations,
        }))
        .expect("wire op")
    }

    fn remove(pane: &str, ids: &[String]) -> ScreenOp {
        serde_json::from_value(serde_json::json!({
            "type": "remove_annotations", "paneId": pane, "ids": ids,
        }))
        .expect("wire op")
    }

    /// The user's replies `replies` to annotation `annotation` of `pane`.
    fn put_replies(pane: &str, annotation: &str, replies: Vec<serde_json::Value>) -> ScreenOp {
        serde_json::from_value(serde_json::json!({
            "type": "put_replies", "paneId": pane, "annotationId": annotation, "replies": replies,
        }))
        .expect("wire op")
    }

    fn user_reply(id: &str, body: &str) -> serde_json::Value {
        serde_json::json!({"id": id, "author": "user", "body": body})
    }

    fn remove_replies(pane: &str, annotation: &str, ids: &[String]) -> ScreenOp {
        serde_json::from_value(serde_json::json!({
            "type": "remove_replies", "paneId": pane, "annotationId": annotation, "ids": ids,
        }))
        .expect("wire op")
    }

    /// `mark_sent` of `entries`, each an entry as a review held it.
    fn mark_sent(pane: &str, entries: Vec<serde_json::Value>) -> ScreenOp {
        serde_json::from_value(serde_json::json!({
            "type": "mark_sent", "paneId": pane, "entries": entries,
        }))
        .expect("wire op")
    }

    /// `mark_sent` of entries that name only their IDs.
    fn mark_ids(pane: &str, ids: &[String]) -> ScreenOp {
        mark_sent(
            pane,
            ids.iter().map(|id| serde_json::json!({"id": id})).collect(),
        )
    }

    /// The entry of `mark_sent` for an annotation or a reply as a put
    /// carried it: its ID with its kind, remark or text, replacement and
    /// passage.
    fn entry_of(put: &serde_json::Value) -> serde_json::Value {
        let mut entry = serde_json::json!({"id": put["id"]});
        for field in ["kind", "body", "replacement", "target"] {
            if let Some(value) = put.get(field) {
                entry[field] = value.clone();
            }
        }
        entry
    }

    /// The trusted op of an agent in `p1`, titled `claude`, that replies
    /// `body` to annotation `number` of `v1` on `screen1`.
    fn agent_reply(number: u32, id: &str, body: &str) -> Op {
        Op::AgentReply {
            screen_id: ScreenId::from("screen1"),
            pane_id: PaneId::from("v1"),
            number,
            reply: AnnotationReply {
                id: id.into(),
                author: ReplyAuthor::Agent,
                body: body.into(),
                sent: false,
                from: Some(ReplyFrom {
                    pane_id: PaneId::from("p1"),
                    title: "claude".into(),
                }),
            },
        }
    }

    fn annotation_ids(state: &ScreenState, pane: &str) -> Vec<String> {
        viewer_of(state, pane)
            .annotations
            .into_iter()
            .map(|annotation| annotation.id)
            .collect()
    }

    fn numbers(state: &ScreenState, pane: &str) -> Vec<u32> {
        viewer_of(state, pane)
            .annotations
            .into_iter()
            .map(|annotation| annotation.number)
            .collect()
    }

    /// The annotation `id` of `v1` on `screen1` of `store`.
    fn annotation_of(store: &crate::mux::Store, id: &str) -> ViewerAnnotation {
        let screen = store
            .get_screen(&ScreenId::from("screen1"))
            .expect("screen");
        viewer_of(&screen, "v1")
            .annotations
            .into_iter()
            .find(|annotation| annotation.id == id)
            .expect("annotation")
    }

    /// `store` with viewer `v1` on `screen1`, holding the handed-over
    /// comments #1 and #2.
    fn sent_store() -> crate::mux::Store {
        let store = store_with(viewer_screen());
        let screen_id = ScreenId::from("screen1");
        for op in [
            put(
                "v1",
                vec![question(&ann(1)), comment(&ann(2), "Shorter steps.")],
            ),
            mark_sent(
                "v1",
                vec![
                    entry_of(&question(&ann(1))),
                    entry_of(&comment(&ann(2), "Shorter steps.")),
                ],
            ),
        ] {
            store.dispatch_screen(&screen_id, op).expect("applied");
        }
        store
    }

    /// viewer-annotations, Annotations On A Viewer Pane and Saving Annotations
    /// Through The Window: the record on the wire, in creation order with
    /// numbers, and without annotations while the pane holds none; unknown
    /// kinds and views are no ops.
    #[test]
    fn annotations_keep_their_wire_shape_and_are_absent_while_there_are_none() {
        let state = viewer_screen();
        let wire = serde_json::to_value(&state.panes[&PaneId::from("v1")]).expect("pane");
        assert!(wire["viewer"].get("annotations").is_none(), "{wire}");
        assert!(
            wire["viewer"].get("lastAnnotationNumber").is_none(),
            "{wire}"
        );

        let change = serde_json::json!({
            "id": ann(2), "kind": "change", "replacement": "retry with backoff",
            "target": {
                "view": "rendered", "quote": "retry three times", "prefix": "", "suffix": ".",
                "startLine": 30, "endLine": 31,
            },
        });
        let general = comment(&ann(3), "Split step 4 into two steps.");
        let (state, _) = reduce_screen(
            state,
            put(
                "v1",
                vec![question(&ann(1)), change.clone(), general.clone()],
            ),
        )
        .expect("put");
        let pane = &state.panes[&PaneId::from("v1")];
        let wire = serde_json::to_value(pane).expect("pane");
        let numbered = |mut annotation: serde_json::Value, number: u32| {
            annotation["number"] = serde_json::json!(number);
            annotation
        };
        assert_eq!(
            wire["viewer"]["annotations"],
            serde_json::json!([
                numbered(question(&ann(1)), 1),
                numbered(change, 2),
                numbered(general, 3),
            ])
        );
        assert_eq!(wire["viewer"]["lastAnnotationNumber"], 3);
        let back: PaneState = serde_json::from_value(wire).expect("pane");
        assert_eq!(back.viewer, pane.viewer);

        let (state, _) =
            reduce_screen(state, remove("v1", &[ann(1), ann(2), ann(3)])).expect("removed");
        let wire = serde_json::to_value(&state.panes[&PaneId::from("v1")]).expect("pane");
        assert!(wire["viewer"].get("annotations").is_none(), "{wire}");
        assert_eq!(wire["viewer"]["lastAnnotationNumber"], 3, "{wire}");

        for annotation in [
            serde_json::json!({"id": ann(1), "kind": "praise", "body": "x"}),
            deletion(1, passage(serde_json::json!({"view": "diff"}))),
            deletion(1, passage(serde_json::json!({"startLine": -1}))),
            deletion(1, serde_json::json!({"view": "source", "quote": "retry"})),
            serde_json::json!({"id": ann(1), "kind": "comment", "body": "x", "status": "done"}),
            serde_json::json!({"id": ann(1), "kind": "comment", "body": "x", "sent": "yes"}),
        ] {
            let op = serde_json::json!({
                "type": "put_annotations", "paneId": "v1", "annotations": [annotation],
            });
            assert!(
                serde_json::from_value::<ScreenOp>(op.clone()).is_err(),
                "{op}"
            );
        }
    }

    /// The wire shape of a thread and of a wait (`docs/protocol.ts`): a
    /// resolved, handed-over annotation with a reply of the user and one of
    /// an agent, and the wait in the record, read back as it was.
    #[test]
    fn a_thread_and_a_wait_have_their_wire_shape() {
        let wire = serde_json::json!({
            "source": {"kind": "file", "path": PLAN},
            "originPaneId": "p1",
            "revision": 2,
            "annotations": [{
                "id": ann(1), "number": 3, "kind": "comment", "body": "Why?",
                "status": "resolved", "sent": true,
                "replies": [
                    {"id": rep(1), "author": "agent", "body": "Because.",
                     "from": {"paneId": "p1", "title": "claude"}},
                    {"id": rep(2), "author": "user", "body": "Fine.", "sent": true},
                    {"id": rep(3), "author": "user", "body": "Not yet."},
                ],
            }],
            "lastAnnotationNumber": 3,
            "reviewWait": {"id": "wait-0123456789abcdef", "paneId": "p1", "title": "claude"},
        });
        let mut viewer = ViewerState::new(
            ViewerSource::File { path: PLAN.into() },
            Some(PaneId::from("p1")),
        );
        viewer.revision = 2;
        viewer.last_annotation_number = 3;
        viewer.review_wait = Some(ReviewWait {
            id: "wait-0123456789abcdef".into(),
            pane_id: PaneId::from("p1"),
            title: "claude".into(),
        });
        viewer.annotations.push(ViewerAnnotation {
            id: ann(1),
            number: 3,
            kind: AnnotationKind::Comment,
            body: Some("Why?".into()),
            replacement: None,
            target: None,
            status: AnnotationStatus::Resolved,
            sent: true,
            replies: vec![
                AnnotationReply {
                    id: rep(1),
                    author: ReplyAuthor::Agent,
                    body: "Because.".into(),
                    sent: false,
                    from: Some(ReplyFrom {
                        pane_id: PaneId::from("p1"),
                        title: "claude".into(),
                    }),
                },
                AnnotationReply {
                    id: rep(2),
                    author: ReplyAuthor::User,
                    body: "Fine.".into(),
                    sent: true,
                    from: None,
                },
                AnnotationReply {
                    id: rep(3),
                    author: ReplyAuthor::User,
                    body: "Not yet.".into(),
                    sent: false,
                    from: None,
                },
            ],
        });
        assert_eq!(serde_json::to_value(&viewer).expect("viewer"), wire);

        // The record is only ever read from `state.json`, which holds no wait.
        let mut stored: ViewerState = serde_json::from_value(wire).expect("viewer");
        assert_eq!(stored.review_wait, None, "a wait is never read back");
        stored.review_wait = viewer.review_wait.clone();
        assert_eq!(stored, viewer);
    }

    /// viewer-annotations (D1 of review-threads): `question` and `delete` of
    /// builds before threads are read as a comment and as a change with an
    /// empty replacement, from a window's op and from `state.json`, and
    /// never written; annotations without a number get theirs in creation
    /// order.
    #[test]
    fn legacy_kinds_are_read_as_comment_and_change_and_never_written() {
        let legacy = || {
            vec![
                serde_json::json!({"id": ann(1), "kind": "question", "body": "Why?"}),
                serde_json::json!({
                    "id": ann(2), "kind": "delete", "body": "Not needed.",
                    "target": passage(serde_json::json!({})),
                }),
            ]
        };
        let (state, _) = reduce_screen(viewer_screen(), put("v1", legacy())).expect("put");
        let annotations = viewer_of(&state, "v1").annotations;
        assert_eq!(annotations[0].kind, AnnotationKind::Comment);
        assert_eq!(annotations[0].body.as_deref(), Some("Why?"));
        assert_eq!(annotations[1].kind, AnnotationKind::Change);
        assert_eq!(annotations[1].replacement.as_deref(), Some(""));
        assert_eq!(annotations[1].body.as_deref(), Some("Not needed."));

        let stored = serde_json::json!({
            "source": {"kind": "file", "path": PLAN}, "revision": 1, "annotations": legacy(),
        });
        let viewer: ViewerState = serde_json::from_value(stored).expect("record");
        assert_eq!(
            viewer
                .annotations
                .iter()
                .map(|a| (a.kind, a.number))
                .collect::<Vec<_>>(),
            [(AnnotationKind::Comment, 1), (AnnotationKind::Change, 2)]
        );
        assert_eq!(viewer.last_annotation_number, 2);
        let written = serde_json::to_string(&viewer).expect("record");
        assert!(
            !written.contains("question") && !written.contains("delete"),
            "{written}"
        );

        // Numbers already given stay, and the counter never falls below them.
        let stored = serde_json::json!({
            "source": {"kind": "file", "path": PLAN}, "revision": 1, "lastAnnotationNumber": 4,
            "annotations": [
                {"id": ann(1), "number": 7, "kind": "comment", "body": "a"},
                {"id": ann(2), "kind": "comment", "body": "b"},
            ],
        });
        let viewer: ViewerState = serde_json::from_value(stored).expect("record");
        assert_eq!(
            viewer
                .annotations
                .iter()
                .map(|a| a.number)
                .collect::<Vec<_>>(),
            [7, 8]
        );
        assert_eq!(viewer.last_annotation_number, 8);
    }

    /// viewer-annotations, Annotations On A Viewer Pane (A question on a
    /// passage): an open comment kept as it came, with the pane's next
    /// number, without an effect.
    #[test]
    fn a_question_on_a_passage_keeps_its_quote_view_lines_context_and_remark() {
        let state = viewer_screen();
        let (next, effects) =
            reduce_screen(state.clone(), put("v1", vec![question(&ann(1))])).expect("put");
        assert!(effects.is_empty());
        assert_eq!(next.revision, state.revision + 1);
        assert_eq!(
            viewer_of(&next, "v1").annotations,
            [ViewerAnnotation {
                id: ann(1),
                number: 1,
                kind: AnnotationKind::Comment,
                body: Some("Why not five times?".into()),
                replacement: None,
                target: Some(AnnotationTarget {
                    view: AnnotationView::Source,
                    quote: "retry three times".into(),
                    prefix: "On failure, ".into(),
                    suffix: " before giving up".into(),
                    start_line: 30,
                    end_line: 30,
                }),
                status: AnnotationStatus::Open,
                sent: false,
                replies: vec![],
            }]
        );
        assert_eq!(
            viewer_of(&next, "v1").revision,
            1,
            "the document is the same"
        );
        assert_eq!(shape(&next.root), shape(&state.root));
        assert_eq!(next.sessions.len(), 1);
    }

    /// viewer-annotations, Annotations On A Viewer Pane (A suggested
    /// deletion): a change whose replacement is empty.
    #[test]
    fn a_suggested_deletion_is_a_change_with_an_empty_replacement() {
        let (next, _) = reduce_screen(
            viewer_screen(),
            put("v1", vec![deletion(1, passage(serde_json::json!({})))]),
        )
        .expect("put");
        let annotation = &viewer_of(&next, "v1").annotations[0];
        assert_eq!(annotation.kind, AnnotationKind::Change);
        assert_eq!(annotation.replacement.as_deref(), Some(""));
        assert_eq!(annotation.body, None, "a change may go without a remark");
    }

    /// viewer-annotations, Annotations On A Viewer Pane (A general comment):
    /// a comment without a passage; a change needs one.
    #[test]
    fn a_general_comment_has_no_target() {
        let (next, effects) = reduce_screen(
            viewer_screen(),
            put(
                "v1",
                vec![
                    comment(&ann(1), "Split step 4 into two steps."),
                    comment(&ann(2), "Is step 5 needed?"),
                ],
            ),
        )
        .expect("put");
        assert!(effects.is_empty());
        let annotations = viewer_of(&next, "v1").annotations;
        assert_eq!(annotations[0].kind, AnnotationKind::Comment);
        assert_eq!(annotations[0].target, None);
        assert_eq!(
            annotations[0].body.as_deref(),
            Some("Split step 4 into two steps.")
        );
        assert_eq!(annotations[1].target, None);
        let error = reduce_screen(
            viewer_screen(),
            put(
                "v1",
                vec![serde_json::json!({"id": ann(3), "kind": "change", "replacement": "x"})],
            ),
        )
        .expect_err("a general change");
        assert!(error.to_string().contains("needs a passage"), "{error}");
    }

    /// viewer-annotations, Saving Annotations Through The Window: a put
    /// replaces an annotation with the same ID in its place and appends new
    /// ones; a remove ignores IDs the pane does not hold; both raise the
    /// screen's revision and have no effect.
    #[test]
    fn a_put_replaces_in_place_and_appends_and_a_remove_ignores_unknown_ids() {
        let (state, _) = reduce_screen(
            viewer_screen(),
            put(
                "v1",
                vec![
                    comment(&ann(1), "a"),
                    comment(&ann(2), "b"),
                    comment(&ann(3), "c"),
                ],
            ),
        )
        .expect("put");
        let (state, effects) = reduce_screen(
            state,
            put(
                "v1",
                vec![comment(&ann(4), "d"), comment(&ann(2), "b, edited")],
            ),
        )
        .expect("put");
        assert!(effects.is_empty());
        assert_eq!(
            annotation_ids(&state, "v1"),
            [ann(1), ann(2), ann(3), ann(4)]
        );
        assert_eq!(numbers(&state, "v1"), [1, 2, 3, 4]);
        assert_eq!(
            viewer_of(&state, "v1").annotations[1].body.as_deref(),
            Some("b, edited")
        );

        let revision = state.revision;
        let (state, effects) =
            reduce_screen(state, remove("v1", &[ann(2), ann(9)])).expect("removed");
        assert!(effects.is_empty());
        assert_eq!(annotation_ids(&state, "v1"), [ann(1), ann(3), ann(4)]);
        assert_eq!(state.revision, revision + 1);

        let (state, _) = reduce_screen(state, remove("v1", &[ann(9)])).expect("unknown IDs");
        assert_eq!(annotation_ids(&state, "v1"), [ann(1), ann(3), ann(4)]);
        assert_eq!(state.revision, revision + 2);
    }

    /// viewer-annotations, Annotations On A Viewer Pane (Numbers are not given
    /// twice): a removed number is not given again, and a replacement keeps
    /// its number.
    #[test]
    fn numbers_are_not_given_twice() {
        let (state, _) = reduce_screen(
            viewer_screen(),
            put(
                "v1",
                vec![
                    comment(&ann(1), "a"),
                    comment(&ann(2), "b"),
                    comment(&ann(3), "c"),
                ],
            ),
        )
        .expect("#1 to #3");
        let (state, _) = reduce_screen(state, remove("v1", &[ann(3)])).expect("#3 removed");
        let (state, _) = reduce_screen(
            state,
            put(
                "v1",
                vec![comment(&ann(4), "d"), comment(&ann(1), "a, edited")],
            ),
        )
        .expect("a new comment and an edit");
        assert_eq!(numbers(&state, "v1"), [1, 2, 4]);
        assert_eq!(viewer_of(&state, "v1").last_annotation_number, 4);
        let (state, _) =
            reduce_screen(state, remove("v1", &[ann(1), ann(2), ann(4)])).expect("all removed");
        let (state, _) =
            reduce_screen(state, put("v1", vec![comment(&ann(5), "e")])).expect("again");
        assert_eq!(numbers(&state, "v1"), [5]);
    }

    /// viewer-annotations, Saving Annotations Through The Window (Two windows
    /// add at the same time): each window's put names only its own
    /// annotation, so in either order the pane holds both, with different
    /// numbers.
    #[test]
    fn two_windows_adding_at_the_same_time_keep_both_annotations() {
        let from_a = || put("v1", vec![question(&ann(1))]);
        let from_b = || put("v1", vec![comment(&ann(2), "Mention the timeout.")]);
        for (first, second, order) in [
            (from_a(), from_b(), [ann(1), ann(2)]),
            (from_b(), from_a(), [ann(2), ann(1)]),
        ] {
            let store = store_with(viewer_screen());
            let screen_id = ScreenId::from("screen1");
            for op in [first, second] {
                assert!(store
                    .dispatch_screen(&screen_id, op)
                    .expect("put")
                    .is_empty());
            }
            let screen = store.get_screen(&screen_id).expect("screen");
            assert_eq!(annotation_ids(&screen, "v1"), order);
            assert_eq!(numbers(&screen, "v1"), [1, 2]);
        }
    }

    /// viewer-annotations, Annotations On A Viewer Pane (One annotation too
    /// many): counted after the op, so a replacement still fits and a new one
    /// fits again once one is removed.
    #[test]
    fn one_annotation_too_many_is_rejected_and_the_pane_keeps_its_hundred() {
        let store = store_with(viewer_screen());
        let screen_id = ScreenId::from("screen1");
        let hundred = (0..100).map(|n| comment(&ann(n), "x")).collect();
        store
            .dispatch_screen(&screen_id, put("v1", hundred))
            .expect("a hundred");
        let before = store.get_screen(&screen_id).expect("screen");

        for op in [
            put("v1", vec![comment(&ann(100), "one more")]),
            put(
                "v1",
                vec![comment(&ann(7), "edited"), comment(&ann(100), "one more")],
            ),
        ] {
            let error = store.dispatch_screen(&screen_id, op).expect_err("too many");
            assert!(
                matches!(
                    error,
                    MuxError::AnnotationsTooMany {
                        limit: 100,
                        got: 101
                    }
                ),
                "{error}"
            );
            let after = store.get_screen(&screen_id).expect("screen");
            assert_eq!(annotation_ids(&after, "v1"), annotation_ids(&before, "v1"));
            assert_eq!(after.revision, before.revision);
        }

        store
            .dispatch_screen(&screen_id, put("v1", vec![comment(&ann(7), "edited")]))
            .expect("a replacement");
        store
            .dispatch_screen(&screen_id, remove("v1", &[ann(0)]))
            .expect("removed");
        store
            .dispatch_screen(
                &screen_id,
                put("v1", vec![comment(&ann(100), "now it fits")]),
            )
            .expect("fits again");
        let screen = store.get_screen(&screen_id).expect("screen");
        assert_eq!(viewer_of(&screen, "v1").annotations.len(), 100);
    }

    /// viewer-annotations, Annotations On A Viewer Pane (A terminal pane):
    /// every annotation op is rejected for a pane that is not a viewer pane
    /// or does not exist, and the workspace stays unchanged.
    #[test]
    fn annotations_on_a_terminal_pane_are_rejected() {
        let store = store_with(viewer_screen());
        let screen_id = ScreenId::from("screen1");
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        for pane in ["p1", "p9"] {
            let expected = if pane == "p1" {
                "pane p1 is not a viewer pane"
            } else {
                "pane p9 not found"
            };
            for op in [
                put(pane, vec![question(&ann(1))]),
                remove(pane, &[ann(1)]),
                put_replies(pane, &ann(1), vec![user_reply(&rep(1), "x")]),
                remove_replies(pane, &ann(1), &[rep(1)]),
                mark_ids(pane, &[ann(1)]),
            ] {
                let name = format!("{op:?}");
                let error = store
                    .dispatch_screen(&screen_id, op)
                    .expect_err(&name)
                    .to_string();
                assert_eq!(error, expected, "{name}");
                assert_eq!(
                    serde_json::to_value(store.get_workspace_state()).expect("workspace"),
                    before
                );
            }
        }
    }

    /// viewer-annotations, Annotations On A Viewer Pane: every rule of an
    /// annotation and of both ops, each rejected as a whole, also when other
    /// annotations of the op are valid, without changing the workspace.
    #[test]
    fn invalid_annotations_are_rejected_and_leave_the_workspace_unchanged() {
        let long = "x".repeat(ANNOTATION_TEXT_LIMIT + 1);
        let long_context = "x".repeat(ANNOTATION_CONTEXT_LIMIT + 1);
        let full_quote = "q".repeat(ANNOTATION_TEXT_LIMIT);
        let change = |replacement: &str| {
            serde_json::json!({
                "id": ann(1), "kind": "change", "replacement": replacement,
                "target": passage(serde_json::json!({})),
            })
        };
        let cases: Vec<(ScreenOp, &str)> = vec![
            (put("v1", vec![]), "1 to 100 entries, got 0"),
            (
                put("v1", (0..101).map(|n| comment(&ann(n), "x")).collect()),
                "1 to 100 entries, got 101",
            ),
            (
                put("v1", vec![comment("ann-0123456789ABCDEF", "x")]),
                "ann- and 16 lowercase hex",
            ),
            (
                put("v1", vec![comment("ann-0123456789abcde", "x")]),
                "ann- and 16 lowercase hex",
            ),
            (
                put("v1", vec![comment("ann-0123456789abcdeg", "x")]),
                "ann- and 16 lowercase hex",
            ),
            (
                put("v1", vec![comment("note-0123456789abcdef", "x")]),
                "ann- and 16 lowercase hex",
            ),
            (
                put("v1", vec![comment(&ann(1), "a"), comment(&ann(1), "b")]),
                "appears more than once",
            ),
            (
                put(
                    "v1",
                    vec![serde_json::json!({"id": ann(1), "kind": "comment"})],
                ),
                "needs a remark",
            ),
            (
                put(
                    "v1",
                    vec![comment(&ann(1), "fine"), comment(&ann(2), " \n\t")],
                ),
                "needs a remark",
            ),
            (
                put(
                    "v1",
                    vec![serde_json::json!({
                        "id": ann(1), "kind": "question", "body": "",
                        "target": passage(serde_json::json!({})),
                    })],
                ),
                "needs a remark",
            ),
            (
                put(
                    "v1",
                    vec![serde_json::json!({
                        "id": ann(1), "kind": "change", "target": passage(serde_json::json!({})),
                    })],
                ),
                "needs a replacement",
            ),
            (
                put(
                    "v1",
                    vec![serde_json::json!({"id": ann(1), "kind": "change", "replacement": "x"})],
                ),
                "needs a passage",
            ),
            (
                put(
                    "v1",
                    vec![serde_json::json!({"id": ann(1), "kind": "delete"})],
                ),
                "needs a passage",
            ),
            (
                put(
                    "v1",
                    vec![{
                        let mut numbered = comment(&ann(2), "x");
                        numbered["number"] = serde_json::json!(5);
                        numbered
                    }],
                ),
                "carries no number",
            ),
            (
                put(
                    "v1",
                    vec![{
                        let mut threaded = comment(&ann(2), "x");
                        threaded["replies"] = serde_json::json!([user_reply(&rep(1), "y")]);
                        threaded
                    }],
                ),
                "carries none",
            ),
            (
                put(
                    "v1",
                    vec![deletion(1, passage(serde_json::json!({"quote": ""})))],
                ),
                "needs a quote",
            ),
            (
                put(
                    "v1",
                    vec![deletion(1, passage(serde_json::json!({"startLine": 0})))],
                ),
                "lines must lie between 1 and 10000000",
            ),
            (
                put(
                    "v1",
                    vec![deletion(
                        1,
                        passage(serde_json::json!({"startLine": 5, "endLine": 4})),
                    )],
                ),
                "lines must lie between 1 and 10000000",
            ),
            (
                put(
                    "v1",
                    vec![deletion(
                        1,
                        passage(serde_json::json!({"endLine": 10_000_001})),
                    )],
                ),
                "lines must lie between 1 and 10000000",
            ),
            (
                put("v1", vec![comment(&ann(1), &long)]),
                "remark holds at most 16384 bytes, got 16385",
            ),
            (
                put("v1", vec![change(&long)]),
                "replacement holds at most 16384 bytes",
            ),
            (
                put(
                    "v1",
                    vec![deletion(1, passage(serde_json::json!({"quote": long})))],
                ),
                "quote holds at most 16384 bytes",
            ),
            (
                put(
                    "v1",
                    vec![deletion(
                        1,
                        passage(serde_json::json!({"prefix": long_context})),
                    )],
                ),
                "prefix holds at most 256 bytes, got 257",
            ),
            (
                put(
                    "v1",
                    vec![deletion(
                        1,
                        passage(serde_json::json!({"suffix": long_context})),
                    )],
                ),
                "suffix holds at most 256 bytes",
            ),
            (
                put(
                    "v1",
                    (0..17)
                        .map(|n| deletion(n, passage(serde_json::json!({"quote": full_quote}))))
                        .collect(),
                ),
                "at most 262144 bytes of text, got 278528",
            ),
            (remove("v1", &[]), "1 to 100 entries, got 0"),
            (
                remove("v1", &(0..101).map(ann).collect::<Vec<_>>()),
                "1 to 100 entries, got 101",
            ),
            (
                remove("v1", &[ann(1), "ann-1".into()]),
                "ann- and 16 lowercase hex",
            ),
        ];
        let store = store_with(viewer_screen());
        let screen_id = ScreenId::from("screen1");
        store
            .dispatch_screen(&screen_id, put("v1", vec![comment(&ann(1), "kept")]))
            .expect("one annotation");
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        for (op, expected) in cases {
            let name = format!("{op:?}").chars().take(160).collect::<String>();
            let error = store
                .dispatch_screen(&screen_id, op)
                .expect_err(&name)
                .to_string();
            assert!(error.contains(expected), "{name}: {error}");
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("workspace"),
                before,
                "{name}"
            );
        }
    }

    /// viewer-annotations, Annotations On A Viewer Pane: the limits themselves
    /// are admitted, and a replaced annotation's text is not counted twice.
    #[test]
    fn annotation_limits_admit_their_bounds() {
        let text = "ä".repeat(ANNOTATION_TEXT_LIMIT / 2);
        let context = "x".repeat(ANNOTATION_CONTEXT_LIMIT);
        let store = store_with(viewer_screen());
        let screen_id = ScreenId::from("screen1");
        store
            .dispatch_screen(
                &screen_id,
                put(
                    "v1",
                    vec![serde_json::json!({
                        "id": ann(1), "kind": "change", "body": text, "replacement": text,
                        "target": passage(serde_json::json!({
                            "quote": text, "prefix": context, "suffix": context,
                            "startLine": 1, "endLine": ANNOTATION_LINE_LIMIT,
                        })),
                    })],
                ),
            )
            .expect("each field at its limit");

        let store = store_with(viewer_screen());
        let quote = "q".repeat(ANNOTATION_TEXT_LIMIT);
        let deletions = |from: u32, to: u32| {
            (from..to)
                .map(|n| deletion(n, passage(serde_json::json!({"quote": quote}))))
                .collect::<Vec<_>>()
        };
        assert_eq!(16 * ANNOTATION_TEXT_LIMIT, ANNOTATION_PANE_TEXT_LIMIT);
        store
            .dispatch_screen(&screen_id, put("v1", deletions(0, 16)))
            .expect("the pane's text at its limit");
        store
            .dispatch_screen(&screen_id, put("v1", deletions(3, 4)))
            .expect("a replacement of the same size");
        let error = store
            .dispatch_screen(&screen_id, put("v1", vec![comment(&ann(16), "x")]))
            .expect_err("one byte more");
        assert!(
            matches!(error, MuxError::AnnotationsTooLong { got, .. } if got == ANNOTATION_PANE_TEXT_LIMIT + 1),
            "{error}"
        );

        let store = store_with(viewer_screen());
        store
            .dispatch_screen(
                &screen_id,
                put("v1", (0..100).map(|n| comment(&ann(n), "x")).collect()),
            )
            .expect("a hundred in one op");
        store
            .dispatch_screen(
                &screen_id,
                remove("v1", &(0..100).map(ann).collect::<Vec<_>>()),
            )
            .expect("a hundred removed in one op");
    }

    /// viewer-annotations, Saving Annotations Through The Window (An edit
    /// after sending): `mark_sent` marks the annotation and the user's
    /// reply; a put of a changed entry makes it pending again, also when it
    /// says `sent`, and a put never hands an entry over: not a new one, and
    /// not one that was pending. A put of an unchanged entry keeps its mark.
    #[test]
    fn an_edit_after_sending_is_pending_again() {
        let store = store_with(viewer_screen());
        let screen_id = ScreenId::from("screen1");
        let reply = user_reply(&rep(1), "And the timeout?");
        for op in [
            put("v1", vec![question(&ann(1)), comment(&ann(2), "Later.")]),
            put_replies("v1", &ann(1), vec![reply.clone()]),
            mark_sent(
                "v1",
                vec![
                    entry_of(&question(&ann(1))),
                    entry_of(&reply),
                    entry_of(&question(&ann(9))),
                    entry_of(&user_reply(&rep(9), "x")),
                ],
            ),
        ] {
            store.dispatch_screen(&screen_id, op).expect("applied");
        }
        let sent = annotation_of(&store, &ann(1));
        assert!(sent.sent);
        assert!(sent.replies[0].sent);
        assert!(!annotation_of(&store, &ann(2)).sent, "not in the review");

        let mut resolved = question(&ann(1));
        resolved["status"] = serde_json::json!("resolved");
        resolved["sent"] = serde_json::json!(true);
        store
            .dispatch_screen(&screen_id, put("v1", vec![resolved]))
            .expect("resolved");
        assert!(annotation_of(&store, &ann(1)).sent, "unchanged, still sent");

        let mut edited = question(&ann(1));
        edited["body"] = serde_json::json!("Why not five times, with backoff?");
        edited["sent"] = serde_json::json!(true);
        store
            .dispatch_screen(&screen_id, put("v1", vec![edited]))
            .expect("edited");
        let edited = annotation_of(&store, &ann(1));
        assert!(!edited.sent, "pending again");
        assert!(edited.replies[0].sent, "the reply was not changed");
        assert_eq!(edited.number, 1);

        let mut sent_reply = user_reply(&rep(1), "And the timeout, too?");
        sent_reply["sent"] = serde_json::json!(true);
        let mut new_reply = user_reply(&rep(2), "Never mind.");
        new_reply["sent"] = serde_json::json!(true);
        let mut pending = comment(&ann(2), "Later.");
        pending["sent"] = serde_json::json!(true);
        let mut new = comment(&ann(3), "Also the cache.");
        new["sent"] = serde_json::json!(true);
        for op in [
            put_replies("v1", &ann(1), vec![sent_reply, new_reply]),
            put("v1", vec![pending, new]),
        ] {
            store.dispatch_screen(&screen_id, op).expect("applied");
        }
        let replies = annotation_of(&store, &ann(1)).replies;
        assert!(
            replies.iter().all(|reply| !reply.sent),
            "a changed and a new reply are pending: {replies:?}"
        );
        assert!(
            !annotation_of(&store, &ann(2)).sent,
            "a put does not hand over"
        );
        assert!(!annotation_of(&store, &ann(3)).sent, "nor does a new one");
    }

    /// viewer-annotations, Handing A Review To A Terminal (Edited
    /// meanwhile): a review of #1 and a reply, both changed by another window
    /// before the review's `mark_sent` arrives; neither is marked, while #2,
    /// unchanged since the review, is.
    #[test]
    fn an_entry_changed_since_the_review_stays_pending() {
        let store = store_with(viewer_screen());
        let screen_id = ScreenId::from("screen1");
        let reply = user_reply(&rep(1), "And the timeout?");
        let held = vec![
            entry_of(&question(&ann(1))),
            entry_of(&comment(&ann(2), "Later.")),
            entry_of(&reply),
        ];
        let mut rewritten = question(&ann(1));
        rewritten["body"] = serde_json::json!("Why not ten times?");
        let mut moved = question(&ann(1));
        moved["target"]["startLine"] = serde_json::json!(31);
        moved["target"]["endLine"] = serde_json::json!(31);
        for op in [
            put("v1", vec![question(&ann(1)), comment(&ann(2), "Later.")]),
            put_replies("v1", &ann(1), vec![reply]),
            // Another window, before the review is marked.
            put("v1", vec![rewritten]),
            put_replies("v1", &ann(1), vec![user_reply(&rep(1), "No timeout.")]),
            mark_sent("v1", held.clone()),
        ] {
            store.dispatch_screen(&screen_id, op).expect("applied");
        }
        let first = annotation_of(&store, &ann(1));
        assert!(!first.sent, "the remark changed");
        assert!(!first.replies[0].sent, "the reply changed");
        assert!(annotation_of(&store, &ann(2)).sent, "#2 reads as it did");

        let mut back = question(&ann(1));
        back["kind"] = serde_json::json!("change");
        back["replacement"] = serde_json::json!("retry five times");
        for (put_op, what) in [
            (put("v1", vec![moved]), "the passage"),
            (put("v1", vec![back]), "the kind and replacement"),
        ] {
            store.dispatch_screen(&screen_id, put_op).expect("put");
            store
                .dispatch_screen(&screen_id, mark_sent("v1", held.clone()))
                .expect("marked");
            assert!(!annotation_of(&store, &ann(1)).sent, "{what} changed");
        }
    }

    /// viewer-annotations, Threads (The user answers an agent, An agent's
    /// reply is not edited): the user's reply follows the agent's; a window
    /// cannot put a reply under the ID of an agent's reply, and the agent's
    /// reply stays as it was written.
    #[test]
    fn the_user_answers_an_agent_whose_reply_is_not_edited() {
        let store = sent_store();
        let screen_id = ScreenId::from("screen1");
        store
            .dispatch(agent_reply(2, &rep(1), "Each step is one commit."))
            .expect("agent's reply");
        store
            .dispatch_screen(
                &screen_id,
                put_replies(
                    "v1",
                    &ann(2),
                    vec![user_reply(&rep(2), "Then split step 4.")],
                ),
            )
            .expect("user's reply");
        let thread = annotation_of(&store, &ann(2)).replies;
        assert_eq!(
            thread
                .iter()
                .map(|reply| (reply.author, reply.body.as_str()))
                .collect::<Vec<_>>(),
            [
                (ReplyAuthor::Agent, "Each step is one commit."),
                (ReplyAuthor::User, "Then split step 4."),
            ]
        );

        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        let error = store
            .dispatch_screen(
                &screen_id,
                put_replies("v1", &ann(2), vec![user_reply(&rep(1), "Rewritten.")]),
            )
            .expect_err("an agent's reply");
        assert!(matches!(error, MuxError::AgentReplyPut(_)), "{error}");
        assert_eq!(
            serde_json::to_value(store.get_workspace_state()).expect("workspace"),
            before
        );

        // A user's reply is replaced in its place; either author's reply can
        // be removed, and unknown IDs are ignored.
        store
            .dispatch_screen(
                &screen_id,
                put_replies(
                    "v1",
                    &ann(2),
                    vec![user_reply(&rep(2), "Then split step 5.")],
                ),
            )
            .expect("edited");
        assert_eq!(
            annotation_of(&store, &ann(2)).replies[1].body,
            "Then split step 5."
        );
        store
            .dispatch_screen(&screen_id, remove_replies("v1", &ann(2), &[rep(1), rep(7)]))
            .expect("the agent's reply removed");
        assert_eq!(
            annotation_of(&store, &ann(2))
                .replies
                .iter()
                .map(|reply| reply.id.clone())
                .collect::<Vec<_>>(),
            [rep(2)]
        );
        store
            .dispatch_screen(&screen_id, remove_replies("v1", &ann(9), &[rep(2)]))
            .expect("an annotation the pane no longer holds");
        // Removing an annotation removes its replies.
        store
            .dispatch_screen(&screen_id, remove("v1", &[ann(2)]))
            .expect("removed");
        let workspace = serde_json::to_string(&store.get_workspace_state()).expect("workspace");
        assert!(!workspace.contains(&rep(2)), "{workspace}");
    }

    /// viewer-annotations, Threads and Saving Annotations Through The Window:
    /// the rules of `put_replies`, `remove_replies` and `mark_sent`, each
    /// rejected as a whole without changing the workspace.
    #[test]
    fn invalid_replies_and_marks_are_rejected_and_leave_the_workspace_unchanged() {
        let store = sent_store();
        let screen_id = ScreenId::from("screen1");
        store
            .dispatch_screen(
                &screen_id,
                put_replies("v1", &ann(1), vec![user_reply(&rep(1), "First.")]),
            )
            .expect("a reply");
        let long = "x".repeat(ANNOTATION_TEXT_LIMIT + 1);
        let cases: Vec<(ScreenOp, &str)> = vec![
            (put_replies("v1", &ann(2), vec![]), "1 to 50 entries, got 0"),
            (
                put_replies(
                    "v1",
                    &ann(2),
                    (10..61).map(|n| user_reply(&rep(n), "x")).collect(),
                ),
                "1 to 50 entries, got 51",
            ),
            (
                put_replies("v1", &ann(9), vec![user_reply(&rep(2), "x")]),
                "holds no annotation",
            ),
            (
                put_replies("v1", &ann(2), vec![user_reply("rep-1", "x")]),
                "rep- and 16 lowercase hex",
            ),
            (
                put_replies("v1", &ann(2), vec![user_reply(&rep(2), " \n")]),
                "not only whitespace",
            ),
            (
                put_replies("v1", &ann(2), vec![user_reply(&rep(2), &long)]),
                "at most 16384 bytes, got 16385",
            ),
            (
                put_replies(
                    "v1",
                    &ann(2),
                    vec![serde_json::json!({"id": rep(2), "author": "agent", "body": "x"})],
                ),
                "only the user's replies",
            ),
            (
                put_replies(
                    "v1",
                    &ann(2),
                    vec![serde_json::json!({
                        "id": rep(2), "author": "user", "body": "x",
                        "from": {"paneId": "p1", "title": "claude"},
                    })],
                ),
                "names the pane it came from",
            ),
            (
                put_replies(
                    "v1",
                    &ann(2),
                    vec![user_reply(&rep(2), "a"), user_reply(&rep(2), "b")],
                ),
                "more than once",
            ),
            (
                put_replies("v1", &ann(2), vec![user_reply(&rep(1), "elsewhere")]),
                "more than once",
            ),
            (remove_replies("v1", &ann(1), &[]), "1 to 50 entries, got 0"),
            (
                remove_replies("v1", &ann(1), &[ann(1)]),
                "rep- and 16 lowercase hex",
            ),
            (mark_ids("v1", &[]), "1 to 5100 entries, got 0"),
            (
                mark_ids("v1", &(0..5101).map(ann).collect::<Vec<_>>()),
                "1 to 5100 entries, got 5101",
            ),
            (
                mark_ids("v1", &[ann(1), "wait-0123456789abcdef".into()]),
                "annotation and reply IDs only",
            ),
        ];
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        for (op, expected) in cases {
            let name = format!("{op:?}").chars().take(160).collect::<String>();
            let error = store
                .dispatch_screen(&screen_id, op)
                .expect_err(&name)
                .to_string();
            assert!(error.contains(expected), "{name}: {error}");
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("workspace"),
                before,
                "{name}"
            );
        }
        let error = serde_json::from_value::<ScreenOp>(serde_json::json!({
            "type": "put_replies", "paneId": "v1", "annotationId": ann(1),
            "replies": [{"id": rep(3), "author": "system", "body": "x"}],
        }))
        .expect_err("an unknown author");
        assert!(error.to_string().contains("unknown variant"), "{error}");
    }

    /// viewer-annotations, Threads and Annotations On A Viewer Pane: a thread
    /// holds 50 replies, replies count toward the pane's text, and the
    /// limits themselves are admitted; `mark_sent` takes 5,100 entries.
    #[test]
    fn reply_limits_admit_their_bounds() {
        let store = sent_store();
        let screen_id = ScreenId::from("screen1");
        store
            .dispatch_screen(
                &screen_id,
                put_replies(
                    "v1",
                    &ann(1),
                    (0..49)
                        .map(|n| user_reply(&rep(n), "x"))
                        .chain([user_reply(&rep(49), &"r".repeat(ANNOTATION_TEXT_LIMIT))])
                        .collect(),
                ),
            )
            .expect("fifty replies, one at its limit");
        let error = store
            .dispatch_screen(
                &screen_id,
                put_replies("v1", &ann(1), vec![user_reply(&rep(50), "x")]),
            )
            .expect_err("a 51st reply");
        assert!(
            matches!(error, MuxError::RepliesTooMany { limit: 50, got: 51 }),
            "{error}"
        );
        let error = store
            .dispatch(agent_reply(1, &rep(51), "x"))
            .expect_err("an agent's 51st reply");
        assert!(
            matches!(error, MuxError::RepliesTooMany { limit: 50, got: 51 }),
            "{error}"
        );
        store
            .dispatch_screen(
                &screen_id,
                put_replies("v1", &ann(1), vec![user_reply(&rep(3), "edited")]),
            )
            .expect("a replacement in a full thread");

        let held = |store: &crate::mux::Store| {
            let screen = store.get_screen(&screen_id).expect("screen");
            viewer_of(&screen, "v1")
                .annotations
                .iter()
                .map(ViewerAnnotation::text_bytes)
                .sum::<usize>()
        };
        let room = ANNOTATION_PANE_TEXT_LIMIT - held(&store);
        let quote = "q".repeat(ANNOTATION_TEXT_LIMIT);
        let fill = room / ANNOTATION_TEXT_LIMIT;
        store
            .dispatch_screen(
                &screen_id,
                put(
                    "v1",
                    (10..10 + u32::try_from(fill).expect("few"))
                        .map(|n| deletion(n, passage(serde_json::json!({"quote": quote}))))
                        .collect(),
                ),
            )
            .expect("the pane nearly full");
        let left = ANNOTATION_PANE_TEXT_LIMIT - held(&store);
        store
            .dispatch_screen(
                &screen_id,
                put_replies("v1", &ann(2), vec![user_reply(&rep(60), &"y".repeat(left))]),
            )
            .expect("a reply that fills the pane");
        let error = store
            .dispatch_screen(
                &screen_id,
                put_replies("v1", &ann(2), vec![user_reply(&rep(61), "z")]),
            )
            .expect_err("one byte more");
        assert!(
            matches!(error, MuxError::AnnotationsTooLong { got, .. } if got == ANNOTATION_PANE_TEXT_LIMIT + 1),
            "{error}"
        );
        let error = store
            .dispatch(agent_reply(2, &rep(62), "z"))
            .expect_err("an agent's byte more");
        assert!(
            matches!(error, MuxError::AnnotationsTooLong { .. }),
            "{error}"
        );

        let ids: Vec<String> = (0..5_100)
            .map(|n| if n % 2 == 0 { ann(n) } else { rep(n) })
            .collect();
        store
            .dispatch_screen(&screen_id, mark_ids("v1", &ids))
            .expect("5,100 entries");
    }

    /// viewer-annotations, Resolving A Thread (Resolved after the agent's
    /// answer, Reopened): the status is part of the annotation a window
    /// puts; resolving and reopening keep the number and the replies, and an
    /// agent may still reply to a resolved annotation without reopening it.
    #[test]
    fn resolving_and_reopening_keep_number_and_replies() {
        let store = sent_store();
        let screen_id = ScreenId::from("screen1");
        store
            .dispatch(agent_reply(2, &rep(1), "Done."))
            .expect("agent's reply");
        let mut resolved = comment(&ann(2), "Shorter steps.");
        resolved["status"] = serde_json::json!("resolved");
        resolved["sent"] = serde_json::json!(true);
        store
            .dispatch_screen(&screen_id, put("v1", vec![resolved]))
            .expect("resolved");
        let annotation = annotation_of(&store, &ann(2));
        assert_eq!(annotation.status, AnnotationStatus::Resolved);
        assert_eq!(annotation.number, 2);
        assert_eq!(annotation.replies.len(), 1);

        store
            .dispatch(agent_reply(2, &rep(2), "Also renamed it."))
            .expect("a reply to a resolved annotation");
        assert_eq!(
            annotation_of(&store, &ann(2)).status,
            AnnotationStatus::Resolved,
            "an agent does not reopen it"
        );

        store
            .dispatch_screen(
                &screen_id,
                put("v1", vec![comment(&ann(2), "Shorter steps.")]),
            )
            .expect("reopened");
        let annotation = annotation_of(&store, &ann(2));
        assert_eq!(annotation.status, AnnotationStatus::Open);
        assert_eq!(annotation.number, 2);
        assert_eq!(annotation.replies.len(), 2);
    }

    /// workspace-model, Layout Operations (No client writes an agent's
    /// reply): the trusted viewer operations have no wire form, and the
    /// window's operations cannot carry an agent's reply.
    #[test]
    fn no_client_writes_an_agents_reply() {
        let reply = serde_json::json!({
            "id": rep(1), "author": "agent", "body": "x",
            "from": {"paneId": "p1", "title": "claude"},
        });
        for op in [
            serde_json::json!({"type": "agent_reply", "paneId": "v1", "number": 1, "reply": reply}),
            serde_json::json!({"type": "add_agent_reply", "paneId": "v1", "number": 1, "reply": reply}),
            serde_json::json!({
                "type": "start_review_wait", "paneId": "v1",
                "wait": {"id": "wait-0123456789abcdef", "paneId": "p1", "title": "claude"},
            }),
            serde_json::json!({"type": "end_review_wait", "paneId": "v1", "waitId": "wait-0123456789abcdef"}),
            serde_json::json!({"type": "review_wait", "paneId": "v1"}),
        ] {
            assert!(
                serde_json::from_value::<ScreenOp>(op.clone()).is_err(),
                "{op}"
            );
        }

        let store = sent_store();
        let screen_id = ScreenId::from("screen1");
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        let mut threaded = comment(&ann(1), "x");
        threaded["replies"] = serde_json::json!([reply]);
        for op in [
            put_replies("v1", &ann(1), vec![reply.clone()]),
            put("v1", vec![threaded]),
        ] {
            assert!(store.dispatch_screen(&screen_id, op).is_err());
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("workspace"),
                before
            );
        }
    }

    /// viewer-annotations, Concurrent Annotation Edits (An agent replies while
    /// the user edits) and Agents Read And Reply To Handed-Over Threads (An
    /// agent replies, A reply to an annotation not sent): the agent's reply
    /// survives a window's replacement of its annotation; a reply needs the
    /// annotation's own hand-over and stays within the rules.
    #[test]
    fn an_agents_reply_survives_an_edit_and_needs_a_sent_annotation() {
        let store = sent_store();
        let screen_id = ScreenId::from("screen1");
        let revision = store.get_screen(&screen_id).expect("screen").revision;
        let effects = store
            .dispatch(agent_reply(
                1,
                &rep(1),
                "Switched to on-demand computation.",
            ))
            .expect("agent's reply");
        assert!(effects.is_empty());
        assert_eq!(
            store.get_screen(&screen_id).expect("screen").revision,
            revision + 1
        );
        let mut edited = question(&ann(1));
        edited["body"] = serde_json::json!("Why not on demand?");
        store
            .dispatch_screen(&screen_id, put("v1", vec![edited]))
            .expect("the window's edit");
        let annotation = annotation_of(&store, &ann(1));
        assert_eq!(annotation.body.as_deref(), Some("Why not on demand?"));
        assert_eq!(
            annotation.replies,
            [AnnotationReply {
                id: rep(1),
                author: ReplyAuthor::Agent,
                body: "Switched to on-demand computation.".into(),
                sent: false,
                from: Some(ReplyFrom {
                    pane_id: PaneId::from("p1"),
                    title: "claude".into(),
                }),
            }]
        );

        store
            .dispatch_screen(&screen_id, put("v1", vec![comment(&ann(3), "Not sent.")]))
            .expect("a pending annotation");
        // #1 is pending again after the edit; #2 is still handed over.
        let aimed = |screen: &str, pane: &str| {
            let mut op = agent_reply(2, &rep(9), "x");
            if let Op::AgentReply {
                screen_id, pane_id, ..
            } = &mut op
            {
                *screen_id = ScreenId::from(screen);
                *pane_id = PaneId::from(pane);
            }
            op
        };
        let with = |change: &dyn Fn(&mut AnnotationReply)| {
            let mut op = agent_reply(2, &rep(9), "x");
            if let Op::AgentReply { reply, .. } = &mut op {
                change(reply);
            }
            op
        };
        let cases: Vec<(Op, &str)> = vec![
            (agent_reply(1, &rep(9), "x"), "has not been handed over"),
            (agent_reply(3, &rep(9), "x"), "has not been handed over"),
            (agent_reply(9, &rep(9), "x"), "holds no annotation #9"),
            (agent_reply(2, &rep(9), " \t"), "not only whitespace"),
            (
                agent_reply(2, &rep(9), &"x".repeat(ANNOTATION_TEXT_LIMIT + 1)),
                "at most 16384 bytes",
            ),
            (agent_reply(2, &rep(1), "again"), "more than once"),
            (agent_reply(2, "rep-x", "x"), "rep- and 16 lowercase hex"),
            (aimed("screen1", "p1"), "is not a viewer pane"),
            (aimed("screen1", "v9"), "pane v9 not found"),
            (aimed("screen9", "v1"), "screen screen9 not found"),
            (
                with(&|reply| reply.from = None),
                "names the pane it came from",
            ),
            (
                with(&|reply| {
                    reply.from = Some(ReplyFrom {
                        pane_id: PaneId::from("p1"),
                        title: "t".repeat(REPLY_TITLE_LIMIT + 1),
                    });
                }),
                "at most 256 bytes",
            ),
            (
                with(&|reply| reply.author = ReplyAuthor::User),
                "author agent",
            ),
            (with(&|reply| reply.sent = true), "author agent"),
        ];
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        for (op, expected) in cases {
            let name = format!("{op:?}").chars().take(200).collect::<String>();
            let error = store.dispatch(op).expect_err(&name).to_string();
            assert!(error.contains(expected), "{name}: {error}");
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("workspace"),
                before,
                "{name}"
            );
        }
    }

    /// viewer-annotations, Waiting For A Review: a wait starts and ends with
    /// trusted operations that raise the revision; one per pane; its end
    /// finds the pane on another screen, and a closed pane has no wait to
    /// end.
    #[test]
    fn a_wait_starts_and_ends_once_per_viewer_pane() {
        let store = two_screen_store();
        let scr1 = ScreenId::from("scr1");
        store
            .dispatch_screen(
                &scr1,
                open_viewer(
                    serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
                ),
            )
            .expect("viewer");
        let wait = |id: &str| ReviewWait {
            id: id.into(),
            pane_id: PaneId::from("p1"),
            title: "claude".into(),
        };
        let start = |screen: &str, pane: &str, id: &str| Op::StartReviewWait {
            screen_id: ScreenId::from(screen),
            pane_id: PaneId::from(pane),
            wait: wait(id),
            revision: None,
        };
        let end = |id: &str| Op::EndReviewWait {
            pane_id: PaneId::from("v1"),
            wait_id: id.into(),
        };
        const W1: &str = "wait-0000000000000001";
        const W2: &str = "wait-0000000000000002";
        let revision = |store: &crate::mux::Store, screen: &str| {
            store
                .get_screen(&ScreenId::from(screen))
                .expect("screen")
                .revision
        };
        let before = revision(&store, "scr1");
        assert!(store
            .dispatch(start("scr1", "v1", W1))
            .expect("started")
            .is_empty());
        assert_eq!(revision(&store, "scr1"), before + 1);
        let screen = store.get_screen(&scr1).expect("screen");
        assert_eq!(viewer_of(&screen, "v1").review_wait, Some(wait(W1)));

        for (op, expected) in [
            (start("scr1", "v1", W2), "already has a process waiting"),
            (start("scr1", "p1", W2), "is not a viewer pane"),
            (start("scr1", "v9", W2), "pane v9 not found"),
            (start("scr9", "v1", W2), "screen scr9 not found"),
            (
                start("scr1", "v1", "wait-1"),
                "already has a process waiting",
            ),
            (end(W2), "no viewer pane holds the wait"),
        ] {
            let error = store.dispatch(op).expect_err(expected).to_string();
            assert!(error.contains(expected), "{error}");
        }
        assert_eq!(revision(&store, "scr1"), before + 1);

        // The pane moved with its record; the wait ends there.
        store
            .dispatch(move_op("scr1", "v1", "scr2", None))
            .expect("moved");
        let moved = revision(&store, "scr2");
        store.dispatch(end(W1)).expect("ended");
        assert_eq!(revision(&store, "scr2"), moved + 1);
        let screen = store.get_screen(&ScreenId::from("scr2")).expect("screen");
        assert_eq!(viewer_of(&screen, "v1").review_wait, None);
        assert!(store.dispatch(end(W1)).is_err(), "ended once");

        let error = store
            .dispatch(start("scr2", "v1", "wait-1"))
            .expect_err("a malformed wait");
        assert!(matches!(error, MuxError::Internal(_)), "{error}");
        store.dispatch(start("scr2", "v1", W2)).expect("a new wait");
        store
            .dispatch_screen(
                &ScreenId::from("scr2"),
                ScreenOp::ClosePane {
                    pane_id: PaneId::from("v1"),
                    resulting_sizes: None,
                },
            )
            .expect("closed");
        assert!(matches!(
            store.dispatch(end(W2)),
            Err(MuxError::ReviewWaitNotFound(_))
        ));
    }

    /// viewer-panes, Viewer Panes and Opening A Viewer (A plan in Markdown):
    /// a text carries its `format` into the record and into snapshots, which
    /// leave out only the text; a revision brings its own format; a format
    /// other than `markdown` is no op at all.
    #[test]
    fn a_text_keeps_the_format_it_was_opened_with() {
        let store = store_with(two_pane_screen());
        let screen_id = ScreenId::from("screen1");
        let plan = |text: &str, format: Option<&str>| {
            let mut source = serde_json::json!({"kind": "text", "title": "Plan", "text": text});
            if let Some(format) = format {
                source["format"] = serde_json::json!(format);
            }
            open_viewer(serde_json::json!({
                "paneId": "p1", "placement": "tab", "replace": true, "source": source,
            }))
        };
        store
            .dispatch_screen(&screen_id, plan("# Plan", Some("markdown")))
            .expect("a plan in Markdown");
        let screen = store.get_screen(&screen_id).expect("screen");
        let (id, pane) = screen
            .panes
            .iter()
            .find(|(_, pane)| pane.viewer.is_some())
            .expect("the viewer");
        let id = id.clone();
        assert_eq!(
            pane.viewer.as_ref().expect("viewer").source,
            ViewerSource::Text {
                title: "Plan".into(),
                text: "# Plan".into(),
                format: Some(TextFormat::Markdown),
            }
        );
        let snapshot =
            serde_json::to_value(screen.clone().without_viewer_texts()).expect("snapshot");
        assert_eq!(
            snapshot["panes"][id.as_str()]["viewer"]["source"],
            serde_json::json!({"kind": "text", "title": "Plan", "format": "markdown"})
        );

        store
            .dispatch_screen(&screen_id, plan("plain words", None))
            .expect("the next revision as plain text");
        let screen = store.get_screen(&screen_id).expect("screen");
        assert_eq!(
            screen.panes[&id].viewer.as_ref().expect("viewer").source,
            ViewerSource::Text {
                title: "Plan".into(),
                text: "plain words".into(),
                format: None,
            }
        );

        let wire = serde_json::json!({
            "type": "open_viewer", "paneId": "p1",
            "source": {"kind": "text", "title": "Plan", "text": "x", "format": "html"},
        });
        assert!(serde_json::from_value::<ScreenOp>(wire).is_err());
    }

    /// viewer-panes, Reopening A File (A revision while a review is pending):
    /// while a process waits for the review of a text, `replace` for it is
    /// refused and changes nothing; once the wait ends, it applies.
    #[test]
    fn a_text_under_review_is_not_replaced() {
        let store = store_with(two_pane_screen());
        let screen_id = ScreenId::from("screen1");
        let plan = |text: &str| {
            open_viewer(serde_json::json!({
                "paneId": "p1", "placement": "tab", "replace": true,
                "source": {"kind": "text", "title": "Plan", "text": text},
            }))
        };
        store
            .dispatch_screen(
                &screen_id,
                open_viewer(serde_json::json!({
                    "paneId": "p1", "newPaneId": "v1", "placement": "tab",
                    "source": {"kind": "text", "title": "Plan", "text": "1. Cache the list."},
                })),
            )
            .expect("the first plan");
        const WAIT: &str = "wait-0000000000000001";
        let stale = store
            .dispatch(Op::StartReviewWait {
                screen_id: screen_id.clone(),
                pane_id: PaneId::from("v1"),
                wait: ReviewWait {
                    id: WAIT.into(),
                    pane_id: PaneId::from("p1"),
                    title: "claude".into(),
                },
                revision: Some(2),
            })
            .expect_err("a wait on a revision the pane does not show");
        assert!(
            matches!(&stale, MuxError::ReviewRevisionChanged(pane) if pane == &PaneId::from("v1")),
            "{stale}"
        );
        store
            .dispatch(Op::StartReviewWait {
                screen_id: screen_id.clone(),
                pane_id: PaneId::from("v1"),
                wait: ReviewWait {
                    id: WAIT.into(),
                    pane_id: PaneId::from("p1"),
                    title: "claude".into(),
                },
                revision: Some(1),
            })
            .expect("waiting on the revision shown");
        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");

        let error = store
            .dispatch_screen(&screen_id, plan("1. Compute the list on demand."))
            .expect_err("refused while the review is pending");
        assert!(
            matches!(&error, MuxError::ReviewWaitBusy(pane) if pane == &PaneId::from("v1")),
            "{error}"
        );
        assert_eq!(
            serde_json::to_value(store.get_workspace_state()).expect("workspace"),
            before
        );

        store
            .dispatch(Op::EndReviewWait {
                pane_id: PaneId::from("v1"),
                wait_id: WAIT.into(),
            })
            .expect("answered");
        store
            .dispatch_screen(&screen_id, plan("1. Compute the list on demand."))
            .expect("replaced after the answer");
        let screen = store.get_screen(&screen_id).expect("screen");
        assert_eq!(
            viewer_of(&screen, "v1").source,
            ViewerSource::Text {
                title: "Plan".into(),
                text: "1. Compute the list on demand.".into(),
                format: None,
            }
        );
    }

    /// viewer-panes, Reopening A File (A revised plan, Another title): with
    /// `replace`, the first viewer of the screen showing a text of the same
    /// title and origin gets the new text, a raised revision and the focus of
    /// its slot, and keeps its annotations; another title or origin opens a
    /// new pane, and so does a text without `replace`.
    #[test]
    fn replace_gives_a_text_to_the_viewer_of_its_title_and_origin() {
        let store = store_with(two_pane_screen());
        let screen_id = ScreenId::from("screen1");
        let text = |title: &str, text: &str, from: &str, replace: bool| {
            let mut fields = serde_json::json!({
                "paneId": from, "placement": "tab",
                "source": {"kind": "text", "title": title, "text": text},
            });
            if replace {
                fields["replace"] = serde_json::json!(true);
            }
            open_viewer(fields)
        };
        store
            .dispatch_screen(
                &screen_id,
                open_viewer(serde_json::json!({
                    "paneId": "p1", "newPaneId": "v1", "placement": "tab", "cwd": "/home/me/app",
                    "source": {"kind": "text", "title": "Plan", "text": "1. Cache the list."},
                })),
            )
            .expect("the first plan");
        store
            .dispatch_screen(&screen_id, put("v1", vec![comment(&ann(1), "Why cache?")]))
            .expect("annotated");
        store
            .dispatch_screen(&screen_id, select_tab("p1"))
            .expect("the terminal in front");
        let before = store.get_screen(&screen_id).expect("screen");

        store
            .dispatch_screen(
                &screen_id,
                text("Plan", "1. Compute the list on demand.", "p1", true),
            )
            .expect("the revised plan");
        let after = store.get_screen(&screen_id).expect("screen");
        assert_eq!(after.panes.len(), before.panes.len(), "no new pane");
        let viewer = viewer_of(&after, "v1");
        assert_eq!(
            viewer.source,
            ViewerSource::Text {
                title: "Plan".into(),
                text: "1. Compute the list on demand.".into(),
                format: None,
            }
        );
        assert_eq!(viewer.revision, 2);
        assert_eq!(viewer.annotations, viewer_of(&before, "v1").annotations);
        assert_eq!(
            shown_viewer(&before, &after),
            Some((PaneId::from("v1"), true))
        );
        assert!(
            after.root.find_slot(&PaneId::from("v1")).is_some_and(|slot| matches!(
                slot,
                LayoutNode::Slot { active_pane_id, .. } if active_pane_id.as_ref() == Some(&PaneId::from("v1"))
            )),
            "{:?}",
            after.root
        );

        for (op, case) in [
            (text("Notes", "x", "p1", true), "another title"),
            (text("Plan", "x", "p2", true), "another origin"),
            (text("Plan", "x", "p1", false), "without replace"),
        ] {
            let before = store.get_screen(&screen_id).expect("screen");
            store.dispatch_screen(&screen_id, op).expect(case);
            let after = store.get_screen(&screen_id).expect("screen");
            assert_eq!(after.panes.len(), before.panes.len() + 1, "{case}");
            assert_eq!(viewer_of(&after, "v1"), viewer_of(&before, "v1"), "{case}");
        }

        let before = serde_json::to_value(store.get_workspace_state()).expect("workspace");
        for op in [
            open_viewer(serde_json::json!({"paneId": "p1", "source": file(PLAN), "replace": true})),
            open_viewer(serde_json::json!({
                "paneId": "p1", "newPaneId": "v9", "replace": true,
                "source": {"kind": "text", "title": "Plan", "text": "x"},
            })),
        ] {
            let error = store.dispatch_screen(&screen_id, op).expect_err("refused");
            assert!(matches!(error, MuxError::ViewerReplaceInvalid), "{error}");
            assert_eq!(
                serde_json::to_value(store.get_workspace_state()).expect("workspace"),
                before
            );
        }

        let wire = serde_json::json!({
            "type": "open_viewer", "source": {"kind": "text", "title": "Plan", "text": "x"},
            "replace": true,
        });
        let op: ScreenOp = serde_json::from_value(wire.clone()).expect("wire op");
        assert_eq!(serde_json::to_value(&op).expect("op"), wire);
        let op = open_viewer(serde_json::json!({"source": file(PLAN)}));
        assert!(serde_json::to_value(&op)
            .expect("op")
            .get("replace")
            .is_none());
    }

    /// viewer-annotations, Annotation Lifetime (Opened again from the CLI): the
    /// reused pane keeps its annotations.
    #[test]
    fn a_viewer_opened_again_keeps_its_annotations() {
        let (state, _) = reduce_screen(
            viewer_screen(),
            put("v1", vec![question(&ann(1)), comment(&ann(2), "Shorter.")]),
        )
        .expect("put");
        let annotations = viewer_of(&state, "v1").annotations;
        let (state, effects) = reduce_screen(
            state,
            open_viewer(serde_json::json!({"paneId": "p1", "source": file(PLAN)})),
        )
        .expect("opened again");
        assert!(effects.is_empty());
        assert_eq!(state.panes.len(), 2, "the pane is reused");
        let viewer = viewer_of(&state, "v1");
        assert_eq!(viewer.revision, 2);
        assert_eq!(viewer.annotations, annotations);
    }

    /// viewer-annotations, Annotation Lifetime (Pane closed): the annotations
    /// end with their pane and with their screen; the same file opens again
    /// in a pane without any.
    #[test]
    fn annotations_end_with_their_pane_and_their_screen() {
        let store = two_screen_store();
        let scr1 = ScreenId::from("scr1");
        for op in [
            open_viewer(
                serde_json::json!({"paneId": "p1", "newPaneId": "v1", "source": file(PLAN)}),
            ),
            put("v1", vec![question(&ann(1))]),
        ] {
            store.dispatch_screen(&scr1, op).expect("applied");
        }
        let effects = store
            .dispatch_screen(
                &scr1,
                ScreenOp::ClosePane {
                    pane_id: PaneId::from("v1"),
                    resulting_sizes: None,
                },
            )
            .expect("closed");
        assert!(effects.is_empty(), "no session ended");
        store
            .dispatch_screen(
                &scr1,
                open_viewer(
                    serde_json::json!({"paneId": "p1", "newPaneId": "v2", "source": file(PLAN)}),
                ),
            )
            .expect("opened again");
        let screen = store.get_screen(&scr1).expect("screen");
        assert!(!screen.panes.contains_key(&PaneId::from("v1")));
        assert!(viewer_of(&screen, "v2").annotations.is_empty());

        store
            .dispatch_screen(&scr1, put("v2", vec![question(&ann(1))]))
            .expect("put");
        store
            .dispatch(Op::CloseScreen {
                screen_id: scr1.clone(),
            })
            .expect("screen closed");
        let workspace = serde_json::to_string(&store.get_workspace_state()).expect("workspace");
        assert!(!workspace.contains(&ann(1)), "{workspace}");
    }
}
