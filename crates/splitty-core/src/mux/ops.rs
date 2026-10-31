//! Operation types dispatched by clients to mutate mux state.
//!
//! `Op` covers workspace-level actions (create/close/rename screen, set active).
//! `ScreenOp` covers screen-level actions (split, close, resize, move, float, dock).
//! Both are consumed by the corresponding reducers.

use serde::{Deserialize, Serialize};

use super::state::{Axis, FloatBounds};
use crate::ids::*;

/// Direction for spatial navigation and move operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    #[serde(alias = "up")]
    Above,
    #[serde(alias = "down")]
    Below,
}

/// Top-level operation. Dispatched by the workspace reducer.
#[derive(Clone, Debug)]
pub enum Op {
    /// An already authorized launch. Authority is never deserialized from a client.
    CreateJob(Box<super::jobs::JobAdmission>),
    /// Server lifecycle facts and owner-only mutations, not public wire operations.
    UpdateJob {
        job_id: JobId,
        update: super::jobs::JobUpdate,
    },
    /// Apply a screen-level operation to a specific screen.
    ModifyScreen { screen_id: ScreenId, op: ScreenOp },

    /// Create a new screen.
    CreateScreen {
        name: Option<String>,
        screen_id: Option<ScreenId>,
        new_pane_id: Option<PaneId>,
        /// Optional explicit profile. Missing means the global default.
        shell_profile_id: Option<String>,
    },

    /// Close and remove a screen.
    CloseScreen { screen_id: ScreenId },

    /// Rename a screen.
    RenameScreen { screen_id: ScreenId, name: String },

    /// Set the active screen.
    SetActiveScreen { screen_id: ScreenId },

    /// Reorder screens.
    ReorderScreens { screen_order: Vec<ScreenId> },

    /// Move a pane and its session to another screen, or to a new one,
    /// without touching the session's process.
    MovePaneToScreen {
        screen_id: ScreenId,
        pane_id: PaneId,
        target_screen_id: ScreenId,
        /// Create the target, named `screen_name`; otherwise it must exist.
        create_screen: bool,
        screen_name: Option<String>,
    },
}

/// Screen-level operation. Dispatched by the screen reducer.
/// Serde-compatible with the wire format (tagged by "type", camelCase fields).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScreenOp {
    /// Split a pane along an axis, creating a new pane.
    #[serde(rename_all = "camelCase")]
    SplitPane {
        pane_id: PaneId,
        axis: Axis,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_pane_index: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_pane_id: Option<PaneId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resulting_sizes: Option<Vec<f64>>,
        /// Working directory for the new pane's session. Clients send the
        /// current directory of the pane being split so the new shell starts
        /// where the old one is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Explicit profile for a profile-menu launch. Missing means inherit
        /// from the source pane (including its legacy `shell` value).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell_profile_id: Option<String>,
    },

    /// Create a terminal tab in the source pane's slot, keeping the split layout.
    #[serde(rename_all = "camelCase")]
    NewTab {
        pane_id: PaneId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_pane_id: Option<PaneId>,
        /// Working directory and shell profile follow the same inheritance as a split.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell_profile_id: Option<String>,
    },

    /// Close a pane (and kill its session).
    #[serde(rename_all = "camelCase")]
    ClosePane {
        pane_id: PaneId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resulting_sizes: Option<Vec<f64>>,
    },

    /// Resize a split node.
    #[serde(rename_all = "camelCase")]
    ResizeSplit { node_id: NodeId, sizes: Vec<f64> },

    /// Move a pane to a different location.
    #[serde(rename_all = "camelCase")]
    MovePane {
        pane_id: PaneId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_slot_id: Option<NodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<Direction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_split_id: Option<NodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_resulting_sizes: Option<Vec<f64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resulting_sizes: Option<Vec<f64>>,
    },

    /// Replace the root layout with a window's arrangement of the panes it
    /// already holds: tab order, splits and sizes, no pane or session changes.
    ArrangePanes { root: Arrangement },

    /// Make a pane the active tab of the slot that holds it, in the root
    /// layout or a floating entry; nothing else changes.
    #[serde(rename_all = "camelCase")]
    SelectTab { pane_id: PaneId },

    /// Float a pane (remove from grid, create floating overlay).
    #[serde(rename_all = "camelCase")]
    FloatPane {
        pane_id: PaneId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bounds: Option<FloatBounds>,
    },

    /// Dock a floating entry back into a slot.
    #[serde(rename_all = "camelCase")]
    DockPane {
        floating_id: FloatingId,
        pane_id: PaneId,
    },

    /// Move a floating entry to new bounds.
    #[serde(rename_all = "camelCase")]
    MoveFloat {
        floating_id: FloatingId,
        bounds: FloatBounds,
    },

    /// Replace a session's scratchpad; an empty text clears it. Only a
    /// window's mux connection sends it (`session-scratchpad`).
    #[serde(rename_all = "camelCase")]
    SetScratchpad { session_id: SessionId, text: String },
}

/// A layout tree as `arrange_panes` carries it: like the root layout, but
/// without node IDs, which the server assigns.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Arrangement {
    #[serde(rename_all = "camelCase")]
    Slot {
        pane_ids: Vec<PaneId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_pane_id: Option<PaneId>,
    },
    Split {
        axis: Axis,
        children: Vec<Arrangement>,
        sizes: Vec<f64>,
    },
}
