//! Common source catalog policy, canonical Screen collection and admission.
use super::*;
use crate::connection::identifier;
pub(super) mod creation;
pub(super) mod floating;
mod order;
mod placement;
pub(super) mod templates;
use floating::PendingFloating;
pub(crate) mod refresh;
pub(super) mod transfer;
use placement::PendingTabPlacement;
use splitty_transport::budget::{Budget, Charged, ConnectionBudget};
use transfer::PendingTransfer;

/// Empty Screens consume model capacity independently of session attachments.
pub const MAX_LOCAL_SCREENS: usize = 128;
pub const MAX_REMOTE_CATALOG: usize = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    Local,
    Remote,
}

/// Catalog membership and opened membership coincide only on the local source.
#[derive(Clone)]
enum CatalogPolicy {
    Local,
    Remote {
        target: String,
        live: Option<Arc<Charged<Vec<ScreenInfo>>>>,
        missing: HashSet<String>,
        budget: ConnectionBudget,
    },
}

#[derive(Clone, Copy)]
pub(super) struct SourceLimits {
    pub screens: usize,
    pub panes: usize,
}
impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            screens: MAX_LOCAL_SCREENS,
            panes: MAX_ATTACHMENTS,
        }
    }
}

#[derive(Clone)]
struct PendingScreen {
    mutation: u64,
}

/// Previously the local all-catalog collection; now one source owns the same
/// canonical Screen models, projection and candidate admission. Only catalog
/// policy differs: local opens all entries, remote opens explicit references.
/// WindowWorkspace owns cross-source selection and supplies the aggregate
/// allowance before each mutable collection borrow.
#[derive(Clone)]
pub struct SourceWorkspace {
    pub(super) source_id: SourceId,
    connection_id: Uuid,
    policy: CatalogPolicy,
    pub(super) ready: bool,
    pub(super) ports: Option<Arc<Charged<PortObservation>>>,
    pub(super) program_status: super::program_status::ProgramStatusTable,
    pub(super) limits: SourceLimits,
    pub(super) staged: Arc<Budget>,
    catalog: Vec<ScreenInfo>,
    models: HashMap<String, ScreenModel>,
    shown: Option<String>,
    pending_screens: HashMap<String, PendingScreen>,
    remote_creation: Option<creation::RemoteScreenCreation>,
    activate_created: Option<String>,
    catalog_supported: bool,
    screen_placement_supported: bool,
    tab_selection_supported: bool,
    tab_placement_supported: bool,
    pending_placement: Option<PendingTabPlacement>,
    pending_transfer: Option<PendingTransfer>,
    pending_floating: Option<PendingFloating>,
    pending_template: Option<templates::PendingTemplate>,
    templates_supported: bool,
    viewers_supported: bool,
    template_failures: HashMap<String, (String, String)>,
    reconciling: bool,
    route_generation: u64,
    catalog_initialized: bool,
    catalog_dirty: bool,
    catalog_in_flight: Option<u64>,
    mutation: u64,
}

impl SourceWorkspace {
    /// Assess this source's terminal impact, retaining pane and session identity
    /// across a later prompt. The existing admission bounds limit this plan.
    pub fn assess_close(&self, request_id: u64, target: CloseTarget) -> io::Result<ClosePlan> {
        if self.kind() == SourceKind::Remote && target == CloseTarget::Window {
            return Err(invalid("remote Screens detach on window close"));
        }
        if (self.reconciling || self.pending_floating.is_some() || self.pending_template.is_some())
            && target != CloseTarget::Window
        {
            return Err(invalid(
                "Screen ownership is being reconciled; close again after refresh",
            ));
        }
        let mut plan = ClosePlan {
            source_id: Some(self.source_id),
            request_id,
            connection_id: Some(self.connection_id),
            target: target.clone(),
            affected: Vec::new(),
            busy_count: 0,
            unresolved_count: 0,
        };
        match target {
            CloseTarget::Pane { screen, pane } => {
                if self.shown.as_deref() != Some(screen.as_str()) {
                    return Err(invalid("pane Screen is no longer shown"));
                }
                let model = self
                    .models
                    .get(&screen)
                    .ok_or_else(|| invalid("close target Screen is gone"))?;
                if !Self::add_pane_impact(model, &screen, &pane, &mut plan) {
                    return Err(invalid("close target pane is gone"));
                }
            }
            CloseTarget::Screen { screen } => {
                if !self.catalog.iter().any(|info| info.id.as_str() == screen)
                    && !self.pending_screens.contains_key(&screen)
                {
                    return Err(invalid("close target Screen is gone"));
                }
                self.add_screen_impact(&screen, &mut plan);
            }
            CloseTarget::Window => {
                if !self.catalog_initialized {
                    plan.affected.push(CloseAffected {
                        identity: CloseIdentity::Catalog,
                        busy: false,
                        unresolved: true,
                    });
                }
                let mut screens: Vec<String> = self
                    .catalog
                    .iter()
                    .map(|info| info.id.to_string())
                    .chain(self.pending_screens.keys().cloned())
                    .collect();
                screens.sort();
                screens.dedup();
                for screen in screens {
                    self.add_screen_impact(&screen, &mut plan);
                }
            }
        }
        if let Some(pending) = &self.pending_template {
            plan.affected.push(CloseAffected {
                identity: CloseIdentity::Template(pending.preview.request_id().to_string()),
                busy: false,
                unresolved: true,
            });
        }
        if self.reconciling {
            plan.affected.push(CloseAffected {
                identity: CloseIdentity::Reconciliation,
                busy: false,
                unresolved: true,
            });
        }
        plan.affected.sort_by(|a, b| a.identity.cmp(&b.identity));
        let mut busy_sessions = HashSet::new();
        for part in &plan.affected {
            if !part.busy {
                continue;
            }
            match &part.identity {
                CloseIdentity::Pane {
                    session: Some(session),
                    ..
                }
                | CloseIdentity::Session { session, .. } => {
                    busy_sessions.insert(session);
                }
                _ => {}
            }
        }
        plan.busy_count = busy_sessions.len();
        plan.unresolved_count = plan.affected.iter().filter(|part| part.unresolved).count();
        Ok(plan)
    }

