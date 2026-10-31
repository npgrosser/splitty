//! Terminal multiplexer: manages screens, panes, layout trees, and sessions.
//!
//! The mux follows a Redux-style architecture:
//! - **State** (`WorkspaceState`, `ScreenState`, `LayoutNode`) -- immutable data model.
//! - **Ops** (`Op`, `ScreenOp`) -- actions dispatched by the client.
//! - **Reducer** -- pure functions that produce new state + side-effect descriptors.
//! - **Effects** -- declarative descriptions of PTY operations (spawn/kill).
//! - **effect_runner** -- executes effects against the PTY backend; the only part of the mux
//!   that touches the PTY layer, so it is a public module of its own and not re-exported here.
//! - **Store** -- thread-safe wrapper: lock, reduce, store, unlock.
//! - **Grid** -- projects the layout tree into 2D rectangles for spatial navigation.
//! - **Validate** -- pre-checks operations before the reducer runs.

#![allow(clippy::collapsible_match)]

pub mod effect_runner;
mod effects;
mod grid;
mod job_reducer;
pub mod jobs;
mod ops;
mod reducer;
mod state;
mod store;
mod validate;

pub use effects::Effect;
pub use grid::{find_neighbor, project_grid, PaneRect};
pub use ops::{Arrangement, Direction, Op, ScreenOp};
pub use state::{
    equal_sizes, replace_node, Axis, FloatBounds, FloatingEntry, LayoutNode, PaneRole, PaneState,
    ParentInfo, ScreenInfo, ScreenState, SessionEntry, SessionStatus, SlotLocation, WorkspaceState,
    SCRATCHPAD_LIMIT,
};
pub use store::Store;

pub use reducer::{reduce_screen, reduce_workspace};
pub use validate::validate_screen_op;

use thiserror::Error;

use crate::ids::*;

/// Errors from mux operations.
#[derive(Debug, Error)]
pub enum MuxError {
    #[error("pane {0} not found")]
    PaneNotFound(PaneId),

    #[error("pane {0} not found in layout")]
    PaneNotInLayout(PaneId),

    #[error("pane {0} already exists")]
    PaneExists(PaneId),

    #[error("screen {0} not found")]
    ScreenNotFound(ScreenId),

    #[error("screen {0} already exists")]
    ScreenExists(ScreenId),

    #[error("node {0} not found")]
    NodeNotFound(NodeId),

    #[error("node {0} is not a split")]
    NotASplit(NodeId),

    #[error("floating entry {0} not found")]
    FloatingNotFound(FloatingId),

    #[error("cannot float the last pane in the grid")]
    LastPane,

    #[error("invalid sizes: expected {expected} got {got}")]
    InvalidSizesLength { expected: usize, got: usize },

    #[error("sizes must be non-negative, got {0}")]
    NegativeSize(f64),

    #[error("sizes must sum to ~1.0, got {0}")]
    SizesSumInvalid(f64),

    #[error("invalid axis")]
    InvalidAxis,

    #[error("either targetSlotId or a direction with targetSplitId is required")]
    MoveTargetRequired,

    #[error("node {0} is not a slot")]
    NotASlot(NodeId),

    #[error("move target {0} is gone once the pane is taken out")]
    MoveTargetGone(NodeId),

    #[error("the arrangement misses pane {0}")]
    ArrangementMissesPane(PaneId),

    #[error("pane {0} appears more than once in the arrangement")]
    ArrangementRepeatsPane(PaneId),

    #[error("a slot of the arrangement holds no pane")]
    ArrangementEmptySlot,

    #[error("a split of the arrangement has fewer than two children")]
    ArrangementSplitTooSmall,

    #[error("active pane {0} is not in its slot")]
    ActivePaneOutsideSlot(PaneId),

    #[error("invalid direction: {0}")]
    InvalidDirection(String),

    #[error("bounds are required")]
    BoundsRequired,

    #[error("screenId is required")]
    ScreenIdRequired,

    #[error("pane {0} cannot move to the screen it is on")]
    SameScreen(PaneId),

    #[error("pane {0} belongs to a managed job and cannot move to another screen")]
    JobPaneMove(PaneId),

    #[error("session {0} not found")]
    SessionNotFound(SessionId),

    #[error("a scratchpad holds at most {limit} bytes, got {got}")]
    ScratchpadTooLong { limit: usize, got: usize },

    #[error("{0}")]
    Internal(String),
}
