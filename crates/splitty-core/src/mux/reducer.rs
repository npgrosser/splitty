//! Pure reducers that transform mux state in response to operations.
//!
//! `reduce_workspace` handles workspace-level ops (create/close/rename screen).
//! `reduce_screen` handles screen-level ops (split, close, resize, move, float, dock).
//! Both return `(new_state, effects)` without performing any I/O.

use super::effects::Effect;
use super::job_reducer::{mark_closing_jobs, reduce_create_job, reduce_update_job};
use super::ops::{Direction, Op, ScreenOp};
use super::state::*;
use super::validate::{arranged_root, validate_screen_op};
use super::MuxError;
use crate::ids::*;

/// Apply a workspace-level operation. Returns new state + effects.
pub fn reduce_workspace(
    mut state: WorkspaceState,
    op: Op,
) -> Result<(WorkspaceState, Vec<Effect>), MuxError> {
    match op {
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
            } = op
            {
                reject_pane_in_use(&state, new_pane_id)?;
            }
            let screen = state
                .screens
                .remove(&screen_id)
                .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;

            let (new_screen, effects) = reduce_screen(screen, op)?;
            state.screens.insert(screen_id, new_screen);
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

            // Re-creating an existing screen resets it to a single pane (a
            // client whose screen ran empty asks for this). Keep its name and
            // its place in the screen order. A screen that still has panes is
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
            let name = name.or_else(|| existing.and_then(|s| s.name.clone()));
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
                },
            );
            screen.sessions.insert(
                session_id.clone(),
                SessionEntry {
                    id: session_id.clone(),
                    job_id: None,
                    restart_policy: Default::default(),
                    shell_profile_id: shell_profile_id.clone(),
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
            let screen = state
                .screens
                .remove(&screen_id)
                .ok_or_else(|| MuxError::ScreenNotFound(screen_id.clone()))?;

            let effects: Vec<Effect> = screen
                .sessions
                .keys()
                .map(|id| Effect::KillSession {
                    session_id: id.clone(),
                })
                .collect();

            state.screen_order.retain(|id| id != &screen_id);
            mark_closing_jobs(&mut state, &effects);

            // Update active screen if needed
            if state.active_screen_id.as_ref() == Some(&screen_id) {
                state.active_screen_id = state.screen_order.first().cloned();
            }

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
        SlotLocation::Floating(index) => close_pane_in_floating(&mut source, &pane_id, index),
        SlotLocation::Root => close_pane_in_root(&mut source, &pane_id, None),
    }
    source.revision += 1;

    let mut target = if create_screen {
        let mut screen = ScreenState::new(target_id.clone(), screen_name);
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
        },
    );
    state.sessions.insert(
        new_session_id.clone(),
        SessionEntry {
            id: new_session_id.clone(),
            job_id: None,
            restart_policy: Default::default(),
            shell_profile_id: shell_profile_id.clone(),
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
    let location = state
        .find_slot_location(&pane_id)
        .ok_or_else(|| MuxError::PaneNotInLayout(pane_id.clone()))?;
    let root = match location {
        SlotLocation::Root => &mut state.root,
        SlotLocation::Floating(index) => &mut state.floating[index].root,
    };
    let slot_id = root.find_slot(&pane_id).unwrap().id().clone();
    if let Some(LayoutNode::Slot {
        pane_ids,
        active_pane_id,
        ..
    }) = root.find_node_mut(&slot_id)
    {
        pane_ids.push(new_pane_id.clone());
        *active_pane_id = Some(new_pane_id);
    }
    Ok((state, vec![effect]))
}

// ── select_tab ─────────────────────────────────────────────────────────────

fn reduce_select_tab(mut state: ScreenState, pane_id: PaneId) -> Result<ScreenState, MuxError> {
    let location = state
        .find_slot_location(&pane_id)
        .ok_or_else(|| MuxError::PaneNotInLayout(pane_id.clone()))?;
    let root = match location {
        SlotLocation::Root => &mut state.root,
        SlotLocation::Floating(index) => &mut state.floating[index].root,
    };
    let slot_id = root.find_slot(&pane_id).unwrap().id().clone();
    if let Some(LayoutNode::Slot { active_pane_id, .. }) = root.find_node_mut(&slot_id) {
        *active_pane_id = Some(pane_id);
    }
    Ok(state)
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
    let new_slot_id = NodeId::with_prefix("slot");

    // Find slot containing the pane
    let slot_id = state
        .root
        .find_slot(&pane_id)
        .ok_or_else(|| MuxError::PaneNotInLayout(pane_id.clone()))?
        .id()
        .clone();

    let new_slot = LayoutNode::new_slot(new_slot_id, vec![new_pane_id.clone()]);
    let new_pane_idx = new_pane_index.unwrap_or(1);

    // Check if parent split has the same axis
    let parent_info = state.root.find_parent_info(&slot_id);

    if let Some(ref info) = parent_info {
        if info.axis == axis {
            // Same axis as parent — add as sibling
            let insert_at = if new_pane_idx == 0 {
                info.index
            } else {
                info.index + 1
            };

            let parent_id = info.id.clone();
            if let Some(parent) = state.root.find_node_mut(&parent_id) {
                if let LayoutNode::Split {
                    children, sizes, ..
                } = parent
                {
                    children.insert(insert_at, new_slot);
                    *sizes = resulting_sizes.unwrap_or_else(|| equal_sizes(children.len()));
                }
            }

            return Ok((state, vec![effect]));
        }
    }

    // Different axis or root slot — wrap in a new split
    let split_id = NodeId::with_prefix("split");
    let old_slot = state
        .root
        .find_node(&slot_id)
        .ok_or_else(|| MuxError::Internal("slot disappeared".to_string()))?
        .clone();

    let children = if new_pane_idx == 0 {
        vec![new_slot, old_slot]
    } else {
        vec![old_slot, new_slot]
    };

    let sizes = resulting_sizes.unwrap_or_else(|| vec![0.5, 0.5]);
    let new_split = LayoutNode::new_split(split_id, axis, children, sizes);
    replace_node(&mut state.root, &slot_id, new_split);

    Ok((state, vec![effect]))
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
            close_pane_in_floating(&mut state, &pane_id, idx);
        }
        SlotLocation::Root => {
            close_pane_in_root(&mut state, &pane_id, resulting_sizes);
        }
    }

    Ok((state, effects))
}

