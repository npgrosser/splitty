//! Mux state types: the data model for the terminal multiplexer.
//!
//! Key types:
//! - `WorkspaceState` -- top-level: a map of screens plus the active screen and screen order.
//! - `ScreenState` -- a single screen with its layout tree, panes, sessions, and floating windows.
//! - `LayoutNode` -- recursive tree: `Slot` (leaf with pane stack) or `Split` (branch with axis/sizes).
//! - `PaneState` / `SessionEntry` -- metadata for panes and PTY sessions.
//! - `ViewerState` -- the document a viewer pane shows (`viewer-panes`), its
//!   annotations with their threads and the wait for a review
//!   (`viewer-annotations`).
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
    /// The document of a pane of the role `viewer`, which has no session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<ViewerState>,
}

/// What a viewer pane shows (`viewer-panes`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "StoredViewer")]
pub struct ViewerState {
    pub source: ViewerSource,
    /// The terminal pane the viewer was opened from, kept when that pane closes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_pane_id: Option<PaneId>,
    /// Starts at 1 and rises when `open_viewer` shows the same file again.
    pub revision: u64,
    /// The pane's annotations in creation order, each with its thread
    /// (`viewer-annotations`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<ViewerAnnotation>,
    /// The highest number the pane has given an annotation; a number is never
    /// given twice in a pane.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub last_annotation_number: u32,
    /// The process that waits for a review of the pane. It lives in memory
    /// only: `state.json` never holds it, and it is never read back
    /// (`server-modes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_wait: Option<ReviewWait>,
}

impl ViewerState {
    /// A new pane's record: revision 1, no annotations.
    pub fn new(source: ViewerSource, origin_pane_id: Option<PaneId>) -> Self {
        Self {
            source,
            origin_pane_id,
            revision: 1,
            annotations: Vec::new(),
            last_annotation_number: 0,
            review_wait: None,
        }
    }
}

/// A viewer record as it is read: from `state.json`, the only place one is
/// read from. A wait is never read back, and annotations of builds before
/// numbers get theirs in creation order.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredViewer {
    source: ViewerSource,
    #[serde(default)]
    origin_pane_id: Option<PaneId>,
    revision: u64,
    #[serde(default)]
    annotations: Vec<ViewerAnnotation>,
    #[serde(default)]
    last_annotation_number: u32,
}

impl From<StoredViewer> for ViewerState {
    fn from(stored: StoredViewer) -> Self {
        let StoredViewer {
            source,
            origin_pane_id,
            revision,
            mut annotations,
            last_annotation_number,
        } = stored;
        let mut last = annotations
            .iter()
            .map(|annotation| annotation.number)
            .fold(last_annotation_number, u32::max);
        for annotation in annotations.iter_mut().filter(|a| a.number == 0) {
            last = last.saturating_add(1);
            annotation.number = last;
        }
        Self {
            source,
            origin_pane_id,
            revision,
            annotations,
            last_annotation_number: last,
            review_wait: None,
        }
    }
}

fn is_zero(number: &u32) -> bool {
    *number == 0
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

/// A process that waits for the user's review of a viewer pane
/// (`viewer-annotations`, Waiting For A Review).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewWait {
    /// `wait-` and 16 lowercase hex characters, chosen by the server.
    pub id: String,
    /// The waiting process's pane.
    pub pane_id: PaneId,
    /// That pane's title when the wait started, at most
    /// [`REPLY_TITLE_LIMIT`] bytes.
    pub title: String,
}

/// A viewer pane's document: a file on the server's machine, read when a
/// window shows it, or a text the server keeps in the pane record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewerSource {
    File {
        path: String,
    },
    Text {
        title: String,
        /// Left out of snapshots and control API answers
        /// (`ScreenState::without_viewer_texts`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
        /// How a window shows the text; plain text when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<TextFormat>,
    },
}

/// How a window shows a `text` document other than as plain text
/// (`viewer-panes`, Showing A Document).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFormat {
    Markdown,
}

/// The longest path of a `file` document, in bytes.
pub const VIEWER_PATH_LIMIT: usize = 4096;

/// The longest text of a `text` document, in bytes of UTF-8.
pub const VIEWER_TEXT_LIMIT: usize = 256 * 1024;

/// The longest title of a `text` document, in characters.
pub const VIEWER_TITLE_LIMIT: usize = 200;

