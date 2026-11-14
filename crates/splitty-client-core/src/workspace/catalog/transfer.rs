//! Captured terminal transfer intent; all layout and session changes remain authoritative.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneActionTarget {
    source_id: SourceId,
    pub connection_id: Uuid,
    pub screen_id: String,
    pub pane_id: String,
    pub session_id: String,
}
impl PaneActionTarget {
    pub fn source_token(&self) -> SourceToken {
        SourceToken::new(self.source_id, self.connection_id)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenTransferTarget {
    Existing { screen: ScreenKey },
    New { screen_id: String },
}
impl ScreenTransferTarget {
    pub fn screen_id(&self) -> &str {
        match self {
            Self::Existing { screen } => screen.screen_id(),
            Self::New { screen_id } => screen_id,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenTransferResult {
    pub connection_id: Uuid,
    pub request_id: Uuid,
    pub outcome: ScreenTransferOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenTransferOutcome {
    Confirmed,
    Rejected(String),
    Unverified(String),
}
/// A whole tab group to move to another Screen, captured from one of its
/// tabs: the slot, its tabs in order with their identities, and `shown`, the
/// tab this window shows in it, which stays shown on the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupActionTarget {
    pub shown: PaneActionTarget,
    pub slot_id: String,
    pub tabs: Vec<(String, String)>,
}
impl GroupActionTarget {
    pub fn source_token(&self) -> SourceToken {
        self.shown.source_token()
    }
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }
}
#[derive(Clone)]
pub(super) struct PendingTransfer {
    pub request_id: Uuid,
    /// The pane moved, or the shown tab of the group moved.
    pub source: PaneActionTarget,
    /// Every pane moved with its identity: `source` alone, or the group.
    pub moved: Vec<(String, String)>,
    /// The op whose `mux.op_failed` rejects this transfer.
    pub op: &'static str,
    pub target: ScreenTransferTarget,
    pub follow: bool,
    pub failure: Option<String>,
}
impl SourceWorkspace {
    pub fn route_generation(&self) -> u64 {
        self.route_generation
    }
    pub(crate) fn transfer_request_pending(&self, id: Uuid) -> bool {
        self.pending_transfer
            .as_ref()
            .is_some_and(|p| p.request_id == id)
    }
    pub fn routes_reconciling(&self) -> bool {
        self.reconciling
    }
    pub fn transfer_pending(&self) -> bool {
        self.pending_transfer.is_some()
    }
    pub(super) fn require_routes_settled(&self) -> io::Result<()> {
        if !self.ready {
            return Err(invalid("source connection is not ready"));
        }
        if self.reconciling || self.pending_floating.is_some() || self.pending_template.is_some() {
            Err(invalid("Screen ownership is being reconciled"))
        } else {
            Ok(())
        }
    }
    pub(super) fn cancel_transfer_focus(&mut self) {
        if let Some(p) = &mut self.pending_transfer {
            p.follow = false;
        }
    }
    pub fn capture_pane_action(&self, pane: &str) -> io::Result<PaneActionTarget> {
        let source = PaneActionTarget {
            source_id: self.source_id,
            connection_id: self.connection_id,
            screen_id: self
                .shown_screen_id()
                .ok_or_else(|| invalid("no Screen selected"))?
                .into(),
            pane_id: pane.into(),
            session_id: self
                .identity_for(pane)
                .ok_or_else(|| invalid("terminal session is not ready"))?,
        };
        self.validate_pane_action(&source)?;
        Ok(source)
    }
    pub fn validate_pane_action(&self, source: &PaneActionTarget) -> io::Result<()> {
        self.require_routes_settled()?;
        if source.source_token() != self.source_token()
            || self.shown_screen_id() != Some(source.screen_id.as_str())
            || self.screen_for(&source.pane_id) != Some(source.screen_id.as_str())
            || self.identity_for(&source.pane_id).as_deref() != Some(source.session_id.as_str())
        {
            return Err(invalid("Tab action belongs to a previous terminal"));
        }
        let screen = self
            .shown()
            .and_then(ScreenModel::snapshot)
            .ok_or_else(|| invalid("Screen snapshot not loaded"))?;
        screen
            .panes
            .get(&PaneId::from(source.pane_id.as_str()))
            .ok_or_else(|| invalid("terminal pane is gone"))?;
        if find_slot(screen, &source.pane_id).is_none() {
            return Err(invalid("Tab action requires a canonical terminal pane"));
        }
        Ok(())
    }
    /// Transfer policy is stricter than same-Screen terminal identity admission.
    pub fn can_transfer_pane(&self, source: &PaneActionTarget) -> bool {
        self.validate_transfer_source(source).is_ok()
    }
    fn validate_transfer_source(&self, source: &PaneActionTarget) -> io::Result<()> {
        self.validate_pane_action(source)?;
        let pane = &self.shown().and_then(ScreenModel::snapshot).unwrap().panes
            [&PaneId::from(source.pane_id.as_str())];
        if pane.job_id.is_some() {
            return Err(invalid(
                "Managed job terminals cannot move to another Screen",
            ));
        }
        Ok(())
    }
    pub(crate) fn transfer_pane(
        &mut self,
        client: &Client,
        request_id: Uuid,
        source: PaneActionTarget,
        target: ScreenTransferTarget,
    ) -> io::Result<()> {
        self.check_connection(client)?;
        self.transfer_with(
            request_id,
            source,
            target,
            |source, pane, target, create| {
                client.submit_screen_transfer(source, pane, target, create)
            },
        )
    }
    pub(super) fn transfer_with(
        &mut self,
        request_id: Uuid,
        source: PaneActionTarget,
        target: ScreenTransferTarget,
        send: impl FnOnce(&str, &str, &str, bool) -> io::Result<()>,
    ) -> io::Result<()> {
        self.require_catalog()?;
        self.validate_transfer_source(&source)?;
        let moved = vec![(source.pane_id.clone(), source.session_id.clone())];
        self.begin_transfer(
            request_id,
            source,
            moved,
            "move_pane_to_screen",
            target,
            |source, target, create| send(&source.screen_id, &source.pane_id, target, create),
        )
    }

    /// The group of tab `pane` on the shown Screen, to move to another
    /// Screen as a whole.
    pub fn capture_group_action(&self, pane: &str) -> io::Result<GroupActionTarget> {
        let shown = self.capture_pane_action(pane)?;
        let screen = self
            .shown()
            .and_then(ScreenModel::snapshot)
            .ok_or_else(|| invalid("Screen snapshot not loaded"))?;
        let slot = find_slot(screen, pane).ok_or_else(|| invalid("terminal pane is gone"))?;
        let LayoutNode::Slot { id, pane_ids, .. } = slot else {
            return Err(invalid("terminal pane is gone"));
        };
        let tabs = pane_ids
            .iter()
            .map(|pane| {
                self.identity_for(pane.as_str())
                    .map(|identity| (pane.to_string(), identity))
                    .ok_or_else(|| invalid("terminal session is not ready"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let shown = match self.selected_pane(slot) {
            Some(selected) if selected != pane => self.capture_pane_action(selected)?,
            _ => shown,
        };
        let group = GroupActionTarget {
            shown,
            slot_id: id.to_string(),
            tabs,
        };
        self.validate_group_source(&group)?;
        Ok(group)
    }
    /// Whether `group` can still move to another Screen as captured.
    pub fn can_transfer_group(&self, group: &GroupActionTarget) -> bool {
        self.validate_group_source(group).is_ok()
    }
    fn validate_group_source(&self, group: &GroupActionTarget) -> io::Result<()> {
        self.validate_pane_action(&group.shown)?;
        let screen = self
            .shown()
            .and_then(ScreenModel::snapshot)
            .ok_or_else(|| invalid("Screen snapshot not loaded"))?;
        let same = match find_node(screen, &group.slot_id) {
            Some(LayoutNode::Slot { pane_ids, .. }) => {
                pane_ids.len() == group.tabs.len()
                    && pane_ids
                        .iter()
                        .zip(&group.tabs)
                        .all(|(pane, (id, identity))| {
                            pane.as_str() == id
                                && self.identity_for(id).as_deref() == Some(identity.as_str())
                        })
            }
            _ => false,
        };
        if !same || !group.tabs.iter().any(|(id, _)| id == &group.shown.pane_id) {
            return Err(invalid("Tab group changed since the action began"));
        }
        if group.tabs.iter().any(|(id, _)| {
            screen
                .panes
                .get(&PaneId::from(id.as_str()))
                .is_none_or(|pane| pane.job_id.is_some())
        }) {
            return Err(invalid(
                "A tab group with a managed job terminal cannot move to another Screen",
            ));
        }
        Ok(())
    }
    pub(crate) fn transfer_group(
        &mut self,
        client: &Client,
        request_id: Uuid,
        group: GroupActionTarget,
        target: ScreenTransferTarget,
    ) -> io::Result<()> {
        self.check_connection(client)?;
        self.transfer_group_with(
            request_id,
            group,
            target,
            |screen, slot, shown, target, create| {
                client.submit_group_transfer(screen, slot, shown, target, create)
            },
        )
    }
    pub(super) fn transfer_group_with(
        &mut self,
        request_id: Uuid,
        group: GroupActionTarget,
        target: ScreenTransferTarget,
        send: impl FnOnce(&str, &str, &str, &str, bool) -> io::Result<()>,
    ) -> io::Result<()> {
        self.require_catalog()?;
        self.validate_group_source(&group)?;
        let slot = group.slot_id;
        self.begin_transfer(
            request_id,
            group.shown,
            group.tabs,
            "move_tab_group_to_screen",
            target,
            |shown, target, create| send(&shown.screen_id, &slot, &shown.pane_id, target, create),
        )
    }

    fn begin_transfer(
        &mut self,
        request_id: Uuid,
        source: PaneActionTarget,
        moved: Vec<(String, String)>,
        op: &'static str,
        target: ScreenTransferTarget,
        send: impl FnOnce(&PaneActionTarget, &str, bool) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.pending_transfer.is_some()
            || self.pending_placement.is_some()
            || self
                .models
                .values()
                .any(|model| model.pending_move.is_some() || !model.pending_panes.is_empty())
            || !self.pending_screens.is_empty()
        {
            return Err(invalid("A conflicting workspace operation is pending"));
        }
        identifier(target.screen_id())?;
        if target.screen_id() == source.screen_id {
            return Err(invalid("Source and target Screen are the same"));
        }
        match &target {
            ScreenTransferTarget::Existing { screen }
                if screen.source_id() != self.source_id
                    || !self
                        .catalog
                        .iter()
                        .any(|info| info.id.as_str() == screen.screen_id()) =>
            {
                return Err(invalid("Target Screen is gone"))
            }
            ScreenTransferTarget::New { screen_id } if self.models.contains_key(screen_id) => {
                return Err(invalid("Target Screen already exists"))
            }
            ScreenTransferTarget::New { .. } if self.screen_count() >= self.limits.screens => {
                return Err(capacity("native workspace Screen limit reached"))
            }
            _ => {}
        }
        let mutation = self.next_mutation()?;
        mutation
            .checked_add(2)
            .ok_or_else(|| invalid("workspace mutation generation exhausted"))?;
        send(
            &source,
            target.screen_id(),
            matches!(target, ScreenTransferTarget::New { .. }),
        )?;
        self.mutation = mutation;
        self.reconciling = true;
        self.catalog_dirty = true;
        if let (SourceKind::Remote, ScreenTransferTarget::New { screen_id }) =
            (self.kind(), &target)
        {
            // A window shows only the Screens it opened: hold a place for the
            // one the host is about to make, so its refresh reads it too.
            self.catalog.push(ScreenInfo {
                id: screen_id.as_str().into(),
                name: None,
            });
        }
        self.pending_transfer = Some(PendingTransfer {
            request_id,
            source,
            moved,
            op,
            target,
            follow: true,
            failure: None,
        });
        Ok(())
    }
    pub(crate) fn observe_transfer_failure(&mut self, connection: Uuid, event: &Value) {
        if connection != self.connection_id || !MessageType::MuxOpFailed.is(event) {
            return;
        }
        if let Some(pending) = &mut self.pending_transfer {
            if event["op"] == pending.op && event["screenId"] == pending.source.screen_id {
                pending.failure = Some("Screen transfer was rejected by the service".into());
            }
        }
    }
    pub(super) fn finish_transfer(
        &mut self,
        error: Option<String>,
    ) -> Option<ScreenTransferResult> {
        let pending = self.pending_transfer.take()?;
        let held = matches!(
            (self.kind(), &pending.target),
            (SourceKind::Remote, ScreenTransferTarget::New { .. })
        );
        let outcome = if let Some(reason) = pending.failure {
            ScreenTransferOutcome::Rejected(reason)
        } else if let Some(reason) = error {
            ScreenTransferOutcome::Unverified(reason)
        } else if !pending.moved.iter().all(|(pane, identity)| {
            self.screen_for(pane) == Some(pending.target.screen_id())
                && self.identity_for(pane).as_deref() == Some(identity.as_str())
                && self
                    .models
                    .get(pending.target.screen_id())
                    .and_then(ScreenModel::snapshot)
                    .is_some_and(|screen| {
                        screen
                            .root
                            .find_slot(&PaneId::from(pane.as_str()))
                            .is_some()
                    })
        }) {
            ScreenTransferOutcome::Rejected(
                "Screen transfer was superseded by authoritative state".into(),
            )
        } else {
            if pending.follow {
                self.shown = Some(pending.target.screen_id().into());
                if let Some(target) = self.models.get_mut(pending.target.screen_id()) {
                    target.select_tab(&pending.source.pane_id);
                }
            }
            ScreenTransferOutcome::Confirmed
        };
        if held
            && outcome != ScreenTransferOutcome::Confirmed
            && self.live_catalog().is_none_or(|live| {
                !live
                    .iter()
                    .any(|info| info.id.as_str() == pending.target.screen_id())
            })
        {
            // The host never made the Screen; its place goes again.
            let id = pending.target.screen_id();
            self.catalog.retain(|info| info.id.as_str() != id);
            self.models.remove(id);
        }
        Some(ScreenTransferResult {
            connection_id: self.connection_id,
            request_id: pending.request_id,
            outcome,
        })
    }
}

#[cfg(test)]
mod tests;
