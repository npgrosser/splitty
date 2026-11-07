//! Connection-wide catalog, admission and local Screen selection.
use super::*;
use crate::connection::identifier;

/// Empty Screens consume model capacity independently of session attachments.
pub const MAX_LOCAL_SCREENS: usize = 128;

#[derive(Clone)]
struct PendingScreen {
    mutation: u64,
}

/// One connection owns every screen model and the shared pending-pane budget.
#[derive(Clone)]
pub struct LocalWorkspace {
    connection_id: Uuid,
    catalog: Vec<ScreenInfo>,
    models: HashMap<String, ScreenModel>,
    shown: Option<String>,
    pending_screens: HashMap<String, PendingScreen>,
    activate_created: Option<String>,
    catalog_supported: bool,
    tab_selection_supported: bool,
    catalog_initialized: bool,
    catalog_dirty: bool,
    catalog_in_flight: Option<u64>,
    mutation: u64,
}

impl LocalWorkspace {
    /// Assess only local terminal impact, retaining pane and session identity
    /// across a later prompt. The existing admission bounds limit this plan.
    pub fn assess_close(&self, request_id: u64, target: CloseTarget) -> io::Result<ClosePlan> {
        let mut plan = ClosePlan {
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
        if plan.connection_id != Some(self.connection_id) {
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
            connection_id,
            catalog: Vec::new(),
            models: HashMap::new(),
            shown: None,
            pending_screens: HashMap::new(),
            activate_created: None,
            catalog_supported: false,
            tab_selection_supported: false,
            catalog_initialized: false,
            catalog_dirty: false,
            catalog_in_flight: None,
            mutation: 0,
        }
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
    pub fn focused_pane(&self) -> Option<&str> {
        self.shown().and_then(ScreenModel::focused_pane)
    }
    pub fn selected_pane<'a>(&'a self, slot: &'a LayoutNode) -> Option<&'a str> {
        self.shown().and_then(|screen| screen.selected_pane(slot))
    }
    pub fn has_pending_create(&self) -> bool {
        !self.pending_screens.is_empty()
    }
    pub fn catalog_initialized(&self) -> bool {
        self.catalog_initialized
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

    fn flush_catalog_request(&mut self, send: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
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

    pub fn apply_catalog(&mut self, connection: Uuid, event: &Value) -> io::Result<WorkspaceDelta> {
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
        if entries.len() > MAX_LOCAL_SCREENS {
            return Err(capacity("native workspace screen limit reached"));
        }
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
        let retained_pending = self
            .pending_screens
            .iter()
            .filter(|(id, pending)| pending.mutation > watermark && !ids.contains(*id))
            .count();
        if catalog.len() + retained_pending > MAX_LOCAL_SCREENS {
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
                let mut screen = ScreenModel::new(
                    ScreenIds {
                        screen_id: info.id.to_string(),
                        pane_id: String::new(),
                    },
                    self.connection_id,
                );
                screen.pending_panes.clear();
                screen.focused_pane = None;
                screen.set_tab_selection_supported(self.tab_selection_supported);
                screen.resync_pending = true;
                screen
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

    pub fn apply_snapshot(
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
        *self = Self::new(connection);
        WorkspaceDelta {
            snapshot_changed: true,
            removed,
            ..WorkspaceDelta::default()
        }
    }

    pub fn create_screen(&mut self, client: &Client, name: &str) -> io::Result<ScreenIds> {
        self.check_connection(client)?;
        self.require_catalog()?;
        if !self.catalog_initialized {
            return Err(capacity("workspace catalog is not loaded"));
        }
        self.admit_new_pane()?;
        if self.models.len() >= MAX_LOCAL_SCREENS {
            return Err(capacity("native workspace screen limit reached"));
        }
        let mutation = self.next_mutation()?;
        let ids = ScreenIds {
            screen_id: Uuid::new_v4().to_string(),
            pane_id: Uuid::new_v4().to_string(),
        };
        let mut screen = ScreenModel::new(ids.clone(), self.connection_id);
        screen.set_tab_selection_supported(self.tab_selection_supported);
        screen.resync_pending = true;
        self.models.insert(ids.screen_id.clone(), screen);
        if let Err(error) = client.create_screen_with_ids(&ids, name) {
            self.models.remove(&ids.screen_id);
            return Err(error);
        }
        self.mutation = mutation;
        self.pending_screens
            .insert(ids.screen_id.clone(), PendingScreen { mutation });
        self.activate_created = Some(ids.screen_id.clone());
        self.mark_catalog_dirty();
        Ok(ids)
    }
    pub fn switch_screen(&mut self, client: &Client, id: &str) -> io::Result<bool> {
        self.check_known(client, id)?;
        if self.shown.as_deref() == Some(id) {
            self.activate_created = None;
            return Ok(false);
        }
        let mutation = self.next_mutation()?;
        client.set_active_screen(id)?;
        self.activate_created = None;
        if let Some(old) = self.shown.as_ref().and_then(|id| self.models.get_mut(id)) {
            old.pending_move = None;
        }
        self.shown = Some(id.to_owned());
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(true)
    }
    pub fn rename_screen(&mut self, client: &Client, id: &str, name: &str) -> io::Result<()> {
        self.check_known(client, id)?;
        let mutation = self.next_mutation()?;
        client.rename_screen(id, name)?;
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(())
    }
    pub fn close_screen(&mut self, client: &Client, id: &str) -> io::Result<()> {
        self.check_known(client, id)?;
        let mutation = self.next_mutation()?;
        client.close_screen(id)?;
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(())
    }

    pub fn select_tab(&mut self, pane: &str) -> bool {
        self.shown_mut().is_ok_and(|screen| screen.select_tab(pane))
    }
    pub fn focus_pane(&mut self, pane: &str) -> bool {
        self.shown_mut().is_ok_and(|screen| screen.focus_pane(pane))
    }
    pub fn sync_selected_tabs(&mut self, client: &Client) -> io::Result<()> {
        if self.shown.is_none() {
            return Ok(());
        }
        self.shown_mut()?.sync_selected_tabs(client)
    }
    pub fn split(&mut self, client: &Client, pane: &str, axis: Axis) -> io::Result<String> {
        self.admit_new_pane()?;
        self.shown_mut()?.split(client, pane, axis)
    }
    pub fn new_tab(&mut self, client: &Client, pane: &str) -> io::Result<String> {
        self.admit_new_pane()?;
        self.shown_mut()?.new_tab(client, pane)
    }
    pub fn new_terminal(&mut self, client: &Client) -> io::Result<String> {
        self.admit_new_pane()?;
        self.shown_mut()?.new_terminal(client)
    }
    pub fn close_pane(&mut self, client: &Client, pane: &str) -> io::Result<()> {
        self.shown_mut()?.close_pane(client, pane)
    }
    /// Execute a captured close against its original Screen.
    pub fn close_pane_on_screen(
        &mut self,
        client: &Client,
        screen: &str,
        pane: &str,
    ) -> io::Result<()> {
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
        self.shown_mut()?.resize_split(client, node, sizes)
    }
    pub fn capture_drag(&self, pane: &str) -> Option<PaneDrag> {
        self.shown()?.capture_drag(pane)
    }
    pub fn can_move(&self, drag: &PaneDrag, target: &str, placement: MovePlacement) -> bool {
        self.shown()
            .is_some_and(|screen| screen.can_move(drag, target, placement))
    }
    pub fn check_move(
        &self,
        drag: &PaneDrag,
        target: &str,
        placement: MovePlacement,
    ) -> io::Result<bool> {
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
        self.shown_mut()?.move_pane(client, drag, target, placement)
    }
    pub fn reject_move(&mut self, connection: Uuid) {
        for screen in self.models.values_mut() {
            screen.reject_move(connection);
        }
    }

    fn shown_mut(&mut self) -> io::Result<&mut ScreenModel> {
        self.shown
            .as_ref()
            .and_then(|id| self.models.get_mut(id))
            .ok_or_else(|| invalid("no screen selected"))
    }
    fn check_connection(&self, client: &Client) -> io::Result<()> {
        if self.connection_id != client.connection_id() {
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
    fn admission_count(&self) -> usize {
        self.models.values().map(screen_admission).sum()
    }
    fn admit_new_pane(&self) -> io::Result<()> {
        if self.admission_count() >= MAX_ATTACHMENTS {
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
        if count > MAX_ATTACHMENTS {
            return Err(capacity("native workspace attachment limit reached"));
        }
        Ok(())
    }
}

fn screen_admission(screen: &ScreenModel) -> usize {
    screen.session_panes.len()
        + screen
            .pending_panes
            .iter()
            .filter(|pane| !screen.pane_sessions.contains_key(*pane))
            .count()
}
fn capacity(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, message)
}

#[cfg(test)]
mod tests;