/// A remark a window made on a viewer pane's document, and its thread
/// (`viewer-annotations`). The server keeps its texts as they came and never
/// interprets them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "StoredAnnotation")]
pub struct ViewerAnnotation {
    /// `ann-` and 16 lowercase hex characters, chosen by the window.
    pub id: String,
    /// Given by the server when it adds the annotation, from 1; 0 only in a
    /// `put_annotations`, which carries none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub number: u32,
    pub kind: AnnotationKind,
    /// The remark; a comment needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The suggested text of a `change`; empty suggests deleting the passage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    /// The passage; none for a general comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AnnotationTarget>,
    #[serde(default, skip_serializing_if = "AnnotationStatus::is_open")]
    pub status: AnnotationStatus,
    /// The user handed the annotation itself over to an agent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sent: bool,
    /// The thread in the order the replies were added; the server keeps it,
    /// and a `put_annotations` carries none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<AnnotationReply>,
}

impl ViewerAnnotation {
    /// Whether `other` says the same as this one: kind, remark, replacement
    /// and passage; status, number, mark and thread aside.
    pub fn reads_as(&self, other: &ViewerAnnotation) -> bool {
        self.kind == other.kind
            && self.body == other.body
            && self.replacement == other.replacement
            && self.target == other.target
    }

    /// The bytes of its texts that count against a pane's limit: quote,
    /// context, remark, replacement, and the text and title of each reply.
    pub fn text_bytes(&self) -> usize {
        let target = self.target.as_ref().map_or(0, |target| {
            target.quote.len() + target.prefix.len() + target.suffix.len()
        });
        let text = |text: &Option<String>| text.as_ref().map_or(0, String::len);
        let replies = self
            .replies
            .iter()
            .map(AnnotationReply::text_bytes)
            .sum::<usize>();
        target + text(&self.body) + text(&self.replacement) + replies
    }
}

/// An annotation as it is read, from a window's operation or from
/// `state.json`: `question` from builds before threads is a comment and
/// `delete` a change with an empty replacement. Neither is ever written.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAnnotation {
    id: String,
    #[serde(default)]
    number: u32,
    kind: StoredKind,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    target: Option<AnnotationTarget>,
    #[serde(default)]
    status: AnnotationStatus,
    #[serde(default)]
    sent: bool,
    #[serde(default)]
    replies: Vec<AnnotationReply>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredKind {
    Comment,
    Question,
    Change,
    Delete,
}

impl From<StoredAnnotation> for ViewerAnnotation {
    fn from(stored: StoredAnnotation) -> Self {
        let (kind, replacement) = match stored.kind {
            StoredKind::Comment | StoredKind::Question => {
                (AnnotationKind::Comment, stored.replacement)
            }
            StoredKind::Change => (AnnotationKind::Change, stored.replacement),
            StoredKind::Delete => (AnnotationKind::Change, Some(String::new())),
        };
        Self {
            id: stored.id,
            number: stored.number,
            kind,
            body: stored.body,
            replacement,
            target: stored.target,
            status: stored.status,
            sent: stored.sent,
            replies: stored.replies,
        }
    }
}

/// What an annotation asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    /// A remark; a question is a comment whose thread the agent answers.
    Comment,
    /// A suggested replacement of the passage; an empty one suggests
    /// deleting it.
    Change,
}

/// Whether an annotation's thread is settled; only a window changes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationStatus {
    #[default]
    Open,
    Resolved,
}

impl AnnotationStatus {
    pub fn is_open(&self) -> bool {
        *self == Self::Open
    }
}

/// A reply in an annotation's thread (`viewer-annotations`, Threads): the
/// user's, from a window, or an agent's, from the control API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationReply {
    /// `rep-` and 16 lowercase hex characters: a window chooses the user's,
    /// the server an agent's.
    pub id: String,
    pub author: ReplyAuthor,
    pub body: String,
    /// The user handed the reply over; the user's replies only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sent: bool,
    /// The pane an agent's reply came from and its title then; agents'
    /// replies only, set by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<ReplyFrom>,
}

impl AnnotationReply {
    /// The bytes that count against a pane's limit: the text and the title.
    pub fn text_bytes(&self) -> usize {
        self.body.len() + self.from.as_ref().map_or(0, |from| from.title.len())
    }
}

/// Who wrote a reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyAuthor {
    User,
    Agent,
}

/// Where an agent's reply came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyFrom {
    pub pane_id: PaneId,
    /// The pane's title when the reply arrived, at most
    /// [`REPLY_TITLE_LIMIT`] bytes.
    pub title: String,
}

