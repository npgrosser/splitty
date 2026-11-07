//! One service-owned local screen, projected without a second layout tree.
use crate::{
    connection::{invalid, MAX_ATTACHMENTS},
    Client, ScreenIds,
};
use serde::Deserialize;
use serde_json::Value;
pub use splitty_core::mux::{Axis, LayoutNode, ScreenState};
use splitty_core::{
    ids::{NodeId, PaneId},
    mux::{self, ScreenOp},
};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRoute {
    pub pane_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceDelta {
    pub snapshot_changed: bool,
    pub added: Vec<SessionRoute>,
    pub removed: Vec<SessionRoute>,
}

#[derive(Clone)]
struct SentTabSelection {
    pane_id: String,
    snapshot_revision: u64,
}

/// The snapshot supplies layout and session identity. Only focus and selected
/// tabs belong to this window; pending IDs prevent another screen's spawn
/// notification from becoming a local session route.
#[derive(Clone)]
pub struct LocalWorkspace {
    ids: ScreenIds,
    connection_id: Uuid,
    snapshot: Option<Arc<ScreenState>>,
    focused_pane: Option<String>,
    pending_focus: Option<String>,
    selected_tabs: HashMap<String, String>,
    tab_selection_supported: bool,
    sent_tab_selections: HashMap<String, SentTabSelection>,
    pending_panes: HashSet<String>,
    pending_recreation: Option<String>,
    pane_sessions: HashMap<String, String>,
    session_panes: HashMap<String, String>,
    ready_sessions: HashSet<String>,
}

impl LocalWorkspace {
    pub fn new(ids: ScreenIds, connection_id: Uuid) -> Self {
        let focused_pane = Some(ids.pane_id.clone());
        let pending_panes = HashSet::from([ids.pane_id.clone()]);
        Self {
            ids,
            connection_id,
            snapshot: None,
            focused_pane,
            pending_focus: None,
            selected_tabs: HashMap::new(),
            tab_selection_supported: false,
            sent_tab_selections: HashMap::new(),
            pending_panes,
            pending_recreation: None,
            pane_sessions: HashMap::new(),
            session_panes: HashMap::new(),
            ready_sessions: HashSet::new(),
        }
    }

    pub fn screen_id(&self) -> &str {
        &self.ids.screen_id
    }

    pub fn snapshot(&self) -> Option<&ScreenState> {
        self.snapshot.as_deref()
    }

    pub fn focused_pane(&self) -> Option<&str> {
        self.focused_pane.as_deref()
    }

    pub fn session_for(&self, pane_id: &str) -> Option<&str> {
        self.pane_sessions.get(pane_id).map(String::as_str)
    }

    pub fn pane_for(&self, session_id: &str) -> Option<&str> {
        self.session_panes.get(session_id).map(String::as_str)
    }

    pub fn session_ready(&self, session_id: &str) -> bool {
        self.ready_sessions.contains(session_id)
    }

    pub fn observe_attached(&mut self, connection_id: Uuid, session_id: &str) -> bool {
        connection_id == self.connection_id
            && self.session_panes.contains_key(session_id)
            && self.ready_sessions.insert(session_id.to_owned())
    }

    /// Return this window's active tab for a canonical slot node.
    pub fn selected_pane<'a>(&'a self, slot: &'a LayoutNode) -> Option<&'a str> {
        let LayoutNode::Slot {
            id,
            pane_ids,
            active_pane_id,
        } = slot
        else {
            return None;
        };
        self.selected_tabs
            .get(id.as_str())
            .filter(|selected| {
                pane_ids
                    .iter()
                    .any(|pane| pane.as_str() == selected.as_str())
            })
            .map(String::as_str)
            .or_else(|| {
                active_pane_id
                    .as_ref()
                    .filter(|active| pane_ids.contains(active))
                    .map(PaneId::as_str)
            })
            .or_else(|| pane_ids.first().map(PaneId::as_str))
    }

    /// Selection changes immediately in this window. Sync persists it on a
    /// capable server without changing another window's visible tab.
    pub fn select_tab(&mut self, pane_id: &str) -> bool {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(LayoutNode::Slot { id, .. }) = find_slot(snapshot, pane_id) else {
            return false;
        };
        let changed = self.selected_tabs.get(id.as_str()).map(String::as_str) != Some(pane_id);
        self.selected_tabs
            .insert(id.as_str().to_owned(), pane_id.to_owned());
        self.focused_pane = Some(pane_id.to_owned());
        changed
    }

    pub fn focus_pane(&mut self, pane_id: &str) -> bool {
        if self
            .snapshot
            .as_ref()
            .is_none_or(|screen| find_slot(screen, pane_id).is_none())
        {
            return false;
        }
        let changed = self.focused_pane.as_deref() != Some(pane_id);
        self.select_tab(pane_id);
        changed
    }

    pub fn set_tab_selection_supported(&mut self, supported: bool) {
        self.tab_selection_supported = supported;
        if !supported {
            self.sent_tab_selections.clear();
        }
    }

    /// Persist local choice only after a user selection. Snapshot state and
    /// successful sends both count as the server's last known active tab.
    pub fn sync_selected_tabs(&mut self, client: &Client) -> io::Result<()> {
        if !self.tab_selection_supported {
            return Ok(());
        }
        if client.connection_id() != self.connection_id {
            return Err(invalid("workspace connection changed"));
        }
        let revision = self
            .snapshot()
            .ok_or_else(|| invalid("screen snapshot not loaded"))?
            .revision;
        for (slot_id, pane_id) in self.selections_to_sync() {
            client.screen_op(
                self.screen_id(),
                &ScreenOp::SelectTab {
                    pane_id: PaneId::from(pane_id.as_str()),
                },
            )?;
            self.sent_tab_selections.insert(
                slot_id,
                SentTabSelection {
                    pane_id,
                    snapshot_revision: revision,
                },
            );
        }
        Ok(())
    }

    /// A new client identity revokes all old input/output routes immediately.
    pub fn reset_connection(&mut self, connection_id: Uuid) -> WorkspaceDelta {
        let old = std::mem::take(&mut self.pane_sessions);
        self.session_panes.clear();
        self.ready_sessions.clear();
        self.pending_panes.clear();
        self.pending_focus = None;
        self.pending_recreation = None;
        self.sent_tab_selections.clear();
        self.snapshot = None;
        self.connection_id = connection_id;
        WorkspaceDelta {
            snapshot_changed: true,
            removed: routes(&old),
            ..WorkspaceDelta::default()
        }
    }

    /// Accept only snapshots of this screen and connection generation. Existing
    /// pending spawn routes survive an earlier snapshot until confirmed or failed.
    pub fn apply_snapshot(
        &mut self,
        connection_id: Uuid,
        event: &Value,
    ) -> io::Result<WorkspaceDelta> {
        if connection_id != self.connection_id {
            return Ok(WorkspaceDelta::default());
        }
        if event["type"] != "mux.screen_snapshot" {
            return Err(invalid("expected screen snapshot"));
        }
        let screen = ScreenState::deserialize(&event["screen"]).map_err(io::Error::other)?;
        if screen.id.as_str() != self.ids.screen_id.as_str() {
            return Ok(WorkspaceDelta::default());
        }
        let recreated = self
            .pending_recreation
            .as_ref()
            .is_some_and(|pane_id| screen.panes.contains_key(&PaneId::from(pane_id.as_str())));
        if self
            .snapshot
            .as_ref()
            .is_some_and(|previous| screen.revision < previous.revision)
            && !recreated
        {
            return Ok(WorkspaceDelta::default());
        }
        if recreated {
            self.pending_recreation = None;
        }
        self.reconcile_sent_tab_selections(&screen);
        let old = std::mem::take(&mut self.pane_sessions);
        let mut next = HashMap::new();
        for pane_id in screen.panes_in_layout_order() {
            if let Some(session_id) = screen
                .panes
                .get(&pane_id)
                .and_then(|pane| pane.session_id.as_ref())
            {
                if screen.sessions.contains_key(session_id) {
                    next.insert(pane_id.to_string(), session_id.to_string());
                }
            }
        }
        // A pane can appear before its session ID. Keep its admission slot
        // until a route is confirmed or a spawn failure rejects it.
        self.pending_panes
            .retain(|pane_id| !next.contains_key(pane_id));
        for pane_id in &self.pending_panes {
            if let Some(session_id) = old.get(pane_id) {
                next.insert(pane_id.clone(), session_id.clone());
            }
        }
        self.snapshot = Some(Arc::new(screen));
        self.install_routes(next);
        self.reconcile_selection();
        if let Some(pane_id) = self
            .pending_focus
            .as_ref()
            .filter(|pane_id| {
                self.snapshot.as_ref().is_some_and(|screen| {
                    screen
                        .root
                        .find_slot(&PaneId::from(pane_id.as_str()))
                        .is_some()
                })
            })
            .cloned()
        {
            self.select_tab(&pane_id);
            self.pending_focus = None;
        }
        let mut delta = route_delta(&old, &self.pane_sessions);
        delta.snapshot_changed = true;
        Ok(delta)
    }

    /// Spawn events omit screen ID. Match only the intended or already known
    /// pane, and never replace a different session stated by the snapshot.
    pub fn observe_spawn(
        &mut self,
        connection_id: Uuid,
        pane_id: &str,
        session_id: &str,
    ) -> WorkspaceDelta {
        if connection_id != self.connection_id
            || (!self.pending_panes.contains(pane_id)
                && self
                    .snapshot
                    .as_ref()
                    .is_none_or(|screen| !screen.panes.contains_key(&PaneId::from(pane_id))))
        {
            return WorkspaceDelta::default();
        }
        if let Some(expected) = self
            .snapshot
            .as_ref()
            .and_then(|screen| screen.panes.get(&PaneId::from(pane_id)))
            .and_then(|pane| pane.session_id.as_ref())
        {
            if expected.as_str() != session_id {
                return WorkspaceDelta::default();
            }
        }
        let old = self.pane_sessions.clone();
        let mut next = old.clone();
        next.insert(pane_id.to_owned(), session_id.to_owned());
        self.install_routes(next);
        route_delta(&old, &self.pane_sessions)
    }

    pub fn observe_spawn_failure(&mut self, connection_id: Uuid, pane_id: &str) -> WorkspaceDelta {
        if connection_id != self.connection_id || !self.pending_panes.remove(pane_id) {
            return WorkspaceDelta::default();
        }
        if self.pending_focus.as_deref() == Some(pane_id) {
            self.pending_focus = None;
        }
        let old = self.pane_sessions.clone();
        let mut next = old.clone();
        if self
            .snapshot
            .as_ref()
            .is_none_or(|screen| !screen.panes.contains_key(&PaneId::from(pane_id)))
        {
            next.remove(pane_id);
        }
        self.install_routes(next);
        route_delta(&old, &self.pane_sessions)
    }

    pub fn split(&mut self, client: &Client, pane_id: &str, axis: Axis) -> io::Result<String> {
        self.admit_new_pane()?;
        let new_pane_id = Uuid::new_v4().to_string();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from(pane_id),
            axis,
            new_pane_index: None,
            new_pane_id: Some(PaneId::from(new_pane_id.clone())),
            resulting_sizes: None,
            cwd: self.cwd_for(pane_id),
            shell_profile_id: None,
        };
        self.send_op(client, &op)?;
        self.pending_panes.insert(new_pane_id.clone());
        self.pending_focus = Some(new_pane_id.clone());
        Ok(new_pane_id)
    }

    pub fn new_tab(&mut self, client: &Client, pane_id: &str) -> io::Result<String> {
        self.admit_new_pane()?;
        let new_pane_id = Uuid::new_v4().to_string();
        let op = ScreenOp::NewTab {
            pane_id: PaneId::from(pane_id),
            new_pane_id: Some(PaneId::from(new_pane_id.clone())),
            cwd: self.cwd_for(pane_id),
            shell_profile_id: None,
        };
        self.send_op(client, &op)?;
        self.pending_panes.insert(new_pane_id.clone());
        self.pending_focus = Some(new_pane_id.clone());
        Ok(new_pane_id)
    }

    /// Refill the last pane's empty screen under its existing ID and name.
    pub fn new_terminal(&mut self, client: &Client) -> io::Result<String> {
        self.admit_new_pane()?;
        if client.connection_id() != self.connection_id {
            return Err(invalid("workspace connection changed"));
        }
        if self
            .snapshot()
            .is_none_or(|screen| !screen.panes.is_empty())
        {
            return Err(invalid("screen is not empty"));
        }
        let pane_id = Uuid::new_v4().to_string();
        client.recreate_screen(self.screen_id(), &pane_id)?;
        self.pending_panes.insert(pane_id.clone());
        self.pending_focus = Some(pane_id.clone());
        self.pending_recreation = Some(pane_id.clone());
        Ok(pane_id)
    }

    pub fn close_pane(&self, client: &Client, pane_id: &str) -> io::Result<()> {
        self.send_op(
            client,
            &ScreenOp::ClosePane {
                pane_id: PaneId::from(pane_id),
                resulting_sizes: None,
            },
        )
    }

    pub fn resize_split(&self, client: &Client, node_id: &str, sizes: Vec<f64>) -> io::Result<()> {
        self.send_op(
            client,
            &ScreenOp::ResizeSplit {
                node_id: NodeId::from(node_id),
                sizes,
            },
        )
    }

    fn send_op(&self, client: &Client, op: &ScreenOp) -> io::Result<()> {
        if client.connection_id() != self.connection_id {
            return Err(invalid("workspace connection changed"));
        }
        let screen = self
            .snapshot()
            .ok_or_else(|| invalid("screen snapshot not loaded"))?;
        mux::validate_screen_op(screen, op).map_err(io::Error::other)?;
        client.screen_op(&self.ids.screen_id, op)
    }

    fn selections_to_sync(&self) -> Vec<(String, String)> {
        if !self.tab_selection_supported {
            return Vec::new();
        }
        let Some(screen) = self.snapshot() else {
            return Vec::new();
        };
        let mut selections = Vec::new();
        collect_selection_ops(
            &screen.root,
            &self.selected_tabs,
            &self.sent_tab_selections,
            &mut selections,
        );
        for floating in &screen.floating {
            collect_selection_ops(
                &floating.root,
                &self.selected_tabs,
                &self.sent_tab_selections,
                &mut selections,
            );
        }
        selections
    }

    fn reconcile_sent_tab_selections(&mut self, screen: &ScreenState) {
        self.sent_tab_selections.retain(|slot_id, sent| {
            let node_id = NodeId::from(slot_id.as_str());
            let node = std::iter::once(&screen.root)
                .chain(screen.floating.iter().map(|entry| &entry.root))
                .find_map(|root| root.find_node(&node_id));
            let Some(LayoutNode::Slot {
                pane_ids,
                active_pane_id,
                ..
            }) = node
            else {
                return false;
            };
            pane_ids.iter().any(|pane| pane.as_str() == sent.pane_id)
                && active_pane_id
                    .as_ref()
                    .is_none_or(|active| active.as_str() != sent.pane_id)
                && screen.revision <= sent.snapshot_revision
        });
    }

    fn admit_new_pane(&self) -> io::Result<()> {
        let pending_without_route = self
            .pending_panes
            .iter()
            .filter(|pane_id| !self.pane_sessions.contains_key(*pane_id))
            .count();
        if self.session_panes.len() + pending_without_route >= MAX_ATTACHMENTS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "native workspace attachment limit reached",
            ));
        }
        Ok(())
    }

    fn cwd_for(&self, pane_id: &str) -> Option<String> {
        let screen = self.snapshot()?;
        let pane = screen.panes.get(&PaneId::from(pane_id))?;
        pane.cwd.clone().or_else(|| {
            pane.session_id
                .as_ref()
                .and_then(|session| screen.sessions.get(session))
                .and_then(|session| session.cwd.clone())
        })
    }

    fn install_routes(&mut self, routes: HashMap<String, String>) {
        self.session_panes = routes
            .iter()
            .map(|(pane, session)| (session.clone(), pane.clone()))
            .collect();
        self.pane_sessions = routes;
        self.ready_sessions
            .retain(|session| self.session_panes.contains_key(session));
    }

    fn reconcile_selection(&mut self) {
        let Some(screen) = self.snapshot.as_ref() else {
            return;
        };
        let mut valid_slots = HashSet::new();
        reconcile_node(&screen.root, &mut self.selected_tabs, &mut valid_slots);
        for floating in &screen.floating {
            reconcile_node(&floating.root, &mut self.selected_tabs, &mut valid_slots);
        }
        self.selected_tabs
            .retain(|slot_id, _| valid_slots.contains(slot_id));
        let mut visible = Vec::new();
        collect_visible(&screen.root, &self.selected_tabs, &mut visible);
        for floating in &screen.floating {
            collect_visible(&floating.root, &self.selected_tabs, &mut visible);
        }
        if self
            .focused_pane
            .as_ref()
            .is_none_or(|pane| !visible.contains(pane))
        {
            self.focused_pane = visible.into_iter().next();
        }
    }
}

