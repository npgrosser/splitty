//! Exact tab intent and verification on the existing ordered session stream.
use super::*;

#[derive(Clone)]
pub(super) struct PendingTabPlacement {
    drag: PaneDrag,
    target: String,
    before: Option<String>,
    mutation: u64,
    fenced: bool,
    failure: Option<String>,
    focus: bool,
}

impl SourceWorkspace {
    pub fn set_tab_placement_supported(&mut self, supported: bool) {
        self.tab_placement_supported = supported;
    }
    pub fn tab_placement_supported(&self) -> bool {
        self.tab_placement_supported && self.catalog_supported
    }
    pub fn placement_pending(&self) -> bool {
        self.pending_placement.is_some()
    }
    pub(super) fn require_no_placement(&self) -> io::Result<()> {
        self.require_routes_settled()?;
        if self.pending_placement.is_some() {
            return Err(invalid(
                "A tab placement is awaiting authoritative verification",
            ));
        }
        Ok(())
    }
    pub(super) fn cancel_placement_focus(&mut self) {
        self.cancel_transfer_focus();
        self.cancel_floating_focus();
        self.cancel_template_follow();
        if let Some(pending) = &mut self.pending_placement {
            pending.focus = false;
        }
    }

    pub fn check_place_tab(
        &self,
        drag: &PaneDrag,
        target: &str,
        before: Option<&str>,
    ) -> io::Result<bool> {
        self.require_no_placement()?;
        if !self.tab_placement_supported() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "service does not support tab-placement-v1",
            ));
        }
        if self
            .models
            .values()
            .any(|model| model.pending_move.is_some())
        {
            return Err(invalid(
                "A pane move is awaiting its authoritative snapshot",
            ));
        }
        let model = self.shown().ok_or_else(|| invalid("no screen selected"))?;
        // Existing drag validation checks owner, Screen, canonical structure,
        // source-slot and live session identity, allowing metadata-only changes.
        model.move_op(drag, target, MovePlacement::Tab)?;
        let screen = model
            .snapshot()
            .ok_or_else(|| invalid("screen snapshot not loaded"))?;
        let Some(LayoutNode::Slot { pane_ids, .. }) = screen.root.find_node(&NodeId::from(target))
        else {
            return Err(invalid("drop destination is no longer a slot"));
        };
        if let Some(anchor) = before {
            if !pane_ids.iter().any(|id| id.as_str() == anchor) {
                return Err(invalid(
                    "tab insertion anchor is no longer in the destination",
                ));
            }
            if anchor == drag.pane {
                return Ok(false);
            }
        }
        Ok(!at_position(pane_ids, &drag.pane, before))
    }

    pub fn can_place_tab(&self, drag: &PaneDrag, target: &str, before: Option<&str>) -> bool {
        self.check_place_tab(drag, target, before).unwrap_or(false)
    }

    pub fn place_tab(
        &mut self,
        client: &Client,
        drag: PaneDrag,
        target: String,
        before: Option<String>,
    ) -> io::Result<bool> {
        self.check_connection(client)?;
        self.place_tab_with(drag, target, before, |screen, op| {
            client.submit_screen_op(screen, op)
        })
    }

    // Keep admission and reservation atomic even if the bounded sender rejects.
    fn place_tab_with(
        &mut self,
        drag: PaneDrag,
        target: String,
        before: Option<String>,
        send: impl FnOnce(&str, &ScreenOp) -> io::Result<()>,
    ) -> io::Result<bool> {
        if !self.check_place_tab(&drag, &target, before.as_deref())? {
            return Ok(false);
        }
        let mutation = self.next_mutation()?;
        let op = ScreenOp::MoveTab {
            pane_id: PaneId::from(drag.pane.as_str()),
            source_slot_id: NodeId::from(drag.source_slot.as_str()),
            target_slot_id: NodeId::from(target.as_str()),
            before_pane_id: before.as_deref().map(PaneId::from),
        };
        send(&drag.screen_id, &op)?;
        let focus = drag.source_slot != target;
        if focus {
            self.shown_mut()?.maximized_slot = None;
        }
        self.pending_placement = Some(PendingTabPlacement {
            drag,
            target,
            before,
            mutation,
            fenced: false,
            failure: None,
            focus,
        });
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(true)
    }

    /// Failures remain attached to this intent until its serial fence arrives.
    /// Clearing sooner could attribute its late failure to a subsequent intent.
    pub fn observe_placement_failure(&mut self, connection: Uuid, event: &Value) {
        if connection != self.connection_id
            || event["type"] != "mux.op_failed"
            || event["op"] != "move_tab"
        {
            return;
        }
        if let Some(pending) = &mut self.pending_placement {
            if event["screenId"] == pending.drag.screen_id {
                pending.failure = Some("Tab placement was rejected by the service".into());
            }
        }
    }

    pub(super) fn placement_catalog(
        &mut self,
        connection: Uuid,
        watermark: Option<u64>,
        error: Option<&io::Error>,
    ) -> Option<PlacementOutcome> {
        if connection != self.connection_id {
            return None;
        }
        let pending = self.pending_placement.as_mut()?;
        if !watermark.is_some_and(|watermark| watermark >= pending.mutation) {
            return None;
        }
        pending.fenced = true;
        let failure = error
            .map(|error| format!("Tab placement could not be verified: {error}"))
            .or_else(|| pending.failure.clone())
            .or_else(|| {
                (!self.models.contains_key(&pending.drag.screen_id))
                    .then(|| "Tab placement Screen was removed".into())
            });
        if let Some(reason) = failure {
            self.pending_placement = None;
            return Some(PlacementOutcome::Rejected(reason));
        }
        let screen = pending.drag.screen_id.clone();
        self.mark_screen_dirty(&screen);
        None
    }

    pub(super) fn placement_snapshot(
        &mut self,
        connection: Uuid,
        event: &Value,
        result: Result<&WorkspaceDelta, &io::Error>,
    ) -> Option<PlacementOutcome> {
        if connection != self.connection_id {
            return None;
        }
        let pending = self.pending_placement.as_ref()?;
        if !pending.fenced {
            return None;
        }
        let id = event["screen"]["id"].as_str();
        if id.is_some_and(|id| id != pending.drag.screen_id) {
            return None;
        }
        if let Err(error) = result {
            self.pending_placement = None;
            return Some(PlacementOutcome::Rejected(format!(
                "Tab placement could not be verified: {error}"
            )));
        }
        if !result.is_ok_and(|delta| delta.snapshot_changed) {
            // Stream order proves this matching frame is post-operation. A
            // lower revision can mean another client recreated the Screen;
            // preserve the cached state but never leave the intent pending.
            self.pending_placement = None;
            return Some(PlacementOutcome::Rejected(
                "Tab placement could not be verified from an older Screen snapshot".into(),
            ));
        }
        let pending = self.pending_placement.take()?;
        let Some(model) = self.models.get_mut(&pending.drag.screen_id) else {
            return Some(PlacementOutcome::Rejected(
                "Tab placement Screen was removed".into(),
            ));
        };
        let confirmed = model.session_for(&pending.drag.pane) == Some(pending.drag.session.as_str())
            && model.snapshot().is_some_and(|screen| {
                matches!(screen.root.find_node(&NodeId::from(pending.target.as_str())),
                    Some(LayoutNode::Slot { pane_ids, .. }) if at_position(pane_ids, &pending.drag.pane, pending.before.as_deref()))
            });
        if !confirmed {
            return Some(PlacementOutcome::Rejected(
                "Tab placement was superseded by authoritative state".into(),
            ));
        }
        if pending.focus && self.shown.as_deref() == Some(pending.drag.screen_id.as_str()) {
            model.select_tab(&pending.drag.pane);
        }
        Some(PlacementOutcome::Confirmed)
    }

    pub(super) fn settle_placement_after_refresh(
        &mut self,
        watermark: u64,
    ) -> Option<PlacementOutcome> {
        if let Some(outcome) = self.placement_catalog(self.connection_id, Some(watermark), None) {
            return Some(outcome);
        }
        let id = self.pending_placement.as_ref()?.drag.screen_id.clone();
        self.placement_snapshot(
            self.connection_id,
            &serde_json::json!({"screen":{"id":id}}),
            Ok(&WorkspaceDelta {
                snapshot_changed: true,
                ..WorkspaceDelta::default()
            }),
        )
    }

    pub fn cycle_tab(
        &mut self,
        connection: Uuid,
        screen: &str,
        slot: &str,
        step: TabStep,
    ) -> io::Result<bool> {
        if connection != self.connection_id
            || self.shown_screen_id() != Some(screen)
            || self.focused_slot_id() != Some(slot)
        {
            return Err(invalid(
                "Tab navigation belongs to a previous focused group",
            ));
        }
        self.cancel_placement_focus();
        self.cycle_tab_in_slot(connection, screen, slot, step)
    }

    /// A hovered header captures its group, independently of keyboard focus.
    pub fn cycle_tab_in_slot(
        &mut self,
        connection: Uuid,
        screen: &str,
        slot: &str,
        step: TabStep,
    ) -> io::Result<bool> {
        if connection != self.connection_id || self.shown_screen_id() != Some(screen) {
            return Err(invalid("Tab navigation belongs to a previous Screen"));
        }
        let model = self.shown().ok_or_else(|| invalid("no screen selected"))?;
        let snapshot = model
            .snapshot()
            .ok_or_else(|| invalid("screen snapshot not loaded"))?;
        let Some(node @ LayoutNode::Slot { pane_ids, .. }) = find_node(snapshot, slot) else {
            return Err(invalid("tab group is gone"));
        };
        let selected = model
            .selected_pane(node)
            .ok_or_else(|| invalid("tab group is empty"))?;
        let index = pane_ids
            .iter()
            .position(|pane| pane.as_str() == selected)
            .ok_or_else(|| invalid("selected tab is gone"))?;
        let next = match step {
            TabStep::Previous => index.saturating_sub(1),
            TabStep::Next => (index + 1).min(pane_ids.len() - 1),
        };
        if index == next {
            return Ok(false);
        }
        let pane = pane_ids[next].to_string();
        Ok(self.select_tab(&pane))
    }
}

fn at_position(panes: &[PaneId], pane: &str, before: Option<&str>) -> bool {
    let Some(index) = panes.iter().position(|id| id.as_str() == pane) else {
        return false;
    };
    match before {
        Some(anchor) => panes.get(index + 1).is_some_and(|id| id.as_str() == anchor),
        None => index + 1 == panes.len(),
    }
}

#[cfg(test)]
mod tests;