    /// A fresh observation must cover the same identities and must not gain
    /// impact that the user's earlier decision did not include.
    pub fn validate_close(&self, plan: &ClosePlan, confirmed: bool) -> io::Result<()> {
        if plan.source_token() != Some(self.source_token()) {
            return Err(invalid("close belongs to a previous connection"));
        }
        let current = self.assess_close(plan.request_id, plan.target.clone())?;
        if plan.affected.len() != current.affected.len()
            || plan
                .affected
                .iter()
                .zip(&current.affected)
                .any(|(before, now)| {
                    before.identity != now.identity
                        || (now.busy && !before.busy && !before.unresolved)
                        || (now.unresolved && !before.unresolved)
                })
        {
            return Err(invalid("close impact changed; request close again"));
        }
        if current.needs_confirmation() && !confirmed {
            return Err(invalid("close now requires confirmation"));
        }
        Ok(())
    }

    fn add_screen_impact(&self, id: &str, plan: &mut ClosePlan) {
        let model = self.models.get(id);
        let snapshot = model.and_then(ScreenModel::snapshot);
        plan.affected.push(CloseAffected {
            identity: CloseIdentity::Screen(id.to_owned()),
            busy: false,
            unresolved: self.pending_screens.contains_key(id) || snapshot.is_none(),
        });
        let Some(model) = model else {
            return;
        };
        let mut panes: Vec<String> = model
            .pending_panes
            .iter()
            .cloned()
            .chain(model.pane_sessions.keys().cloned())
            .chain(
                snapshot
                    .into_iter()
                    .flat_map(|screen| screen.panes.keys().map(ToString::to_string)),
            )
            .collect();
        panes.sort();
        panes.dedup();
        let mut linked_sessions = HashSet::new();
        for pane in panes {
            Self::add_pane_impact(model, id, &pane, plan);
            if let Some(session) = model
                .snapshot()
                .and_then(|screen| screen.panes.get(&PaneId::from(pane.as_str())))
                .and_then(|pane| pane.session_id.as_ref())
            {
                linked_sessions.insert(session.to_string());
            }
            if let Some(session) = model.pane_sessions.get(&pane) {
                linked_sessions.insert(session.clone());
            }
        }
        if let Some(snapshot) = snapshot {
            for (session_id, session) in &snapshot.sessions {
                if !linked_sessions.contains(session_id.as_str()) {
                    plan.affected.push(CloseAffected {
                        identity: CloseIdentity::Session {
                            screen: id.to_owned(),
                            session: session_id.to_string(),
                        },
                        busy: session.busy && session.status == mux::SessionStatus::Running,
                        unresolved: session.status == mux::SessionStatus::Starting,
                    });
                }
            }
        }
    }

    fn add_pane_impact(
        model: &ScreenModel,
        screen: &str,
        pane: &str,
        plan: &mut ClosePlan,
    ) -> bool {
        let snapshot = model.snapshot();
        let state = snapshot.and_then(|snapshot| snapshot.panes.get(&PaneId::from(pane)));
        let pending = model.pending_panes.contains(pane);
        if state.is_none() && !pending && !model.pane_sessions.contains_key(pane) {
            return false;
        }
        let session = state
            .and_then(|pane| pane.session_id.as_ref().map(ToString::to_string))
            .or_else(|| model.pane_sessions.get(pane).cloned());
        let terminal = pending
            || state.is_none_or(|pane| {
                pane.role
                    .as_ref()
                    .is_none_or(|role| *role == mux::PaneRole::Terminal)
            });
        let metadata = session.as_ref().and_then(|session| {
            snapshot.and_then(|snapshot| snapshot.sessions.get(&SessionId::from(session.as_str())))
        });
        plan.affected.push(CloseAffected {
            identity: CloseIdentity::Pane {
                screen: screen.to_owned(),
                pane: pane.to_owned(),
                session,
            },
            busy: terminal
                && metadata
                    .is_some_and(|entry| entry.busy && entry.status == mux::SessionStatus::Running),
            unresolved: terminal
                && (pending
                    || state.is_none()
                    || metadata.is_none_or(|entry| entry.status == mux::SessionStatus::Starting)),
        });
        true
    }

    pub fn new(connection_id: Uuid) -> Self {
        Self {
            source_id: SourceId::initial(connection_id),
            connection_id,
            policy: CatalogPolicy::Local,
            ready: true,
            ports: None,
            program_status: Default::default(),
            limits: SourceLimits::default(),
            staged: Budget::new(splitty_transport::wire::MAX_FRAME_BYTES),
            catalog: Vec::new(),
            models: HashMap::new(),
            shown: None,
            pending_screens: HashMap::new(),
            remote_creation: None,
            activate_created: None,
            catalog_supported: false,
            screen_placement_supported: false,
            tab_selection_supported: false,
            tab_placement_supported: false,
            pending_placement: None,
            pending_transfer: None,
            pending_floating: None,
            pending_template: None,
            templates_supported: false,
            viewers_supported: false,
            template_failures: HashMap::new(),
            reconciling: false,
            route_generation: 0,
            catalog_initialized: false,
            catalog_dirty: false,
            catalog_in_flight: None,
            mutation: 0,
        }
    }

    pub(super) fn remote(
        source: SourceId,
        connection: Uuid,
        target: String,
        budget: ConnectionBudget,
    ) -> Self {
        let mut workspace = Self::new(connection);
        workspace.source_id = source;
        workspace.policy = CatalogPolicy::Remote {
            target,
            live: None,
            missing: HashSet::new(),
            budget,
        };
        workspace
    }