fn find_slot<'a>(screen: &'a ScreenState, pane_id: &str) -> Option<&'a LayoutNode> {
    let pane_id = PaneId::from(pane_id);
    std::iter::once(&screen.root)
        .chain(screen.floating.iter().map(|entry| &entry.root))
        .find_map(|root| root.find_slot(&pane_id))
}

fn collect_selection_ops(
    node: &LayoutNode,
    selected: &HashMap<String, String>,
    sent: &HashMap<String, SentTabSelection>,
    result: &mut Vec<(String, String)>,
) {
    match node {
        LayoutNode::Slot {
            id,
            pane_ids,
            active_pane_id,
        } if pane_ids.len() >= 2 => {
            let chosen = selected
                .get(id.as_str())
                .filter(|pane| pane_ids.iter().any(|id| id.as_str() == pane.as_str()))
                .map(String::as_str)
                .or_else(|| {
                    active_pane_id
                        .as_ref()
                        .filter(|pane| pane_ids.contains(pane))
                        .map(PaneId::as_str)
                })
                .or_else(|| pane_ids.first().map(PaneId::as_str));
            let known = sent
                .get(id.as_str())
                .map(|selection| selection.pane_id.as_str())
                .or_else(|| active_pane_id.as_ref().map(PaneId::as_str));
            if let Some(chosen) = chosen.filter(|chosen| Some(*chosen) != known) {
                result.push((id.to_string(), chosen.to_owned()));
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                collect_selection_ops(child, selected, sent, result);
            }
        }
        LayoutNode::Slot { .. } => {}
    }
}

