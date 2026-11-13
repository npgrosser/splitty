//! Operation types dispatched by clients to mutate mux state.
//!
//! `Op` covers workspace-level actions (create/close/rename screen, set active)
//! and the trusted operations the server derives itself (job lifecycle, an
//! agent's reply, the start and end of a wait for a review); it has no wire
//! form. `ScreenOp` covers screen-level actions (split, close, resize, move,
//! float, dock, viewers, their annotations and threads), as clients send them.
//! Both are consumed by the corresponding reducers.

use serde::{Deserialize, Serialize};

use super::state::{
    AnnotationReply, Axis, FloatBounds, ReviewWait, SentEntry, ViewerAnnotation, ViewerSource,
};
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

/// Where `open_viewer` puts a new viewer pane relative to its anchor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewerPlacement {
    #[default]
    Right,
    Left,
    Above,
    Below,
    /// The last tab of the anchor's slot, in the root layout or a floating entry.
    Tab,
}

/// Top-level operation. Dispatched by the workspace reducer.
#[derive(Clone, Debug)]
pub enum Op {
    /// Trusted process facts; never decoded from a client operation.
    UpdateSessionMetadata {
        session_id: SessionId,
        display_title: String,
        cwd: String,
        busy: bool,
        exited: bool,
        exit_code: Option<i32>,
        close: bool,
    },
    ApplyTemplate {
        screen_id: ScreenId,
        request: Box<super::ApplyTemplate>,
    },
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

    /// `CreateScreen` for the first screen of an empty workspace
    /// (`onlyIfEmpty`): rejected while the workspace holds any screen, so of
    /// concurrent first screens exactly one is created.
    CreateFirstScreen {
        name: Option<String>,
        screen_id: Option<ScreenId>,
        new_pane_id: Option<PaneId>,
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

    /// Place one currently ordered Screen before another, or at the current end.
    MoveScreen {
        screen_id: ScreenId,
        before_screen_id: Option<ScreenId>,
    },

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

    /// An agent's reply to annotation `number` of viewer pane `pane_id` on
    /// `screen_id`, derived from a control API request: the server chose the
    /// reply's ID and its `from` (`viewer-annotations`, Agents Read And Reply
    /// To Handed-Over Threads). Never deserialized from a client.
    AgentReply {
        screen_id: ScreenId,
        pane_id: PaneId,
        number: u32,
        reply: AnnotationReply,
    },

    /// A process starts waiting for a review of viewer pane `pane_id` on
    /// `screen_id` (`viewer-annotations`, Waiting For A Review). Never
    /// deserialized from a client.
    StartReviewWait {
        screen_id: ScreenId,
        pane_id: PaneId,
        wait: ReviewWait,
        /// The viewer's `revision` the waiting process showed; another one
        /// refuses the wait, so that an answer judges that revision.
        revision: Option<u64>,
    },

    /// The wait `wait_id` of viewer pane `pane_id` ended, on whichever screen
    /// the pane is now. Never deserialized from a client.
    EndReviewWait { pane_id: PaneId, wait_id: String },
}

fn is_false(flag: &bool) -> bool {
    !*flag
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

    /// Place one root-layout tab before an existing destination tab, or at
    /// the end. Unlike MovePane, same-slot reorder preserves active selection.
    #[serde(rename_all = "camelCase")]
    MoveTab {
        pane_id: PaneId,
        source_slot_id: NodeId,
        target_slot_id: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_pane_id: Option<PaneId>,
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

    /// Show a document in a viewer pane next to `pane_id`, or around the root
    /// layout without it; no session, no effect (`viewer-panes`). Without
    /// `new_pane_id`, a file a viewer pane of the screen already shows is
    /// shown again in that pane.
    #[serde(rename_all = "camelCase")]
    OpenViewer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<PaneId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<ViewerPlacement>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_pane_id: Option<PaneId>,
        source: ViewerSource,
        /// The working directory of a `text` document; a file's is its directory.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// With a `text` source and without `new_pane_id`: give the text to the
        /// first viewer pane of the screen that shows a text of the same title
        /// and origin, keeping its annotations (`viewer-panes`, Reopening A
        /// File).
        #[serde(default, skip_serializing_if = "is_false")]
        replace: bool,
    },

    /// Add annotations to a viewer pane, or replace those with the same ID in
    /// their place. Only a window's mux connection sends it
    /// (`viewer-annotations`).
    #[serde(rename_all = "camelCase")]
    PutAnnotations {
        pane_id: PaneId,
        annotations: Vec<ViewerAnnotation>,
    },

    /// Remove a viewer pane's annotations by ID with their replies, ignoring
    /// IDs it does not hold. Only a window's mux connection sends it
    /// (`viewer-annotations`).
    #[serde(rename_all = "camelCase")]
    RemoveAnnotations { pane_id: PaneId, ids: Vec<String> },

    /// Add replies of the user to an annotation's thread, or replace the
    /// user's replies with the same ID in their place. Only a window's mux
    /// connection sends it (`viewer-annotations`).
    #[serde(rename_all = "camelCase")]
    PutReplies {
        pane_id: PaneId,
        annotation_id: String,
        replies: Vec<AnnotationReply>,
    },

    /// Remove replies of either author from an annotation's thread, ignoring
    /// IDs it does not hold. Only a window's mux connection sends it.
    #[serde(rename_all = "camelCase")]
    RemoveReplies {
        pane_id: PaneId,
        annotation_id: String,
        ids: Vec<String>,
    },

    /// Mark annotations and the user's replies as handed over to an agent,
    /// each named with the content the review held: an entry that reads
    /// otherwise by now stays pending, and agents' replies and IDs the pane
    /// does not hold are ignored. Only a window's mux connection sends it.
    #[serde(rename_all = "camelCase")]
    MarkSent {
        pane_id: PaneId,
        entries: Vec<SentEntry>,
    },
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