    pub fn kind(&self) -> SourceKind {
        match self.policy {
            CatalogPolicy::Local => SourceKind::Local,
            CatalogPolicy::Remote { .. } => SourceKind::Remote,
        }
    }
    pub fn target(&self) -> Option<&str> {
        match &self.policy {
            CatalogPolicy::Local => None,
            CatalogPolicy::Remote { target, .. } => Some(target),
        }
    }
    pub fn is_ready(&self) -> bool {
        self.ready
    }
    pub(super) fn replace_catalog_budget(&mut self, next: ConnectionBudget) {
        if let CatalogPolicy::Remote { budget, .. } = &mut self.policy {
            *budget = next;
        }
    }
    /// Remote metadata does not imply an opened model or a session attachment.
    pub fn live_catalog(&self) -> Option<&[ScreenInfo]> {
        match &self.policy {
            CatalogPolicy::Local => self.catalog_initialized.then_some(self.catalog.as_slice()),
            CatalogPolicy::Remote { live, .. } => live.as_ref().map(|live| live.value.as_slice()),
        }
    }
    pub fn is_missing(&self, id: &str) -> bool {
        matches!(&self.policy, CatalogPolicy::Remote { missing, .. } if missing.contains(id))
    }
    fn require_local(&self) -> io::Result<()> {
        if self.kind() != SourceKind::Local {
            return Err(invalid("operation is only available on the local source"));
        }
        Ok(())
    }
    pub(super) fn open_existing(&mut self, id: &str) -> io::Result<()> {
        if self.kind() != SourceKind::Remote || !self.ready {
            return Err(invalid("remote source is not ready"));
        }
        self.require_catalog()?;
        let info = self
            .live_catalog()
            .and_then(|entries| entries.iter().find(|entry| entry.id.as_str() == id))
            .cloned()
            .ok_or_else(|| invalid("Screen is not in the live remote catalog"))?;
        if !self.catalog.iter().any(|entry| entry.id.as_str() == id) {
            if self.catalog.len() >= self.limits.screens {
                return Err(capacity("window Screen limit reached"));
            }
            self.catalog.push(info);
        }
        if let CatalogPolicy::Remote { missing, .. } = &mut self.policy {
            missing.remove(id);
        }
        let source = self.source_id;
        let connection = self.connection_id;
        let tabs = self.tab_selection_supported;
        self.models
            .entry(id.to_owned())
            .or_insert_with(|| Self::existing_screen(source, connection, id, tabs));
        self.shown = Some(id.to_owned());
        Ok(())
    }
    pub(super) fn detach_reference(&mut self, id: &str) -> io::Result<WorkspaceDelta> {
        if self.kind() != SourceKind::Remote {
            return Err(invalid("local Screens cannot be detached"));
        }
        let generation = self
            .route_generation
            .checked_add(1)
            .ok_or_else(|| invalid("route generation exhausted"))?;
        let previous = self.route_map();
        self.catalog.retain(|entry| entry.id.as_str() != id);
        self.models.remove(id);
        if self
            .remote_creation
            .as_ref()
            .is_some_and(|creation| creation.screen_id() == id)
        {
            self.cancel_remote_creation_follow();
            self.remote_creation = None;
        }
        if let CatalogPolicy::Remote { missing, .. } = &mut self.policy {
            missing.remove(id);
        }
        if self.shown.as_deref() == Some(id) {
            self.shown = self.catalog.first().map(|entry| entry.id.to_string());
        }
        let mut delta = route_delta(&previous, &self.route_map());
        delta.snapshot_changed = true;
        self.route_generation = generation;
        Ok(delta)
    }
    fn existing_screen(source: SourceId, connection: Uuid, id: &str, tabs: bool) -> ScreenModel {
        let mut screen = ScreenModel::new(
            ScreenIds {
                screen_id: id.to_owned(),
                pane_id: String::new(),
            },
            connection,
        );
        screen.source_id = source;
        screen.pending_panes.clear();
        screen.focused_pane = None;
        screen.set_tab_selection_supported(tabs);
        screen.resync_pending = true;
        screen
    }
    pub(super) fn select_opened(&mut self, id: &str) -> io::Result<()> {
        if !self.catalog.iter().any(|entry| entry.id.as_str() == id) {
            return Err(invalid("Screen reference is no longer open"));
        }
        self.cancel_navigation_follow();
        self.shown = Some(id.to_owned());
        Ok(())
    }
    pub(super) fn cancel_navigation_follow(&mut self) {
        self.activate_created = None;
        self.cancel_remote_creation_follow();
        self.cancel_placement_focus();
        if let Some(screen) = self.shown.as_ref().and_then(|id| self.models.get_mut(id)) {
            screen.pending_move = None;
        }
    }
    pub(super) fn selection_follow_target(&self) -> Option<&str> {
        self.pending_transfer
            .as_ref()
            .filter(|pending| pending.follow)
            .map(|pending| pending.target.screen_id())
            .or_else(|| self.pending_template().map(|pending| pending.screen_id()))
            .or(self.activate_created.as_deref())
    }
    pub(super) fn screen_count(&self) -> usize {
        let mut ids: HashSet<&str> = self
            .models
            .keys()
            .map(String::as_str)
            .chain(self.catalog.iter().map(|info| info.id.as_str()))
            .collect();
        if let Some(pending) = &self.pending_transfer {
            ids.insert(pending.target.screen_id());
        }
        if let Some(pending) = &self.pending_template {
            ids.insert(pending.preview.screen_id());
        }
        ids.len()
    }

    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn mutation_generation(&self) -> u64 {
        self.mutation
    }
    pub fn catalog_request_generation(&self) -> Option<u64> {
        self.catalog_in_flight
    }
    pub fn screens(&self) -> &[ScreenInfo] {
        &self.catalog
    }
    pub fn shown_screen_id(&self) -> Option<&str> {
        self.shown.as_deref()
    }
    pub fn shown(&self) -> Option<&ScreenModel> {
        self.shown.as_ref().and_then(|id| self.models.get(id))
    }
    pub fn screen(&self, id: &str) -> Option<&ScreenModel> {
        self.models.get(id)
    }
    pub fn snapshot(&self) -> Option<&ScreenState> {
        self.shown().and_then(ScreenModel::snapshot)
    }
    pub fn presentation_root(&self) -> Option<&LayoutNode> {
        self.shown().and_then(ScreenModel::presentation_root)
    }
    pub fn presented_floats(&self) -> &[FloatingEntry] {
        self.shown().map_or(&[], ScreenModel::presented_floats)
    }
    pub fn floating(&self, id: &str) -> Option<&FloatingEntry> {
        self.shown()?.floating(id)
    }
    pub fn floating_for_pane(&self, pane: &str) -> Option<&FloatingEntry> {
        self.shown()?.floating_for_pane(pane)
    }
    pub fn is_root_pane(&self, pane: &str) -> bool {
        self.snapshot()
            .is_some_and(|screen| screen.root.find_slot(&PaneId::from(pane)).is_some())
    }
    pub fn maximized_slot_id(&self) -> Option<&str> {
        self.shown().and_then(ScreenModel::maximized_slot_id)
    }
    pub fn focused_slot_id(&self) -> Option<&str> {
        self.shown().and_then(ScreenModel::focused_slot_id)
    }
    pub fn displayed_panes(&self) -> Vec<String> {
        if self.pending_template().is_some() {
            return Vec::new();
        }
        self.shown()
            .map(ScreenModel::displayed_panes)
            .unwrap_or_default()
    }
    pub fn is_displayed_pane(&self, pane: &str) -> bool {
        if self.pending_template().is_some() {
            return false;
        }
        self.shown()
            .is_some_and(|screen| screen.is_displayed_pane(pane))
    }
    pub fn toggle_maximize_focused(&mut self) -> bool {
        let changed = self
            .shown_mut()
            .is_ok_and(ScreenModel::toggle_maximize_focused);
        if changed {
            self.cancel_placement_focus();
        }
        changed
    }
    pub fn set_maximized_slot(&mut self, slot: Option<&str>, pane: Option<&str>) -> bool {
        let changed = self
            .shown_mut()
            .is_ok_and(|screen| screen.set_maximized_slot(slot, pane));
        if changed {
            self.cancel_placement_focus();
        }
        changed
    }
    pub fn focused_pane(&self) -> Option<&str> {
        self.shown().and_then(ScreenModel::focused_pane)
    }
    pub fn selected_pane<'a>(&'a self, slot: &'a LayoutNode) -> Option<&'a str> {
        self.shown().and_then(|screen| screen.selected_pane(slot))
    }
    pub fn has_pending_create(&self) -> bool {
        !self.pending_screens.is_empty()
            || self
                .remote_creation
                .as_ref()
                .is_some_and(|creation| creation.is_unresolved())
    }
    /// A missing canonical route is conclusive only after the entire opened
    /// collection has settled. A scheduled metadata refresh alone does not
    /// invalidate an existing snapshot. This is not send authority.
    pub(crate) fn route_observation_complete(&self) -> bool {
        self.catalog_initialized
            && !self.reconciling
            && !self.catalog_dirty
            && self.catalog_in_flight.is_none()
            && !self.has_pending_create()
            && self.pending_transfer.is_none()
            && self.pending_floating.is_none()
            && self.pending_template.is_none()
            && self.catalog.iter().all(|entry| {
                self.is_missing(entry.id.as_str())
                    || self.models.get(entry.id.as_str()).is_some_and(|model| {
                        model.snapshot().is_some() && model.pending_panes.is_empty()
                    })
            })
    }
    pub fn catalog_initialized(&self) -> bool {
        self.catalog_initialized
    }
    /// See `ScreenModel::identity_for`.
    pub fn identity_for(&self, pane: &str) -> Option<String> {
        self.models
            .values()
            .find_map(|screen| screen.identity_for(pane))
    }
    pub fn session_for(&self, pane: &str) -> Option<&str> {
        self.models
            .values()
            .find_map(|screen| screen.session_for(pane))
    }
    pub fn pane_for(&self, session: &str) -> Option<&str> {
        self.models
            .values()
            .find_map(|screen| screen.pane_for(session))
    }
    pub fn screen_for(&self, pane: &str) -> Option<&str> {
        self.models
            .values()
            .find(|screen| {
                screen.pending_panes.contains(pane)
                    || screen.pane_sessions.contains_key(pane)
                    || screen
                        .snapshot()
                        .is_some_and(|snapshot| snapshot.panes.contains_key(&PaneId::from(pane)))
            })
            .map(ScreenModel::screen_id)
    }
    pub fn routes(&self) -> Vec<SessionRoute> {
        self.models
            .values()
            .flat_map(|screen| routes(&screen.pane_sessions))
            .collect()
    }
    pub fn session_ready(&self, session: &str) -> bool {
        self.models
            .values()
            .any(|screen| screen.session_ready(session))
    }
    pub fn observe_attached(&mut self, connection: Uuid, session: &str) -> bool {
        self.models
            .values_mut()
            .any(|screen| screen.observe_attached(connection, session))
    }

