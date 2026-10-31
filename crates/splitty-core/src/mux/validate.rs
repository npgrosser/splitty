//! Pre-validation of screen operations before the reducer runs.
//!
//! Checks that referenced panes, nodes, and floating entries exist, that size
//! arrays have correct length and sum to ~1.0, and that moves have a valid target.

use std::collections::HashSet;

use super::ops::{Arrangement, ScreenOp};
use super::state::{equal_sizes, LayoutNode, ScreenState, SCRATCHPAD_LIMIT};
use super::MuxError;
use crate::ids::*;

/// Validate a screen-level operation against the current state.
pub fn validate_screen_op(state: &ScreenState, op: &ScreenOp) -> Result<(), MuxError> {
    match op {
        ScreenOp::SplitPane {
            pane_id,
            resulting_sizes,
            ..
        } => validate_split_pane(state, pane_id, resulting_sizes.as_deref()),
        ScreenOp::NewTab { pane_id, .. }
        | ScreenOp::ClosePane { pane_id, .. }
        | ScreenOp::SelectTab { pane_id } => validate_located_pane(state, pane_id),
        ScreenOp::ResizeSplit { node_id, sizes } => validate_resize_split(state, node_id, sizes),
        ScreenOp::MovePane {
            pane_id,
            target_slot_id,
            direction,
            target_split_id,
            ..
        } => validate_move_pane(
            state,
            pane_id,
            target_slot_id.as_ref(),
            direction.as_ref(),
            target_split_id.as_ref(),
        ),
        ScreenOp::ArrangePanes { root } => arranged_root(state, root).map(|_| ()),
        ScreenOp::FloatPane { pane_id, .. } => validate_float_pane(state, pane_id),
        ScreenOp::DockPane { floating_id, .. } => validate_dock_pane(state, floating_id),
        ScreenOp::MoveFloat { floating_id, .. } => validate_move_float(state, floating_id),
        ScreenOp::SetScratchpad { session_id, text } => {
            validate_set_scratchpad(state, session_id, text)
        }
    }
}

fn validate_set_scratchpad(
    state: &ScreenState,
    session_id: &SessionId,
    text: &str,
) -> Result<(), MuxError> {
    if !state.sessions.contains_key(session_id) {
        return Err(MuxError::SessionNotFound(session_id.clone()));
    }
    if text.len() > SCRATCHPAD_LIMIT {
        return Err(MuxError::ScratchpadTooLong {
            limit: SCRATCHPAD_LIMIT,
            got: text.len(),
        });
    }
    Ok(())
}

fn validate_split_pane(
    state: &ScreenState,
    pane_id: &PaneId,
    resulting_sizes: Option<&[f64]>,
) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.root.find_slot(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    if let Some(sizes) = resulting_sizes {
        validate_sizes(sizes, 2)?;
    }
    Ok(())
}

fn validate_located_pane(state: &ScreenState, pane_id: &PaneId) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.find_slot_location(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    Ok(())
}

fn validate_resize_split(
    state: &ScreenState,
    node_id: &NodeId,
    sizes: &[f64],
) -> Result<(), MuxError> {
    let node = state
        .root
        .find_node(node_id)
        .ok_or_else(|| MuxError::NodeNotFound(node_id.clone()))?;
    match node {
        super::state::LayoutNode::Split { children, .. } => {
            validate_sizes(sizes, children.len())?;
        }
        _ => return Err(MuxError::NotASplit(node_id.clone())),
    }
    Ok(())
}

fn validate_move_pane(
    state: &ScreenState,
    pane_id: &PaneId,
    target_slot_id: Option<&NodeId>,
    direction: Option<&super::ops::Direction>,
    target_split_id: Option<&NodeId>,
) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.root.find_slot(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    match (target_slot_id, direction, target_split_id) {
        (Some(slot), _, _) => match state.root.find_node(slot) {
            Some(LayoutNode::Slot { .. }) => Ok(()),
            Some(_) => Err(MuxError::NotASlot(slot.clone())),
            None => Err(MuxError::NodeNotFound(slot.clone())),
        },
        (None, Some(_), Some(target)) => {
            let pane = PaneId::from(target.as_str());
            if state.root.find_node(target).is_some() || state.root.find_slot(&pane).is_some() {
                Ok(())
            } else {
                Err(MuxError::NodeNotFound(target.clone()))
            }
        }
        _ => Err(MuxError::MoveTargetRequired),
    }
}

/// The root layout `arrange_panes` asks for. Pane IDs the screen does not
/// hold are left out with the slots and splits they empty; what remains must
/// hold every pane of the root layout exactly once and no floating pane.
pub(super) fn arranged_root(
    state: &ScreenState,
    root: &Arrangement,
) -> Result<LayoutNode, MuxError> {
    let in_root = state.root.collect_pane_ids();
    let arranged = match root {
        // The grid of a screen whose panes all float, or that has none.
        Arrangement::Slot { pane_ids, .. } if pane_ids.is_empty() && in_root.is_empty() => None,
        _ => arranged_node(state, root)?,
    };
    let mut seen = HashSet::new();
    let mut rejected = None;
    if let Some(node) = &arranged {
        node.walk(&mut |node| {
            let LayoutNode::Slot { pane_ids, .. } = node else {
                return true;
            };
            for id in pane_ids {
                if !in_root.contains(id) {
                    rejected = Some(MuxError::PaneNotInLayout(id.clone()));
                } else if !seen.insert(id.clone()) {
                    rejected = Some(MuxError::ArrangementRepeatsPane(id.clone()));
                }
            }
            rejected.is_none()
        });
    }
    if let Some(error) = rejected {
        return Err(error);
    }
    if let Some(missing) = in_root.into_iter().find(|id| !seen.contains(id)) {
        return Err(MuxError::ArrangementMissesPane(missing));
    }
    Ok(arranged.unwrap_or_else(|| LayoutNode::new_slot(NodeId::with_prefix("slot"), vec![])))
}

