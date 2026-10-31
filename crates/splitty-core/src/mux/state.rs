//! Mux state types: the data model for the terminal multiplexer.
//!
//! Key types:
//! - `WorkspaceState` -- top-level: a map of screens plus the active screen and screen order.
//! - `ScreenState` -- a single screen with its layout tree, panes, sessions, and floating windows.
//! - `LayoutNode` -- recursive tree: `Slot` (leaf with pane stack) or `Split` (branch with axis/sizes).
//! - `PaneState` / `SessionEntry` -- metadata for panes and PTY sessions.
//! - `FloatingEntry` -- a detached overlay window with its own layout and bounds.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ids::*;

/// Split axis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// Session lifecycle status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Starting,
    Running,
    Exited,
}

/// Pane role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PaneRole {
    #[default]
    Terminal,
    Webview,
    Viewer,
}

/// Relative position/size within the viewport (0.0–1.0).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloatBounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Default for FloatBounds {
    fn default() -> Self {
        Self {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        }
    }
}

/// A node in the layout tree — either a Slot (leaf) or Split (branch).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LayoutNode {
    #[serde(rename = "slot", rename_all = "camelCase")]
    Slot {
        id: NodeId,
        pane_ids: Vec<PaneId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_pane_id: Option<PaneId>,
    },
    #[serde(rename = "split")]
    Split {
        id: NodeId,
        axis: Axis,
        children: Vec<LayoutNode>,
        sizes: Vec<f64>,
    },
}

/// A floating window overlay with its own layout tree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloatingEntry {
    pub id: FloatingId,
    pub root: LayoutNode,
    pub bounds: FloatBounds,
}

/// A UI viewport in the layout. References a session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneState {
    pub id: PaneId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<JobId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<PaneRole>,
}

/// A PTY process record in the mux state (not the actual PTY — just the data).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<JobId>,
    #[serde(default, skip_serializing_if = "super::jobs::RestartPolicy::is_shell")]
    pub restart_policy: super::jobs::RestartPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The user's scratch text for this session (`session-scratchpad`), empty
    /// when none. It lives and ends with the record; the control API never
    /// hands it out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scratchpad: String,
}

/// The longest scratchpad, in bytes of UTF-8.
pub const SCRATCHPAD_LIMIT: usize = 64 * 1024;

/// Complete state of a single screen.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenState {
    /// Snapshot projection only; authoritative records live in WorkspaceState.jobs.
    #[serde(default, skip_deserializing, skip_serializing_if = "HashMap::is_empty")]
    pub jobs: HashMap<JobId, super::jobs::JobSnapshot>,
    pub id: ScreenId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub revision: u64,
    pub root: LayoutNode,
    #[serde(default)]
    pub floating: Vec<FloatingEntry>,
    pub panes: HashMap<PaneId, PaneState>,
    pub sessions: HashMap<SessionId, SessionEntry>,
}

/// Top-level workspace state: multiple screens, one active.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub screens: HashMap<ScreenId, ScreenState>,
    #[serde(default)]
    pub jobs: HashMap<JobId, super::jobs::JobRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_screen_id: Option<ScreenId>,
    /// Ordered list of screen IDs. Persists screen ordering across reconnects.
    #[serde(default)]
    pub screen_order: Vec<ScreenId>,
}

/// Summary of a screen (for welcome message, not the full state).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenInfo {
    pub id: ScreenId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Info about a node's parent in the layout tree.
#[derive(Clone, Debug)]
pub struct ParentInfo {
    pub id: NodeId,
    pub index: usize,
    pub axis: Axis,
    pub num_children: usize,
}

/// Where a pane's slot was found.
#[derive(Clone, Debug)]
pub enum SlotLocation {
    Root,
    Floating(usize),
}

// ── LayoutNode constructors and helpers ────────────────────────────────────

impl LayoutNode {
    /// Create a leaf slot containing the given panes.
    pub fn new_slot(id: NodeId, pane_ids: Vec<PaneId>) -> Self {
        Self::Slot {
            id,
            pane_ids,
            active_pane_id: None,
        }
    }