    pub fn set_catalog_supported(&mut self, supported: bool) {
        self.catalog_supported = supported;
        if !supported {
            self.catalog_dirty = false;
            self.catalog_in_flight = None;
        }
    }
    pub fn set_tab_selection_supported(&mut self, supported: bool) {
        self.tab_selection_supported = supported;
        for screen in self.models.values_mut() {
            screen.set_tab_selection_supported(supported);
        }
    }
    pub fn mark_catalog_dirty(&mut self) {
        self.catalog_dirty = true;
    }

    /// A successful mutation is independent of its retryable resync. Keeping
    /// the dirty flag until admission succeeds prevents losing its reservation.
    pub fn flush_catalog(&mut self, client: &Client) -> io::Result<()> {
        self.check_connection(client)?;
        self.flush_catalog_request(|| client.request_workspace())?;
        for screen in self.models.values_mut() {
            screen.flush_resync(client)?;
        }
        Ok(())
    }

    pub(super) fn flush_catalog_request(
        &mut self,
        send: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if self.catalog_dirty && self.catalog_in_flight.is_none() {
            self.require_catalog()?;
            send()?;
            self.catalog_in_flight = Some(self.mutation);
            self.catalog_dirty = false;
        }
        Ok(())
    }

    pub fn mark_screen_dirty(&mut self, id: &str) {
        if let Some(screen) = self.models.get_mut(id) {
            screen.resync_pending = true;
        }
    }

    /// A `session_metadata` nudge: a program in one of the Screen's panes
    /// changed its title, working directory or foreground process. The local
    /// source reads that Screen again as for any nudge; a host source reads it
    /// alone instead of refreshing all its routes, and falls back to that
    /// refresh when the read shows more than metadata (a pane that closed on
    /// exit sends the same nudge).
    pub fn mark_screen_metadata_dirty(&mut self, id: &str) {
        let remote = self.kind() == SourceKind::Remote;
        if let Some(screen) = self.models.get_mut(id) {
            if remote {
                screen.metadata_pending = true;
            } else {
                screen.resync_pending = true;
            }
        }
    }

    /// Whether a host source's routes are settled, so that one Screen's
    /// metadata may be read again without the coherent refresh: no catalog
    /// read, creation, transfer, floating action, template or tab placement
    /// is waiting for one.
    pub(super) fn metadata_reads_admissible(&self) -> bool {
        self.route_observation_complete() && self.pending_placement.is_none()
    }

    pub fn apply_catalog(&mut self, connection: Uuid, event: &Value) -> io::Result<WorkspaceDelta> {
        let watermark = self.catalog_in_flight;
        let result = self.apply_catalog_inner(connection, event);
        let outcome = self.placement_catalog(connection, watermark, result.as_ref().err());
        let floating = self.floating_catalog(connection, watermark, result.as_ref().err());
        attach_floating_result(result, outcome, floating)
    }