fn collect_visible(
    node: &LayoutNode,
    selected: &HashMap<String, String>,
    visible: &mut Vec<String>,
) {
    match node {
        LayoutNode::Slot {
            id,
            pane_ids,
            active_pane_id,
        } => {
            let chosen = selected
                .get(id.as_str())
                .filter(|pane| pane_ids.iter().any(|id| id.as_str() == pane.as_str()))
                .map(String::as_str)
                .or_else(|| {
                    active_pane_id
                        .as_ref()
                        .filter(|pane| pane_ids.contains(pane))
                        .map(PaneId::as_str)
                })
                .or_else(|| pane_ids.first().map(PaneId::as_str));
            if let Some(pane) = chosen {
                visible.push(pane.to_owned());
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                collect_visible(child, selected, visible);
            }
        }
    }
}

fn reconcile_node(
    node: &LayoutNode,
    selected: &mut HashMap<String, String>,
    valid_slots: &mut HashSet<String>,
) {
    match node {
        LayoutNode::Slot {
            id,
            pane_ids,
            active_pane_id,
        } => {
            valid_slots.insert(id.to_string());
            if selected
                .get(id.as_str())
                .is_none_or(|pane| !pane_ids.iter().any(|id| id.as_str() == pane))
            {
                if let Some(pane) = active_pane_id
                    .as_ref()
                    .filter(|pane| pane_ids.contains(pane))
                    .or_else(|| pane_ids.first())
                {
                    selected.insert(id.to_string(), pane.to_string());
                } else {
                    selected.remove(id.as_str());
                }
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                reconcile_node(child, selected, valid_slots);
            }
        }
    }
}

