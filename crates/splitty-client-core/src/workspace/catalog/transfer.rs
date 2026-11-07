//! Captured terminal transfer intent; all layout and session changes remain authoritative.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneActionTarget {
    pub connection_id: Uuid,
    pub screen_id: String,
    pub pane_id: String,
    pub session_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenTransferTarget {
    Existing { screen_id: String },
    New { screen_id: String },
}
impl ScreenTransferTarget {
    pub fn screen_id(&self) -> &str {
        match self {
            Self::Existing { screen_id } | Self::New { screen_id } => screen_id,
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
#[derive(Clone)]
pub(super) struct PendingTransfer {
    pub request_id: Uuid,
    pub source: PaneActionTarget,
    pub target: ScreenTransferTarget,
    pub follow: bool,
    pub failure: Option<String>,
}
impl LocalWorkspace {
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
            connection_id: self.connection_id,
            screen_id: self
                .shown_screen_id()
                .ok_or_else(|| invalid("no Screen selected"))?
                .into(),
            pane_id: pane.into(),
            session_id: self
                .session_for(pane)
                .ok_or_else(|| invalid("terminal session is not ready"))?
                .into(),
        };
        self.validate_pane_action(&source)?;
        Ok(source)
    }
    pub fn validate_pane_action(&self, source: &PaneActionTarget) -> io::Result<()> {
        self.require_routes_settled()?;
        if source.connection_id != self.connection_id
            || self.shown_screen_id() != Some(source.screen_id.as_str())
            || self.screen_for(&source.pane_id) != Some(source.screen_id.as_str())
            || self.session_for(&source.pane_id) != Some(source.session_id.as_str())
        {
            return Err(invalid("Tab action belongs to a previous terminal"));
        }
        let screen = self
            .shown()
            .and_then(ScreenModel::snapshot)
            .ok_or_else(|| invalid("Screen snapshot not loaded"))?;
        let pane = screen
            .panes
            .get(&PaneId::from(source.pane_id.as_str()))
            .ok_or_else(|| invalid("terminal pane is gone"))?;
        if find_slot(screen, &source.pane_id).is_none()
            || pane
                .role
                .as_ref()
                .is_some_and(|role| *role != mux::PaneRole::Terminal)
        {
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
            ScreenTransferTarget::Existing { screen_id }
                if !self
                    .catalog
                    .iter()
                    .any(|info| info.id.as_str() == screen_id) =>
            {
                return Err(invalid("Target Screen is gone"))
            }
            ScreenTransferTarget::New { screen_id } if self.models.contains_key(screen_id) => {
                return Err(invalid("Target Screen already exists"))
            }
            ScreenTransferTarget::New { .. } if self.models.len() >= MAX_LOCAL_SCREENS => {
                return Err(capacity("native workspace Screen limit reached"))
            }
            _ => {}
        }
        let mutation = self.next_mutation()?;
        mutation
            .checked_add(2)
            .ok_or_else(|| invalid("workspace mutation generation exhausted"))?;
        send(
            &source.screen_id,
            &source.pane_id,
            target.screen_id(),
            matches!(target, ScreenTransferTarget::New { .. }),
        )?;
        self.mutation = mutation;
        self.reconciling = true;
        self.catalog_dirty = true;
        self.pending_transfer = Some(PendingTransfer {
            request_id,
            source,
            target,
            follow: true,
            failure: None,
        });
        Ok(())
    }
    pub(crate) fn observe_transfer_failure(&mut self, connection: Uuid, event: &Value) {
        if connection != self.connection_id
            || event["type"] != "mux.op_failed"
            || event["op"] != "move_pane_to_screen"
        {
            return;
        }
        if let Some(pending) = &mut self.pending_transfer {
            if event["screenId"] == pending.source.screen_id {
                pending.failure = Some("Screen transfer was rejected by the service".into());
            }
        }
    }
    pub(super) fn finish_transfer(
        &mut self,
        error: Option<String>,
    ) -> Option<ScreenTransferResult> {
        let pending = self.pending_transfer.take()?;
        let outcome = if let Some(reason) = pending.failure {
            ScreenTransferOutcome::Rejected(reason)
        } else if let Some(reason) = error {
            ScreenTransferOutcome::Unverified(reason)
        } else if self.screen_for(&pending.source.pane_id) != Some(pending.target.screen_id())
            || self.session_for(&pending.source.pane_id) != Some(pending.source.session_id.as_str())
            || !self
                .models
                .get(pending.target.screen_id())
                .and_then(ScreenModel::snapshot)
                .is_some_and(|screen| {
                    screen
                        .root
                        .find_slot(&PaneId::from(pending.source.pane_id.as_str()))
                        .is_some()
                })
        {
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
        Some(ScreenTransferResult {
            connection_id: self.connection_id,
            request_id: pending.request_id,
            outcome,
        })
    }
}
