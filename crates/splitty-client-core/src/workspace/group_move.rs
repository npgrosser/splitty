//! Moving a whole tab group of a Screen's root layout by drag and drop
//! (`workspace-model`, Native Tab Group Move). The window sends the result as
//! one `arrange_panes`: the root layout as it shows it, with the group taken
//! out and placed at the drop target. No pane or session changes.
use super::*;
use splitty_core::mux::Arrangement;

/// A drag of one tab group of the root layout, valid only within its captured
/// connection and layout, like a [`PaneDrag`].
#[derive(Clone, Debug)]
pub struct GroupDrag {
    source_id: SourceId,
    connection_id: Uuid,
    screen_id: String,
    slot: String,
    /// The group's tabs in order, each with what it stands for.
    tabs: Vec<(String, String)>,
    /// The tab this window shows in the group.
    shown: String,
    snapshot: Arc<ScreenState>,
}

impl GroupDrag {
    pub fn source_token(&self) -> SourceToken {
        SourceToken::new(self.source_id, self.connection_id)
    }
    pub fn slot_id(&self) -> &str {
        &self.slot
    }
    pub fn shown_pane(&self) -> &str {
        &self.shown
    }
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }
}

impl ScreenModel {
    /// A group of the root layout with tabs whose identities are known.
    /// Floating groups move by their own header instead.
    pub fn capture_group_drag(&self, slot: &str) -> Option<GroupDrag> {
        let screen = self.snapshot()?;
        let node = screen.root.find_node(&NodeId::from(slot))?;
        let LayoutNode::Slot { pane_ids, .. } = node else {
            return None;
        };
        if pane_ids.is_empty() {
            return None;
        }
        let tabs = pane_ids
            .iter()
            .map(|pane| Some((pane.to_string(), self.identity_for(pane.as_str())?)))
            .collect::<Option<Vec<_>>>()?;
        Some(GroupDrag {
            source_id: self.source_id,
            connection_id: self.connection_id,
            screen_id: self.screen_id().to_owned(),
            slot: slot.to_owned(),
            tabs,
            shown: self.selected_pane(node)?.to_owned(),
            snapshot: self.snapshot.as_ref()?.clone(),
        })
    }

    /// The `arrange_panes` a drop of `drag` at `target` sends: beside the
    /// group or the whole layout (`target` the root split) named by
    /// `placement`, or with `MovePlacement::Tab` into the group, before tab
    /// `before` or last. None when the drop changes nothing.
    pub(super) fn group_move_op(
        &self,
        drag: &GroupDrag,
        target: &str,
        placement: MovePlacement,
        before: Option<&str>,
    ) -> io::Result<Option<ScreenOp>> {
        if self.pending_move.is_some() {
            return Err(invalid(
                "A pane move is awaiting its authoritative snapshot",
            ));
        }
        let screen = self
            .snapshot()
            .ok_or_else(|| invalid("screen snapshot not loaded"))?;
        if drag.source_id != self.source_id
            || drag.connection_id != self.connection_id
            || drag.screen_id != self.screen_id()
            || !same_layout(&drag.snapshot.root, &screen.root)
            || drag
                .tabs
                .iter()
                .any(|(pane, identity)| self.identity_for(pane).as_deref() != Some(identity))
        {
            return Err(invalid("Group drag is stale; drag the group again"));
        }
        if !matches!(
            screen.root.find_node(&NodeId::from(drag.slot.as_str())),
            Some(LayoutNode::Slot { .. })
        ) {
            return Err(invalid("dragged group is no longer in the layout"));
        }
        let root_edge = placement != MovePlacement::Tab
            && screen.root.id().as_str() == target
            && matches!(screen.root, LayoutNode::Split { .. });
        let Some(target_node) = screen.root.find_node(&NodeId::from(target)) else {
            return Err(invalid("drop destination is no longer in the layout"));
        };
        if !root_edge && !matches!(target_node, LayoutNode::Slot { .. }) {
            return Err(invalid("drop destination is no longer a group"));
        }
        if target == drag.slot {
            return Ok(None);
        }
        if let (Some(anchor), LayoutNode::Slot { pane_ids, .. }) = (before, target_node) {
            if !pane_ids.iter().any(|pane| pane.as_str() == anchor) {
                return Err(invalid(
                    "tab insertion anchor is no longer in the destination",
                ));
            }
        }
        let shown = self.with_shown_tabs(&screen.root);
        let current = arrangement(&shown);
        let moved = arrangement(&moved_group_root(
            &shown, drag, target, placement, before, root_edge,
        ));
        if same_arrangement(&current, &moved) {
            return Ok(None);
        }
        Ok(Some(ScreenOp::ArrangePanes { root: moved }))
    }

