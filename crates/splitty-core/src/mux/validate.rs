//! Pre-validation of screen operations before the reducer runs.
//!
//! Checks that referenced panes, nodes, and floating entries exist, that size
//! arrays have correct length and sum to ~1.0, and that moves have a valid target.

use std::collections::HashSet;

use super::ops::{Arrangement, ScreenOp, ViewerPlacement};
use super::state::{
    equal_sizes, AnnotationKind, AnnotationReply, LayoutNode, PaneRole, ReplyAuthor, ReviewWait,
    ScreenState, SentEntry, SlotLocation, ViewerAnnotation, ViewerSource, ViewerState,
    ANNOTATION_CONTEXT_LIMIT, ANNOTATION_COUNT_LIMIT, ANNOTATION_LINE_LIMIT,
    ANNOTATION_PANE_TEXT_LIMIT, ANNOTATION_TEXT_LIMIT, MARK_SENT_LIMIT, REPLY_COUNT_LIMIT,
    REPLY_TITLE_LIMIT, SCRATCHPAD_LIMIT, VIEWER_PATH_LIMIT, VIEWER_TEXT_LIMIT, VIEWER_TITLE_LIMIT,
};
use super::MuxError;
use crate::ids::*;

/// Validate a screen-level operation against the current state.
pub fn validate_screen_op(state: &ScreenState, op: &ScreenOp) -> Result<(), MuxError> {
    match op {
        ScreenOp::SplitPane {
            pane_id,
            resulting_sizes,
            ..
        } => validate_split_pane(state, pane_id, resulting_sizes.as_deref()),
        ScreenOp::NewTab { pane_id, .. }
        | ScreenOp::ClosePane { pane_id, .. }
        | ScreenOp::SelectTab { pane_id } => validate_located_pane(state, pane_id),
        ScreenOp::ResizeSplit { node_id, sizes } => validate_resize_split(state, node_id, sizes),
        ScreenOp::MovePane {
            pane_id,
            target_slot_id,
            direction,
            target_split_id,
            ..
        } => validate_move_pane(
            state,
            pane_id,
            target_slot_id.as_ref(),
            direction.as_ref(),
            target_split_id.as_ref(),
        ),
        ScreenOp::ArrangePanes { root } => arranged_root(state, root).map(|_| ()),
        ScreenOp::FloatPane { pane_id, .. } => validate_float_pane(state, pane_id),
        ScreenOp::DockPane { floating_id, .. } => validate_dock_pane(state, floating_id),
        ScreenOp::MoveFloat { floating_id, .. } => validate_move_float(state, floating_id),
        ScreenOp::SetScratchpad { session_id, text } => {
            validate_set_scratchpad(state, session_id, text)
        }
        ScreenOp::OpenViewer {
            pane_id,
            placement,
            new_pane_id,
            source,
            cwd,
            replace,
        } => {
            // Only the CLI replaces a text, by title and origin; a window's
            // viewer is always a new pane.
            if *replace && (new_pane_id.is_some() || !matches!(source, ViewerSource::Text { .. })) {
                return Err(MuxError::ViewerReplaceInvalid);
            }
            validate_open_viewer(
                state,
                pane_id.as_ref(),
                placement.unwrap_or_default(),
                source,
                cwd.as_deref(),
            )
        }
        ScreenOp::PutAnnotations {
            pane_id,
            annotations,
        } => validate_put_annotations(state, pane_id, annotations),
        ScreenOp::RemoveAnnotations { pane_id, ids } => {
            validate_remove_annotations(state, pane_id, ids)
        }
        ScreenOp::PutReplies {
            pane_id,
            annotation_id,
            replies,
        } => validate_put_replies(state, pane_id, annotation_id, replies),
        ScreenOp::RemoveReplies { pane_id, ids, .. } => {
            validate_remove_replies(state, pane_id, ids)
        }
        ScreenOp::MarkSent { pane_id, entries } => validate_mark_sent(state, pane_id, entries),
    }
}

