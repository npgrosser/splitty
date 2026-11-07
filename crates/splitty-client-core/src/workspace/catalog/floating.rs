//! Captured floating-group intents on the existing ordered Screen protocol.
use super::*;
use splitty_core::ids::FloatingId;

#[derive(Clone, Debug)]
pub struct FloatingTarget {
    source_id: SourceId,
    connection: Uuid,
    snapshot: Arc<ScreenState>,
    id: String,
}
impl FloatingTarget {
    pub fn source_token(&self) -> SourceToken {
        SourceToken::new(self.source_id, self.connection)
    }
    pub fn floating_id(&self) -> &str {
        &self.id
    }
    pub fn connection_id(&self) -> Uuid {
        self.connection
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatingActionResult {
    pub connection_id: Uuid,
    pub request_id: Uuid,
    pub outcome: FloatingActionOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FloatingActionOutcome {
    Confirmed,
    Rejected(String),
    Unverified(String),
}
#[derive(Clone)]
enum Intent {
    Float(PaneActionTarget),
    Dock {
        target: FloatingTarget,
        slot: String,
    },
    Move {
        target: FloatingTarget,
        bounds: FloatBounds,
    },
}
impl Intent {
    fn screen(&self) -> &str {
        match self {
            Self::Float(source) => &source.screen_id,
            Self::Dock { target, .. } | Self::Move { target, .. } => target.snapshot.id.as_str(),
        }
    }
    fn operation(&self) -> &str {
        match self {
            Self::Float(_) => "float_pane",
            Self::Dock { .. } => "dock_pane",
            Self::Move { .. } => "move_float",
        }
    }
}
#[derive(Clone)]
pub(super) struct PendingFloating {
    request: Uuid,
    intent: Intent,
    mutation: u64,
    fenced: bool,
    failure: Option<String>,
    follow: Option<String>,
}
impl SourceWorkspace {
    pub fn floating_action_pending(&self) -> bool {
        self.pending_floating.is_some()
    }
    pub(crate) fn floating_request_pending(&self, id: Uuid) -> bool {
        self.pending_floating
            .as_ref()
            .is_some_and(|pending| pending.request == id)
    }
    pub(super) fn cancel_floating_focus(&mut self) {
        if let Some(pending) = &mut self.pending_floating {
            pending.follow = None;
        }
    }
    fn require_floating_idle(&self) -> io::Result<()> {
        self.require_catalog()?;
        self.require_routes_settled()?;
        if self.pending_placement.is_some()
            || self.pending_transfer.is_some()
            || !self.pending_screens.is_empty()
            || self
                .models
                .values()
                .any(|screen| screen.pending_move.is_some() || !screen.pending_panes.is_empty())
        {
            return Err(invalid("A conflicting workspace operation is pending"));
        }
        Ok(())
    }
    pub fn can_float_pane(&self, source: &PaneActionTarget) -> bool {
        self.check_float_pane(source).is_ok()
    }
    fn check_float_pane(&self, source: &PaneActionTarget) -> io::Result<()> {
        self.require_floating_idle()?;
        self.validate_pane_action(source)?;
        let screen = self
            .snapshot()
            .ok_or_else(|| invalid("Screen snapshot is not loaded"))?;
        if screen
            .root
            .find_slot(&PaneId::from(source.pane_id.as_str()))
            .is_none()
        {
            return Err(invalid("Only a tiled tab can be floated"));
        }
        if screen.count_root_panes() <= 1 {
            return Err(invalid("Cannot float the last tiled pane"));
        }
        Ok(())
    }
    pub fn capture_float(&self, id: &str) -> io::Result<FloatingTarget> {
        self.require_floating_idle()?;
        let snapshot = self
            .shown()
            .and_then(|model| model.snapshot.clone())
            .ok_or_else(|| invalid("Screen snapshot is not loaded"))?;
        if !snapshot
            .floating
            .iter()
            .any(|entry| entry.id.as_str() == id)
        {
            return Err(invalid("Floating group is gone"));
        }
        Ok(FloatingTarget {
            source_id: self.source_id,
            connection: self.connection_id,
            snapshot,
            id: id.into(),
        })
    }
    pub fn validate_float(&self, target: &FloatingTarget) -> io::Result<()> {
        self.require_floating_idle()?;
        if target.source_token() != self.source_token()
            || self.shown_screen_id() != Some(target.snapshot.id.as_str())
        {
            return Err(invalid("Floating action belongs to a previous Screen"));
        }
        let current = self
            .snapshot()
            .ok_or_else(|| invalid("Screen snapshot is not loaded"))?;
        let before = entry(&target.snapshot, &target.id)
            .ok_or_else(|| invalid("Captured floating group is gone"))?;
        let now = entry(current, &target.id).ok_or_else(|| invalid("Floating group is gone"))?;
        if before.bounds != now.bounds || !same_members(target, current) {
            return Err(invalid("Floating group changed during the action"));
        }
        Ok(())
    }
    pub(crate) fn float_pane(
        &mut self,
        client: &Client,
        request: Uuid,
        source: PaneActionTarget,
    ) -> io::Result<bool> {
        self.check_connection(client)?;
        self.float_with(request, source, |screen, op| {
            client.submit_screen_op(screen, op)
        })
    }
    fn float_with(
        &mut self,
        request: Uuid,
        source: PaneActionTarget,
        send: impl FnOnce(&str, &ScreenOp) -> io::Result<()>,
    ) -> io::Result<bool> {
        self.check_float_pane(&source)?;
        let follow = Some(source.pane_id.clone());
        let op = ScreenOp::FloatPane {
            pane_id: PaneId::from(source.pane_id.as_str()),
            bounds: None,
        };
        self.admit_floating(request, Intent::Float(source), op, follow, send)
    }
    pub(crate) fn dock_float(
        &mut self,
        client: &Client,
        request: Uuid,
        target: FloatingTarget,
    ) -> io::Result<bool> {
        self.check_connection(client)?;
        self.dock_with(request, target, |screen, op| {
            client.submit_screen_op(screen, op)
        })
    }
    fn dock_with(
        &mut self,
        request: Uuid,
        target: FloatingTarget,
        send: impl FnOnce(&str, &ScreenOp) -> io::Result<()>,
    ) -> io::Result<bool> {
        self.validate_float(&target)?;
        let current = self.snapshot().expect("validated Screen");
        let root_slot =
            first_slot(&current.root).ok_or_else(|| invalid("No tiled slot to dock into"))?;
        let panes = entry(current, &target.id)
            .expect("validated float")
            .root
            .collect_pane_ids();
        let first = panes
            .first()
            .ok_or_else(|| invalid("Floating group is empty"))?;
        let anchor = root_slot
            .collect_pane_ids()
            .into_iter()
            .next()
            .unwrap_or_else(|| first.clone());
        let follow = Some(
            self.focused_pane()
                .filter(|pane| panes.iter().any(|id| id.as_str() == *pane))
                .unwrap_or(first.as_str())
                .to_owned(),
        );
        let op = ScreenOp::DockPane {
            floating_id: FloatingId::from(target.id.as_str()),
            pane_id: anchor,
        };
        self.admit_floating(
            request,
            Intent::Dock {
                target,
                slot: root_slot.id().to_string(),
            },
            op,
            follow,
            send,
        )
    }
    pub(crate) fn move_float(
        &mut self,
        client: &Client,
        request: Uuid,
        target: FloatingTarget,
        bounds: FloatBounds,
    ) -> io::Result<bool> {
        self.check_connection(client)?;
        self.move_float_with(request, target, bounds, |screen, op| {
            client.submit_screen_op(screen, op)
        })
    }
    fn move_float_with(
        &mut self,
        request: Uuid,
        target: FloatingTarget,
        bounds: FloatBounds,
        send: impl FnOnce(&str, &ScreenOp) -> io::Result<()>,
    ) -> io::Result<bool> {
        self.validate_float(&target)?;
        if !bounds.is_valid() {
            return Err(invalid(
                "Floating bounds must be finite with positive dimensions",
            ));
        }
        if entry(&target.snapshot, &target.id).is_some_and(|entry| entry.bounds == bounds) {
            return Ok(false);
        }
        let op = ScreenOp::MoveFloat {
            floating_id: FloatingId::from(target.id.as_str()),
            bounds: bounds.clone(),
        };
        self.admit_floating(request, Intent::Move { target, bounds }, op, None, send)
    }
    fn admit_floating(
        &mut self,
        request: Uuid,
        intent: Intent,
        op: ScreenOp,
        follow: Option<String>,
        send: impl FnOnce(&str, &ScreenOp) -> io::Result<()>,
    ) -> io::Result<bool> {
        let mutation = self.next_mutation()?;
        send(intent.screen(), &op)?;
        self.pending_floating = Some(PendingFloating {
            request,
            intent,
            mutation,
            fenced: false,
            failure: None,
            follow,
        });
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(true)
    }
    pub(crate) fn observe_floating_failure(&mut self, connection: Uuid, event: &Value) {
        if connection != self.connection_id || event["type"] != "mux.op_failed" {
            return;
        }
        if let Some(pending) = &mut self.pending_floating {
            if event["screenId"] == pending.intent.screen()
                && event["op"] == pending.intent.operation()
            {
                pending.failure = Some("Floating action was rejected by the service".into());
            }
        }
    }
    pub(super) fn floating_catalog(
        &mut self,
        connection: Uuid,
        watermark: Option<u64>,
        error: Option<&io::Error>,
    ) -> Option<FloatingActionResult> {
        if connection != self.connection_id {
            return None;
        }
        let pending = self.pending_floating.as_mut()?;
        if !watermark.is_some_and(|generation| generation >= pending.mutation) {
            return None;
        }
        pending.fenced = true;
        let failure = error
            .map(|error| format!("Floating action could not be verified: {error}"))
            .or_else(|| pending.failure.clone())
            .or_else(|| {
                (!self.models.contains_key(pending.intent.screen()))
                    .then(|| "Floating action Screen was removed".into())
            });
        if let Some(reason) = failure {
            return self.finish_floating(Some(reason));
        }
        let screen = pending.intent.screen().to_owned();
        self.mark_screen_dirty(&screen);
        None
    }
    pub(super) fn floating_snapshot(
        &mut self,
        connection: Uuid,
        event: &Value,
        result: Result<&WorkspaceDelta, &io::Error>,
    ) -> Option<FloatingActionResult> {
        if connection != self.connection_id {
            return None;
        }
        let pending = self.pending_floating.as_ref()?;
        if !pending.fenced
            || event["screen"]["id"]
                .as_str()
                .is_some_and(|id| id != pending.intent.screen())
        {
            return None;
        }
        if let Err(error) = result {
            return self.finish_floating(Some(format!(
                "Floating action could not be verified: {error}"
            )));
        }
        if !result.is_ok_and(|delta| delta.snapshot_changed) {
            return self.finish_floating(Some(
                "Floating action could not be verified from an older snapshot".into(),
            ));
        }
        self.finish_floating(None)
    }
    pub(super) fn settle_floating_after_refresh(
        &mut self,
        watermark: u64,
    ) -> Option<FloatingActionResult> {
        if watermark < self.pending_floating.as_ref()?.mutation {
            return None;
        }
        if let Some(result) = self.floating_catalog(self.connection_id, Some(watermark), None) {
            return Some(result);
        }
        if !self.pending_floating.as_ref()?.fenced {
            return None;
        }
        self.finish_floating(None)
    }
    pub(super) fn finish_floating(
        &mut self,
        error: Option<String>,
    ) -> Option<FloatingActionResult> {
        let pending = self.pending_floating.take()?;
        let screen = self
            .models
            .get(pending.intent.screen())
            .and_then(ScreenModel::snapshot);
        let confirmed = screen.is_some_and(|screen| match &pending.intent {
            Intent::Float(source) => {
                screen.floating.iter().any(|entry| {
                    entry
                        .root
                        .find_slot(&PaneId::from(source.pane_id.as_str()))
                        .is_some()
                }) && screen
                    .panes
                    .get(&PaneId::from(source.pane_id.as_str()))
                    .and_then(|p| p.session_id.as_ref())
                    .map(SessionId::as_str)
                    == Some(source.session_id.as_str())
            }
            Intent::Dock { target, slot } => {
                entry(screen, &target.id).is_none()
                    && entry(&target.snapshot, &target.id).is_some_and(|old| {
                        old.root.collect_pane_ids().iter().all(|pane| {
                            screen
                                .root
                                .find_slot(pane)
                                .is_some_and(|node| node.id().as_str() == slot)
                                && screen.panes.get(pane).map(|pane| &pane.session_id)
                                    == target.snapshot.panes.get(pane).map(|pane| &pane.session_id)
                        })
                    })
            }
            Intent::Move { target, bounds } => {
                entry(screen, &target.id).is_some_and(|entry| entry.bounds == *bounds)
                    && same_members(target, screen)
            }
        });
        let outcome = if let Some(reason) = pending.failure {
            FloatingActionOutcome::Rejected(reason)
        } else if let Some(reason) = error {
            FloatingActionOutcome::Unverified(reason)
        } else if !confirmed {
            FloatingActionOutcome::Rejected(
                "Floating action was superseded by authoritative state".into(),
            )
        } else {
            if self.shown_screen_id() == Some(pending.intent.screen()) {
                if let Some(pane) = pending.follow {
                    if let Some(model) = self.models.get_mut(pending.intent.screen()) {
                        model.select_tab(&pane);
                    }
                }
            }
            FloatingActionOutcome::Confirmed
        };
        Some(FloatingActionResult {
            connection_id: self.connection_id,
            request_id: pending.request,
            outcome,
        })
    }
}
fn entry<'a>(screen: &'a ScreenState, id: &str) -> Option<&'a FloatingEntry> {
    screen.floating.iter().find(|entry| entry.id.as_str() == id)
}
fn same_members(target: &FloatingTarget, screen: &ScreenState) -> bool {
    let (Some(before), Some(now)) = (
        entry(&target.snapshot, &target.id),
        entry(screen, &target.id),
    ) else {
        return false;
    };
    same_layout(&before.root, &now.root)
        && before.root.collect_pane_ids().iter().all(|id| {
            target.snapshot.panes.get(id).map(|p| &p.session_id)
                == screen.panes.get(id).map(|p| &p.session_id)
        })
}
fn first_slot(root: &LayoutNode) -> Option<&LayoutNode> {
    match root {
        LayoutNode::Slot { .. } => Some(root),
        LayoutNode::Split { children, .. } => children.iter().find_map(first_slot),
    }
}

#[cfg(test)]
mod tests;
