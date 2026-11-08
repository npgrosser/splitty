//! Explicit remote creation retains observation intent, never a retryable send.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// One deliberate group action. Clones share only revocable navigation permission.
#[derive(Clone, Debug)]
pub struct RemoteScreenCreateRequest {
    id: Uuid,
    target: HostActionTarget,
    source: SourceToken,
    follow: Arc<AtomicBool>,
}
impl RemoteScreenCreateRequest {
    pub(crate) fn new(target: HostActionTarget, source: SourceToken) -> Self {
        Self {
            id: Uuid::new_v4(),
            target,
            source,
            follow: Arc::new(AtomicBool::new(true)),
        }
    }
    pub fn request_id(&self) -> Uuid {
        self.id
    }
    pub fn target(&self) -> &HostActionTarget {
        &self.target
    }
    pub fn source(&self) -> SourceToken {
        self.source
    }
    /// Revoke only this request's navigation permission, even when the worker queue is full.
    pub fn cancel_follow(&self) {
        self.follow.store(false, Ordering::Release);
    }
    pub fn is_follow_requested(&self) -> bool {
        self.follow.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteScreenCreateStatus {
    Pending,
    Uncertain,
    Present,
    Missing,
    Rejected,
}

/// Latest bounded creation observation for a source; current presence is not a wire acknowledgement.
#[derive(Clone, Debug)]
pub struct RemoteScreenCreation {
    request: RemoteScreenCreateRequest,
    ids: ScreenIds,
    mutation: Option<u64>,
    status: RemoteScreenCreateStatus,
    unresolved: bool,
    uncertain: bool,
    reason: Option<String>,
}
impl RemoteScreenCreation {
    pub fn request_id(&self) -> Uuid {
        self.request.request_id()
    }
    pub(super) fn initial_pane_id(&self) -> &str {
        &self.ids.pane_id
    }
    pub fn screen_id(&self) -> &str {
        &self.ids.screen_id
    }
    pub fn status(&self) -> RemoteScreenCreateStatus {
        self.status
    }
    pub fn is_unresolved(&self) -> bool {
        self.unresolved
    }
    pub fn was_uncertain(&self) -> bool {
        self.uncertain
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}
impl SourceWorkspace {
    pub fn remote_screen_creation(&self) -> Option<&RemoteScreenCreation> {
        self.remote_creation.as_ref()
    }

    pub(crate) fn remote_creation_available(&self) -> io::Result<()> {
        if self.kind() != SourceKind::Remote {
            return Err(invalid("Creation requires a remote host group"));
        }
        self.require_routes_settled()?;
        self.require_catalog()?;
        if !self.catalog_initialized || self.live_catalog().is_none() || self.catalog.is_empty() {
            return Err(invalid(
                "An opened host group and live catalog are required",
            ));
        }
        if self
            .remote_creation
            .as_ref()
            .is_some_and(|creation| creation.unresolved)
        {
            return Err(capacity(
                "A Screen creation is still unresolved on this host",
            ));
        }
        if self
            .live_catalog()
            .is_some_and(|catalog| catalog.len() >= MAX_REMOTE_CATALOG)
        {
            return Err(capacity("native workspace Screen limit reached"));
        }
        Ok(())
    }

    pub(crate) fn remote_creation_is_request(&self, request: &RemoteScreenCreateRequest) -> bool {
        self.remote_creation
            .as_ref()
            .is_some_and(|creation| creation.request_id() == request.id)
    }

    pub(crate) fn create_remote_screen(
        &mut self,
        client: &Client,
        request: RemoteScreenCreateRequest,
    ) -> io::Result<()> {
        self.check_connection(client)?;
        self.create_remote_screen_with(request, |ids| client.create_default_screen_with_ids(ids))
    }

    fn create_remote_screen_with(
        &mut self,
        request: RemoteScreenCreateRequest,
        send: impl FnOnce(&ScreenIds) -> io::Result<()>,
    ) -> io::Result<()> {
        self.remote_creation_available()?;
        self.admit_new_pane()?;
        if self.screen_count() >= self.limits.screens {
            return Err(capacity("native workspace Screen limit reached"));
        }
        if self.source_token() != request.source || self.target() != Some(request.target.target()) {
            return Err(invalid("Creation belongs to an obsolete remote connection"));
        }
        let mutation = self.next_mutation()?;
        let ids = ScreenIds::new();
        let mut screen = ScreenModel::new(ids.clone(), self.connection_id);
        screen.source_id = self.source_id;
        screen.set_tab_selection_supported(self.tab_selection_supported);
        screen.resync_pending = true;
        self.models.insert(ids.screen_id.clone(), screen);
        if let Err(error) = send(&ids) {
            self.models.remove(&ids.screen_id);
            return Err(error);
        }
        self.catalog.push(ScreenInfo {
            id: ids.screen_id.as_str().into(),
            name: None,
        });
        self.mutation = mutation;
        self.remote_creation = Some(RemoteScreenCreation {
            request,
            ids,
            mutation: Some(mutation),
            status: RemoteScreenCreateStatus::Pending,
            unresolved: true,
            uncertain: false,
            reason: None,
        });
        self.mark_catalog_dirty();
        Ok(())
    }

    #[cfg(any(test, feature = "ui-test-support"))]
    pub(crate) fn fixture_remote_creation(
        &mut self,
        request: RemoteScreenCreateRequest,
        missing: bool,
    ) -> io::Result<()> {
        self.create_remote_screen_with(request, |_| Ok(()))?;
        if !missing {
            return Ok(());
        }
        let catalog = serde_json::json!({"type":"mux.workspace_snapshot", "screens": self.live_catalog().unwrap()});
        let snapshots = self
            .models
            .values()
            .filter_map(|model| model.snapshot())
            .map(|state| serde_json::json!({"type":"mux.screen_snapshot", "screen": state}))
            .collect::<Vec<_>>();
        let mut refresh = super::refresh::WorkspaceRefresh::default();
        refresh.begin(self)?;
        self.catalog_in_flight = Some(self.mutation);
        refresh.apply_catalog(self, self.connection_id, &catalog)?;
        for snapshot in snapshots {
            refresh.apply_snapshot(self, self.connection_id, &snapshot)?;
        }
        refresh.test_trailing(self);
        let delta = refresh.apply_catalog(self, self.connection_id, &catalog)?;
        if !delta.coherent_refresh {
            return Err(invalid("fixture observation did not settle"));
        }
        Ok(())
    }

    pub(crate) fn cancel_remote_creation_follow(&self) {
        if let Some(creation) = &self.remote_creation {
            creation.request.cancel_follow();
        }
    }

    pub(crate) fn remote_creation_uncertain(&mut self, reason: &str) {
        if let Some(creation) = &mut self.remote_creation {
            if creation.unresolved {
                creation.uncertain = true;
                if creation.status != RemoteScreenCreateStatus::Rejected {
                    creation.status = RemoteScreenCreateStatus::Uncertain;
                }
                creation.reason = Some(
                    reason
                        .chars()
                        .take(512)
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect(),
                );
            }
        }
    }

    pub(crate) fn remote_creation_lost(&mut self) {
        self.cancel_remote_creation_follow();
        self.remote_creation_uncertain("Connection lost before Screen creation could be observed");
        if let Some(creation) = &mut self.remote_creation {
            creation.mutation = None;
        }
    }

    pub(crate) fn observe_remote_creation_failure(&mut self, connection: Uuid, event: &Value) {
        if connection != self.connection_id {
            return;
        }
        if let Some(creation) = &mut self.remote_creation {
            if creation.unresolved
                && creation.request.source.connection_id() == connection
                && event["op"] == "create_screen"
                && event["paneId"].as_str() == Some(&creation.ids.pane_id)
            {
                creation.status = RemoteScreenCreateStatus::Rejected;
                creation.reason = Some(
                    event["error"]
                        .as_str()
                        .unwrap_or("Screen creation was rejected")
                        .chars()
                        .take(512)
                        .collect(),
                );
                creation.request.cancel_follow();
            }
        }
    }

    pub(super) fn remote_creation_newer_than(&self, id: &str, watermark: u64) -> bool {
        self.remote_creation.as_ref().is_some_and(|creation| {
            creation.unresolved
                && creation.ids.screen_id == id
                && creation
                    .mutation
                    .is_some_and(|mutation| mutation > watermark)
        })
    }

    /// Called only after the complete collector candidate has validated its snapshots.
    pub(super) fn settle_remote_creation(&mut self, watermark: u64) {
        let Some(creation) = &mut self.remote_creation else {
            return;
        };
        if !creation.unresolved
            || creation
                .mutation
                .is_some_and(|mutation| mutation > watermark)
        {
            return;
        }
        let present = match &self.policy {
            CatalogPolicy::Remote { live, .. } => live.as_ref().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.id.as_str() == creation.ids.screen_id)
            }),
            CatalogPolicy::Local => false,
        };
        if present
            && self
                .models
                .get(&creation.ids.screen_id)
                .is_none_or(|screen| screen.snapshot().is_none())
        {
            return;
        }
        if let Some(screen) = self.models.get_mut(&creation.ids.screen_id) {
            screen.pending_panes.remove(&creation.ids.pane_id);
        }
        creation.unresolved = false;
        creation.mutation = None;
        if creation.status != RemoteScreenCreateStatus::Rejected {
            creation.status = if present {
                RemoteScreenCreateStatus::Present
            } else {
                RemoteScreenCreateStatus::Missing
            };
            creation.reason = creation.uncertain.then(|| {
                "Current Screen state observed; the earlier creation outcome was uncertain".into()
            });
        }
    }