/// The passage an annotation is about, found again by its quote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationTarget {
    /// The text the quote was taken from.
    pub view: AnnotationView,
    pub quote: String,
    /// Context before and after the quote in that text.
    pub prefix: String,
    pub suffix: String,
    /// The passage's 1-based source lines when it was annotated.
    pub start_line: u32,
    pub end_line: u32,
}

/// The view of a viewer pane a passage was selected in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationView {
    Source,
    Rendered,
}

/// The most annotations of one viewer pane, and of one operation.
pub const ANNOTATION_COUNT_LIMIT: usize = 100;

/// The longest remark, replacement, quote or reply, in bytes of UTF-8.
pub const ANNOTATION_TEXT_LIMIT: usize = 16 * 1024;

/// The longest context before or after a quote, in bytes of UTF-8.
pub const ANNOTATION_CONTEXT_LIMIT: usize = 256;

/// The highest source line a passage names.
pub const ANNOTATION_LINE_LIMIT: u32 = 10_000_000;

/// The most text of all annotations of one viewer pane and their replies
/// ([`ViewerAnnotation::text_bytes`]), in bytes.
pub const ANNOTATION_PANE_TEXT_LIMIT: usize = 256 * 1024;

/// The most replies of one thread, and of one reply operation.
pub const REPLY_COUNT_LIMIT: usize = 50;

/// The longest title of an agent's reply or of a waiting pane, in bytes of
/// UTF-8.
pub const REPLY_TITLE_LIMIT: usize = 256;

/// The most entries of one `mark_sent`: 100 threads of 51 entries.
pub const MARK_SENT_LIMIT: usize = 5_100;

/// An entry of the user as a review held it, named in `mark_sent`: an
/// annotation with its kind, remark, replacement and passage, or one of the
/// user's replies with its text (`viewer-annotations`, Handing A Review To A
/// Terminal).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentEntry {
    pub id: String,
    /// An annotation's kind; none for a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AnnotationKind>,
    /// An annotation's remark, or a reply's text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AnnotationTarget>,
}

impl SentEntry {
    /// Whether `annotation` still reads as the review held it.
    pub fn holds_annotation(&self, annotation: &ViewerAnnotation) -> bool {
        self.id == annotation.id
            && self.kind == Some(annotation.kind)
            && self.body == annotation.body
            && self.replacement == annotation.replacement
            && self.target == annotation.target
    }

    /// Whether the user's `reply` still reads as the review held it.
    pub fn holds_reply(&self, reply: &AnnotationReply) -> bool {
        self.id == reply.id
            && reply.author == ReplyAuthor::User
            && self.body.as_deref() == Some(reply.body.as_str())
    }
}

/// A PTY process record in the mux state (not the actual PTY — just the data).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    #[serde(default)]
    pub busy: bool,
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

    /// Find the slot that contains the given pane ID, for changing it.
    pub fn find_slot_mut(&mut self, pane_id: &PaneId) -> Option<&mut Self> {
        if matches!(self, Self::Slot { pane_ids, .. } if pane_ids.contains(pane_id)) {
            return Some(self);
        }
        match self {
            Self::Slot { .. } => None,
            Self::Split { children, .. } => children
                .iter_mut()
                .find_map(|child| child.find_slot_mut(pane_id)),
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

    /// The IDs of the panes in layout order: the root layout's slots, then
    /// those of the floating entries, each slot's tabs in their order.
    pub fn panes_in_layout_order(&self) -> impl Iterator<Item = PaneId> + '_ {
        std::iter::once(&self.root)
            .chain(self.floating.iter().map(|entry| &entry.root))
            .flat_map(LayoutNode::collect_pane_ids)
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

    /// The screen without the text of its `text` viewer documents, as
    /// snapshots and the control API show it; windows read the text with
    /// `viewer.read`.
    pub fn without_viewer_texts(mut self) -> Self {
        for pane in self.panes.values_mut() {
            if let Some(ViewerState {
                source: ViewerSource::Text { text, .. },
                ..
            }) = &mut pane.viewer
            {
                *text = String::new();
            }
        }
        self
    }
}

impl WorkspaceState {
    /// The workspace without the text of its `text` viewer documents
    /// (`ScreenState::without_viewer_texts`).
    pub fn without_viewer_texts(mut self) -> Self {
        self.screens = self
            .screens
            .into_iter()
            .map(|(id, screen)| (id, screen.without_viewer_texts()))
            .collect();
        self
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