fn routes(map: &HashMap<String, String>) -> Vec<SessionRoute> {
    map.iter()
        .map(|(pane_id, session_id)| SessionRoute {
            pane_id: pane_id.clone(),
            session_id: session_id.clone(),
        })
        .collect()
}

fn route_delta(old: &HashMap<String, String>, next: &HashMap<String, String>) -> WorkspaceDelta {
    WorkspaceDelta {
        added: routes(next)
            .into_iter()
            .filter(|route| old.get(&route.pane_id) != Some(&route.session_id))
            .collect(),
        removed: routes(old)
            .into_iter()
            .filter(|route| next.get(&route.pane_id) != Some(&route.session_id))
            .collect(),
        ..WorkspaceDelta::default()
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;
    use crate::{EmbeddedService, Event};
    use std::time::Duration;

    #[test]
    fn pending_spawns_count_toward_attachment_capacity_without_double_counting_routes() {
        let connection_id = Uuid::new_v4();
        let mut workspace = LocalWorkspace::new(
            ScreenIds {
                screen_id: "screen".into(),
                pane_id: "first".into(),
            },
            connection_id,
        );
        let mut routes = HashMap::from([("first".into(), "session-first".into())]);
        for index in 1..MAX_ATTACHMENTS - 1 {
            routes.insert(format!("pane-{index}"), format!("session-{index}"));
        }
        workspace.install_routes(routes);
        assert!(
            workspace.admit_new_pane().is_ok(),
            "first pending pane already has a route"
        );
        workspace.pending_panes.insert("not-yet-spawned".into());
        assert_eq!(
            workspace.admit_new_pane().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        workspace.observe_spawn_failure(connection_id, "not-yet-spawned");
        assert!(
            workspace.admit_new_pane().is_ok(),
            "a rejected spawn releases its slot"
        );
    }

    #[test]
    fn earlier_snapshot_keeps_pending_focus_until_new_pane_is_confirmed() {
        let connection_id = Uuid::new_v4();
        let mut workspace = LocalWorkspace::new(
            ScreenIds {
                screen_id: "screen".into(),
                pane_id: "first".into(),
            },
            connection_id,
        );
        workspace.pending_focus = Some("next".into());
        let first = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":"screen","revision":1,"root":{"type":"slot","id":"root","paneIds":["first"]},
                "panes":{"first":{"id":"first"}},"sessions":{}}
        });
        workspace.apply_snapshot(connection_id, &first).unwrap();
        assert_eq!(workspace.pending_focus.as_deref(), Some("next"));
        assert_eq!(workspace.focused_pane(), Some("first"));
        let confirmed = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":"screen","revision":2,"root":{"type":"slot","id":"root","paneIds":["first","next"]},
                "panes":{"first":{"id":"first"},"next":{"id":"next"}},"sessions":{}}
        });
        workspace.apply_snapshot(connection_id, &confirmed).unwrap();
        assert_eq!(workspace.focused_pane(), Some("next"));
        assert!(workspace.pending_focus.is_none());
        assert_eq!(
            workspace.selected_pane(&workspace.snapshot().unwrap().root),
            Some("next")
        );
    }

    #[test]
    fn closing_focused_split_falls_back_to_visible_selected_tab() {
        let connection_id = Uuid::new_v4();
        let mut workspace = LocalWorkspace::new(
            ScreenIds {
                screen_id: "screen".into(),
                pane_id: "a".into(),
            },
            connection_id,
        );
        let before = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":"screen","revision":1,
                "root":{"type":"split","id":"split","axis":"horizontal","sizes":[0.5,0.5],"children":[
                    {"type":"slot","id":"left","paneIds":["a","b"]},
                    {"type":"slot","id":"right","paneIds":["c"]}
                ]},
                "panes":{"a":{"id":"a"},"b":{"id":"b"},"c":{"id":"c"}},"sessions":{}}
        });
        workspace.apply_snapshot(connection_id, &before).unwrap();
        assert!(workspace.select_tab("b"));
        assert!(workspace.focus_pane("c"));
        let after = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":"screen","revision":2,
                "root":{"type":"slot","id":"left","paneIds":["a","b"]},
                "panes":{"a":{"id":"a"},"b":{"id":"b"}},"sessions":{}}
        });
        workspace.apply_snapshot(connection_id, &after).unwrap();
        assert_eq!(workspace.focused_pane(), Some("b"));
        assert_eq!(
            workspace.selected_pane(&workspace.snapshot().unwrap().root),
            Some("b")
        );
    }

    #[test]
    fn tab_sync_requires_capability_and_skips_already_known_selection() {
        let connection_id = Uuid::new_v4();
        let mut workspace = LocalWorkspace::new(
            ScreenIds {
                screen_id: "screen".into(),
                pane_id: "a".into(),
            },
            connection_id,
        );
        let snapshot = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":"screen","revision":1,
                "root":{"type":"slot","id":"root","paneIds":["a","b"],"activePaneId":"a"},
                "panes":{"a":{"id":"a"},"b":{"id":"b"}},"sessions":{}}
        });
        workspace.apply_snapshot(connection_id, &snapshot).unwrap();
        workspace.select_tab("b");
        assert!(workspace.selections_to_sync().is_empty());
        workspace.set_tab_selection_supported(true);
        assert_eq!(
            workspace.selections_to_sync(),
            vec![("root".into(), "b".into())]
        );
        workspace.sent_tab_selections.insert(
            "root".into(),
            SentTabSelection {
                pane_id: "b".into(),
                snapshot_revision: 1,
            },
        );
        assert!(workspace.selections_to_sync().is_empty());
        workspace.select_tab("a");
        assert_eq!(
            workspace.selections_to_sync(),
            vec![("root".into(), "a".into())]
        );
        workspace.set_tab_selection_supported(false);
        assert!(workspace.selections_to_sync().is_empty());
        assert!(workspace.sent_tab_selections.is_empty());
    }

    async fn next(client: &mut Client, workspace: &mut LocalWorkspace) -> Event {
        let event = tokio::time::timeout(Duration::from_secs(10), client.next_event())
            .await
            .expect("native event before timeout")
            .expect("valid native event")
            .expect("open native stream");
        if let Some(value) = event.json() {
            let delta = match value["type"].as_str() {
                Some("mux.screen_snapshot") => workspace
                    .apply_snapshot(event.connection_id, value)
                    .expect("typed snapshot"),
                Some("mux.pane_spawned") => workspace.observe_spawn(
                    event.connection_id,
                    value["paneId"].as_str().expect("pane ID"),
                    value["sessionId"].as_str().expect("session ID"),
                ),
                Some("mux.op_failed") => {
                    if let Some(pane_id) = value["paneId"].as_str() {
                        workspace.observe_spawn_failure(event.connection_id, pane_id)
                    } else {
                        WorkspaceDelta::default()
                    }
                }
                Some("attached") => {
                    workspace.observe_attached(
                        event.connection_id,
                        value["sessionId"].as_str().expect("attached session"),
                    );
                    WorkspaceDelta::default()
                }
                _ => WorkspaceDelta::default(),
            };
            for route in delta.added {
                client.attach(&route.session_id).expect("attach route");
            }
        }
        event
    }

    async fn until(
        client: &mut Client,
        workspace: &mut LocalWorkspace,
        mut done: impl FnMut(&LocalWorkspace) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !done(workspace) {
                next(client, workspace).await;
            }
        })
        .await
        .expect("workspace update before timeout");
    }

    #[tokio::test]
    async fn real_service_keeps_distinct_session_routes_through_split_tab_resize_and_close() {
        if !splitty_core::pty::testing::pty_available() {
            return;
        }
        let service = EmbeddedService::start();
        let mut client = service.connect().await.expect("embedded connection");
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), client.next_event())
                .await
                .expect("welcome timeout")
                .expect("welcome read")
                .expect("open stream");
            if event
                .json()
                .is_some_and(|value| value["type"] == "mux.welcome")
            {
                break;
            }
        }
        client.activity(true).expect("take activity");
        let ids = client
            .create_screen("Native workspace test")
            .expect("screen");
        let first_pane = ids.pane_id.clone();
        let mut workspace = LocalWorkspace::new(ids, client.connection_id());
        until(&mut client, &mut workspace, |workspace| {
            workspace.snapshot().is_some() && workspace.session_for(&first_pane).is_some()
        })
        .await;
        let first_session = workspace.session_for(&first_pane).unwrap().to_owned();
        let second_pane = workspace
            .split(&client, &first_pane, Axis::Horizontal)
            .expect("split operation");
        until(&mut client, &mut workspace, |workspace| {
            workspace.snapshot().is_some_and(|screen| {
                screen
                    .panes
                    .contains_key(&PaneId::from(second_pane.as_str()))
            }) && workspace.session_for(&second_pane).is_some()
        })
        .await;
        let second_session = workspace.session_for(&second_pane).unwrap().to_owned();
        assert_ne!(first_session, second_session);
        assert_eq!(
            workspace.session_for(&first_pane),
            Some(first_session.as_str())
        );
        let split_id = workspace.snapshot().unwrap().root.id().to_string();
        let third_pane = workspace.new_tab(&client, &second_pane).expect("new tab");
        until(&mut client, &mut workspace, |workspace| {
            workspace.snapshot().is_some_and(|screen| {
                screen
                    .panes
                    .contains_key(&PaneId::from(third_pane.as_str()))
            }) && workspace.session_for(&third_pane).is_some()
        })
        .await;
        let third_session = workspace.session_for(&third_pane).unwrap().to_owned();
        assert_ne!(third_session, second_session);
        assert_eq!(workspace.focused_pane(), Some(third_pane.as_str()));
        let slot = workspace
            .snapshot()
            .unwrap()
            .root
            .find_slot(&PaneId::from(second_pane.as_str()))
            .unwrap();
        assert_eq!(workspace.selected_pane(slot), Some(third_pane.as_str()));
        assert!(workspace.select_tab(&second_pane));
        let slot = workspace
            .snapshot()
            .unwrap()
            .root
            .find_slot(&PaneId::from(second_pane.as_str()))
            .unwrap();
        assert_eq!(workspace.selected_pane(slot), Some(second_pane.as_str()));
        workspace.set_tab_selection_supported(true);
        workspace
            .sync_selected_tabs(&client)
            .expect("persist selected tab");
        until(&mut client, &mut workspace, |workspace| {
            workspace
                .snapshot()
                .and_then(|screen| screen.root.find_slot(&PaneId::from(second_pane.as_str())))
                .is_some_and(|slot| {
                    matches!(slot, LayoutNode::Slot { active_pane_id: Some(active), .. } if active.as_str() == second_pane)
                })
        })
        .await;
        assert_eq!(workspace.focused_pane(), Some(second_pane.as_str()));

        workspace
            .resize_split(&client, &split_id, vec![0.3, 0.7])
            .expect("resize operation");
        until(&mut client, &mut workspace, |workspace| {
            matches!(&workspace.snapshot().unwrap().root, LayoutNode::Split { sizes, .. } if sizes == &vec![0.3, 0.7])
        })
        .await;
        assert_eq!(workspace.focused_pane(), Some(second_pane.as_str()));
        client
            .resync(workspace.screen_id())
            .expect("repeat snapshot");
        let repeated = loop {
            let event = next(&mut client, &mut workspace).await;
            if event
                .json()
                .is_some_and(|value| value["type"] == "mux.screen_snapshot")
            {
                break event;
            }
        };
        let delta = workspace
            .apply_snapshot(repeated.connection_id, repeated.json().unwrap())
            .expect("repeat typed snapshot");
        assert!(delta.added.is_empty() && delta.removed.is_empty());

        until(&mut client, &mut workspace, |workspace| {
            [&first_session, &second_session, &third_session]
                .iter()
                .all(|session| workspace.session_ready(session))
        })
        .await;
        for (session, marker) in [
            (&first_session, "ROUTE_FIRST"),
            (&second_session, "ROUTE_SECOND"),
            (&third_session, "ROUTE_THIRD"),
        ] {
            client
                .input(session, &format!("printf '{marker}\\n'\n"))
                .expect("session input");
        }
        let mut output = HashMap::<String, Vec<u8>>::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = next(&mut client, &mut workspace).await;
                if let Some(chunk) = event.output() {
                    if let Some(pane) = workspace.pane_for(chunk.session_id) {
                        output
                            .entry(pane.to_owned())
                            .or_default()
                            .extend_from_slice(chunk.bytes);
                    }
                }
                if [
                    (&first_pane, b"ROUTE_FIRST".as_slice()),
                    (&second_pane, b"ROUTE_SECOND".as_slice()),
                    (&third_pane, b"ROUTE_THIRD".as_slice()),
                ]
                .iter()
                .all(|(pane, marker)| {
                    output.get(*pane).is_some_and(|bytes| {
                        bytes.windows(marker.len()).any(|part| part == *marker)
                    })
                }) {
                    break;
                }
            }
        })
        .await
        .expect("distinct output markers");
        assert_eq!(
            workspace.pane_for(&first_session),
            Some(first_pane.as_str())
        );
        assert_eq!(
            workspace.pane_for(&second_session),
            Some(second_pane.as_str())
        );
        assert_eq!(
            workspace.pane_for(&third_session),
            Some(third_pane.as_str())
        );

        workspace
            .close_pane(&client, &third_pane)
            .expect("close tab");
        until(&mut client, &mut workspace, |workspace| {
            workspace.snapshot().is_some_and(|screen| {
                !screen
                    .panes
                    .contains_key(&PaneId::from(third_pane.as_str()))
            })
        })
        .await;
        assert_eq!(workspace.pane_for(&third_session), None);
        assert_eq!(
            workspace.session_for(&first_pane),
            Some(first_session.as_str())
        );
        assert_eq!(
            workspace.session_for(&second_pane),
            Some(second_session.as_str())
        );
        workspace
            .close_pane(&client, &second_pane)
            .expect("close split pane");
        until(&mut client, &mut workspace, |workspace| {
            workspace.snapshot().is_some_and(|screen| {
                !screen
                    .panes
                    .contains_key(&PaneId::from(second_pane.as_str()))
            })
        })
        .await;
        workspace
            .close_pane(&client, &first_pane)
            .expect("close final pane");
        until(&mut client, &mut workspace, |workspace| {
            workspace
                .snapshot()
                .is_some_and(|screen| screen.panes.is_empty())
        })
        .await;
        let old_screen_id = workspace.screen_id().to_owned();
        let old_screen_name = workspace.snapshot().unwrap().name.clone();
        let restored_pane = workspace
            .new_terminal(&client)
            .expect("refill empty screen");
        until(&mut client, &mut workspace, |workspace| {
            workspace.snapshot().is_some_and(|screen| {
                screen
                    .panes
                    .contains_key(&PaneId::from(restored_pane.as_str()))
            }) && workspace.session_for(&restored_pane).is_some()
        })
        .await;
        assert_eq!(workspace.screen_id(), old_screen_id);
        assert_eq!(workspace.snapshot().unwrap().name, old_screen_name);
        assert_eq!(workspace.focused_pane(), Some(restored_pane.as_str()));
        let old_connection = client.connection_id();
        let revoked = workspace.reset_connection(Uuid::new_v4());
        assert_eq!(revoked.removed.len(), 1);
        assert_eq!(workspace.pane_for(&first_session), None);
        assert!(workspace
            .observe_spawn(old_connection, &first_pane, &first_session)
            .added
            .is_empty());
        client.finish().await.expect("finish connection");
        service.close().await.expect("close owned service");
    }
}
