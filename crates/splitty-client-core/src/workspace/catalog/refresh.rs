//! Bounded authoritative observations, owned by the worker, never UI model clones.
use super::*;
use splitty_transport::{codec, wire::MAX_FRAME_BYTES};

#[derive(Default)]
pub(crate) struct WorkspaceRefresh {
    batch: Option<Batch>,
    draining: Option<u64>,
}
struct Batch {
    generation: u64,
    opening: Option<Catalog>,
    requested: HashSet<String>,
    snapshots: HashMap<String, (Arc<ScreenState>, usize)>,
    bytes: usize,
    closing: bool,
    repeated: bool,
}
struct Catalog {
    entries: Vec<ScreenInfo>,
    active: Option<String>,
}
impl Catalog {
    fn event(&self) -> Value {
        serde_json::json!({"type":"mux.workspace_snapshot","screens":self.entries,"activeScreenId":self.active})
    }
}
impl WorkspaceRefresh {
    #[cfg(test)]
    pub(super) fn test_trailing(&mut self, model: &mut LocalWorkspace) {
        let batch = self.batch.as_mut().expect("test active collection");
        batch.closing = true;
        model.catalog_in_flight = Some(batch.generation);
    }

    pub fn begin(&mut self, model: &mut LocalWorkspace) -> io::Result<()> {
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
    pub fn flush(&mut self, model: &mut LocalWorkspace, client: &Client) -> io::Result<()> {
        if self.draining.is_some() {
            return Ok(());
        }
        if self.batch.is_none() && model.reconciling && model.catalog_dirty {
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
                for info in &opening.entries {
                    if !batch.requested.contains(info.id.as_str()) {
                        client.resync(info.id.as_str())?;
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
    fn failed(&mut self, model: &mut LocalWorkspace, reason: String) -> WorkspaceDelta {
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
        }
        // Reconciliation remains unresolved, but quiescent. Another nudge can
        // restart reads; output and assessed whole-window close remain available.
        model.reconciling = true;
        WorkspaceDelta {
            snapshot_changed: true,
            transfer: model.finish_transfer(Some(reason.clone())),
            reconciliation_error: Some(reason),
            ..WorkspaceDelta::default()
        }
    }
    pub fn apply_catalog(
        &mut self,
        model: &mut LocalWorkspace,
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
            let entries = match catalog(event) {
                Ok(entries) => entries,
                Err(error) => {
                    model.catalog_in_flight = None;
                    return Err(error);
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
        let entries = match catalog(event) {
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
        model: &mut LocalWorkspace,
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
            return Err(invalid("screen snapshot missing ID"));
        };
        let Some(existing) = model.models.get(id) else {
            return Ok(WorkspaceDelta::default());
        };
        let mut next = existing.clone();
        let delta = next.apply_snapshot(connection, event)?;
        let cross_owner = next.snapshot().is_some_and(|screen| {
            screen.panes.keys().any(|pane| {
                model
                    .screen_for(pane.as_str())
                    .is_some_and(|owner| owner != id)
            })
        });
        if !delta.removed.is_empty() || cross_owner || model.reconciling {
            self.begin(model)?;
            return Ok(WorkspaceDelta {
                snapshot_changed: true,
                ..WorkspaceDelta::default()
            });
        }
        model.apply_snapshot(connection, event)
    }
    fn collect(&mut self, model: &LocalWorkspace, event: &Value) -> io::Result<()> {
        let batch = self.batch.as_mut().expect("collection exists");
        let Some(opening) = &batch.opening else {
            return Ok(());
        };
        let id = event["screen"]["id"]
            .as_str()
            .ok_or_else(|| invalid("screen snapshot missing ID"))?;
        if !opening.entries.iter().any(|info| info.id.as_str() == id) {
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
            .map(|(_, (state, _))| state.panes.len())
            .sum();
        let other_sessions: usize = batch
            .snapshots
            .iter()
            .filter(|(other, _)| other.as_str() != id)
            .map(|(_, (state, _))| state.sessions.len())
            .sum();
        let pending: HashSet<&str> = model
            .models
            .values()
            .flat_map(|m| m.pending_panes.iter().map(String::as_str))
            .collect();
        let unresolved = pending
            .iter()
            .filter(|pane| {
                !panes.contains_key(**pane)
                    && !batch
                        .snapshots
                        .values()
                        .any(|(s, _)| s.panes.contains_key(&PaneId::from(**pane)))
            })
            .count();
        if panes.len() + other_panes + unresolved > MAX_ATTACHMENTS
            || sessions.len() + other_sessions > MAX_ATTACHMENTS
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
        let old = batch.snapshots.get(id).map_or(0, |(_, bytes)| *bytes);
        let remaining = MAX_FRAME_BYTES - (batch.bytes - old);
        let bytes = codec::json_size(screen, remaining)?;
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
        batch.snapshots.insert(id.into(), (Arc::new(state), bytes));
        Ok(())
    }
}
fn catalog(event: &Value) -> io::Result<Catalog> {
    if event["type"] != "mux.workspace_snapshot" {
        return Err(invalid("expected workspace catalog"));
    }
    let values = event["screens"]
        .as_array()
        .ok_or_else(|| invalid("invalid workspace catalog"))?;
    if values.len() > MAX_LOCAL_SCREENS {
        return Err(capacity("native workspace Screen limit reached"));
    }
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
        entries: result,
        active,
    })
}
fn commit(model: &mut LocalWorkspace, batch: &Batch, ending: &Value) -> io::Result<WorkspaceDelta> {
    let old_routes = model.route_map();
    let ready: HashSet<String> = model
        .models
        .values()
        .flat_map(|screen| screen.ready_sessions.iter().cloned())
        .collect();
    let mut next = model.clone();
    next.catalog_in_flight = Some(batch.generation);
    next.apply_catalog_inner(next.connection_id, ending)?;
    for info in &next.catalog {
        let screen = next
            .models
            .get_mut(info.id.as_str())
            .expect("catalog model");
        let state = batch
            .snapshots
            .get(info.id.as_str())
            .expect("ending catalog collected")
            .0
            .clone();
        screen.apply_screen(state)?;
        // apply_screen confirms a still-applicable local move. An unmatched
        // old move cannot remain a permanent admission lock after this fence.
        screen.pending_move = None;
        // Preserve resync_pending: a metadata nudge may have arrived after this
        // Screen's collected observation and still needs one fresh read.
        for (pane, session) in &screen.pane_sessions {
            if old_routes.get(pane) == Some(session) && ready.contains(session) {
                screen.ready_sessions.insert(session.clone());
            }
        }
    }
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
    delta.transfer = next.finish_transfer(None);
    delta.snapshot_changed = true;
    delta.coherent_refresh = true;
    *model = next;
    Ok(delta)
}

#[cfg(test)]
mod tests;