fn close_pane_in_floating(state: &mut ScreenState, pane_id: &PaneId, floating_idx: usize) {
    // First, gather info with an immutable borrow
    let slot_info = state.floating[floating_idx]
        .root
        .find_slot(pane_id)
        .map(|s| {
            let id = s.id().clone();
            let multi = matches!(s, LayoutNode::Slot { pane_ids, .. } if pane_ids.len() > 1);
            (id, multi)
        });

    match slot_info {
        Some((slot_id, true)) => {
            // Remove from stack within floating
            if let Some(slot) = state.floating[floating_idx].root.find_node_mut(&slot_id) {
                remove_pane_from_slot(slot, pane_id);
            }
        }
        _ => {
            // Remove entire floating entry
            state.floating.remove(floating_idx);
        }
    }
}

fn close_pane_in_root(
    state: &mut ScreenState,
    pane_id: &PaneId,
    resulting_sizes: Option<Vec<f64>>,
) {
    let slot_id = state.root.find_slot(pane_id).unwrap().id().clone();
    let slot_pane_count = match state.root.find_node(&slot_id).unwrap() {
        LayoutNode::Slot { pane_ids, .. } => pane_ids.len(),
        _ => 0,
    };

    if slot_pane_count > 1 {
        // Multi-pane slot — just remove from stack
        if let Some(slot) = state.root.find_node_mut(&slot_id) {
            remove_pane_from_slot(slot, pane_id);
        }
    } else {
        // Single-pane slot — remove from tree
        let parent_info = state.root.find_parent_info(&slot_id);
        if let Some(info) = parent_info {
            let parent_id = info.id.clone();
            if let Some(parent) = state.root.find_node_mut(&parent_id) {
                if let LayoutNode::Split {
                    children, sizes, ..
                } = parent
                {
                    children.remove(info.index);
                    *sizes = resulting_sizes.unwrap_or_else(|| equal_sizes(children.len()));

                    if children.len() == 1 {
                        // Collapse: replace parent split with its single child
                        let child = children.remove(0);
                        replace_node(&mut state.root, &parent_id, child);
                    }
                }
            }
        } else {
            // Root slot — keep a bare empty slot
            state.root = LayoutNode::new_slot(slot_id, vec![]);
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
            },
        );
        let mut sessions = HashMap::new();
        sessions.insert(
            session_id.clone(),
            SessionEntry {
                id: session_id,
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
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
            },
        );

        let mut sessions = HashMap::new();
        sessions.insert(
            s1.clone(),
            SessionEntry {
                id: s1,
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
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
                id: s2,
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
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

        // The screen runs empty: its last pane is closed.
        let screen = state.screens.remove(&ScreenId::from("scr1")).unwrap();
        let close = ScreenOp::ClosePane {
            pane_id: PaneId::from("p1"),
            resulting_sizes: None,
        };
        let (screen, _) = reduce_screen(screen, close).unwrap();
        assert!(screen.panes.is_empty());
        state.screens.insert(ScreenId::from("scr1"), screen);

        // A client whose screen ran empty re-creates it around a fresh pane.
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
        // scr2's grid runs empty, keeping a bare root slot.
        store
            .dispatch(Op::ModifyScreen {
                screen_id: ScreenId::from("scr2"),
                op: ScreenOp::ClosePane {
                    pane_id: PaneId::from("p2"),
                    resulting_sizes: None,
                },
            })
            .unwrap();

        store.dispatch(move_op("scr1", "p3", "scr2", None)).unwrap();

        let after = store.get_workspace_state();
        assert!(after.screens[&ScreenId::from("scr1")].floating.is_empty());
        let target = &after.screens[&ScreenId::from("scr2")];
        assert!(matches!(
            &target.root,
            LayoutNode::Slot { pane_ids, .. } if pane_ids == &[PaneId::from("p3")]
        ));
        assert_eq!(
            store.screen_of_movable_pane(&PaneId::from("p3")),
            Some(ScreenId::from("scr2"))
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
                id: SessionId::from("s1"),
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
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
}