    fn apply_catalog_inner(
        &mut self,
        connection: Uuid,
        event: &Value,
    ) -> io::Result<WorkspaceDelta> {
        if connection != self.connection_id || !self.catalog_supported {
            return Ok(WorkspaceDelta::default());
        }
        let Some(watermark) = self.catalog_in_flight.take() else {
            return Ok(WorkspaceDelta::default());
        };
        if event["type"] != "mux.workspace_snapshot" {
            return Err(invalid("expected workspace catalog"));
        }
        // Validate the complete replacement before changing selection or routes.
        let entries = event["screens"]
            .as_array()
            .ok_or_else(|| invalid("invalid workspace catalog"))?;
        let catalog_limit = if self.kind() == SourceKind::Local {
            self.limits.screens
        } else {
            MAX_REMOTE_CATALOG
        };
        if entries.len() > catalog_limit {
            return Err(capacity("native workspace screen limit reached"));
        }
        let remote_lease = match &self.policy {
            CatalogPolicy::Remote { budget, .. } => {
                let bytes = splitty_transport::codec::json_size(
                    &event["screens"],
                    splitty_core::native_api::MAX_API_BODY,
                )?;
                Some(
                    budget
                        .reserve(bytes.saturating_mul(8), false)
                        .map_err(io::Error::other)?,
                )
            }
            CatalogPolicy::Local => None,
        };
        let mut catalog = Vec::with_capacity(entries.len());
        let mut ids = HashSet::new();
        for value in entries {
            let info = ScreenInfo::deserialize(value).map_err(io::Error::other)?;
            identifier(info.id.as_str())?;
            if info.name.as_ref().is_some_and(|name| name.len() > 256) {
                return Err(invalid("screen name too long"));
            }
            if !ids.insert(info.id.to_string()) {
                return Err(invalid("duplicate workspace screen"));
            }
            catalog.push(info);
        }
        if self.kind() == SourceKind::Remote {
            return self.apply_remote_catalog(
                remote_lease.expect("remote catalog lease").charge(catalog),
                watermark,
            );
        }
        let mut reserved: HashSet<&str> = ids.iter().map(String::as_str).collect();
        reserved.extend(
            self.pending_screens
                .iter()
                .filter(|(_, pending)| pending.mutation > watermark)
                .map(|(id, _)| id.as_str()),
        );
        if let Some(pending) = &self.pending_transfer {
            reserved.insert(pending.target.screen_id());
        }
        if let Some(pending) = &self.pending_template {
            reserved.insert(pending.preview.screen_id());
        }
        if reserved.len() > self.limits.screens {
            return Err(capacity("native workspace screen limit reached"));
        }
        let old_routes = self.route_map();
        let old_shown = self.shown.clone();
        let was_initialized = self.catalog_initialized;
        let changed_catalog = self
            .catalog
            .iter()
            .map(|screen| (&screen.id, &screen.name))
            .ne(catalog.iter().map(|screen| (&screen.id, &screen.name)));
        self.models.retain(|id, _| {
            ids.contains(id)
                || self
                    .pending_screens
                    .get(id)
                    .is_some_and(|pending| pending.mutation > watermark)
        });
        self.pending_screens
            .retain(|id, pending| !ids.contains(id) && pending.mutation > watermark);
        for info in &catalog {
            self.models.entry(info.id.to_string()).or_insert_with(|| {
                Self::existing_screen(
                    self.source_id,
                    self.connection_id,
                    info.id.as_str(),
                    self.tab_selection_supported,
                )
            });
        }
        if !self.catalog_initialized {
            self.shown = event["activeScreenId"]
                .as_str()
                .filter(|id| ids.contains(*id))
                .map(str::to_owned)
                .or_else(|| catalog.first().map(|screen| screen.id.to_string()));
        }
        if let Some(created) = self.activate_created.as_ref() {
            if ids.contains(created) {
                self.shown = self.activate_created.take();
            } else if !self.pending_screens.contains_key(created) {
                self.activate_created = None;
            }
        }
        if self.shown.as_ref().is_none_or(|id| !ids.contains(id)) {
            self.shown = catalog.first().map(|screen| screen.id.to_string());
        }
        self.catalog = catalog;
        self.catalog_initialized = true;
        self.catalog_in_flight = None;
        let mut delta = route_delta(&old_routes, &self.route_map());
        delta.snapshot_changed = !was_initialized
            || changed_catalog
            || old_shown != self.shown
            || !delta.removed.is_empty();
        Ok(delta)
    }

    fn apply_remote_catalog(
        &mut self,
        catalog: Charged<Vec<ScreenInfo>>,
        watermark: u64,
    ) -> io::Result<WorkspaceDelta> {
        let previous = self.route_map();
        let old_shown = self.shown.clone();
        let newer_creation = self
            .remote_creation
            .as_ref()
            .filter(|creation| self.remote_creation_newer_than(creation.screen_id(), watermark))
            .map(|creation| creation.screen_id().to_owned());
        // The Screen a pending move or template makes is not on the host
        // until that is done.
        let moving_to = self
            .pending_transfer
            .as_ref()
            .map(|pending| pending.target.screen_id().to_owned());
        let template_for = self
            .pending_template
            .as_ref()
            .map(|pending| pending.preview.screen_id().to_owned());
        let CatalogPolicy::Remote { live, missing, .. } = &mut self.policy else {
            unreachable!()
        };
        missing.clear();
        for opened in &mut self.catalog {
            let id = opened.id.as_str();
            if let Some(current) = catalog.iter().find(|entry| entry.id == opened.id) {
                opened.name.clone_from(&current.name);
                self.models.entry(id.to_owned()).or_insert_with(|| {
                    Self::existing_screen(
                        self.source_id,
                        self.connection_id,
                        id,
                        self.tab_selection_supported,
                    )
                });
            } else if newer_creation.as_deref() != Some(id)
                && moving_to.as_deref() != Some(id)
                && template_for.as_deref() != Some(id)
            {
                missing.insert(id.to_owned());
                self.models.remove(id);
            }
        }
        *live = Some(Arc::new(catalog));
        self.catalog_initialized = true;
        let mut delta = route_delta(&previous, &self.route_map());
        delta.snapshot_changed = true;
        // No host activation hint, catalog insertion or disappearance changes
        // the window's explicitly chosen reference, including a missing one.
        debug_assert_eq!(self.shown, old_shown);
        Ok(delta)
    }

    pub fn apply_snapshot(
        &mut self,
        connection: Uuid,
        event: &Value,
    ) -> io::Result<WorkspaceDelta> {
        let result = self.apply_snapshot_inner(connection, event);
        let outcome = self.placement_snapshot(connection, event, result.as_ref());
        let floating = self.floating_snapshot(connection, event, result.as_ref());
        attach_floating_result(result, outcome, floating)
    }