/// What the reducer can see of a viewer's document; whether a file exists is
/// decided when it is read.
fn validate_open_viewer(
    state: &ScreenState,
    anchor: Option<&PaneId>,
    placement: ViewerPlacement,
    source: &ViewerSource,
    cwd: Option<&str>,
) -> Result<(), MuxError> {
    match source {
        ViewerSource::File { path } => {
            if !viewer_path_is_valid(path) {
                return Err(MuxError::ViewerPathInvalid);
            }
        }
        ViewerSource::Text { title, text, .. } => {
            if text.len() > VIEWER_TEXT_LIMIT {
                return Err(MuxError::ViewerTextTooLong {
                    limit: VIEWER_TEXT_LIMIT,
                    got: text.len(),
                });
            }
            let chars = title.chars().count();
            if chars == 0 || chars > VIEWER_TITLE_LIMIT || title.chars().any(char::is_control) {
                return Err(MuxError::ViewerTitleInvalid);
            }
        }
    }
    if cwd.is_some_and(|cwd| !cwd.starts_with('/')) {
        return Err(MuxError::ViewerCwdNotAbsolute);
    }
    match (anchor, placement) {
        (None, _) => Ok(()),
        (Some(anchor), ViewerPlacement::Tab) => validate_located_pane(state, anchor),
        // A floating entry holds no splits: a directional placement there is a tab.
        (Some(anchor), _)
            if matches!(
                state.find_slot_location(anchor),
                Some(SlotLocation::Floating(_))
            ) =>
        {
            validate_located_pane(state, anchor)
        }
        (Some(anchor), _) => validate_split_pane(state, anchor, None),
    }
}

/// Whether a viewer pane may name the file at `path`: absolute, at most
/// [`VIEWER_PATH_LIMIT`] bytes long, and without a leading `//`, a backslash,
/// a `.` or `..` segment, control characters or the invisible formatting
/// characters U+200B..U+200F, U+202A..U+202E, U+2060..U+206F and U+FEFF.
/// The paths of editor requests follow the same rules.
pub fn viewer_path_is_valid(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= VIEWER_PATH_LIMIT
        && !path.contains('\\')
        && !path.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{206f}'
                        | '\u{feff}'
                )
        })
        && !path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
}

/// The record of the viewer pane `pane_id`.
fn held_viewer<'a>(state: &'a ScreenState, pane_id: &PaneId) -> Result<&'a ViewerState, MuxError> {
    let pane = state
        .panes
        .get(pane_id)
        .ok_or_else(|| MuxError::PaneNotFound(pane_id.clone()))?;
    match &pane.viewer {
        Some(viewer) if pane.role == Some(PaneRole::Viewer) => Ok(viewer),
        _ => Err(MuxError::NotAViewerPane(pane_id.clone())),
    }
}

/// The annotations of the viewer pane `pane_id`.
fn held_annotations<'a>(
    state: &'a ScreenState,
    pane_id: &PaneId,
) -> Result<&'a [ViewerAnnotation], MuxError> {
    held_viewer(state, pane_id).map(|viewer| viewer.annotations.as_slice())
}

/// An annotation op carries 1 to [`ANNOTATION_COUNT_LIMIT`] entries.
fn validate_annotation_batch(len: usize) -> Result<(), MuxError> {
    if len == 0 || len > ANNOTATION_COUNT_LIMIT {
        return Err(MuxError::AnnotationBatchInvalid {
            limit: ANNOTATION_COUNT_LIMIT,
            got: len,
        });
    }
    Ok(())
}

/// The text of a pane's annotations stays within
/// [`ANNOTATION_PANE_TEXT_LIMIT`].
fn validate_pane_bytes(bytes: usize) -> Result<(), MuxError> {
    if bytes > ANNOTATION_PANE_TEXT_LIMIT {
        return Err(MuxError::AnnotationsTooLong {
            limit: ANNOTATION_PANE_TEXT_LIMIT,
            got: bytes,
        });
    }
    Ok(())
}

