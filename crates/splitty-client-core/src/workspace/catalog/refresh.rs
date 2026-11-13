//! Bounded authoritative observations, owned by the worker, never UI model clones.
use super::*;
use splitty_transport::{
    budget::{Lease, Permit},
    codec,
    wire::MAX_FRAME_BYTES,
};

#[derive(Default)]
pub(crate) struct WorkspaceRefresh {
    batch: Option<Batch>,
    draining: Option<u64>,
}
struct Batch {
    generation: u64,
    opening: Option<Catalog>,
    requested: HashSet<String>,
    snapshots: HashMap<String, (Arc<ScreenState>, usize, Permit)>,
    bytes: usize,
    closing: bool,
    repeated: bool,
}
struct Catalog {
    _lease: Option<Lease>,
    entries: Vec<ScreenInfo>,
    active: Option<String>,
}
impl Catalog {
    fn event(&self) -> Value {
        serde_json::json!({"type":"mux.workspace_snapshot","screens":self.entries,"activeScreenId":self.active})
    }
}
impl WorkspaceRefresh {
    #[cfg(any(test, feature = "ui-test-support"))]
    pub(super) fn test_trailing(&mut self, model: &mut SourceWorkspace) {
        let batch = self.batch.as_mut().expect("test active collection");
        batch.closing = true;
        model.catalog_in_flight = Some(batch.generation);
    }