/// One node of an arrangement as sent, checked, then without the panes the
/// screen does not hold; `None` when none of its panes is left.
fn arranged_node(state: &ScreenState, node: &Arrangement) -> Result<Option<LayoutNode>, MuxError> {
    match node {
        Arrangement::Slot {
            pane_ids,
            active_pane_id,
        } => {
            if pane_ids.is_empty() {
                return Err(MuxError::ArrangementEmptySlot);
            }
            if let Some(active) = active_pane_id.as_ref().filter(|id| !pane_ids.contains(id)) {
                return Err(MuxError::ActivePaneOutsideSlot(active.clone()));
            }
            let known: Vec<PaneId> = pane_ids
                .iter()
                .filter(|id| state.panes.contains_key(*id))
                .cloned()
                .collect();
            if known.is_empty() {
                return Ok(None);
            }
            Ok(Some(LayoutNode::Slot {
                id: NodeId::with_prefix("slot"),
                active_pane_id: active_pane_id.clone().filter(|id| known.contains(id)),
                pane_ids: known,
            }))
        }
        Arrangement::Split {
            axis,
            children,
            sizes,
        } => {
            if children.len() < 2 {
                return Err(MuxError::ArrangementSplitTooSmall);
            }
            validate_sizes(sizes, children.len())?;
            let mut kept = Vec::new();
            let mut kept_sizes = Vec::new();
            for (child, size) in children.iter().zip(sizes) {
                if let Some(node) = arranged_node(state, child)? {
                    kept.push(node);
                    kept_sizes.push(*size);
                }
            }
            if kept.len() < 2 {
                return Ok(kept.pop());
            }
            // The share of a left-out child goes to the others.
            let sum: f64 = kept_sizes.iter().sum();
            let sizes = if sum > 0.0 {
                kept_sizes.iter().map(|size| size / sum).collect()
            } else {
                equal_sizes(kept.len())
            };
            Ok(Some(LayoutNode::new_split(
                NodeId::with_prefix("split"),
                axis.clone(),
                kept,
                sizes,
            )))
        }
    }
}

fn validate_float_pane(state: &ScreenState, pane_id: &PaneId) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.root.find_slot(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    if state.count_root_panes() <= 1 {
        return Err(MuxError::LastPane);
    }
    Ok(())
}

fn validate_dock_pane(state: &ScreenState, floating_id: &FloatingId) -> Result<(), MuxError> {
    if !state.floating.iter().any(|f| &f.id == floating_id) {
        return Err(MuxError::FloatingNotFound(floating_id.clone()));
    }
    Ok(())
}

fn validate_move_float(state: &ScreenState, floating_id: &FloatingId) -> Result<(), MuxError> {
    if !state.floating.iter().any(|f| &f.id == floating_id) {
        return Err(MuxError::FloatingNotFound(floating_id.clone()));
    }
    Ok(())
}

/// Validate that a sizes slice has the expected length and sums to ~1.0.
pub fn validate_sizes(sizes: &[f64], expected_len: usize) -> Result<(), MuxError> {
    if sizes.len() != expected_len {
        return Err(MuxError::InvalidSizesLength {
            expected: expected_len,
            got: sizes.len(),
        });
    }
    let sum: f64 = sizes.iter().sum();
    for &s in sizes {
        if s < 0.0 {
            return Err(MuxError::NegativeSize(s));
        }
    }
    if (sum - 1.0).abs() > 0.01 {
        return Err(MuxError::SizesSumInvalid(sum));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::state::*;
    use std::collections::HashMap;

    fn test_screen() -> ScreenState {
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
                role: None,
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

    #[test]
    fn validate_split_pane_ok() {
        let state = test_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: None,
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };
        assert!(validate_screen_op(&state, &op).is_ok());
    }

    #[test]
    fn validate_split_pane_not_found() {
        let state = test_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("nonexistent"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: None,
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };
        assert!(validate_screen_op(&state, &op).is_err());
    }

    #[test]
    fn validate_sizes_ok() {
        assert!(validate_sizes(&[0.5, 0.5], 2).is_ok());
        assert!(validate_sizes(&[0.33, 0.33, 0.34], 3).is_ok());
    }

    #[test]
    fn validate_sizes_bad_sum() {
        assert!(validate_sizes(&[0.5, 0.3], 2).is_err());
    }

    #[test]
    fn validate_sizes_bad_length() {
        assert!(validate_sizes(&[0.5, 0.5], 3).is_err());
    }

    #[test]
    fn validate_sizes_negative() {
        assert!(validate_sizes(&[-0.5, 1.5], 2).is_err());
    }

    #[test]
    fn validate_float_last_pane_rejected() {
        let state = test_screen();
        let op = ScreenOp::FloatPane {
            pane_id: PaneId::from("p1"),
            bounds: None,
        };
        assert!(matches!(
            validate_screen_op(&state, &op),
            Err(MuxError::LastPane)
        ));
    }
}