    /// Create a branch split with the given axis, children, and sizes.
    pub fn new_split(id: NodeId, axis: Axis, children: Vec<Self>, sizes: Vec<f64>) -> Self {
        Self::Split {
            id,
            axis,
            children,
            sizes,
        }
    }

    /// Get this node's ID.
    pub fn id(&self) -> &NodeId {
        match self {
            Self::Slot { id, .. } | Self::Split { id, .. } => id,
        }
    }

    /// Find the slot that contains the given pane ID.
    pub fn find_slot(&self, pane_id: &PaneId) -> Option<&Self> {
        match self {
            Self::Slot { pane_ids, .. } => {
                if pane_ids.contains(pane_id) {
                    Some(self)
                } else {
                    None
                }
            }
            Self::Split { children, .. } => {
                for child in children {
                    if let Some(found) = child.find_slot(pane_id) {
                        return Some(found);
                    }
                }
                None
            }
        }
    }

    /// Find a node by its ID.
    pub fn find_node(&self, node_id: &NodeId) -> Option<&Self> {
        if self.id() == node_id {
            return Some(self);
        }
        if let Self::Split { children, .. } = self {
            for child in children {
                if let Some(found) = child.find_node(node_id) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Find a mutable reference to a node by its ID.
    pub fn find_node_mut(&mut self, node_id: &NodeId) -> Option<&mut Self> {
        if self.id() == node_id {
            return Some(self);
        }
        if let Self::Split { children, .. } = self {
            for child in children {
                if let Some(found) = child.find_node_mut(node_id) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Find the parent of a node by ID. Returns parent info including index and axis.
    pub fn find_parent_info(&self, node_id: &NodeId) -> Option<ParentInfo> {
        if let Self::Split {
            id, axis, children, ..
        } = self
        {
            for (i, child) in children.iter().enumerate() {
                if child.id() == node_id {
                    return Some(ParentInfo {
                        id: id.clone(),
                        index: i,
                        axis: axis.clone(),
                        num_children: children.len(),
                    });
                }
            }
            for child in children {
                if let Some(info) = child.find_parent_info(node_id) {
                    return Some(info);
                }
            }
        }
        None
    }

    /// Walk the tree depth-first. Return false from the callback to stop.
    pub fn walk(&self, f: &mut impl FnMut(&LayoutNode) -> bool) -> bool {
        if !f(self) {
            return false;
        }
        if let Self::Split { children, .. } = self {
            for child in children {
                if !child.walk(f) {
                    return false;
                }
            }
        }
        true
    }

    /// Count total panes in this subtree.
    pub fn count_panes(&self) -> usize {
        let mut count = 0;
        self.walk(&mut |node| {
            if let Self::Slot { pane_ids, .. } = node {
                count += pane_ids.len();
            }
            true
        });
        count
    }

    /// Collect all pane IDs in this subtree.
    pub fn collect_pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.walk(&mut |node| {
            if let Self::Slot { pane_ids, .. } = node {
                ids.extend(pane_ids.iter().cloned());
            }
            true
        });
        ids
    }
}

/// Replace a node with the given ID in the tree.
pub fn replace_node(root: &mut LayoutNode, node_id: &NodeId, replacement: LayoutNode) {
    if root.id() == node_id {
        *root = replacement;
        return;
    }
    if let LayoutNode::Split { children, .. } = root {
        for child in children.iter_mut() {
            if child.id() == node_id {
                *child = replacement;
                return;
            }
            replace_node(child, node_id, replacement.clone());
        }
    }
}

// ── ScreenState constructors and helpers ───────────────────────────────────

impl ScreenState {
    /// Create a new empty screen with a single root slot.
    pub fn new(id: ScreenId, name: Option<String>) -> Self {
        let root_id = NodeId::with_prefix("root");
        Self {
            id,
            name,
            jobs: HashMap::new(),
            revision: 0,
            root: LayoutNode::new_slot(root_id, vec![]),
            floating: vec![],
            panes: HashMap::new(),
            sessions: HashMap::new(),
        }
    }

    /// Find the slot location for a pane (root or floating).
    pub fn find_slot_location(&self, pane_id: &PaneId) -> Option<SlotLocation> {
        if self.root.find_slot(pane_id).is_some() {
            return Some(SlotLocation::Root);
        }
        for (i, entry) in self.floating.iter().enumerate() {
            if entry.root.find_slot(pane_id).is_some() {
                return Some(SlotLocation::Floating(i));
            }
        }
        None
    }

    /// Count total panes in the root layout (not floating).
    pub fn count_root_panes(&self) -> usize {
        self.root.count_panes()
    }
}

/// Generate uniform size distribution.
pub fn equal_sizes(n: usize) -> Vec<f64> {
    if n == 0 {
        return vec![];
    }
    vec![1.0 / n as f64; n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_node_find_slot() {
        let pane = PaneId::from("p1");
        let root = LayoutNode::new_slot(NodeId::from("s1"), vec![pane.clone()]);
        assert!(root.find_slot(&pane).is_some());
        assert!(root.find_slot(&PaneId::from("p2")).is_none());
    }

    #[test]
    fn layout_node_find_in_split() {
        let p1 = PaneId::from("p1");
        let p2 = PaneId::from("p2");
        let root = LayoutNode::new_split(
            NodeId::from("split1"),
            Axis::Horizontal,
            vec![
                LayoutNode::new_slot(NodeId::from("s1"), vec![p1.clone()]),
                LayoutNode::new_slot(NodeId::from("s2"), vec![p2.clone()]),
            ],
            vec![0.5, 0.5],
        );
        assert!(root.find_slot(&p1).is_some());
        assert!(root.find_slot(&p2).is_some());
        assert_eq!(root.count_panes(), 2);
    }

    #[test]
    fn find_parent_info() {
        let root = LayoutNode::new_split(
            NodeId::from("split1"),
            Axis::Horizontal,
            vec![
                LayoutNode::new_slot(NodeId::from("s1"), vec![]),
                LayoutNode::new_slot(NodeId::from("s2"), vec![]),
            ],
            vec![0.5, 0.5],
        );
        let info = root.find_parent_info(&NodeId::from("s2")).unwrap();
        assert_eq!(info.id, NodeId::from("split1"));
        assert_eq!(info.index, 1);
        assert_eq!(info.axis, Axis::Horizontal);
    }

    #[test]
    fn replace_node_works() {
        let mut root = LayoutNode::new_split(
            NodeId::from("split1"),
            Axis::Horizontal,
            vec![
                LayoutNode::new_slot(NodeId::from("s1"), vec![PaneId::from("p1")]),
                LayoutNode::new_slot(NodeId::from("s2"), vec![PaneId::from("p2")]),
            ],
            vec![0.5, 0.5],
        );

        let replacement = LayoutNode::new_slot(NodeId::from("s3"), vec![PaneId::from("p3")]);
        replace_node(&mut root, &NodeId::from("s1"), replacement);

        assert!(root.find_slot(&PaneId::from("p3")).is_some());
        assert!(root.find_slot(&PaneId::from("p1")).is_none());
    }

    #[test]
    fn equal_sizes_works() {
        let s = equal_sizes(3);
        assert_eq!(s.len(), 3);
        let sum: f64 = s.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn serde_layout_node_roundtrip() {
        let node = LayoutNode::new_split(
            NodeId::from("split1"),
            Axis::Horizontal,
            vec![
                LayoutNode::Slot {
                    id: NodeId::from("s1"),
                    pane_ids: vec![PaneId::from("p1")],
                    active_pane_id: Some(PaneId::from("p1")),
                },
                LayoutNode::new_slot(NodeId::from("s2"), vec![PaneId::from("p2")]),
            ],
            vec![0.5, 0.5],
        );

        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"type\":\"split\""));
        assert!(json.contains("\"type\":\"slot\""));
        assert!(json.contains("\"paneIds\""));
        assert!(json.contains("\"activePaneId\""));

        let back: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id(), &NodeId::from("split1"));
    }
}