    pub fn begin(&mut self, model: &mut SourceWorkspace) -> io::Result<()> {
        if self.batch.is_some() {
            return Ok(());
        }
        let generation = if model.pending_transfer.is_some() && !model.reconciling {
            model.mutation
        } else {
            model.next_mutation()?
        };
        model.mutation = generation;
        model.reconciling = true;
        model.catalog_dirty = false;
        self.batch = Some(Batch {
            generation,
            opening: None,
            requested: HashSet::new(),
            snapshots: HashMap::new(),
            bytes: 0,
            closing: false,
            repeated: false,
        });
        Ok(())
    }
    pub(crate) fn needs_flush(&self, model: &SourceWorkspace) -> bool {
        if self.draining.is_some() {
            return false;
        }
        if self.batch.is_some() {
            return model.catalog_in_flight.is_none();
        }
        (model.catalog_dirty && model.catalog_in_flight.is_none())
            || model
                .models
                .values()
                .any(|screen| screen.resync_pending || screen.metadata_pending)
    }
    /// A host source observes every change through the coherent refresh of
    /// all its opened Screens, because only that orders a pane's removal from
    /// one Screen with its arrival in another. A Screen whose session metadata
    /// alone changed is read by itself while the source's routes are settled;
    /// `apply_snapshot` turns that read into the coherent refresh when it
    /// shows a change of layout or routes.
    fn needs_coherent(&self, model: &SourceWorkspace) -> bool {
        if !self.needs_flush(model) {
            return false;
        }
        let structural = (model.catalog_dirty && model.catalog_in_flight.is_none())
            || model.models.values().any(|screen| screen.resync_pending);
        structural || !model.metadata_reads_admissible()
    }
    pub(crate) fn observation_pending(&self, model: &SourceWorkspace) -> bool {
        self.batch.is_some()
            || self.draining.is_some()
            || model.catalog_in_flight.is_some()
            || self.needs_flush(model)
    }
    pub(crate) fn observation_active(&self) -> bool {
        self.batch.is_some()
    }
    pub(crate) fn abort_observation(
        &mut self,
        model: &mut SourceWorkspace,
        reason: String,
    ) -> WorkspaceDelta {
        // A timeout consumes this pass; a nudge or explicit Retry may start a
        // fresh bounded pass, but must first drain any old ordered catalog fence.
        model.catalog_dirty = false;
        self.failed(model, reason)
    }
    pub fn flush(&mut self, model: &mut SourceWorkspace, client: &Client) -> io::Result<()> {
        if self.draining.is_some() {
            return Ok(());
        }
        if self.batch.is_none()
            && ((model.reconciling && model.catalog_dirty)
                || (model.kind() == SourceKind::Remote && self.needs_coherent(model)))
        {
            self.begin(model)?;
        }
        let Some(batch) = &mut self.batch else {
            return model.flush_catalog(client);
        };
        if model.catalog_in_flight.is_some() {
            return Ok(());
        }
        if let Some(opening) = &batch.opening {
            if !batch.closing {
                let all = model.kind() == SourceKind::Local;
                let opened = &model.catalog;
                for info in opening
                    .entries
                    .iter()
                    .filter(|info| all || opened.iter().any(|entry| entry.id == info.id))
                {
                    if !batch.requested.contains(info.id.as_str()) {
                        client.resync(info.id.as_str())?;
                        if let Some(screen) = model.models.get_mut(info.id.as_str()) {
                            screen.resync_pending = false;
                            screen.metadata_pending = false;
                        }
                        batch.requested.insert(info.id.to_string());
                    }
                }
                // Fence after every admitted resync, including missing Screens.
                client.request_workspace()?;
                model.catalog_in_flight = Some(batch.generation);
                batch.closing = true;
            }
        } else {
            client.request_workspace()?;
            model.catalog_in_flight = Some(batch.generation);
        }
        Ok(())
    }
    fn failed(&mut self, model: &mut SourceWorkspace, reason: String) -> WorkspaceDelta {
        let previous = self.batch.take(); // release every staged allocation
        if model.catalog_dirty && previous.as_ref().is_some_and(|batch| !batch.repeated) {
            // One bounded recollection consumes a nudge that arrived during the
            // failed pass. Preserve any old fence until its response is drained.
            if self.begin(model).is_ok() {
                self.batch.as_mut().expect("retry batch").repeated = true;
                return WorkspaceDelta {
                    snapshot_changed: true,
                    ..WorkspaceDelta::default()
                };
            }
        }
        self.draining = model.catalog_in_flight;
        model.catalog_dirty = false;
        for screen in model.models.values_mut() {
            screen.resync_pending = false;
            screen.metadata_pending = false;
        }
        // Reconciliation remains unresolved, but quiescent. Another nudge can
        // restart reads; output and assessed whole-window close remain available.
        model.reconciling = true;
        model.remote_creation_uncertain(&reason);
        WorkspaceDelta {
            snapshot_changed: true,
            transfer: model.finish_transfer(Some(reason.clone())),
            floating: model.finish_floating(Some(reason.clone())),
            template: model.finish_template(Some(reason.clone())),
            reconciliation_error: Some(reason),
            ..WorkspaceDelta::default()
        }
    }
    pub fn apply_catalog(
        &mut self,
        model: &mut SourceWorkspace,
        connection: Uuid,
        event: &Value,
    ) -> io::Result<WorkspaceDelta> {
        if connection != model.connection_id || model.catalog_in_flight.is_none() {
            return Ok(WorkspaceDelta::default());
        }
        if self.draining.take().is_some() {
            model.catalog_in_flight = None;
            return Ok(WorkspaceDelta::default());
        }
        if self.batch.is_none() {
            let entries = match catalog(event, model) {
                Ok(entries) => entries,
                Err(error) => {
                    let watermark = model.catalog_in_flight.take();
                    let placement = model.placement_catalog(connection, watermark, Some(&error));
                    let floating = model.floating_catalog(connection, watermark, Some(&error));
                    return attach_floating_result(Err(error), placement, floating);
                }
            };
            let removes_routes = model.models.iter().any(|(id, screen)| {
                !screen.pane_sessions.is_empty()
                    && !entries.entries.iter().any(|info| info.id.as_str() == id)
            });
            if removes_routes || model.reconciling {
                // This response may precede the external operation. Consume its
                // old watermark, then issue a fresh ordered opening fence.
                model.catalog_in_flight = None;
                self.begin(model)?;
                return Ok(WorkspaceDelta {
                    snapshot_changed: true,
                    ..WorkspaceDelta::default()
                });
            }
            return model.apply_catalog(connection, event);
        }
        let watermark = model.catalog_in_flight.take();
        let batch = self.batch.as_mut().expect("active batch");
        if watermark != Some(batch.generation) {
            return Ok(WorkspaceDelta::default());
        }
        let entries = match catalog(event, model) {
            Ok(entries) => entries,
            Err(error) => {
                return Ok(self.failed(model, format!("Workspace refresh rejected: {error}")))
            }
        };
        if batch.opening.is_none() {
            batch.opening = Some(entries);
            return Ok(WorkspaceDelta::default());
        }
        if !batch.closing {
            return Ok(WorkspaceDelta::default());
        }
        if entries
            .entries
            .iter()
            .filter(|info| {
                model.kind() == SourceKind::Local
                    || model.catalog.iter().any(|opened| opened.id == info.id)
            })
            .any(|info| !batch.snapshots.contains_key(info.id.as_str()))
        {
            if !batch.repeated {
                batch.repeated = true;
                batch.opening = Some(entries);
                batch.requested.clear();
                batch.snapshots.clear();
                batch.bytes = 0;
                batch.closing = false;
                return Ok(WorkspaceDelta::default());
            }
            return Ok(self.failed(
                model,
                "Workspace changed during refresh; waiting for another invalidation".into(),
            ));
        }
        let batch = self.batch.take().expect("complete batch");
        match commit(model, &batch, &entries.event()) {
            Ok(delta) => Ok(delta),
            Err(error) => {
                self.batch = Some(batch);
                Ok(self.failed(
                    model,
                    format!("Workspace refresh could not be verified: {error}"),
                ))
            }
        }
    }
    pub fn apply_snapshot(
        &mut self,
        model: &mut SourceWorkspace,
        connection: Uuid,
        event: &Value,
    ) -> io::Result<WorkspaceDelta> {
        if connection != model.connection_id {
            return Ok(WorkspaceDelta::default());
        }
        if self.batch.is_some() {
            let result = self.collect(model, event);
            return Ok(match result {
                Ok(()) => WorkspaceDelta::default(),
                Err(error) => self.failed(model, format!("Workspace snapshot rejected: {error}")),
            });
        }
        if self.draining.is_some() || model.reconciling {
            return Ok(WorkspaceDelta::default());
        }
        let Some(id) = event["screen"]["id"].as_str() else {
            return model.apply_snapshot(connection, event);
        };
        let Some(existing) = model.models.get(id) else {
            return Ok(WorkspaceDelta::default());
        };
        let mut next = existing.clone();
        let delta = match next.apply_snapshot(connection, event) {
            Ok(delta) => delta,
            Err(_) => return model.apply_snapshot(connection, event),
        };
        let cross_owner = next.snapshot().is_some_and(|screen| {
            screen.panes.keys().any(|pane| {
                model
                    .screen_for(pane.as_str())
                    .is_some_and(|owner| owner != id)
            })
        });
        // A host Screen read alone for its metadata keeps its routes only when
        // it shows the same layout and routes; anything else is observed
        // through the coherent refresh, as every other host change is.
        let host_changed = model.kind() == SourceKind::Remote
            && (!delta.added.is_empty()
                || !model.metadata_reads_admissible()
                || !same_layout(existing, &next));
        if !delta.removed.is_empty() || cross_owner || model.reconciling || host_changed {
            self.begin(model)?;
            return Ok(WorkspaceDelta {
                snapshot_changed: true,
                ..WorkspaceDelta::default()
            });
        }
        model.apply_snapshot(connection, event)
    }
    fn collect(&mut self, model: &SourceWorkspace, event: &Value) -> io::Result<()> {
        let batch = self.batch.as_mut().expect("collection exists");
        let Some(opening) = &batch.opening else {
            return Ok(());
        };
        let id = event["screen"]["id"]
            .as_str()
            .ok_or_else(|| invalid("screen snapshot missing ID"))?;
        if !opening.entries.iter().any(|info| info.id.as_str() == id)
            || (model.kind() == SourceKind::Remote
                && !model.catalog.iter().any(|info| info.id.as_str() == id))
        {
            return Ok(());
        }
        let screen = &event["screen"];
        let panes = screen["panes"]
            .as_object()
            .ok_or_else(|| invalid("invalid Screen panes"))?;
        let sessions = screen["sessions"]
            .as_object()
            .ok_or_else(|| invalid("invalid Screen sessions"))?;
        let other_panes: usize = batch
            .snapshots
            .iter()
            .filter(|(other, _)| other.as_str() != id)
            .map(|(_, (state, _, _))| state.panes.len())
            .sum();
        let other_sessions: usize = batch
            .snapshots
            .iter()
            .filter(|(other, _)| other.as_str() != id)
            .map(|(_, (state, _, _))| state.sessions.len())
            .sum();
        let pending: HashSet<&str> = model
            .models
            .values()
            .flat_map(|m| m.pending_panes.iter().map(String::as_str))
            .collect();
        let converted_creation = model.remote_creation.as_ref().filter(|creation| {
            creation.is_unresolved()
                && !model.remote_creation_newer_than(creation.screen_id(), batch.generation)
                && (creation.screen_id() == id
                    || batch.snapshots.contains_key(creation.screen_id()))
        });
        let unresolved = pending
            .iter()
            .filter(|pane| {
                !converted_creation.is_some_and(|creation| creation.initial_pane_id() == **pane)
                    && !panes.contains_key(**pane)
                    && !batch
                        .snapshots
                        .values()
                        .any(|(s, _, _)| s.panes.contains_key(&PaneId::from(**pane)))
            })
            .count();
        if panes.len() + other_panes + unresolved > model.limits.panes
            || sessions.len() + other_sessions > model.limits.panes
        {
            return Err(capacity("workspace refresh attachment limit reached"));
        }
        for id in panes.keys().chain(sessions.keys()) {
            identifier(id)?;
        }
        if screen["name"].as_str().is_some_and(|name| name.len() > 256) {
            return Err(invalid("Screen name too long"));
        }
        // The ENTIRE staged generation is capped at one existing transport
        // MAX_FRAME_BYTES (16 MiB), not 16 MiB per Screen. Size before typed
        // allocation, and never retain delivery credits while awaiting others.
        let old = batch.snapshots.get(id).map_or(0, |(_, bytes, _)| *bytes);
        let remaining = MAX_FRAME_BYTES - (batch.bytes - old);
        let bytes = codec::json_size(screen, remaining)?;
        // Replace a staged observation within the shared window allowance. A
        // failed replacement abandons the batch, so retaining its old copy is
        // neither useful nor a reason to double-reserve the same slot.
        batch.snapshots.remove(id);
        let permit = model.staged.acquire(bytes).map_err(io::Error::other)?;
        let state = ScreenState::deserialize(screen).map_err(io::Error::other)?;
        if model
            .models
            .get(id)
            .and_then(ScreenModel::snapshot)
            .is_some_and(|old| state.revision < old.revision)
        {
            return Err(invalid("older Screen snapshot cannot verify workspace"));
        }
        batch.bytes = batch.bytes - old + bytes;
        batch
            .snapshots
            .insert(id.into(), (Arc::new(state), bytes, permit));
        Ok(())
    }
}
/// Whether two observations of a Screen have the same name, layout and panes
/// with the same sessions, so that they differ at most in metadata.
fn same_layout(old: &ScreenModel, new: &ScreenModel) -> bool {
    let (Some(old), Some(new)) = (old.server_snapshot(), new.server_snapshot()) else {
        return false;
    };
    let layout = |screen: &ScreenState| {
        serde_json::to_value((&screen.name, &screen.root, &screen.floating)).ok()
    };
    let sessions = |screen: &ScreenState| -> HashMap<String, Option<String>> {
        screen
            .panes
            .iter()
            .map(|(pane, state)| {
                (
                    pane.to_string(),
                    state.session_id.as_ref().map(ToString::to_string),
                )
            })
            .collect()
    };
    let old_layout = layout(old);
    old_layout.is_some()
        && old_layout == layout(new)
        && sessions(old) == sessions(new)
        && old.sessions.len() == new.sessions.len()
        && old.sessions.keys().all(|id| new.sessions.contains_key(id))
}
fn catalog(event: &Value, model: &SourceWorkspace) -> io::Result<Catalog> {
    let limit = if model.kind() == SourceKind::Local {
        model.limits.screens
    } else {
        MAX_REMOTE_CATALOG
    };
    if event["type"] != "mux.workspace_snapshot" {
        return Err(invalid("expected workspace catalog"));
    }
    let values = event["screens"]
        .as_array()
        .ok_or_else(|| invalid("invalid workspace catalog"))?;
    if values.len() > limit {
        return Err(capacity("native workspace Screen limit reached"));
    }
    let lease = match &model.policy {
        CatalogPolicy::Local => None,
        CatalogPolicy::Remote { budget, .. } => {
            let bytes =
                codec::json_size(&event["screens"], splitty_core::native_api::MAX_API_BODY)?;
            Some(
                budget
                    .reserve(bytes.saturating_mul(8), false)
                    .map_err(io::Error::other)?,
            )
        }
    };
    let mut ids = HashSet::new();
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let info = ScreenInfo::deserialize(value).map_err(io::Error::other)?;
        identifier(info.id.as_str())?;
        if info.name.as_ref().is_some_and(|name| name.len() > 256) || !ids.insert(info.id.clone()) {
            return Err(invalid("invalid workspace Screen catalog"));
        }
        result.push(info);
    }
    let active = event["activeScreenId"]
        .as_str()
        .filter(|id| ids.iter().any(|known| known.as_str() == *id))
        .map(str::to_owned);
    Ok(Catalog {
        _lease: lease,
        entries: result,
        active,
    })
}
fn commit(
    model: &mut SourceWorkspace,
    batch: &Batch,
    ending: &Value,
) -> io::Result<WorkspaceDelta> {
    let old_routes = model.route_map();
    let ready: HashSet<String> = model
        .models
        .values()
        .flat_map(|screen| screen.ready_sessions.iter().cloned())
        .collect();
    let mut next = model.clone();
    next.catalog_in_flight = Some(batch.generation);
    next.apply_catalog_inner(next.connection_id, ending)?;
    for (id, screen) in &mut next.models {
        // A newer local creation may be retained beyond this collection fence.
        // Only members of the collected generation have an observation to apply.
        let Some((state, _, _)) = batch.snapshots.get(id) else {
            continue;
        };
        screen.apply_screen(state.clone())?;
        // apply_screen confirms a still-applicable local move. An unmatched
        // old move cannot remain a permanent admission lock after this fence.
        screen.pending_move = None;
        // Preserve resync_pending and metadata_pending: a nudge may have
        // arrived after this Screen's collected observation and still needs
        // one fresh read.
        for (pane, session) in &screen.pane_sessions {
            if old_routes.get(pane) == Some(session) && ready.contains(session) {
                screen.ready_sessions.insert(session.clone());
            }
        }
    }
    // Resolve the reservation in this private, complete candidate before checking
    // capacity: another client may already have replaced the initial pane.
    // This changes only candidate state, never the shared follow permission.
    next.settle_remote_creation(batch.generation);
    if let Some((id, screen)) = next.models.iter().next() {
        next.check_candidate(id, screen)?;
    }
    next.reconciling = false;
    next.route_generation = next
        .route_generation
        .checked_add(1)
        .ok_or_else(|| invalid("route generation exhausted"))?;
    let mut delta = route_delta(&old_routes, &next.route_map());
    delta.placement = next.settle_placement_after_refresh(batch.generation);
    delta.floating = next.settle_floating_after_refresh(batch.generation);
    delta.template = next.settle_template_after_refresh(batch.generation);
    delta.transfer = next.finish_transfer(None);
    delta.snapshot_changed = true;
    delta.coherent_refresh = true;
    *model = next;
    if model
        .remote_screen_creation()
        .is_some_and(|creation| creation.status() == creation::RemoteScreenCreateStatus::Missing)
    {
        model.cancel_remote_creation_follow();
    }
    Ok(delta)
}

#[cfg(test)]
mod tests;