    fn apply_snapshot_inner(
        &mut self,
        connection: Uuid,
        event: &Value,
    ) -> io::Result<WorkspaceDelta> {
        if connection != self.connection_id {
            return Ok(WorkspaceDelta::default());
        }
        let Some(id) = event["screen"]["id"].as_str() else {
            return Err(invalid("screen snapshot missing ID"));
        };
        // A submitted creation gets route authority only from the complete collector.
        if self
            .remote_creation
            .as_ref()
            .is_some_and(|creation| creation.is_unresolved() && creation.screen_id() == id)
        {
            return Ok(WorkspaceDelta::default());
        }
        let Some(existing) = self.models.get(id) else {
            return Ok(WorkspaceDelta::default());
        };
        let mut next = existing.clone();
        let delta = next.apply_snapshot(connection, event)?;
        self.check_candidate(id, &next)?;
        self.models.insert(id.to_owned(), next);
        Ok(delta)
    }

    pub fn observe_spawn(&mut self, connection: Uuid, pane: &str, session: &str) -> WorkspaceDelta {
        if connection != self.connection_id
            || self
                .pane_for(session)
                .is_some_and(|expected| expected != pane)
        {
            return WorkspaceDelta::default();
        }
        let Some(id) = self.screen_for(pane).map(str::to_owned) else {
            return WorkspaceDelta::default();
        };
        if self
            .remote_creation
            .as_ref()
            .is_some_and(|creation| creation.is_unresolved() && creation.screen_id() == id)
        {
            return WorkspaceDelta::default();
        }
        self.models
            .get_mut(&id)
            .map(|screen| screen.observe_spawn(connection, pane, session))
            .unwrap_or_default()
    }
    pub fn observe_spawn_failure(&mut self, connection: Uuid, pane: &str) -> WorkspaceDelta {
        let Some(id) = self.screen_for(pane).map(str::to_owned) else {
            return WorkspaceDelta::default();
        };
        let delta = self
            .models
            .get_mut(&id)
            .map(|screen| screen.observe_spawn_failure(connection, pane))
            .unwrap_or_default();
        if connection == self.connection_id {
            self.mark_catalog_dirty();
        }
        delta
    }
    pub fn reset_connection(&mut self, connection: Uuid) -> WorkspaceDelta {
        let removed = self.routes();
        self.remote_creation_lost();
        let remote_creation = self.remote_creation.take();
        let source = self.source_id;
        let policy = self.policy.clone();
        let opened = self.catalog.clone();
        let shown = self.shown.clone();
        let mut screens = std::mem::take(&mut self.models);
        let limits = self.limits;
        let staged = self.staged.clone();
        *self = match policy {
            CatalogPolicy::Local => Self::new(connection),
            CatalogPolicy::Remote { target, budget, .. } => {
                let mut next = Self::remote(source, connection, target, budget);
                next.remote_creation = remote_creation;
                next.catalog = opened;
                next.shown = shown;
                // Reuse the canonical model's existing reset boundary: it
                // revokes snapshots, routes and pending operations while
                // retaining only per-window tab/focus choices. Fresh canonical
                // snapshots revalidate those choices before they are visible.
                for screen in screens.values_mut() {
                    screen.reset_connection(connection);
                    screen.set_tab_selection_supported(false);
                    screen.resync_pending = false;
                    screen.metadata_pending = false;
                }
                next.models = screens;
                next
            }
        };
        self.source_id = source;
        self.limits = limits;
        self.staged = staged;
        WorkspaceDelta {
            snapshot_changed: true,
            removed,
            ..WorkspaceDelta::default()
        }
    }

    pub fn create_screen(&mut self, client: &Client, name: &str) -> io::Result<ScreenIds> {
        self.require_local()?;
        self.require_routes_settled()?;
        self.check_connection(client)?;
        self.require_catalog()?;
        if !self.catalog_initialized {
            return Err(capacity("workspace catalog is not loaded"));
        }
        self.admit_new_pane()?;
        if self.screen_count() >= self.limits.screens {
            return Err(capacity("native workspace screen limit reached"));
        }
        let mutation = self.next_mutation()?;
        let ids = ScreenIds {
            screen_id: Uuid::new_v4().to_string(),
            pane_id: Uuid::new_v4().to_string(),
        };
        let mut screen = ScreenModel::new(ids.clone(), self.connection_id);
        screen.source_id = self.source_id;
        screen.set_tab_selection_supported(self.tab_selection_supported);
        screen.resync_pending = true;
        self.models.insert(ids.screen_id.clone(), screen);
        if let Err(error) = client.create_screen_with_ids(&ids, name) {
            self.models.remove(&ids.screen_id);
            return Err(error);
        }
        self.cancel_placement_focus();
        self.mutation = mutation;
        self.pending_screens
            .insert(ids.screen_id.clone(), PendingScreen { mutation });
        self.activate_created = Some(ids.screen_id.clone());
        self.mark_catalog_dirty();
        Ok(ids)
    }
    pub fn switch_screen(&mut self, client: &Client, id: &str) -> io::Result<bool> {
        self.check_known(client, id)?;
        self.cancel_placement_focus();
        if self.shown.as_deref() == Some(id) {
            self.activate_created = None;
            return Ok(false);
        }
        let mutation = self.next_mutation()?;
        if self.kind() == SourceKind::Local {
            client.set_active_screen(id)?;
        }
        self.activate_created = None;
        if let Some(old) = self.shown.as_ref().and_then(|id| self.models.get_mut(id)) {
            old.pending_move = None;
        }
        self.cancel_placement_focus();
        self.shown = Some(id.to_owned());
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(true)
    }
    /// Resolve each relative intent against the latest local selection. The
    /// connection capture prevents queued input from navigating a replacement.
    pub fn cycle_screen(
        &mut self,
        client: &Client,
        connection: Uuid,
        step: TabStep,
    ) -> io::Result<bool> {
        if connection != self.connection_id {
            return Err(invalid(
                "Screen navigation belongs to a previous connection",
            ));
        }
        self.check_connection(client)?;
        self.require_catalog()?;
        let Some(shown) = self.shown_screen_id() else {
            return Ok(false);
        };
        let index = self
            .catalog
            .iter()
            .position(|screen| screen.id.as_str() == shown)
            .ok_or_else(|| invalid("selected Screen is no longer in the workspace"))?;
        let next = match step {
            TabStep::Previous => index.saturating_sub(1),
            TabStep::Next => (index + 1).min(self.catalog.len() - 1),
        };
        if next == index {
            return Ok(false);
        }
        let target = self.catalog[next].id.to_string();
        self.switch_screen(client, &target)
    }
    /// Check that an editor still belongs to this current, canonically present Screen.
    /// Transient mutation barriers do not discard its input; commit checks them below.
    pub fn validate_screen_rename_target(&self, id: &str) -> io::Result<()> {
        if !self.ready {
            return Err(invalid("source connection is not ready"));
        }
        self.require_catalog()?;
        if !self.catalog.iter().any(|screen| screen.id.as_str() == id) {
            return Err(invalid("screen is no longer in the workspace"));
        }
        if self.kind() == SourceKind::Remote
            && (self.is_missing(id)
                || self.screen(id).is_none()
                || self
                    .live_catalog()
                    .is_none_or(|live| !live.iter().any(|screen| screen.id.as_str() == id)))
        {
            return Err(invalid("Screen is not present in the live remote catalog"));
        }
        Ok(())
    }
    /// Check a rename without selecting a Screen or reserving mutation state.
    pub fn validate_screen_rename(&self, id: &str) -> io::Result<()> {
        self.require_routes_settled()?;
        self.validate_screen_rename_target(id)
    }
    pub fn validate_screen_rename_input(&self, id: &str, name: &str) -> io::Result<()> {
        self.validate_screen_rename(id)?;
        crate::connection::validate_rename_name(name)
    }
    pub fn rename_screen(&mut self, client: &Client, id: &str, name: &str) -> io::Result<()> {
        self.validate_screen_rename_input(id, name)?;
        self.check_known(client, id)?;
        let mutation = self.next_mutation()?;
        client.rename_screen(id, name)?;
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(())
    }
    /// Whether the Screen's confirmed snapshot holds no pane at all, in its
    /// layout or floating.
    pub fn screen_ran_empty(&self, id: &str) -> bool {
        self.screen(id)
            .and_then(ScreenModel::server_snapshot)
            .is_some_and(|screen| screen.panes.is_empty())
    }