    pub fn check_group_move(
        &self,
        drag: &GroupDrag,
        target: &str,
        placement: MovePlacement,
        before: Option<&str>,
    ) -> io::Result<bool> {
        self.group_move_op(drag, target, placement, before)
            .map(|op| op.is_some())
    }

    /// Send the move and show it at once; the service's snapshot confirms it
    /// or, after a rejection, its layout comes back.
    pub fn move_group(
        &mut self,
        client: &Client,
        drag: GroupDrag,
        target: &str,
        placement: MovePlacement,
        before: Option<&str>,
    ) -> io::Result<bool> {
        let Some(op) = self.group_move_op(&drag, target, placement, before)? else {
            return Ok(false);
        };
        self.send_op(client, &op)?;
        self.maximized_slot = None;
        self.preview(&op);
        self.select_tab(&drag.shown);
        Ok(true)
    }

    /// `node` with each group's active tab the one this window shows: the
    /// new node IDs of `arrange_panes` end window-local choices.
    fn with_shown_tabs(&self, node: &LayoutNode) -> LayoutNode {
        match node {
            LayoutNode::Slot { id, pane_ids, .. } => LayoutNode::Slot {
                id: id.clone(),
                pane_ids: pane_ids.clone(),
                active_pane_id: self.selected_pane(node).map(PaneId::from),
            },
            LayoutNode::Split {
                id,
                axis,
                children,
                sizes,
            } => LayoutNode::Split {
                id: id.clone(),
                axis: axis.clone(),
                children: children
                    .iter()
                    .map(|child| self.with_shown_tabs(child))
                    .collect(),
                sizes: sizes.clone(),
            },
        }
    }
}

/// The root layout after the drop, with fresh IDs where it made nodes. The
/// dragged group's shown tab is the active tab where its tabs land.
fn moved_group_root(
    root: &LayoutNode,
    drag: &GroupDrag,
    target: &str,
    placement: MovePlacement,
    before: Option<&str>,
    root_edge: bool,
) -> LayoutNode {
    let tabs: Vec<PaneId> = drag
        .tabs
        .iter()
        .map(|(pane, _)| PaneId::from(pane.as_str()))
        .collect();
    let Some(mut rest) = without_slot(root, &NodeId::from(drag.slot.as_str())) else {
        // The group was the whole layout; there is nothing to drop beside.
        return root.clone();
    };
    let shown = Some(PaneId::from(drag.shown.as_str()));
    if placement == MovePlacement::Tab {
        if let Some(LayoutNode::Slot {
            pane_ids,
            active_pane_id,
            ..
        }) = rest.find_node_mut(&NodeId::from(target))
        {
            let at = before
                .and_then(|anchor| pane_ids.iter().position(|pane| pane.as_str() == anchor))
                .unwrap_or(pane_ids.len());
            pane_ids.splice(at..at, tabs);
            *active_pane_id = shown;
        }
        return rest;
    }
    let group = LayoutNode::Slot {
        id: NodeId::with_prefix("slot"),
        pane_ids: tabs,
        active_pane_id: shown,
    };
    let (axis, first) = match placement {
        MovePlacement::Left => (Axis::Horizontal, true),
        MovePlacement::Right => (Axis::Horizontal, false),
        MovePlacement::Above => (Axis::Vertical, true),
        MovePlacement::Below | MovePlacement::Tab => (Axis::Vertical, false),
    };
    let beside = |node: LayoutNode| LayoutNode::Split {
        id: NodeId::with_prefix("split"),
        axis,
        children: if first {
            vec![group, node]
        } else {
            vec![node, group]
        },
        sizes: vec![0.5, 0.5],
    };
    if root_edge {
        return beside(rest);
    }
    replace_node(&mut rest, &NodeId::from(target), beside);
    rest
}