/// The pane's annotations after the op, replaced in place with their numbers
/// and replies kept and new ones appended, stay within the pane's limits
/// (`viewer-annotations`, Annotations On A Viewer Pane). A put carries no
/// number and no replies: the server keeps both.
fn validate_put_annotations(
    state: &ScreenState,
    pane_id: &PaneId,
    annotations: &[ViewerAnnotation],
) -> Result<(), MuxError> {
    let viewer = held_viewer(state, pane_id)?;
    let held = viewer.annotations.as_slice();
    validate_annotation_batch(annotations.len())?;
    let mut put = HashSet::new();
    for annotation in annotations {
        validate_annotation(annotation)?;
        let invalid = |reason| {
            Err(MuxError::AnnotationPutInvalid {
                id: annotation.id.clone(),
                reason,
            })
        };
        if annotation.number != 0 {
            return invalid("the server numbers annotations; a put carries no number");
        }
        if !annotation.replies.is_empty() {
            return invalid("replies are put with put_replies; a put carries none");
        }
        if !put.insert(annotation.id.as_str()) {
            return Err(MuxError::AnnotationRepeated(annotation.id.clone()));
        }
    }
    let added = annotations
        .iter()
        .filter(|annotation| !held.iter().any(|held| held.id == annotation.id))
        .count();
    let count = held.len() + added;
    if count > ANNOTATION_COUNT_LIMIT {
        return Err(MuxError::AnnotationsTooMany {
            limit: ANNOTATION_COUNT_LIMIT,
            got: count,
        });
    }
    // A pane that has given every number takes no new annotation; only a
    // hand-edited record gets near.
    let numbers_left = u32::MAX - viewer.last_annotation_number;
    if usize::try_from(numbers_left).is_ok_and(|left| added > left) {
        return Err(MuxError::Internal(
            "the viewer pane has given every annotation number".into(),
        ));
    }
    let replaced = |held: &ViewerAnnotation| annotations.iter().find(|put| put.id == held.id);
    let bytes = held
        .iter()
        .map(|held| match replaced(held) {
            Some(put) => {
                put.text_bytes()
                    + held
                        .replies
                        .iter()
                        .map(AnnotationReply::text_bytes)
                        .sum::<usize>()
            }
            None => held.text_bytes(),
        })
        .sum::<usize>()
        + annotations
            .iter()
            .filter(|put| !held.iter().any(|held| held.id == put.id))
            .map(ViewerAnnotation::text_bytes)
            .sum::<usize>();
    validate_pane_bytes(bytes)
}

/// A removal names 1 to [`ANNOTATION_COUNT_LIMIT`] IDs of the annotation ID
/// form; the pane need not hold them.
fn validate_remove_annotations(
    state: &ScreenState,
    pane_id: &PaneId,
    ids: &[String],
) -> Result<(), MuxError> {
    held_annotations(state, pane_id)?;
    validate_annotation_batch(ids.len())?;
    if !ids.iter().all(|id| annotation_id_is_valid(id)) {
        return Err(MuxError::AnnotationIdInvalid);
    }
    Ok(())
}

/// One annotation on its own: its ID, the fields its kind needs, its
/// passage and the limits of each text.
fn validate_annotation(annotation: &ViewerAnnotation) -> Result<(), MuxError> {
    if !annotation_id_is_valid(&annotation.id) {
        return Err(MuxError::AnnotationIdInvalid);
    }
    let target = annotation.target.as_ref();
    for (field, text, limit) in [
        ("remark", annotation.body.as_deref(), ANNOTATION_TEXT_LIMIT),
        (
            "replacement",
            annotation.replacement.as_deref(),
            ANNOTATION_TEXT_LIMIT,
        ),
        (
            "quote",
            target.map(|target| target.quote.as_str()),
            ANNOTATION_TEXT_LIMIT,
        ),
        (
            "prefix",
            target.map(|target| target.prefix.as_str()),
            ANNOTATION_CONTEXT_LIMIT,
        ),
        (
            "suffix",
            target.map(|target| target.suffix.as_str()),
            ANNOTATION_CONTEXT_LIMIT,
        ),
    ] {
        let got = text.map_or(0, str::len);
        if got > limit {
            return Err(MuxError::AnnotationFieldTooLong {
                id: annotation.id.clone(),
                field,
                limit,
                got,
            });
        }
    }
    let invalid = |reason| {
        Err(MuxError::AnnotationInvalid {
            id: annotation.id.clone(),
            reason,
        })
    };
    let remark = annotation
        .body
        .as_deref()
        .is_some_and(|body| !body.trim().is_empty());
    match annotation.kind {
        AnnotationKind::Comment if !remark => {
            return invalid("a comment needs a remark");
        }
        // An empty replacement suggests deleting the passage.
        AnnotationKind::Change if annotation.replacement.is_none() => {
            return invalid("a change needs a replacement");
        }
        AnnotationKind::Change if target.is_none() => {
            return invalid("a change needs a passage");
        }
        _ => {}
    }
    if let Some(target) = target {
        if target.quote.is_empty() {
            return invalid("a passage needs a quote");
        }
        if target.start_line < 1
            || target.start_line > target.end_line
            || target.end_line > ANNOTATION_LINE_LIMIT
        {
            return Err(MuxError::AnnotationLinesInvalid {
                id: annotation.id.clone(),
            });
        }
    }
    Ok(())
}

