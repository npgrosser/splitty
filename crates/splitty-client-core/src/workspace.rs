//! Service-owned local screens, projected without a second layout tree.
mod catalog;
use crate::{
    connection::{invalid, MAX_ATTACHMENTS},
    Client, ScreenIds,
};
pub use catalog::LocalWorkspace;
use serde::Deserialize;
use serde_json::Value;
pub use splitty_core::mux::{Axis, Direction, LayoutNode, ScreenInfo, ScreenState};
use splitty_core::{
    ids::{NodeId, PaneId, SessionId},
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
    pub placement: Option<PlacementOutcome>,
}

/// An exact tab intent has reached its ordered verification point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementOutcome {
    Confirmed,
    Rejected(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabStep {
    Previous,
    Next,
}

/// A native close request names the exact scope the user chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseTarget {
    Pane { screen: String, pane: String },
    Screen { screen: String },
    Window,
}

/// Canonical membership captured when a close decision was shown.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CloseIdentity {
    Catalog,
    Screen(String),
    Pane {
        screen: String,
        pane: String,
        session: Option<String>,
    },
    Session {
        screen: String,
        session: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseAffected {
    pub identity: CloseIdentity,
    pub busy: bool,
    pub unresolved: bool,
}

/// A bounded, UI-free assessment. Busy and unresolved counts describe the
/// prompt; identities and connection generation bind its eventual approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosePlan {
    pub request_id: u64,
    pub connection_id: Option<Uuid>,
    pub target: CloseTarget,
    pub affected: Vec<CloseAffected>,
    pub busy_count: usize,
    pub unresolved_count: usize,
}

impl ClosePlan {
    pub fn needs_confirmation(&self) -> bool {
        self.busy_count != 0 || self.unresolved_count != 0
    }

    /// Before discovery, the only meaningful target is the owned window.
    pub fn startup_window(request_id: u64) -> Self {
        Self {
            request_id,
            connection_id: None,
            target: CloseTarget::Window,
            affected: vec![CloseAffected {
                identity: CloseIdentity::Catalog,
                busy: false,
                unresolved: true,
            }],
            busy_count: 0,
            unresolved_count: 1,
        }
    }
}

/// A drag capability is meaningful only within its captured connection and snapshot.
#[derive(Clone, Debug)]
pub struct PaneDrag {
    connection_id: Uuid,
    screen_id: String,
    revision: u64,
    pane: String,
    session: String,
    source_slot: String,
    snapshot: Arc<ScreenState>,
}