    pub fn close_screen(&mut self, client: &Client, id: &str) -> io::Result<()> {
        self.require_routes_settled()?;
        self.check_known(client, id)?;
        let mutation = self.next_mutation()?;
        client.close_screen(id)?;
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(())
    }

    pub fn select_tab(&mut self, pane: &str) -> bool {
        self.cancel_placement_focus();
        self.shown_mut().is_ok_and(|screen| screen.select_tab(pane))
    }
    pub fn focus_pane(&mut self, pane: &str) -> bool {
        self.cancel_placement_focus();
        self.shown_mut().is_ok_and(|screen| screen.focus_pane(pane))
    }
    pub fn sync_selected_tabs(&mut self, client: &Client) -> io::Result<()> {
        if self.reconciling {
            return Ok(());
        }
        if self.shown.is_none() {
            return Ok(());
        }
        self.shown_mut()?.sync_selected_tabs(client)
    }
    pub fn split(&mut self, client: &Client, pane: &str, axis: Axis) -> io::Result<String> {
        self.require_routes_settled()?;
        self.admit_new_pane()?;
        let result = self.shown_mut()?.split(client, pane, axis);
        if result.is_ok() {
            self.cancel_placement_focus();
        }
        result
    }
    pub fn new_tab(&mut self, client: &Client, pane: &str) -> io::Result<String> {
        self.require_routes_settled()?;
        self.admit_new_pane()?;
        let result = self.shown_mut()?.new_tab(client, pane);
        if result.is_ok() {
            self.cancel_placement_focus();
        }
        result
    }
    pub fn new_tab_with_profile(
        &mut self,
        client: &Client,
        pane: &str,
        profile_id: &str,
    ) -> io::Result<String> {
        self.require_local()?;
        self.require_routes_settled()?;
        self.admit_new_pane()?;
        let result = self
            .shown_mut()?
            .new_tab_with_profile(client, pane, Some(profile_id));
        if result.is_ok() {
            self.cancel_placement_focus();
        }
        result
    }
    /// Start a terminal in the shown Screen when it has no pane. A host's
    /// Screen takes it as a local one does: the Screen's own service starts
    /// the session.
    pub fn new_terminal(&mut self, client: &Client) -> io::Result<String> {
        self.require_routes_settled()?;
        self.admit_new_pane()?;
        let result = self.shown_mut()?.new_terminal(client);
        if result.is_ok() {
            self.cancel_placement_focus();
        }
        result
    }
    pub fn close_pane(&mut self, client: &Client, pane: &str) -> io::Result<()> {
        self.require_routes_settled()?;
        self.shown_mut()?.close_pane(client, pane)
    }
    /// Execute a captured close against its original Screen.
    pub fn close_pane_on_screen(
        &mut self,
        client: &Client,
        screen: &str,
        pane: &str,
    ) -> io::Result<()> {
        self.require_routes_settled()?;
        let mutation = self.next_mutation()?;
        self.models
            .get_mut(screen)
            .ok_or_else(|| invalid("close target Screen is gone"))?
            .close_pane(client, pane)?;
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(())
    }
    pub fn resize_split(&mut self, client: &Client, node: &str, sizes: Vec<f64>) -> io::Result<()> {
        self.require_routes_settled()?;
        self.shown_mut()?.resize_split(client, node, sizes)
    }
    pub fn set_scratchpad(
        &mut self,
        client: &Client,
        screen: &str,
        session: &str,
        text: String,
    ) -> io::Result<()> {
        self.check_known(client, screen)?;
        self.models
            .get_mut(screen)
            .ok_or_else(|| invalid("scratchpad target Screen is gone"))?
            .set_scratchpad(client, session, text)
    }
    /// Send an operation on a viewer pane's annotations to the pane's Screen.
    pub(crate) fn annotation_op(
        &mut self,
        client: &Client,
        screen: &str,
        op: &ScreenOp,
    ) -> io::Result<()> {
        self.check_known(client, screen)?;
        self.models
            .get_mut(screen)
            .ok_or_else(|| invalid("The document's Screen is gone"))?
            .send_op(client, op)
    }
    pub fn capture_drag(&self, pane: &str) -> Option<PaneDrag> {
        self.shown()?.capture_drag(pane)
    }
    pub fn can_move(&self, drag: &PaneDrag, target: &str, placement: MovePlacement) -> bool {
        !self.reconciling
            && self.pending_floating.is_none()
            && self.pending_placement.is_none()
            && self
                .shown()
                .is_some_and(|screen| screen.can_move(drag, target, placement))
    }
    pub fn check_move(
        &self,
        drag: &PaneDrag,
        target: &str,
        placement: MovePlacement,
    ) -> io::Result<bool> {
        self.require_no_placement()?;
        self.shown()
            .ok_or_else(|| invalid("no screen selected"))?
            .check_move(drag, target, placement)
    }
    pub fn move_pane(
        &mut self,
        client: &Client,
        drag: PaneDrag,
        target: String,
        placement: MovePlacement,
    ) -> io::Result<bool> {
        self.require_no_placement()?;
        self.shown_mut()?.move_pane(client, drag, target, placement)
    }
    pub fn reject_move(&mut self, connection: Uuid) {
        for screen in self.models.values_mut() {
            screen.reject_move(connection);
        }
    }
    /// A rejected divider change returns its screen to the service's layout.
    pub fn reject_resize(&mut self, connection: Uuid, event: &Value) {
        if connection != self.connection_id {
            return;
        }
        if let Some(screen) = event["screenId"]
            .as_str()
            .and_then(|id| self.models.get_mut(id))
        {
            screen.drop_preview();
        }
    }