    pub(crate) fn remote_creation_follow(&self) -> Option<RemoteScreenCreateRequest> {
        let creation = self.remote_creation.as_ref()?;
        (!creation.unresolved
            && creation.status == RemoteScreenCreateStatus::Present
            && creation.request.is_follow_requested())
        .then(|| creation.request.clone())
    }

    pub(crate) fn consume_remote_creation_follow(
        &self,
        request: &RemoteScreenCreateRequest,
    ) -> Option<ScreenKey> {
        let creation = self.remote_creation.as_ref()?;
        if creation.request_id() != request.request_id()
            || creation.unresolved
            || creation.status != RemoteScreenCreateStatus::Present
            || !creation.request.follow.swap(false, Ordering::AcqRel)
        {
            return None;
        }
        Some(self.screen_key(&creation.ids.screen_id))
    }

    pub(super) fn remote_creation_reservation(&self) -> usize {
        usize::from(self.remote_creation.as_ref().is_some_and(|creation| {
            creation.unresolved
                && !self.models.values().any(|screen| {
                    screen.pending_panes.contains(&creation.ids.pane_id)
                        || screen.pane_sessions.contains_key(&creation.ids.pane_id)
                        || screen.snapshot().is_some_and(|state| {
                            state
                                .panes
                                .contains_key(&PaneId::from(creation.ids.pane_id.as_str()))
                        })
                })
        }))
    }
}

#[cfg(test)]
mod tests;