/// A layout as `arrange_panes` carries it, without node IDs.
fn arrangement(node: &LayoutNode) -> Arrangement {
    match node {
        LayoutNode::Slot {
            pane_ids,
            active_pane_id,
            ..
        } => Arrangement::Slot {
            pane_ids: pane_ids.clone(),
            active_pane_id: active_pane_id.clone(),
        },
        LayoutNode::Split {
            axis,
            children,
            sizes,
            ..
        } => Arrangement::Split {
            axis: axis.clone(),
            children: children.iter().map(arrangement).collect(),
            sizes: sizes.clone(),
        },
    }
}

/// `node` without slot `slot`: a split left with one child becomes that
/// child, and the other children keep their relative sizes.
fn without_slot(node: &LayoutNode, slot: &NodeId) -> Option<LayoutNode> {
    match node {
        LayoutNode::Slot { id, .. } => (id != slot).then(|| node.clone()),
        LayoutNode::Split {
            id,
            axis,
            children,
            sizes,
        } => {
            let mut kept = Vec::new();
            let mut kept_sizes = Vec::new();
            for (child, size) in children.iter().zip(sizes) {
                if let Some(child) = without_slot(child, slot) {
                    kept.push(child);
                    kept_sizes.push(*size);
                }
            }
            match kept.len() {
                0 => None,
                1 => kept.pop(),
                count => {
                    let total: f64 = kept_sizes.iter().sum();
                    let sizes = if total > 0. {
                        kept_sizes.iter().map(|size| size / total).collect()
                    } else {
                        mux::equal_sizes(count)
                    };
                    Some(LayoutNode::Split {
                        id: id.clone(),
                        axis: axis.clone(),
                        children: kept,
                        sizes,
                    })
                }
            }
        }
    }
}

/// Replace node `id` of `node` with `with(node)`.
fn replace_node(node: &mut LayoutNode, id: &NodeId, with: impl FnOnce(LayoutNode) -> LayoutNode) {
    fn find<'a>(node: &'a mut LayoutNode, id: &NodeId) -> Option<&'a mut LayoutNode> {
        if node.id() == id {
            return Some(node);
        }
        match node {
            LayoutNode::Split { children, .. } => {
                children.iter_mut().find_map(|child| find(child, id))
            }
            LayoutNode::Slot { .. } => None,
        }
    }
    if let Some(found) = find(node, id) {
        let taken = std::mem::replace(found, LayoutNode::new_slot(id.clone(), vec![]));
        *found = with(taken);
    }
}

/// The same rows, columns, tabs and shown tabs, sizes within rounding.
fn same_arrangement(a: &Arrangement, b: &Arrangement) -> bool {
    match (a, b) {
        (
            Arrangement::Slot {
                pane_ids: a,
                active_pane_id: aa,
            },
            Arrangement::Slot {
                pane_ids: b,
                active_pane_id: ba,
            },
        ) => a == b && aa == ba,
        (
            Arrangement::Split {
                axis: aa,
                children: ac,
                sizes: az,
            },
            Arrangement::Split {
                axis: ba,
                children: bc,
                sizes: bz,
            },
        ) => {
            aa == ba
                && ac.len() == bc.len()
                && az.iter().zip(bz).all(|(a, b)| (a - b).abs() < 1e-9)
                && ac.iter().zip(bc).all(|(a, b)| same_arrangement(a, b))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