/// Whether `id` has the form of an annotation ID: `ann-` and 16 lowercase
/// hex characters.
pub fn annotation_id_is_valid(id: &str) -> bool {
    hex_id_is_valid(id, "ann-")
}

/// Whether `id` has the form of a reply ID: `rep-` and 16 lowercase hex
/// characters.
pub fn reply_id_is_valid(id: &str) -> bool {
    hex_id_is_valid(id, "rep-")
}

/// Whether `id` is `prefix` and 16 lowercase hex characters.
fn hex_id_is_valid(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == 16
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Whether `body` may be a reply's text: not only whitespace, and at most
/// [`ANNOTATION_TEXT_LIMIT`] bytes.
pub fn reply_text_is_valid(body: &str) -> bool {
    !body.trim().is_empty() && body.len() <= ANNOTATION_TEXT_LIMIT
}

/// A reply's own fields: its ID and its text.
fn validate_reply_text(reply: &AnnotationReply) -> Result<(), MuxError> {
    if !reply_id_is_valid(&reply.id) {
        return Err(MuxError::ReplyIdInvalid);
    }
    if reply.body.len() > ANNOTATION_TEXT_LIMIT {
        return Err(MuxError::ReplyTooLong {
            id: reply.id.clone(),
            limit: ANNOTATION_TEXT_LIMIT,
            got: reply.body.len(),
        });
    }
    if reply.body.trim().is_empty() {
        return Err(MuxError::ReplyInvalid {
            id: reply.id.clone(),
            reason: "a reply needs a text that is not only whitespace",
        });
    }
    Ok(())
}

/// The replies of the user that `put_replies` adds to annotation
/// `annotation_id` or replaces in its thread: 1 to [`REPLY_COUNT_LIMIT`] of
/// them, with IDs unique in the pane, none of them an agent's, and the
/// thread and the pane within their limits afterwards (`viewer-annotations`,
/// Threads).
fn validate_put_replies(
    state: &ScreenState,
    pane_id: &PaneId,
    annotation_id: &str,
    replies: &[AnnotationReply],
) -> Result<(), MuxError> {
    let held = held_annotations(state, pane_id)?;
    if replies.is_empty() || replies.len() > REPLY_COUNT_LIMIT {
        return Err(MuxError::ReplyBatchInvalid {
            limit: REPLY_COUNT_LIMIT,
            got: replies.len(),
        });
    }
    let annotation = held
        .iter()
        .find(|annotation| annotation.id == annotation_id)
        .ok_or_else(|| MuxError::AnnotationNotFound {
            pane: pane_id.clone(),
            id: annotation_id.to_owned(),
        })?;
    let mut put = HashSet::new();
    for reply in replies {
        validate_reply_text(reply)?;
        let invalid = |reason| {
            Err(MuxError::ReplyInvalid {
                id: reply.id.clone(),
                reason,
            })
        };
        if reply.author != ReplyAuthor::User {
            return invalid("a window writes only the user's replies");
        }
        if reply.from.is_some() {
            return invalid("only an agent's reply names the pane it came from");
        }
        if !put.insert(reply.id.as_str()) {
            return Err(MuxError::ReplyRepeated(reply.id.clone()));
        }
        if let Some(own) = annotation.replies.iter().find(|held| held.id == reply.id) {
            if own.author == ReplyAuthor::Agent {
                return Err(MuxError::AgentReplyPut(reply.id.clone()));
            }
        } else if held
            .iter()
            .filter(|other| other.id != annotation.id)
            .any(|other| other.replies.iter().any(|held| held.id == reply.id))
        {
            return Err(MuxError::ReplyRepeated(reply.id.clone()));
        }
    }
    let added = replies
        .iter()
        .filter(|reply| !annotation.replies.iter().any(|held| held.id == reply.id))
        .count();
    let count = annotation.replies.len() + added;
    if count > REPLY_COUNT_LIMIT {
        return Err(MuxError::RepliesTooMany {
            limit: REPLY_COUNT_LIMIT,
            got: count,
        });
    }
    let replaced = annotation
        .replies
        .iter()
        .filter(|held| put.contains(held.id.as_str()))
        .map(AnnotationReply::text_bytes)
        .sum::<usize>();
    let bytes = held.iter().map(ViewerAnnotation::text_bytes).sum::<usize>() - replaced
        + replies
            .iter()
            .map(AnnotationReply::text_bytes)
            .sum::<usize>();
    validate_pane_bytes(bytes)
}

/// A removal of replies names 1 to [`REPLY_COUNT_LIMIT`] IDs of the reply ID
/// form; neither the annotation nor the replies need to exist any more.
fn validate_remove_replies(
    state: &ScreenState,
    pane_id: &PaneId,
    ids: &[String],
) -> Result<(), MuxError> {
    held_annotations(state, pane_id)?;
    if ids.is_empty() || ids.len() > REPLY_COUNT_LIMIT {
        return Err(MuxError::ReplyBatchInvalid {
            limit: REPLY_COUNT_LIMIT,
            got: ids.len(),
        });
    }
    if !ids.iter().all(|id| reply_id_is_valid(id)) {
        return Err(MuxError::ReplyIdInvalid);
    }
    Ok(())
}

/// `mark_sent` names 1 to [`MARK_SENT_LIMIT`] entries with IDs of
/// annotations and replies; the pane need not hold them.
fn validate_mark_sent(
    state: &ScreenState,
    pane_id: &PaneId,
    entries: &[SentEntry],
) -> Result<(), MuxError> {
    held_annotations(state, pane_id)?;
    if entries.is_empty() || entries.len() > MARK_SENT_LIMIT {
        return Err(MuxError::MarkSentBatchInvalid {
            limit: MARK_SENT_LIMIT,
            got: entries.len(),
        });
    }
    if !entries
        .iter()
        .all(|entry| annotation_id_is_valid(&entry.id) || reply_id_is_valid(&entry.id))
    {
        return Err(MuxError::MarkSentIdInvalid);
    }
    Ok(())
}

/// An agent's reply to annotation `number` of viewer pane `pane_id`: the
/// annotation exists and was handed over, the reply is an agent's with the
/// pane it came from, its ID is new in the pane, and the thread and the pane
/// stay within their limits (`viewer-annotations`, Agents Read And Reply To
/// Handed-Over Threads).
pub(super) fn validate_agent_reply(
    state: &ScreenState,
    pane_id: &PaneId,
    number: u32,
    reply: &AnnotationReply,
) -> Result<(), MuxError> {
    let held = held_annotations(state, pane_id)?;
    let annotation = held
        .iter()
        .find(|annotation| annotation.number == number)
        .ok_or_else(|| MuxError::AnnotationNumberNotFound {
            pane: pane_id.clone(),
            number,
        })?;
    if !annotation.sent {
        return Err(MuxError::AnnotationNotSent {
            pane: pane_id.clone(),
            number,
        });
    }
    validate_reply_text(reply)?;
    let invalid = |reason| {
        Err(MuxError::ReplyInvalid {
            id: reply.id.clone(),
            reason,
        })
    };
    if reply.author != ReplyAuthor::Agent || reply.sent {
        return invalid("an agent's reply has the author agent and is never handed over");
    }
    match &reply.from {
        None => return invalid("an agent's reply names the pane it came from"),
        Some(from) if from.title.len() > REPLY_TITLE_LIMIT => {
            return invalid("the title of an agent's reply holds at most 256 bytes");
        }
        Some(_) => {}
    }
    if held
        .iter()
        .any(|other| other.replies.iter().any(|held| held.id == reply.id))
    {
        return Err(MuxError::ReplyRepeated(reply.id.clone()));
    }
    let count = annotation.replies.len() + 1;
    if count > REPLY_COUNT_LIMIT {
        return Err(MuxError::RepliesTooMany {
            limit: REPLY_COUNT_LIMIT,
            got: count,
        });
    }
    let bytes = held.iter().map(ViewerAnnotation::text_bytes).sum::<usize>() + reply.text_bytes();
    validate_pane_bytes(bytes)
}

/// The start of a wait for a review of viewer pane `pane_id`: at most one
/// per pane, with an ID of the wait form and a title within its limit.
pub(super) fn validate_review_wait(
    state: &ScreenState,
    pane_id: &PaneId,
    wait: &ReviewWait,
    revision: Option<u64>,
) -> Result<(), MuxError> {
    let viewer = held_viewer(state, pane_id)?;
    if viewer.review_wait.is_some() {
        return Err(MuxError::ReviewWaitBusy(pane_id.clone()));
    }
    if revision.is_some_and(|revision| revision != viewer.revision) {
        return Err(MuxError::ReviewRevisionChanged(pane_id.clone()));
    }
    if !hex_id_is_valid(&wait.id, "wait-") || wait.title.len() > REPLY_TITLE_LIMIT {
        return Err(MuxError::Internal(
            "a wait has a wait- ID and a title of at most 256 bytes".into(),
        ));
    }
    Ok(())
}

fn validate_set_scratchpad(
    state: &ScreenState,
    session_id: &SessionId,
    text: &str,
) -> Result<(), MuxError> {
    if !state.sessions.contains_key(session_id) {
        return Err(MuxError::SessionNotFound(session_id.clone()));
    }
    if text.len() > SCRATCHPAD_LIMIT {
        return Err(MuxError::ScratchpadTooLong {
            limit: SCRATCHPAD_LIMIT,
            got: text.len(),
        });
    }
    Ok(())
}

fn validate_split_pane(
    state: &ScreenState,
    pane_id: &PaneId,
    resulting_sizes: Option<&[f64]>,
) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.root.find_slot(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    if let Some(sizes) = resulting_sizes {
        validate_sizes(sizes, 2)?;
    }
    Ok(())
}

fn validate_located_pane(state: &ScreenState, pane_id: &PaneId) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.find_slot_location(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    Ok(())
}

fn validate_resize_split(
    state: &ScreenState,
    node_id: &NodeId,
    sizes: &[f64],
) -> Result<(), MuxError> {
    let node = state
        .root
        .find_node(node_id)
        .ok_or_else(|| MuxError::NodeNotFound(node_id.clone()))?;
    match node {
        super::state::LayoutNode::Split { children, .. } => {
            validate_sizes(sizes, children.len())?;
        }
        _ => return Err(MuxError::NotASplit(node_id.clone())),
    }
    Ok(())
}

fn validate_move_pane(
    state: &ScreenState,
    pane_id: &PaneId,
    target_slot_id: Option<&NodeId>,
    direction: Option<&super::ops::Direction>,
    target_split_id: Option<&NodeId>,
) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.root.find_slot(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    match (target_slot_id, direction, target_split_id) {
        (Some(slot), _, _) => match state.root.find_node(slot) {
            Some(LayoutNode::Slot { .. }) => Ok(()),
            Some(_) => Err(MuxError::NotASlot(slot.clone())),
            None => Err(MuxError::NodeNotFound(slot.clone())),
        },
        (None, Some(_), Some(target)) => {
            let pane = PaneId::from(target.as_str());
            if state.root.find_node(target).is_some() || state.root.find_slot(&pane).is_some() {
                Ok(())
            } else {
                Err(MuxError::NodeNotFound(target.clone()))
            }
        }
        _ => Err(MuxError::MoveTargetRequired),
    }
}

/// The root layout `arrange_panes` asks for. Pane IDs the screen does not
/// hold are left out with the slots and splits they empty; what remains must
/// hold every pane of the root layout exactly once and no floating pane.
pub(super) fn arranged_root(
    state: &ScreenState,
    root: &Arrangement,
) -> Result<LayoutNode, MuxError> {
    let in_root = state.root.collect_pane_ids();
    let arranged = match root {
        // The grid of a screen whose panes all float, or that has none.
        Arrangement::Slot { pane_ids, .. } if pane_ids.is_empty() && in_root.is_empty() => None,
        _ => arranged_node(state, root)?,
    };
    let mut seen = HashSet::new();
    let mut rejected = None;
    if let Some(node) = &arranged {
        node.walk(&mut |node| {
            let LayoutNode::Slot { pane_ids, .. } = node else {
                return true;
            };
            for id in pane_ids {
                if !in_root.contains(id) {
                    rejected = Some(MuxError::PaneNotInLayout(id.clone()));
                } else if !seen.insert(id.clone()) {
                    rejected = Some(MuxError::ArrangementRepeatsPane(id.clone()));
                }
            }
            rejected.is_none()
        });
    }
    if let Some(error) = rejected {
        return Err(error);
    }
    if let Some(missing) = in_root.into_iter().find(|id| !seen.contains(id)) {
        return Err(MuxError::ArrangementMissesPane(missing));
    }
    Ok(arranged.unwrap_or_else(|| LayoutNode::new_slot(NodeId::with_prefix("slot"), vec![])))
}

/// One node of an arrangement as sent, checked, then without the panes the
/// screen does not hold; `None` when none of its panes is left.
fn arranged_node(state: &ScreenState, node: &Arrangement) -> Result<Option<LayoutNode>, MuxError> {
    match node {
        Arrangement::Slot {
            pane_ids,
            active_pane_id,
        } => {
            if pane_ids.is_empty() {
                return Err(MuxError::ArrangementEmptySlot);
            }
            if let Some(active) = active_pane_id.as_ref().filter(|id| !pane_ids.contains(id)) {
                return Err(MuxError::ActivePaneOutsideSlot(active.clone()));
            }
            let known: Vec<PaneId> = pane_ids
                .iter()
                .filter(|id| state.panes.contains_key(*id))
                .cloned()
                .collect();
            if known.is_empty() {
                return Ok(None);
            }
            Ok(Some(LayoutNode::Slot {
                id: NodeId::with_prefix("slot"),
                active_pane_id: active_pane_id.clone().filter(|id| known.contains(id)),
                pane_ids: known,
            }))
        }
        Arrangement::Split {
            axis,
            children,
            sizes,
        } => {
            if children.len() < 2 {
                return Err(MuxError::ArrangementSplitTooSmall);
            }
            validate_sizes(sizes, children.len())?;
            let mut kept = Vec::new();
            let mut kept_sizes = Vec::new();
            for (child, size) in children.iter().zip(sizes) {
                if let Some(node) = arranged_node(state, child)? {
                    kept.push(node);
                    kept_sizes.push(*size);
                }
            }
            if kept.len() < 2 {
                return Ok(kept.pop());
            }
            // The share of a left-out child goes to the others.
            let sum: f64 = kept_sizes.iter().sum();
            let sizes = if sum > 0.0 {
                kept_sizes.iter().map(|size| size / sum).collect()
            } else {
                equal_sizes(kept.len())
            };
            Ok(Some(LayoutNode::new_split(
                NodeId::with_prefix("split"),
                axis.clone(),
                kept,
                sizes,
            )))
        }
    }
}

fn validate_float_pane(state: &ScreenState, pane_id: &PaneId) -> Result<(), MuxError> {
    if !state.panes.contains_key(pane_id) {
        return Err(MuxError::PaneNotFound(pane_id.clone()));
    }
    if state.root.find_slot(pane_id).is_none() {
        return Err(MuxError::PaneNotInLayout(pane_id.clone()));
    }
    if state.count_root_panes() <= 1 {
        return Err(MuxError::LastPane);
    }
    Ok(())
}

fn validate_dock_pane(state: &ScreenState, floating_id: &FloatingId) -> Result<(), MuxError> {
    if !state.floating.iter().any(|f| &f.id == floating_id) {
        return Err(MuxError::FloatingNotFound(floating_id.clone()));
    }
    Ok(())
}

fn validate_move_float(state: &ScreenState, floating_id: &FloatingId) -> Result<(), MuxError> {
    if !state.floating.iter().any(|f| &f.id == floating_id) {
        return Err(MuxError::FloatingNotFound(floating_id.clone()));
    }
    Ok(())
}

/// Validate that a sizes slice has the expected length and sums to ~1.0.
pub fn validate_sizes(sizes: &[f64], expected_len: usize) -> Result<(), MuxError> {
    if sizes.len() != expected_len {
        return Err(MuxError::InvalidSizesLength {
            expected: expected_len,
            got: sizes.len(),
        });
    }
    let sum: f64 = sizes.iter().sum();
    for &s in sizes {
        if s < 0.0 {
            return Err(MuxError::NegativeSize(s));
        }
    }
    if (sum - 1.0).abs() > 0.01 {
        return Err(MuxError::SizesSumInvalid(sum));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::state::*;
    use std::collections::HashMap;

    fn test_screen() -> ScreenState {
        let pane_id = PaneId::from("p1");
        let session_id = SessionId::from("s1");
        let mut panes = HashMap::new();
        panes.insert(
            pane_id.clone(),
            PaneState {
                id: pane_id.clone(),
                job_id: None,
                session_id: Some(session_id.clone()),
                title: None,
                cwd: None,
                role: None,
                viewer: None,
            },
        );
        let mut sessions = HashMap::new();
        sessions.insert(
            session_id.clone(),
            SessionEntry {
                id: session_id,
                job_id: None,
                restart_policy: Default::default(),
                shell_profile_id: None,
                command: None,
                cwd: None,
                shell: None,
                status: SessionStatus::Running,
                exit_code: None,
                scratchpad: String::new(),
            },
        );
        ScreenState {
            id: ScreenId::from("screen1"),
            jobs: HashMap::new(),
            name: None,
            revision: 0,
            root: LayoutNode::new_slot(NodeId::from("slot1"), vec![pane_id]),
            floating: vec![],
            panes,
            sessions,
        }
    }

    #[test]
    fn validate_split_pane_ok() {
        let state = test_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("p1"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: None,
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };
        assert!(validate_screen_op(&state, &op).is_ok());
    }

    #[test]
    fn validate_split_pane_not_found() {
        let state = test_screen();
        let op = ScreenOp::SplitPane {
            pane_id: PaneId::from("nonexistent"),
            axis: Axis::Horizontal,
            new_pane_index: None,
            new_pane_id: None,
            resulting_sizes: None,
            cwd: None,
            shell_profile_id: None,
        };
        assert!(validate_screen_op(&state, &op).is_err());
    }

    #[test]
    fn validate_sizes_ok() {
        assert!(validate_sizes(&[0.5, 0.5], 2).is_ok());
        assert!(validate_sizes(&[0.33, 0.33, 0.34], 3).is_ok());
    }

    #[test]
    fn validate_sizes_bad_sum() {
        assert!(validate_sizes(&[0.5, 0.3], 2).is_err());
    }

    #[test]
    fn validate_sizes_bad_length() {
        assert!(validate_sizes(&[0.5, 0.5], 3).is_err());
    }

    #[test]
    fn validate_sizes_negative() {
        assert!(validate_sizes(&[-0.5, 1.5], 2).is_err());
    }

    #[test]
    fn validate_float_last_pane_rejected() {
        let state = test_screen();
        let op = ScreenOp::FloatPane {
            pane_id: PaneId::from("p1"),
            bounds: None,
        };
        assert!(matches!(
            validate_screen_op(&state, &op),
            Err(MuxError::LastPane)
        ));
    }
}