    fn shown_mut(&mut self) -> io::Result<&mut ScreenModel> {
        self.shown
            .as_ref()
            .and_then(|id| self.models.get_mut(id))
            .ok_or_else(|| invalid("no screen selected"))
    }
    fn check_connection(&self, client: &Client) -> io::Result<()> {
        if !self.ready || self.connection_id != client.connection_id() {
            return Err(invalid("workspace connection changed"));
        }
        Ok(())
    }
    fn require_catalog(&self) -> io::Result<()> {
        if !self.catalog_supported {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "service does not support workspace-catalog-v1",
            ));
        }
        Ok(())
    }
    fn check_known(&self, client: &Client, id: &str) -> io::Result<()> {
        self.check_connection(client)?;
        self.require_catalog()?;
        if !self.catalog.iter().any(|screen| screen.id.as_str() == id) {
            return Err(invalid("screen is no longer in the workspace"));
        }
        Ok(())
    }
    fn next_mutation(&self) -> io::Result<u64> {
        self.mutation
            .checked_add(1)
            .ok_or_else(|| invalid("workspace mutation generation exhausted"))
    }
    fn route_map(&self) -> HashMap<String, String> {
        self.models
            .values()
            .flat_map(|screen| {
                screen
                    .pane_sessions
                    .iter()
                    .map(|(pane, session)| (pane.clone(), session.clone()))
            })
            .collect()
    }
    pub(super) fn admission_count(&self) -> usize {
        let current = self.models.values().map(screen_admission).sum::<usize>()
            + self.remote_creation_reservation();
        self.template_reserved_admission()
            .map_or(current, |(screen, panes)| {
                let other: usize = self
                    .models
                    .iter()
                    .filter(|(id, _)| id.as_str() != screen)
                    .map(|(_, model)| screen_admission(model))
                    .sum();
                current.max(other + panes)
            })
    }
    fn admit_new_pane(&self) -> io::Result<()> {
        if self.admission_count() >= self.limits.panes {
            return Err(capacity("native workspace attachment limit reached"));
        }
        Ok(())
    }
    fn check_candidate(&self, id: &str, candidate: &ScreenModel) -> io::Result<()> {
        let others = self
            .models
            .iter()
            .filter(|(other, _)| other.as_str() != id)
            .map(|(_, screen)| screen);
        let mut panes = HashSet::new();
        let mut sessions = HashSet::new();
        let mut count = 0;
        for screen in others.chain(std::iter::once(candidate)) {
            count += screen_admission(screen);
            let screen_panes: HashSet<&str> = screen
                .pending_panes
                .iter()
                .map(String::as_str)
                .chain(screen.pane_sessions.keys().map(String::as_str))
                .chain(
                    screen
                        .snapshot()
                        .into_iter()
                        .flat_map(|snapshot| snapshot.panes.keys().map(PaneId::as_str)),
                )
                .collect();
            for pane in screen_panes {
                if !panes.insert(pane) {
                    return Err(invalid("pane belongs to another screen"));
                }
            }
            for session in screen.pane_sessions.values() {
                if !sessions.insert(session.as_str()) {
                    return Err(invalid("session route belongs to another screen"));
                }
            }
        }
        if self
            .remote_creation
            .as_ref()
            .is_some_and(|creation| creation.is_unresolved())
        {
            let reserved = self.remote_creation.as_ref().expect("checked creation");
            // Pending and canonical IDs were collected together above; do not count both.
            if !panes.contains(reserved.initial_pane_id()) {
                count += 1;
            }
        }
        if let Some((target, reserved)) = self.template_reserved_admission() {
            let present = if target == id {
                screen_admission(candidate)
            } else {
                self.models.get(target).map_or(0, screen_admission)
            };
            count = count.max(count - present + reserved);
        }
        if count > self.limits.panes {
            return Err(capacity("native workspace attachment limit reached"));
        }
        Ok(())
    }
}

fn screen_admission(screen: &ScreenModel) -> usize {
    screen
        .snapshot()
        .map_or(screen.session_panes.len(), |snapshot| {
            snapshot
                .panes
                .len()
                .max(snapshot.sessions.len())
                .max(screen.session_panes.len())
        })
        + screen
            .pending_panes
            .iter()
            .filter(|pane| {
                !screen.pane_sessions.contains_key(*pane)
                    && !screen.snapshot().is_some_and(|snapshot| {
                        snapshot.panes.contains_key(&PaneId::from(pane.as_str()))
                    })
            })
            .count()
}
fn capacity(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, message)
}

#[cfg(test)]
mod tests;

fn attach_floating_result(
    result: io::Result<WorkspaceDelta>,
    placement: Option<PlacementOutcome>,
    floating: Option<FloatingActionResult>,
) -> io::Result<WorkspaceDelta> {
    match result {
        Ok(mut delta) => {
            delta.placement = placement;
            delta.floating = floating;
            Ok(delta)
        }
        Err(error) if floating.is_some() => Ok(WorkspaceDelta {
            snapshot_changed: true,
            floating,
            reconciliation_error: Some(error.to_string()),
            ..WorkspaceDelta::default()
        }),
        Err(error) => Err(error),
    }
}