impl PaneDrag {
    pub fn pane_id(&self) -> &str {
        &self.pane
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovePlacement {
    Tab,
    Left,
    Right,
    Above,
    Below,
}
impl MovePlacement {
    fn direction(self) -> Option<Direction> {
        match self {
            Self::Tab => None,
            Self::Left => Some(Direction::Left),
            Self::Right => Some(Direction::Right),
            Self::Above => Some(Direction::Above),
            Self::Below => Some(Direction::Below),
        }
    }
}

#[derive(Clone)]
struct PendingMove {
    drag: PaneDrag,
    target: String,
    placement: MovePlacement,
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
pub struct ScreenModel {
    ids: ScreenIds,
    connection_id: Uuid,
    snapshot: Option<Arc<ScreenState>>,
    focused_pane: Option<String>,
    maximized_slot: Option<String>,
    pending_focus: Option<String>,
    pending_move: Option<PendingMove>,
    selected_tabs: HashMap<String, String>,
    tab_selection_supported: bool,
    sent_tab_selections: HashMap<String, SentTabSelection>,
    pending_panes: HashSet<String>,
    pending_recreation: Option<String>,
    pane_sessions: HashMap<String, String>,
    session_panes: HashMap<String, String>,
    ready_sessions: HashSet<String>,
    resync_pending: bool,
}

impl ScreenModel {
    pub fn new(ids: ScreenIds, connection_id: Uuid) -> Self {
        let focused_pane = Some(ids.pane_id.clone());
        let pending_panes = HashSet::from([ids.pane_id.clone()]);
        Self {
            ids,
            connection_id,
            snapshot: None,
            focused_pane,
            maximized_slot: None,
            pending_focus: None,
            pending_move: None,
            selected_tabs: HashMap::new(),
            tab_selection_supported: false,
            sent_tab_selections: HashMap::new(),
            pending_panes,
            pending_recreation: None,
            pane_sessions: HashMap::new(),
            session_panes: HashMap::new(),
            ready_sessions: HashSet::new(),
            resync_pending: false,
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

    /// A window-local projection of the authoritative layout. The slot is
    /// borrowed from the current snapshot, never copied into another tree.
    pub fn presentation_root(&self) -> Option<&LayoutNode> {
        let screen = self.snapshot()?;
        Some(
            self.maximized_slot
                .as_ref()
                .and_then(|id| screen.root.find_node(&NodeId::from(id.as_str())))
                .filter(|node| matches!(node, LayoutNode::Slot { pane_ids, .. } if !pane_ids.is_empty()))
                .unwrap_or(&screen.root),
        )
    }

    pub fn maximized_slot_id(&self) -> Option<&str> {
        self.maximized_slot.as_deref()
    }

    pub fn focused_slot_id(&self) -> Option<&str> {
        self.snapshot()?
            .root
            .find_slot(&PaneId::from(self.focused_pane()?))
            .map(|slot| slot.id().as_str())
    }

    pub fn displayed_panes(&self) -> Vec<String> {
        let mut panes = Vec::new();
        if let Some(root) = self.presentation_root() {
            collect_visible(root, &self.selected_tabs, &mut panes);
        }
        panes
    }

    pub fn is_displayed_pane(&self, pane: &str) -> bool {
        self.presentation_root()
            .and_then(|root| root.find_slot(&PaneId::from(pane)))
            .is_some_and(|slot| self.selected_pane(slot) == Some(pane))
    }

    pub fn toggle_maximize_focused(&mut self) -> bool {
        if self.maximized_slot.is_some() {
            return self.set_maximized_slot(None, None);
        }
        let slot = self.focused_slot_id().map(str::to_owned);
        let pane = self.focused_pane().map(str::to_owned);
        self.set_maximized_slot(slot.as_deref(), pane.as_deref())
    }

    /// Apply an intent captured at the gesture, ignoring a slot whose pane
    /// changed before the worker received it.
    pub fn set_maximized_slot(&mut self, slot: Option<&str>, pane: Option<&str>) -> bool {
        match (slot, pane) {
            (None, None) => self.maximized_slot.take().is_some(),
            (Some(slot), Some(pane)) => {
                let valid = self.snapshot().is_some_and(|screen| {
                    matches!(screen.root.find_node(&NodeId::from(slot)),
                        Some(LayoutNode::Slot { pane_ids, .. })
                            if pane_ids.iter().any(|id| id.as_str() == pane))
                });
                if !valid || self.maximized_slot.as_deref() == Some(slot) {
                    return false;
                }
                self.maximized_slot = Some(slot.to_owned());
                if !self.is_displayed_pane(self.focused_pane().unwrap_or("")) {
                    self.focused_pane = self.displayed_panes().into_iter().next();
                }
                true
            }
            _ => false,
        }
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
        self.pending_move = None;
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(LayoutNode::Slot { id, .. }) = find_slot(snapshot, pane_id) else {
            return false;
        };
        if self
            .maximized_slot
            .as_deref()
            .is_some_and(|maximized| maximized != id.as_str())
        {
            self.maximized_slot = None;
        }
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
            client.submit_screen_op(
                self.screen_id(),
                &ScreenOp::SelectTab {
                    pane_id: PaneId::from(pane_id.as_str()),
                },
            )?;
            self.resync_pending = true;
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
        self.pending_move = None;
        self.sent_tab_selections.clear();
        self.maximized_slot = None;
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
        self.confirm_move();
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

    pub fn capture_drag(&self, pane: &str) -> Option<PaneDrag> {
        let screen = self.snapshot()?;
        let slot = screen.root.find_slot(&PaneId::from(pane))?;
        Some(PaneDrag {
            connection_id: self.connection_id,
            screen_id: self.screen_id().to_owned(),
            revision: screen.revision,
            pane: pane.to_owned(),
            session: self.session_for(pane)?.to_owned(),
            source_slot: slot.id().to_string(),
            snapshot: self.snapshot.as_ref()?.clone(),
        })
    }

    /// No optimistic layout is installed. None is an intentional same-slot no-op.
    fn move_op(
        &self,
        drag: &PaneDrag,
        target: &str,
        placement: MovePlacement,
    ) -> io::Result<Option<ScreenOp>> {
        if self.pending_move.is_some() {
            return Err(invalid(
                "A pane move is awaiting its authoritative snapshot",
            ));
        }
        let screen = self
            .snapshot()
            .ok_or_else(|| invalid("screen snapshot not loaded"))?;
        if drag.connection_id != self.connection_id
            || drag.screen_id != self.screen_id()
            || !same_layout(&drag.snapshot.root, &screen.root)
            || self.session_for(&drag.pane) != Some(drag.session.as_str())
        {
            return Err(invalid("Pane drag is stale; drag the tab again"));
        }
        let source = screen
            .root
            .find_slot(&PaneId::from(drag.pane.as_str()))
            .ok_or_else(|| invalid("dragged pane is no longer in the layout"))?;
        if source.id().as_str() != drag.source_slot {
            return Err(invalid("dragged pane changed slots"));
        }
        if !matches!(
            screen.root.find_node(&NodeId::from(target)),
            Some(LayoutNode::Slot { .. })
        ) {
            return Err(invalid("drop destination is no longer a slot"));
        }
        let LayoutNode::Slot { pane_ids, .. } = source else {
            unreachable!()
        };
        if source.id().as_str() == target
            && (placement == MovePlacement::Tab || pane_ids.len() == 1)
        {
            return Ok(None);
        }
        let source_resulting_sizes = if pane_ids.len() == 1 {
            screen
                .root
                .find_parent_info(source.id())
                .and_then(|parent| {
                    let LayoutNode::Split { sizes, .. } = screen.root.find_node(&parent.id)? else {
                        return None;
                    };
                    let mut remaining: Vec<_> = sizes
                        .iter()
                        .enumerate()
                        .filter_map(|(i, size)| (i != parent.index).then_some(*size))
                        .collect();
                    let total: f64 = remaining.iter().sum();
                    let equal = 1. / remaining.len().max(1) as f64;
                    for size in &mut remaining {
                        *size = if total > 0. { *size / total } else { equal };
                    }
                    Some(remaining)
                })
        } else {
            None
        };
        if let Some(sizes) = &source_resulting_sizes {
            if sizes.is_empty()
                || sizes.iter().any(|size| !size.is_finite() || *size < 0.)
                || (sizes.iter().sum::<f64>() - 1.).abs() > 0.01
            {
                return Err(invalid("source split has invalid proportions"));
            }
        }
        Ok(Some(ScreenOp::MovePane {
            pane_id: PaneId::from(drag.pane.as_str()),
            target_slot_id: (placement == MovePlacement::Tab).then(|| NodeId::from(target)),
            target_split_id: (placement != MovePlacement::Tab).then(|| NodeId::from(target)),
            direction: placement.direction(),
            source_resulting_sizes,
            resulting_sizes: None,
        }))
    }

    pub fn check_move(
        &self,
        drag: &PaneDrag,
        target: &str,
        placement: MovePlacement,
    ) -> io::Result<bool> {
        self.move_op(drag, target, placement).map(|op| op.is_some())
    }

    pub fn can_move(&self, drag: &PaneDrag, target: &str, placement: MovePlacement) -> bool {
        self.check_move(drag, target, placement).unwrap_or(false)
    }

    pub fn move_pane(
        &mut self,
        client: &Client,
        drag: PaneDrag,
        target: String,
        placement: MovePlacement,
    ) -> io::Result<bool> {
        let Some(op) = self.move_op(&drag, &target, placement)? else {
            return Ok(false);
        };
        self.send_op(client, &op)?;
        self.maximized_slot = None;
        self.pending_move = Some(PendingMove {
            drag,
            target,
            placement,
        });
        Ok(true)
    }

    pub fn reject_move(&mut self, connection: Uuid) {
        if connection == self.connection_id {
            self.pending_move = None;
        }
    }

    fn confirm_move(&mut self) {
        let Some(pending) = self.pending_move.as_ref() else {
            return;
        };
        let Some(screen) = self.snapshot() else {
            return;
        };
        if screen.revision <= pending.drag.revision {
            return;
        }
        if self.session_for(&pending.drag.pane) != Some(pending.drag.session.as_str()) {
            self.pending_move = None;
            return;
        }
        let Some(moved) = screen
            .root
            .find_slot(&PaneId::from(pending.drag.pane.as_str()))
        else {
            return;
        };
        let confirmed = if pending.placement == MovePlacement::Tab {
            moved.id().as_str() == pending.target
        } else {
            let moved_parent = screen.root.find_parent_info(moved.id());
            let target_parent = screen
                .root
                .find_parent_info(&NodeId::from(pending.target.as_str()));
            moved_parent.zip(target_parent).is_some_and(|(a, b)| {
                let before = matches!(pending.placement, MovePlacement::Left | MovePlacement::Above);
                let axis = if matches!(pending.placement, MovePlacement::Left | MovePlacement::Right) { Axis::Horizontal } else { Axis::Vertical };
                a.id == b.id && a.index == usize::from(!before) && b.index == usize::from(before)
                    && matches!(screen.root.find_node(&a.id), Some(LayoutNode::Split { axis: actual, children, .. }) if *actual == axis && children.len() == 2)
            })
        };
        if confirmed {
            let pane = pending.drag.pane.clone();
            self.pending_move = None;
            self.select_tab(&pane);
        }
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
        self.pending_panes.insert(new_pane_id.clone());
        if let Err(error) = self.send_op(client, &op) {
            self.pending_panes.remove(&new_pane_id);
            return Err(error);
        }
        self.maximized_slot = None;
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
        self.pending_panes.insert(new_pane_id.clone());
        if let Err(error) = self.send_op(client, &op) {
            self.pending_panes.remove(&new_pane_id);
            return Err(error);
        }
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
        self.pending_panes.insert(pane_id.clone());
        if let Err(error) = client.submit_recreate_screen(self.screen_id(), &pane_id) {
            self.pending_panes.remove(&pane_id);
            return Err(error);
        }
        self.resync_pending = true;
        self.pending_focus = Some(pane_id.clone());
        self.pending_recreation = Some(pane_id.clone());
        Ok(pane_id)
    }

    pub fn close_pane(&mut self, client: &Client, pane_id: &str) -> io::Result<()> {
        self.send_op(
            client,
            &ScreenOp::ClosePane {
                pane_id: PaneId::from(pane_id),
                resulting_sizes: None,
            },
        )
    }

    pub fn resize_split(
        &mut self,
        client: &Client,
        node_id: &str,
        sizes: Vec<f64>,
    ) -> io::Result<()> {
        self.send_op(
            client,
            &ScreenOp::ResizeSplit {
                node_id: NodeId::from(node_id),
                sizes,
            },
        )
    }

    fn send_op(&mut self, client: &Client, op: &ScreenOp) -> io::Result<()> {
        if client.connection_id() != self.connection_id {
            return Err(invalid("workspace connection changed"));
        }
        let screen = self
            .snapshot()
            .ok_or_else(|| invalid("screen snapshot not loaded"))?;
        mux::validate_screen_op(screen, op).map_err(io::Error::other)?;
        client.submit_screen_op(&self.ids.screen_id, op)?;
        self.resync_pending = true;
        Ok(())
    }

    fn flush_resync(&mut self, client: &Client) -> io::Result<()> {
        if self.resync_pending {
            client.resync(self.screen_id())?;
            self.resync_pending = false;
        }
        Ok(())
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
        if self.maximized_slot.as_ref().is_some_and(|slot| {
            !matches!(
                screen.root.find_node(&NodeId::from(slot.as_str())),
                Some(LayoutNode::Slot { pane_ids, .. }) if !pane_ids.is_empty()
            )
        }) {
            self.maximized_slot = None;
        }
        let mut visible = Vec::new();
        let root = self
            .maximized_slot
            .as_ref()
            .and_then(|id| screen.root.find_node(&NodeId::from(id.as_str())))
            .unwrap_or(&screen.root);
        collect_visible(root, &self.selected_tabs, &mut visible);
        if self.maximized_slot.is_none() {
            for floating in &screen.floating {
                collect_visible(&floating.root, &self.selected_tabs, &mut visible);
            }
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

// Compare only the canonical layout, allowing titles/activity and tab focus to
// advance without making an otherwise valid drag of a running shell stale.
fn same_layout(a: &LayoutNode, b: &LayoutNode) -> bool {
    match (a, b) {
        (
            LayoutNode::Slot {
                id: a,
                pane_ids: ap,
                ..
            },
            LayoutNode::Slot {
                id: b,
                pane_ids: bp,
                ..
            },
        ) => a == b && ap == bp,
        (
            LayoutNode::Split {
                id: a,
                axis: aa,
                sizes: az,
                children: ac,
            },
            LayoutNode::Split {
                id: b,
                axis: ba,
                sizes: bz,
                children: bc,
            },
        ) => {
            a == b
                && aa == ba
                && az == bz
                && ac.len() == bc.len()
                && ac.iter().zip(bc).all(|(a, b)| same_layout(a, b))
        }
        _ => false,
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

    fn drag_workspace() -> ScreenModel {
        let mut model = ScreenModel::new(
            ScreenIds {
                screen_id: "screen".into(),
                pane_id: "a".into(),
            },
            Uuid::new_v4(),
        );
        let event = serde_json::json!({"type":"mux.screen_snapshot", "screen": {
            "id":"screen", "revision":1,
            "root":{"type":"split","id":"root","axis":"horizontal","sizes":[0.2,0.3,0.5],"children":[
                {"type":"slot","id":"slot-a","paneIds":["a"]},
                {"type":"slot","id":"slot-b","paneIds":["b"]},
                {"type":"slot","id":"slot-c","paneIds":["c"]}]},
            "panes":{"a":{"id":"a","sessionId":"sa"},"b":{"id":"b","sessionId":"sb"},"c":{"id":"c","sessionId":"sc"}},
            "sessions":{"sa":{"id":"sa","status":"running"},"sb":{"id":"sb","status":"running"},"sc":{"id":"sc","status":"running"}}
        }});
        model.apply_snapshot(model.connection_id, &event).unwrap();
        model
    }

    fn apply_screen(model: &mut ScreenModel, screen: ScreenState) -> WorkspaceDelta {
        model
            .apply_snapshot(
                model.connection_id,
                &serde_json::json!({"type":"mux.screen_snapshot","screen":screen}),
            )
            .unwrap()
    }

    #[test]
    fn pane_drag_rejects_changed_structure_or_route_but_allows_metadata() {
        let mut model = drag_workspace();
        let drag = model.capture_drag("c").unwrap();
        assert!(model.can_move(&drag, "slot-a", MovePlacement::Tab));
        let mut metadata = model.snapshot().unwrap().clone();
        metadata.revision += 1;
        metadata.panes.get_mut(&PaneId::from("c")).unwrap().title = Some("busy shell".into());
        apply_screen(&mut model, metadata);
        assert!(model.can_move(&drag, "slot-a", MovePlacement::Tab));
        assert!(model.check_move(&drag, "gone", MovePlacement::Tab).is_err());
        model.pane_sessions.insert("c".into(), "replacement".into());
        assert!(!model.can_move(&drag, "slot-a", MovePlacement::Tab));
        model.pane_sessions.insert("c".into(), "sc".into());
        let mut resized = model.snapshot().unwrap().clone();
        resized.revision += 1;
        if let LayoutNode::Split { sizes, .. } = &mut resized.root {
            *sizes = vec![0.3, 0.2, 0.5];
        }
        apply_screen(&mut model, resized);
        assert!(!model.can_move(&drag, "slot-a", MovePlacement::Tab));
        let fresh = model.capture_drag("c").unwrap();
        let same_screen = model.snapshot().unwrap().clone();
        model.reset_connection(Uuid::new_v4());
        apply_screen(&mut model, same_screen);
        assert!(!model.can_move(&fresh, "slot-a", MovePlacement::Tab));
    }

    #[test]
    fn pane_move_keeps_proportions_and_selects_only_confirmed_placement() {
        let mut model = drag_workspace();
        let drag = model.capture_drag("c").unwrap();
        assert!(model
            .move_op(&drag, "slot-c", MovePlacement::Tab)
            .unwrap()
            .is_none());
        assert!(model
            .move_op(&drag, "slot-c", MovePlacement::Left)
            .unwrap()
            .is_none());
        let op = model
            .move_op(&drag, "slot-a", MovePlacement::Tab)
            .unwrap()
            .unwrap();
        match &op {
            ScreenOp::MovePane {
                source_resulting_sizes: Some(sizes),
                ..
            } => assert_eq!(sizes, &vec![0.4, 0.6]),
            _ => panic!("source proportions are required"),
        }
        model.pending_move = Some(PendingMove {
            drag,
            target: "slot-a".into(),
            placement: MovePlacement::Tab,
        });
        let mut earlier = model.snapshot().unwrap().clone();
        earlier.revision += 1; // An unrelated metadata snapshot is not a move acknowledgement.
        apply_screen(&mut model, earlier);
        assert!(model.pending_move.is_some());
        assert_eq!(model.focused_pane(), Some("a"));
        let (next, effects) = mux::reduce_screen(model.snapshot().unwrap().clone(), op).unwrap();
        assert!(effects.is_empty());
        let delta = apply_screen(&mut model, next);
        assert!(delta.added.is_empty() && delta.removed.is_empty());
        assert_eq!(model.focused_pane(), Some("c"));
        assert_eq!(model.session_for("c"), Some("sc"));
        assert!(model.pending_move.is_none());
        let same_slot = model.capture_drag("c").unwrap();
        assert!(model.can_move(&same_slot, "slot-a", MovePlacement::Below));
        model.pending_move = Some(PendingMove {
            drag: same_slot,
            target: "slot-a".into(),
            placement: MovePlacement::Below,
        });
        model.reject_move(Uuid::new_v4());
        assert!(model.pending_move.is_some());
        model.reject_move(model.connection_id);
        assert!(model.pending_move.is_none());
    }

    #[test]
    fn all_four_edges_confirm_without_session_effects_and_user_focus_cancels_deferred_focus() {
        for placement in [
            MovePlacement::Left,
            MovePlacement::Right,
            MovePlacement::Above,
            MovePlacement::Below,
        ] {
            let mut model = drag_workspace();
            let drag = model.capture_drag("c").unwrap();
            let op = model.move_op(&drag, "slot-a", placement).unwrap().unwrap();
            model.pending_move = Some(PendingMove {
                drag: drag.clone(),
                target: "slot-a".into(),
                placement,
            });
            let (next, effects) =
                mux::reduce_screen(model.snapshot().unwrap().clone(), op).unwrap();
            assert!(effects.is_empty());
            apply_screen(&mut model, next);
            assert_eq!(model.focused_pane(), Some("c"));
            assert!(model.pending_move.is_none());
            model.pending_move = Some(PendingMove {
                drag,
                target: "slot-a".into(),
                placement,
            });
            model.select_tab("b");
            assert!(model.pending_move.is_none());
        }
    }

    #[test]
    fn pending_spawns_count_toward_attachment_capacity_without_double_counting_routes() {
        let connection_id = Uuid::new_v4();
        let mut workspace = ScreenModel::new(
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
        let mut workspace = ScreenModel::new(
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
        let mut workspace = ScreenModel::new(
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
        let mut workspace = ScreenModel::new(
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

    async fn next(client: &mut Client, workspace: &mut ScreenModel) -> Event {
        workspace.flush_resync(client).expect("screen resync");
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
        workspace: &mut ScreenModel,
        mut done: impl FnMut(&ScreenModel) -> bool,
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
        let mut workspace = ScreenModel::new(ids, client.connection_id());
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

        // Moving a real terminal changes only its layout: attachment, shell and
        // pane-to-session route remain stable through stack and edge placement.
        let moved = workspace
            .split(&client, &second_pane, Axis::Horizontal)
            .expect("extra real terminal");
        until(&mut client, &mut workspace, |w| {
            w.session_for(&moved).is_some()
                && w.snapshot().is_some_and(|screen| {
                    screen
                        .root
                        .find_slot(&PaneId::from(moved.as_str()))
                        .is_some()
                })
        })
        .await;
        let moved_session = workspace.session_for(&moved).unwrap().to_owned();
        let root = workspace.snapshot().unwrap().root.id().to_string();
        workspace
            .resize_split(&client, &root, vec![0.2, 0.3, 0.5])
            .unwrap();
        until(&mut client, &mut workspace, |w| matches!(&w.snapshot().unwrap().root,LayoutNode::Split{sizes,..} if sizes == &vec![0.2,0.3,0.5])).await;
        let target = workspace
            .snapshot()
            .unwrap()
            .root
            .find_slot(&PaneId::from(first_pane.as_str()))
            .unwrap()
            .id()
            .to_string();
        let drag = workspace.capture_drag(&moved).unwrap();
        workspace
            .move_pane(&client, drag, target.clone(), MovePlacement::Tab)
            .expect("move into existing stack");
        until(&mut client, &mut workspace, |w| w.pending_move.is_none()).await;
        assert_eq!(workspace.focused_pane(), Some(moved.as_str()));
        assert!(
            matches!(&workspace.snapshot().unwrap().root,LayoutNode::Split{sizes,..} if sizes == &vec![0.4,0.6])
        );
        for placement in [MovePlacement::Above, MovePlacement::Tab] {
            let drag = workspace.capture_drag(&moved).unwrap();
            workspace
                .move_pane(&client, drag, target.clone(), placement)
                .expect("move own tab at edge and return");
            until(&mut client, &mut workspace, |w| w.pending_move.is_none()).await;
            assert_eq!(workspace.session_for(&moved), Some(moved_session.as_str()));
            assert_eq!(workspace.focused_pane(), Some(moved.as_str()));
        }
        until(&mut client, &mut workspace, |w| {
            w.session_ready(&moved_session)
        })
        .await;
        client
            .input(&moved_session, "printf 'MOVED_TERMINAL_ALIVE\\n'\n")
            .unwrap();
        let mut moved_output = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !moved_output
                .windows(b"MOVED_TERMINAL_ALIVE".len())
                .any(|part| part == b"MOVED_TERMINAL_ALIVE")
            {
                let event = next(&mut client, &mut workspace).await;
                if let Some(chunk) = event
                    .output()
                    .filter(|chunk| chunk.session_id == moved_session)
                {
                    moved_output.extend_from_slice(chunk.bytes);
                }
            }
        })
        .await
        .expect("moved session continues receiving input");
        workspace.close_pane(&client, &moved).unwrap();
        until(&mut client, &mut workspace, |w| {
            w.session_for(&moved).is_none()
        })
        .await;

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
