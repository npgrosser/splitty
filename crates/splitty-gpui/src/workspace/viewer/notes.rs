//! Annotations of a viewer pane in the window: the list beside the document
//! with its threads, a field for a comment on the selected passage or for a
//! reply, the marks in the document, and the review handed to a terminal or
//! to a waiting process. The pane's service anchors the annotations and
//! writes the review (`viewer.anchor`, `viewer.review`); the window maps the
//! anchors into its views and hands the review over.
use super::*;
use gpui_kit::component::input::{Textarea, TextareaState};
use splitty_client_core::annotations::{
    self, Anchor, AnnotationKind, AnnotationReply, AnnotationStatus, AnnotationTarget, ReplyAuthor,
    Review, ReviewDelivery, ReviewForm, SentEntry, Verdict, ViewerAnnotation,
};
use splitty_client_core::native_workspace::{
    AnnotationOp, ReviewAnswered, ViewerAnchors, ViewerReviewed,
};
use splitty_client_core::workspace::ViewerState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

const NOTES_WIDTH: f32 = 300.;
const NOTE_TEXT: f32 = 12.;
const NOTE_META: f32 = 10.5;

/// Whether an annotation's passage is marked in the document: the open
/// ones, the resolved ones on request, and with the marks off only the one
/// the user chose.
fn is_marked(entry: &super::ViewerPane, annotation: &ViewerAnnotation) -> bool {
    if entry.marks_hidden {
        entry.chosen.as_deref() == Some(annotation.id.as_str())
    } else {
        annotation.status.is_open() || entry.resolved_marks
    }
}

/// How a review leaves the window.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Handover {
    Paste,
    File,
    Scratchpad,
}

/// The passages of a pane's annotations in the document version shown, and
/// the order of the list.
pub(super) struct AnchorCache {
    version: Option<String>,
    annotations: Vec<ViewerAnnotation>,
    pub(super) anchors: Vec<Anchor>,
    order: Vec<usize>,
}

/// The service's anchors for one version of the document: by annotation,
/// with the target each was asked for, and the list's order.
pub(super) struct Answered {
    version: String,
    by_id: HashMap<String, (Option<AnnotationTarget>, Anchor)>,
    order: Vec<String>,
}

/// What anchors were last asked for: a version and the annotations' targets.
#[derive(Clone, PartialEq)]
pub(super) struct Asked {
    version: String,
    targets: Vec<(String, Option<AnnotationTarget>)>,
}

/// What a review asked of the service is to do once it comes.
pub(super) enum ReviewPurpose {
    Copy,
    Send {
        target: Option<PaneKey>,
        how: Handover,
    },
    Answer {
        verdict: Verdict,
        wait_id: String,
    },
}

/// Tokens of the reviews asked for, unique in the window.
static NEXT_REVIEW: AtomicU64 = AtomicU64::new(1);

/// The entries of a review that wait for the service to accept its verdict.
pub(super) struct Answering {
    wait_id: String,
    entries: Vec<SentEntry>,
}

/// The passages of `annotations` as the service's anchors for the version
/// shown place them: an annotation whose target the service has not anchored
/// in this version yet is pending, and is listed after the others.
fn anchor_cache(entry: &super::ViewerPane, annotations: Vec<ViewerAnnotation>) -> AnchorCache {
    let answered = entry
        .answered
        .as_ref()
        .filter(|answered| Some(&answered.version) == entry.version.as_ref());
    let anchors: Vec<Anchor> = annotations
        .iter()
        .map(|annotation| match (&entry.text, answered) {
            _ if annotation.target.is_none() => Anchor::General,
            (None, _) => Anchor::NotFound,
            (Some(_), Some(answered)) => answered
                .by_id
                .get(&annotation.id)
                .filter(|(target, _)| *target == annotation.target)
                .map_or(Anchor::Pending, |(_, anchor)| anchor.clone()),
            (Some(_), None) => Anchor::Pending,
        })
        .collect();
    let place = |index: usize| {
        answered
            .filter(|_| anchors[index] != Anchor::Pending)
            .and_then(|answered| {
                answered
                    .order
                    .iter()
                    .position(|id| *id == annotations[index].id)
            })
            .unwrap_or(usize::MAX)
    };
    let mut order: Vec<usize> = (0..annotations.len()).collect();
    order.sort_by_key(|index| (place(*index), *index));
    AnchorCache {
        version: entry.version.clone(),
        annotations,
        anchors,
        order,
    }
}

fn excerpt(text: &str, limit: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > limit {
        format!("{}…", line.chars().take(limit).collect::<String>())
    } else {
        line
    }
}

impl Workspace {
    /// The Screen and the viewer record of a pane that is still in the window.
    fn viewer_record(&self, key: &PaneKey) -> Option<(String, SourceToken, ViewerState)> {
        let model = self.model_for_source(key.source_id())?;
        let screen = model.screen_for(key.pane_id())?;
        let record = model
            .screen(screen)?
            .snapshot()?
            .panes
            .values()
            .find(|entry| entry.id.as_str() == key.pane_id())?
            .viewer
            .clone()?;
        Some((screen.to_owned(), model.source_token(), record))
    }

    /// The process waiting for this pane's review: the wait's ID and the
    /// waiting pane's title.
    pub(super) fn viewer_wait(&self, key: &PaneKey) -> Option<(String, String)> {
        let model = self.model_for_source(key.source_id())?;
        let screen = model.screen_for(key.pane_id())?;
        let (id, title) = model.screen(screen)?.review_wait(key.pane_id())?;
        Some((id.to_owned(), title.to_owned()))
    }

    /// Anchor a pane's annotations again when its document or they changed,
    /// and mark the open ones in the rendered view once it is laid out.
    pub(super) fn refresh_viewer_notes(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        let Some((_, source, record)) = self.viewer_record(key) else {
            return;
        };
        let supported = self.viewer_reviews_supported(key);
        let wait = self.viewer_wait(key).map(|(id, _)| id);
        let Some(entry) = self.viewers.get_mut(key) else {
            return;
        };
        // A review is answered about the version shown when it was asked for.
        match wait {
            Some(id) if entry.wait_seen.as_ref().is_none_or(|(seen, _)| *seen != id) => {
                entry.wait_seen = Some((id, entry.version.clone()));
            }
            Some(_) => {}
            None => entry.wait_seen = None,
        }
        // The service anchors this version's annotations once for their
        // targets; an edit of a remark or a reply asks for nothing.
        let ask = match (&entry.version, &entry.text) {
            (Some(version), Some(_)) if supported => {
                let wanted = Asked {
                    version: version.clone(),
                    targets: record
                        .annotations
                        .iter()
                        .map(|annotation| (annotation.id.clone(), annotation.target.clone()))
                        .collect(),
                };
                (entry.asked.as_ref() != Some(&wanted)).then_some(wanted)
            }
            _ => None,
        };
        if let Some(wanted) = ask {
            if self
                .commands
                .try_send(WorkerCommand::new(
                    source,
                    Command::AnchorViewer {
                        pane: key.pane_id().to_owned(),
                    },
                ))
                .is_ok()
            {
                if let Some(entry) = self.viewers.get_mut(key) {
                    entry.asked = Some(wanted);
                }
            }
        }
        let Some(entry) = self.viewers.get_mut(key) else {
            return;
        };
        let stale = entry.anchors.as_ref().is_none_or(|cache| {
            cache.version != entry.version || cache.annotations != record.annotations
        });
        if stale {
            entry.anchors = Some(anchor_cache(entry, record.annotations));
            entry.marks_dirty = true;
            cx.notify();
        }
        if !entry.marks_dirty {
            return;
        }
        let (Some(Shown::Markdown { view, .. }), Some(text), Some(cache)) =
            (&entry.shown, &entry.text, &entry.anchors)
        else {
            entry.marks_dirty = false;
            return;
        };
        let rendered = view.read(cx).rendered_text();
        // The view parses on its own time; marks wait for this version.
        if rendered.source() != &**text {
            return;
        }
        let mark: Hsla = rgba(self.appearance.warning << 8 | 0x44).into();
        let highlights: Vec<_> = cache
            .annotations
            .iter()
            .zip(&cache.anchors)
            .filter(|(annotation, _)| is_marked(entry, annotation))
            .filter_map(|(_, anchor)| match anchor {
                Anchor::Found { range, .. } => rendered.range_for_source(range.clone()),
                _ => None,
            })
            .map(|range| gpui_kit::base::text::RangeHighlight::new(range, mark))
            .collect();
        let view = view.clone();
        if view
            .update(cx, |state, cx| state.set_range_highlights(highlights, cx))
            .is_ok()
        {
            if let Some(entry) = self.viewers.get_mut(key) {
                entry.marks_dirty = false;
            }
        }
    }

    fn viewer_annotate(&mut self, key: &PaneKey, op: AnnotationOp, cx: &mut Context<Self>) {
        let Some((screen, source, _)) = self.viewer_record(key) else {
            return;
        };
        if self.execution_token(source.source_id()) != Some(source) {
            self.status = "Host is offline; the change was not sent".into();
            cx.notify();
            return;
        }
        let command = Command::Annotate {
            screen,
            pane: key.pane_id().to_owned(),
            op,
        };
        self.send_captured(source, command, cx);
    }

    pub(super) fn toggle_viewer_notes(
        &mut self,
        key: &PaneKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.viewers.get_mut(key) else {
            return;
        };
        entry.notes_open = !entry.notes_open;
        if entry.notes_open && entry.input.is_none() {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 8)
                    .placeholder("Comment…")
            });
            entry.replacement = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 8)
                    .placeholder("Replacement; leave empty to suggest deleting the passage")
            }));
            let submit = key.clone();
            entry._input_events = Some(cx.subscribe_in(
                &input,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    // Enter adds a line; with Ctrl or Cmd it sends.
                    if matches!(
                        event,
                        InputEvent::PressEnter {
                            secondary: true,
                            ..
                        }
                    ) {
                        this.submit_viewer_note(&submit, window, cx);
                    }
                },
            ));
            entry.input = Some(input);
        }
        cx.notify();
    }

    /// The passage the user has selected in the pane, if any: the selection of
    /// the rendered view, or the chosen lines of the source view.
    fn viewer_selection(&self, key: &PaneKey, cx: &App) -> Option<annotations::AnnotationTarget> {
        let entry = self.viewers.get(key)?;
        let text = entry.text.as_ref()?;
        let range = match (&entry.shown, entry.show_source, entry.lines) {
            (Some(Shown::Markdown { view, .. }), false, _) => {
                // A press outside the document empties or drops the view's
                // selection; the passage chosen last stands until it is used
                // or dropped.
                return view
                    .read(cx)
                    .selected_source_range()
                    .and_then(|range| annotations::target_for(text, range).ok())
                    .or_else(|| entry.passage.clone());
            }
            (_, _, Some((first, last))) => {
                let mut offset = 0;
                let mut start = None;
                let mut end = text.len();
                for (index, line) in text.split_inclusive('\n').enumerate() {
                    if index == first {
                        start = Some(offset);
                    }
                    offset += line.len();
                    if index == last {
                        end = offset - usize::from(line.ends_with('\n'));
                        break;
                    }
                }
                start?..end
            }
            _ => return None,
        };
        annotations::target_for(text, range).ok()
    }

    /// Send the field's text: a reply to the chosen thread, a comment on the
    /// selected passage, or a general comment.
    fn submit_viewer_note(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.viewers.get(key) else {
            return;
        };
        let Some(input) = entry.input.clone() else {
            return;
        };
        let body = input.read(cx).value().trim().to_owned();
        let target = self.viewer_selection(key, cx);
        // A suggested change needs a passage; its remark is optional.
        let change = (entry.suggesting && entry.reply_to.is_none() && target.is_some())
            .then(|| {
                entry
                    .replacement
                    .as_ref()
                    .map(|field| field.read(cx).value().to_string())
            })
            .flatten();
        if body.is_empty() && change.is_none() {
            return;
        }
        if body.len() > annotations::TEXT_LIMIT
            || change
                .as_ref()
                .is_some_and(|text| text.len() > annotations::TEXT_LIMIT)
        {
            self.status = "The comment is too long".into();
            cx.notify();
            return;
        }
        let op = match entry.reply_to.clone() {
            Some(annotation_id) => AnnotationOp::PutReplies {
                annotation_id,
                replies: vec![AnnotationReply {
                    id: annotations::new_reply_id(),
                    author: ReplyAuthor::User,
                    body,
                    sent: false,
                    from: None,
                }],
            },
            None => AnnotationOp::Put(vec![ViewerAnnotation {
                id: annotations::new_annotation_id(),
                number: 0,
                kind: if change.is_some() {
                    AnnotationKind::Change
                } else {
                    AnnotationKind::Comment
                },
                body: (!body.is_empty()).then_some(body),
                replacement: change,
                target,
                status: AnnotationStatus::Open,
                sent: false,
                replies: Vec::new(),
            }]),
        };
        self.viewer_annotate(key, op, cx);
        input.update(cx, |state, cx| state.set_value("", window, cx));
        if let Some(entry) = self.viewers.get_mut(key) {
            if let Some(field) = &entry.replacement {
                field.update(cx, |state, cx| state.set_value("", window, cx));
            }
            entry.suggesting = false;
            entry.reply_to = None;
            entry.lines = None;
            entry.passage = None;
            if let Some(Shown::Markdown { view, .. }) = &entry.shown {
                view.update(cx, |state, cx| state.clear_selection(cx));
            }
        }
        cx.notify();
    }

    pub(super) fn viewer_drop_passage(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        if let Some(entry) = self.viewers.get_mut(key) {
            if entry.passage.take().is_some() {
                entry.suggesting = false;
                cx.notify();
            }
        }
    }

    /// The rendered view changed, possibly its selection. The view knows the
    /// selected source only once it has painted, so it is read a frame later.
    pub(super) fn viewer_selection_changed(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        let Some(entry) = self.viewers.get_mut(key) else {
            return;
        };
        let key = key.clone();
        entry.passage_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(40))
                .await;
            let _ = this.update(cx, |this, cx| this.viewer_capture_passage(&key, cx));
        }));
    }

    /// What is selected in the rendered document is the passage a comment
    /// would be on; it stands when the selection goes with the focus.
    fn viewer_capture_passage(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        let Some(entry) = self.viewers.get(key) else {
            return;
        };
        let passage = match (&entry.shown, &entry.text, entry.show_source) {
            (Some(Shown::Markdown { view, .. }), Some(text), false) => view
                .read(cx)
                .selected_source_range()
                .and_then(|range| annotations::target_for(text, range).ok()),
            _ => None,
        };
        if passage.is_none() {
            return;
        }
        if let Some(entry) = self.viewers.get_mut(key) {
            if entry.passage != passage {
                entry.passage = passage;
                cx.notify();
            }
        }
    }

    /// Suggest a change of the selected passage, or go back to a comment.
    /// The replacement starts as the passage itself, to be edited.
    fn viewer_suggest(&mut self, key: &PaneKey, window: &mut Window, cx: &mut Context<Self>) {
        let quote = self.viewer_selection(key, cx).map(|target| target.quote);
        let Some(entry) = self.viewers.get_mut(key) else {
            return;
        };
        entry.suggesting = !entry.suggesting && quote.is_some();
        if let (true, Some(field), Some(quote)) =
            (entry.suggesting, entry.replacement.clone(), quote)
        {
            field.update(cx, |state, cx| state.set_value(quote, window, cx));
            field.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Choose a thread: its card stands out, and a source view shows its lines.
    fn viewer_choose(&mut self, key: &PaneKey, id: &str, cx: &mut Context<Self>) {
        let Some(entry) = self.viewers.get_mut(key) else {
            return;
        };
        entry.chosen = Some(id.to_owned());
        // With the marks off, the chosen thread's passage is the one marked.
        entry.marks_dirty |= entry.marks_hidden;
        let line = entry.anchors.as_ref().and_then(|cache| {
            cache
                .annotations
                .iter()
                .zip(&cache.anchors)
                .find(|(annotation, _)| annotation.id == id)
                .and_then(|(_, anchor)| match anchor {
                    Anchor::Found { start_line, .. } => Some(*start_line),
                    _ => None,
                })
        });
        let lines_shown = matches!(entry.shown, Some(Shown::Text(_)))
            || (entry.show_source && matches!(entry.shown, Some(Shown::Markdown { .. })));
        if let (true, Some(line)) = (lines_shown, line) {
            entry.scroll.scroll_to_item_strict(
                (line as usize).saturating_sub(2),
                gpui_kit::ScrollStrategy::Top,
            );
        }
        cx.notify();
    }

    fn viewer_reply_to(
        &mut self,
        key: &PaneKey,
        annotation: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self.viewers.get_mut(key) {
            entry.reply_to = annotation;
            if let Some(input) = &entry.input {
                input.read(cx).focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        }
    }

    /// Hide or show the marks in the document, or those of resolved threads.
    fn viewer_toggle_marks(&mut self, key: &PaneKey, resolved: bool, cx: &mut Context<Self>) {
        if let Some(entry) = self.viewers.get_mut(key) {
            if resolved {
                entry.resolved_marks = !entry.resolved_marks;
            } else {
                entry.marks_hidden = !entry.marks_hidden;
            }
            entry.marks_dirty = true;
        }
        self.refresh_viewer_notes(key, cx);
        cx.notify();
    }

    /// Move a thread to the passage selected in the pane, which becomes its
    /// target: for a passage that was not found, or one that was meant.
    fn viewer_move_note(&mut self, key: &PaneKey, id: &str, cx: &mut Context<Self>) {
        let Some(target) = self.viewer_selection(key, cx) else {
            return;
        };
        let Some(mut annotation) = self.viewer_record(key).and_then(|(_, _, record)| {
            record
                .annotations
                .into_iter()
                .find(|annotation| annotation.id == id)
        }) else {
            return;
        };
        annotation.target = Some(target);
        // The service keeps number and thread; a put carries neither.
        annotation.number = 0;
        annotation.replies.clear();
        if let Some(entry) = self.viewers.get_mut(key) {
            entry.lines = None;
            entry.passage = None;
            entry.suggesting = false;
            if let Some(Shown::Markdown { view, .. }) = &entry.shown {
                view.update(cx, |state, cx| state.clear_selection(cx));
            }
        }
        self.viewer_annotate(key, AnnotationOp::Put(vec![annotation]), cx);
    }

    /// The terminal panes of the Screen shown that can take a review, the
    /// one the document was opened from first.
    fn viewer_review_targets(&self, key: &PaneKey) -> Vec<(PaneKey, String)> {
        let Some(model) = self.selected_model() else {
            return Vec::new();
        };
        let origin = self
            .viewer_record(key)
            .and_then(|(_, _, record)| record.origin_pane_id)
            .map(|pane| pane.to_string());
        let Some(screen) = model.snapshot() else {
            return Vec::new();
        };
        let mut targets: Vec<(bool, PaneKey, String)> = screen
            .panes
            .values()
            .filter_map(|pane| {
                let pane_key = model.pane_key(pane.id.as_str());
                self.panes.contains_key(&pane_key).then(|| {
                    (
                        origin.as_deref() != Some(pane.id.as_str()),
                        pane_key,
                        pane.title
                            .clone()
                            .filter(|title| !title.is_empty())
                            .unwrap_or_else(|| "Terminal".to_owned()),
                    )
                })
            })
            .collect();
        targets.sort_by(|a, b| (a.0, a.1.pane_id()).cmp(&(b.0, b.1.pane_id())));
        targets
            .into_iter()
            .map(|(_, pane, title)| (pane, title))
            .collect()
    }

    fn viewer_toggle_sending(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        if let Some(entry) = self.viewers.get_mut(key) {
            entry.sending = !entry.sending;
            cx.notify();
        }
    }

    /// Resolve an open thread, or open a resolved one again.
    fn viewer_resolve(&mut self, key: &PaneKey, id: &str, cx: &mut Context<Self>) {
        let Some(mut annotation) = self.viewer_record(key).and_then(|(_, _, record)| {
            record
                .annotations
                .into_iter()
                .find(|annotation| annotation.id == id)
        }) else {
            return;
        };
        annotation.status = match annotation.status {
            AnnotationStatus::Open => AnnotationStatus::Resolved,
            AnnotationStatus::Resolved => AnnotationStatus::Open,
        };
        // The service keeps number and thread; a put carries neither.
        annotation.number = 0;
        annotation.replies.clear();
        self.viewer_annotate(key, AnnotationOp::Put(vec![annotation]), cx);
    }

    /// Choose a line of the source view; with Shift, the lines up to it.
    pub(super) fn viewer_choose_line(
        &mut self,
        key: &PaneKey,
        line: usize,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self.viewers.get_mut(key) {
            // The mark beside a line leads to its thread: the shortest
            // passage first.
            let number = u32::try_from(line + 1).unwrap_or(u32::MAX);
            let marked = entry.anchors.as_ref().and_then(|cache| {
                cache
                    .annotations
                    .iter()
                    .zip(&cache.anchors)
                    .filter(|(annotation, _)| annotation.status.is_open())
                    .filter_map(|(annotation, anchor)| match anchor {
                        Anchor::Found {
                            start_line,
                            end_line,
                            ..
                        } if (*start_line..=*end_line).contains(&number) => {
                            Some((end_line - start_line, annotation.id.clone()))
                        }
                        _ => None,
                    })
                    .min()
            });
            if let Some((_, id)) = marked.filter(|_| !extend) {
                entry.chosen = Some(id);
            }
            entry.lines = match (entry.lines, extend) {
                (Some((first, last)), true) => Some((first.min(line), last.max(line))),
                (Some((first, last)), false) if first == line && last == line => None,
                _ => Some((line, line)),
            };
            cx.notify();
        }
    }

    pub(super) fn viewer_lines(&self, key: &PaneKey) -> Option<(usize, usize)> {
        self.viewers.get(key).and_then(|entry| entry.lines)
    }

    /// The lines of the open annotations found in this version, for the marks
    /// beside the source.
    pub(super) fn viewer_marked_lines(&self, key: &PaneKey) -> Vec<(u32, u32)> {
        self.viewers
            .get(key)
            .and_then(|entry| Some((entry, entry.anchors.as_ref()?)))
            .map(|(entry, cache)| {
                cache
                    .annotations
                    .iter()
                    .zip(&cache.anchors)
                    .filter(|(annotation, _)| is_marked(entry, annotation))
                    .filter_map(|(_, anchor)| match anchor {
                        Anchor::Found {
                            start_line,
                            end_line,
                            ..
                        } => Some((*start_line, *end_line)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn viewer_open_notes(&self, key: &PaneKey) -> usize {
        self.viewer_record(key).map_or(0, |(_, _, record)| {
            record
                .annotations
                .iter()
                .filter(|annotation| annotation.status.is_open())
                .count()
        })
    }

    /// Whether the document changed since the waiting review was first seen.
    fn viewer_changed_during_wait(&self, key: &PaneKey) -> bool {
        self.viewers
            .get(key)
            .and_then(|entry| Some((entry, entry.wait_seen.as_ref()?)))
            .is_some_and(|(entry, (_, version))| *version != entry.version)
    }

    /// Whether the pane's service anchors annotations and writes reviews.
    pub(super) fn viewer_reviews_supported(&self, key: &PaneKey) -> bool {
        self.model_for_source(key.source_id())
            .is_some_and(|model| model.reviews_supported())
    }

    /// The service's anchors for a version of a pane's document arrived;
    /// they stand for the targets they were asked for.
    pub(in crate::workspace) fn viewer_anchors(
        &mut self,
        update: ViewerAnchors,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self
            .model_for_source(update.source.source_id())
            .filter(|model| model.source_token() == update.source)
            .map(|model| model.pane_key(&update.pane))
        else {
            return;
        };
        let Some(entry) = self
            .viewers
            .get_mut(&key)
            .filter(|entry| entry.source == update.source)
        else {
            return;
        };
        // Anchors of another version wait for the window's own read of it.
        let (Some(text), Some(asked)) = (entry.text.clone(), entry.asked.as_ref()) else {
            return;
        };
        if entry.version.as_deref() != Some(update.version.as_str())
            || asked.version != update.version
        {
            return;
        }
        let targets: HashMap<&str, &Option<AnnotationTarget>> = asked
            .targets
            .iter()
            .map(|(id, target)| (id.as_str(), target))
            .collect();
        let mut by_id = HashMap::new();
        let mut order = Vec::new();
        for anchor in &update.anchors {
            let Some(target) = targets.get(anchor.id.as_str()) else {
                continue;
            };
            order.push(anchor.id.clone());
            by_id.insert(
                anchor.id.clone(),
                ((*target).clone(), Anchor::from_wire(anchor, &text)),
            );
        }
        entry.answered = Some(Answered {
            version: update.version,
            by_id,
            order,
        });
        entry.anchors = None;
        self.refresh_viewer_notes(&key, cx);
        cx.notify();
    }

    /// Ask the pane's service for its review, to do `purpose` with it.
    fn viewer_ask_review(&mut self, key: &PaneKey, purpose: ReviewPurpose, cx: &mut Context<Self>) {
        let Some((_, source, _)) = self.viewer_record(key) else {
            return;
        };
        if !self.viewer_reviews_supported(key) {
            self.status = "Reviews need a newer Splitty on this host".into();
            cx.notify();
            return;
        }
        let verdict = match &purpose {
            ReviewPurpose::Answer { verdict, .. } => Some(*verdict),
            _ => None,
        };
        let changed_during_wait = verdict.is_some() && self.viewer_changed_during_wait(key);
        let token = NEXT_REVIEW.fetch_add(1, Ordering::Relaxed);
        if let Some(entry) = self.viewers.get_mut(key) {
            if let ReviewPurpose::Answer { wait_id, .. } = &purpose {
                // The banner says the answer is on its way meanwhile.
                entry.answering = Some(Answering {
                    wait_id: wait_id.clone(),
                    entries: Vec::new(),
                });
            }
            entry.reviewing = Some((token, purpose));
        }
        let command = Command::ReviewViewer {
            pane: key.pane_id().to_owned(),
            token,
            verdict,
            changed_during_wait,
        };
        self.send_captured(source, command, cx);
    }

    /// The service wrote a review asked for: do with it what it was asked for.
    pub(in crate::workspace) fn viewer_reviewed(
        &mut self,
        reviewed: ViewerReviewed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self
            .model_for_source(reviewed.source.source_id())
            .filter(|model| model.source_token() == reviewed.source)
            .map(|model| model.pane_key(&reviewed.pane))
        else {
            return;
        };
        let Some(entry) = self
            .viewers
            .get_mut(&key)
            .filter(|entry| entry.source == reviewed.source)
        else {
            return;
        };
        if entry
            .reviewing
            .as_ref()
            .is_none_or(|(token, _)| *token != reviewed.token)
        {
            return;
        }
        let Some((_, purpose)) = entry.reviewing.take() else {
            return;
        };
        let review = match reviewed.review {
            Ok(review) => review,
            Err(error) => {
                if matches!(purpose, ReviewPurpose::Answer { .. }) {
                    entry.answering = None;
                }
                self.status = format!(
                    "The review could not be written: {}",
                    super::hosts::display(&error)
                );
                cx.notify();
                return;
            }
        };
        match purpose {
            ReviewPurpose::Copy => {
                if review.count > 0 {
                    cx.write_to_clipboard(ClipboardItem::new_string(review.text));
                    self.status = "Review copied".into();
                    cx.notify();
                }
            }
            ReviewPurpose::Send { target, how } => {
                self.viewer_deliver_review(&key, review, target, how, window, cx);
            }
            ReviewPurpose::Answer { verdict, wait_id } => {
                self.viewer_deliver_answer(&key, review, verdict, wait_id, cx);
            }
        }
    }

    fn viewer_copy_review(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        self.viewer_ask_review(key, ReviewPurpose::Copy, cx);
    }

    /// Hand the review to a terminal of the Screen, by default the one the
    /// viewer came from, once the service wrote it.
    fn viewer_send_review(
        &mut self,
        key: &PaneKey,
        target: Option<PaneKey>,
        how: Handover,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self.viewers.get_mut(key) {
            entry.sending = false;
        }
        self.viewer_ask_review(key, ReviewPurpose::Send { target, how }, cx);
    }

    /// Hand `review` to a terminal of the Screen: pasted, stored as a file
    /// whose path is inserted, or appended to the terminal's scratchpad. A
    /// paste of several lines into a program without bracketed paste goes as
    /// a file. The review's entries are handed over once it went out; a file
    /// only once it is stored and its path inserted.
    fn viewer_deliver_review(
        &mut self,
        key: &PaneKey,
        review: Review,
        target: Option<PaneKey>,
        how: Handover,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if review.count == 0 {
            return;
        }
        let target = target
            .filter(|pane| self.panes.contains_key(pane))
            .or_else(|| {
                self.viewer_review_targets(key)
                    .into_iter()
                    .map(|(pane, _)| pane)
                    .next()
            });
        let Some(target) = target else {
            self.status = "No terminal on this Screen takes the review; it can be copied".into();
            cx.notify();
            return;
        };
        let terminal = self.panes[&target].terminal.clone();
        let form = match how {
            Handover::Paste => ReviewForm::Paste,
            Handover::File => ReviewForm::File,
            Handover::Scratchpad => ReviewForm::Scratchpad,
        };
        let bracketed = terminal.read(cx).bracketed_paste();
        match annotations::review_delivery(&review.text, form, bracketed) {
            ReviewDelivery::Paste(text) => {
                terminal.update(cx, |terminal, _| terminal.paste_text(&text));
                self.viewer_review_sent(
                    key.clone(),
                    target,
                    review.entries,
                    "The review is at the prompt",
                    window,
                    cx,
                );
            }
            ReviewDelivery::File { instead_of_paste } => {
                let route = self.selected_model().and_then(|model| {
                    Some((
                        model.source_token(),
                        model.terminal_token(target.pane_id())?,
                    ))
                });
                let Some((source, route)) = route else {
                    self.status = "This terminal cannot take a file now".into();
                    cx.notify();
                    return;
                };
                self.store_pasted_files(
                    source,
                    vec![crate::terminal::PastedFile::Image {
                        extension: "md",
                        bytes: review.text.into_bytes(),
                    }],
                    super::file_transfer::StoreTarget::Review {
                        route,
                        terminal: terminal.downgrade(),
                        viewer: key.clone(),
                        target,
                        entries: review.entries,
                        instead_of_paste,
                    },
                    cx,
                );
                cx.notify();
            }
            ReviewDelivery::Scratchpad => {
                if let Err(reason) =
                    self.append_scratchpad(target.pane_id(), &review.text, window, cx)
                {
                    self.status = format!("The review was not sent: {reason}");
                    cx.notify();
                    return;
                }
                self.viewer_annotate(key, AnnotationOp::MarkSent(review.entries), cx);
                self.status = "The review is in the terminal's scratchpad".into();
                cx.notify();
            }
        }
    }

    /// The review went to `target` as a paste or as a stored file's path:
    /// its entries are handed over and the terminal takes the keyboard.
    pub(in crate::workspace) fn viewer_review_sent(
        &mut self,
        viewer: PaneKey,
        target: PaneKey,
        entries: Vec<SentEntry>,
        status: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.viewer_annotate(&viewer, AnnotationOp::MarkSent(entries), cx);
        self.select(target.pane_id(), window, cx);
        self.status = status.into();
        cx.notify();
    }

    /// Answer the process that waits for this pane's review: the service
    /// writes the review with the verdict, and it goes to the process.
    fn viewer_answer_review(&mut self, key: &PaneKey, verdict: Verdict, cx: &mut Context<Self>) {
        let Some((wait_id, _)) = self.viewer_wait(key) else {
            return;
        };
        self.viewer_ask_review(key, ReviewPurpose::Answer { verdict, wait_id }, cx);
    }

    fn viewer_deliver_answer(
        &mut self,
        key: &PaneKey,
        review: Review,
        verdict: Verdict,
        wait_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some((screen, source, _)) = self.viewer_record(key) else {
            return;
        };
        // The process stopped waiting meanwhile, or another wait began.
        if self.viewer_wait(key).is_none_or(|(id, _)| id != wait_id) {
            if let Some(entry) = self.viewers.get_mut(key) {
                entry.answering = None;
            }
            cx.notify();
            return;
        }
        // An approval without anything pending carries no review.
        let text =
            (verdict == Verdict::ChangesRequested || review.count > 0).then_some(review.text);
        let command = Command::AnswerReview {
            screen,
            pane: key.pane_id().to_owned(),
            wait_id: wait_id.clone(),
            verdict,
            review: text,
        };
        self.send_captured(source, command, cx);
        if let Some(entry) = self.viewers.get_mut(key) {
            entry.answering = Some(Answering {
                wait_id,
                entries: review.entries,
            });
        }
        cx.notify();
    }

    /// The service took or refused a verdict: an accepted review's entries
    /// are handed over.
    pub(in crate::workspace) fn review_answered(
        &mut self,
        answer: ReviewAnswered,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self
            .model_for_source(answer.source.source_id())
            .filter(|model| model.source_token() == answer.source)
            .map(|model| model.pane_key(&answer.pane))
        else {
            return;
        };
        let Some(answering) = self
            .viewers
            .get_mut(&key)
            .and_then(|entry| entry.answering.take())
            .filter(|answering| answering.wait_id == answer.wait_id)
        else {
            return;
        };
        if !answer.accepted {
            self.status =
                "The review was not accepted; the process may have stopped waiting".into();
        } else if !answering.entries.is_empty() {
            self.viewer_annotate(&key, AnnotationOp::MarkSent(answering.entries), cx);
        }
        cx.notify();
    }

    fn note_action(
        &self,
        id: String,
        label: impl Into<SharedString>,
        primary: bool,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let colors = &self.appearance;
        div()
            .id(SharedString::from(id))
            .flex_none()
            .px(chrome_rems(6.))
            .py(chrome_rems(2.))
            .rounded(chrome_rems(4.))
            .cursor_pointer()
            .text_size(chrome_rems(NOTE_META))
            .text_color(rgb(if primary {
                colors.accent
            } else {
                colors.secondary
            }))
            .when(primary, |action| {
                action.border_1().border_color(rgb(colors.accent))
            })
            .hover(|style| style.bg(rgba(colors.hover_overlay)))
            .child(label.into())
            .on_click(cx.listener(move |this, _, window, cx| run(this, window, cx)))
    }

    /// The list of a pane's annotations with its field, beside the document.
    pub(super) fn render_viewer_notes(&self, key: &PaneKey, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.appearance;
        let Some(entry) = self.viewers.get(key) else {
            return div().into_any_element();
        };
        let record = self.viewer_record(key).map(|(_, _, record)| record);
        let cache = entry.anchors.as_ref();
        let mut panel = div()
            .id(SharedString::from(format!("viewer-notes-{key}")))
            .flex()
            .flex_col()
            .flex_none()
            .w(chrome_rems(NOTES_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(rgb(colors.subtle_border))
            .bg(rgb(colors.raised))
            .text_size(chrome_rems(NOTE_TEXT))
            .text_color(rgb(colors.text));
        let _ = &record;
        // An older host neither anchors annotations nor writes the review:
        // nothing to annotate or answer there until it is updated.
        if !self.viewer_reviews_supported(key) {
            return panel
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(chrome_rems(6.))
                        .p(chrome_rems(10.))
                        .child("Annotations need a newer Splitty on this host.")
                        .child(
                            div()
                                .text_size(chrome_rems(NOTE_META))
                                .text_color(rgb(colors.muted))
                                .child("Update the host on the Hosts page to annotate this document and answer reviews."),
                        ),
                )
                .into_any_element();
        }
        if let Some((_, title)) = self.viewer_wait(key) {
            let (approve, request) = (key.clone(), key.clone());
            let pending = entry.answering.is_some();
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(chrome_rems(6.))
                    .p(chrome_rems(10.))
                    .border_b_1()
                    .border_color(rgb(colors.subtle_border))
                    .child(format!(
                        "{} waits for your review",
                        if title.is_empty() {
                            "A process".to_owned()
                        } else {
                            super::hosts::display(&title)
                        }
                    ))
                    .when(self.viewer_changed_during_wait(key), |banner| {
                        banner.child(
                            div()
                                .text_size(chrome_rems(NOTE_META))
                                .text_color(rgb(colors.warning))
                                .child("The document changed since the review was asked for."),
                        )
                    })
                    .child(if pending {
                        div()
                            .text_color(rgb(colors.muted))
                            .child("Sending your answer…")
                    } else {
                        div()
                            .flex()
                            .gap(chrome_rems(6.))
                            .child(self.note_action(
                                format!("viewer-approve-{key}"),
                                "Approve",
                                true,
                                cx,
                                move |this, _, cx| {
                                    this.viewer_answer_review(&approve, Verdict::Approved, cx)
                                },
                            ))
                            .child(self.note_action(
                                format!("viewer-request-{key}"),
                                "Request changes",
                                false,
                                cx,
                                move |this, _, cx| {
                                    this.viewer_answer_review(
                                        &request,
                                        Verdict::ChangesRequested,
                                        cx,
                                    )
                                },
                            ))
                    }),
            );
        }
        // The field, with what it will be sent as.
        let replying = entry.reply_to.as_ref().and_then(|id| {
            cache?
                .annotations
                .iter()
                .find(|annotation| &annotation.id == id)
                .map(|annotation| annotation.number)
        });
        let selection = self.viewer_selection(key, cx);
        let purpose = match (&replying, &selection) {
            (Some(number), _) => format!("Reply to #{number}"),
            (None, Some(target)) if target.start_line == target.end_line => {
                format!("Comment on line {}", target.start_line)
            }
            (None, Some(target)) => {
                format!("Comment on lines {}-{}", target.start_line, target.end_line)
            }
            (None, None) => "General comment · select a passage to comment on it".to_owned(),
        };
        let mut compose = div()
            .flex()
            .flex_col()
            .gap(chrome_rems(4.))
            .p(chrome_rems(10.))
            .border_b_1()
            .border_color(rgb(colors.subtle_border));
        let mut purpose_row = div()
            .flex()
            .items_center()
            .gap(chrome_rems(6.))
            .text_size(chrome_rems(NOTE_META))
            .text_color(rgb(colors.muted))
            .child(div().flex_1().min_w_0().child(purpose));
        if replying.is_none() && selection.is_some() && entry.input.is_some() {
            let suggest = key.clone();
            purpose_row = purpose_row.child(self.note_action(
                format!("viewer-suggest-{key}"),
                if entry.suggesting {
                    "Comment only"
                } else {
                    "Suggest change"
                },
                false,
                cx,
                move |this, window, cx| this.viewer_suggest(&suggest, window, cx),
            ));
        }
        if replying.is_some() {
            let cancel = key.clone();
            purpose_row = purpose_row.child(self.note_action(
                format!("viewer-reply-cancel-{key}"),
                "Cancel",
                false,
                cx,
                move |this, window, cx| this.viewer_reply_to(&cancel, None, window, cx),
            ));
        }
        compose = compose.child(purpose_row);
        if let Some(input) = &entry.input {
            let send = key.clone();
            let field = |state: &Entity<TextareaState>| {
                div()
                    .px(chrome_rems(8.))
                    .py(chrome_rems(4.))
                    .border_1()
                    .border_color(rgb(colors.border))
                    .rounded(chrome_rems(5.))
                    .bg(rgb(colors.surface))
                    .child(
                        Textarea::new(state)
                            .appearance(false)
                            .bordered(false)
                            .p_0()
                            .text_size(chrome_rems(NOTE_TEXT))
                            .text_color(rgb(colors.text)),
                    )
            };
            if let Some(replacement) = entry
                .replacement
                .as_ref()
                .filter(|_| entry.suggesting && replying.is_none() && selection.is_some())
            {
                compose = compose
                    .child(
                        div()
                            .text_size(chrome_rems(NOTE_META))
                            .text_color(rgb(colors.muted))
                            .child("Replace the passage with"),
                    )
                    .child(field(replacement))
                    .child(
                        div()
                            .text_size(chrome_rems(NOTE_META))
                            .text_color(rgb(colors.muted))
                            .child("Remark (optional)"),
                    );
            }
            compose = compose
                .child(field(input))
                .child(div().flex().justify_end().child(self.note_action(
                    format!("viewer-note-send-{key}"),
                    "Add",
                    true,
                    cx,
                    move |this, window, cx| this.submit_viewer_note(&send, window, cx),
                )));
        }
        panel = panel.child(compose);

        let mut list = div()
            .id(SharedString::from(format!("viewer-note-list-{key}")))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        let mut total = 0;
        if let Some(cache) = cache {
            let order = &cache.order;
            // The open threads first, the resolved ones after them.
            for open in [true, false] {
                for index in order
                    .iter()
                    .copied()
                    .filter(|index| cache.annotations[*index].status.is_open() == open)
                {
                    let annotation = &cache.annotations[index];
                    total += 1;
                    let place = match &cache.anchors[index] {
                        Anchor::General => "General".to_owned(),
                        Anchor::Found {
                            start_line,
                            end_line,
                            changed,
                            ..
                        } => {
                            let lines = if start_line == end_line {
                                format!("Line {start_line}")
                            } else {
                                format!("Lines {start_line}-{end_line}")
                            };
                            if *changed {
                                format!("{lines} · passage changed")
                            } else {
                                lines
                            }
                        }
                        Anchor::NotFound => "Passage not found".to_owned(),
                        // Until the service anchored it: the lines it was taken from.
                        Anchor::Pending => match &annotation.target {
                            Some(target) if target.start_line == target.end_line => {
                                format!("Line {}", target.start_line)
                            }
                            Some(target) => {
                                format!("Lines {}-{}", target.start_line, target.end_line)
                            }
                            None => "General".to_owned(),
                        },
                    };
                    let mut meta = format!("#{} · {place}", annotation.number);
                    if !open {
                        meta.push_str(" · resolved");
                    } else if !annotations::is_pending(annotation) {
                        meta.push_str(" · handed over");
                    }
                    let is_chosen = entry.chosen.as_deref() == Some(annotation.id.as_str());
                    let (choose, choose_id) = (key.clone(), annotation.id.clone());
                    let mut card = div()
                        .id(SharedString::from(format!(
                            "viewer-note-{key}-{}",
                            annotation.id
                        )))
                        .when(is_chosen, |card| card.bg(rgba(colors.hover_overlay)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.viewer_choose(&choose, &choose_id, cx)
                        }))
                        .flex()
                        .flex_col()
                        .gap(chrome_rems(4.))
                        .px(chrome_rems(10.))
                        .py(chrome_rems(8.))
                        .border_b_1()
                        .border_color(rgb(colors.subtle_border))
                        .when(!open, |card| card.opacity(0.6))
                        .child(
                            div()
                                .text_size(chrome_rems(NOTE_META))
                                .text_color(rgb(colors.muted))
                                .child(meta),
                        );
                    if let Some(target) = &annotation.target {
                        card = card.child(
                            div()
                                .pl(chrome_rems(6.))
                                .border_l_2()
                                .border_color(rgb(colors.warning))
                                .text_size(chrome_rems(NOTE_META))
                                .text_color(rgb(colors.secondary))
                                .child(excerpt(&target.quote, 90)),
                        );
                    }
                    if annotation.kind == AnnotationKind::Change {
                        card = card.child(match annotation.replacement.as_deref() {
                            Some(text) if !text.is_empty() => div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(chrome_rems(NOTE_META))
                                        .text_color(rgb(colors.muted))
                                        .child("Replace with"),
                                )
                                .child(
                                    div()
                                        .px(chrome_rems(6.))
                                        .py(chrome_rems(3.))
                                        .rounded(chrome_rems(4.))
                                        .bg(rgb(colors.surface))
                                        .font_family(crate::terminal::terminal_family(cx))
                                        .text_size(chrome_rems(NOTE_META))
                                        .child(text.to_owned()),
                                ),
                            _ => div()
                                .text_color(rgb(colors.danger))
                                .child("Delete this passage"),
                        });
                    }
                    if let Some(body) = annotation.body.as_deref().filter(|body| !body.is_empty()) {
                        card = card.child(div().child(super::hosts::display(body)));
                    }
                    for reply in &annotation.replies {
                        let author = match reply.author {
                            ReplyAuthor::User => "You".to_owned(),
                            ReplyAuthor::Agent => reply
                                .from
                                .as_ref()
                                .map(|from| excerpt(&from.title, 40))
                                .filter(|title| !title.is_empty())
                                .map_or_else(
                                    || "Agent".to_owned(),
                                    |title| format!("Agent ({title})"),
                                ),
                        };
                        card = card.child(
                            div()
                                .pl(chrome_rems(10.))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(chrome_rems(NOTE_META))
                                        .text_color(rgb(colors.muted))
                                        .child(author),
                                )
                                .child(super::hosts::display(&reply.body)),
                        );
                    }
                    let (reply, resolve, remove) = (key.clone(), key.clone(), key.clone());
                    let (reply_id, resolve_id, remove_id) = (
                        annotation.id.clone(),
                        annotation.id.clone(),
                        annotation.id.clone(),
                    );
                    let mut actions = div().flex().flex_wrap().gap(chrome_rems(2.));
                    if selection.is_some() && selection != annotation.target {
                        let (moved, moved_id) = (key.clone(), annotation.id.clone());
                        actions = actions.child(self.note_action(
                            format!("viewer-note-move-{key}-{}", annotation.id),
                            "Move to selection",
                            false,
                            cx,
                            move |this, _, cx| this.viewer_move_note(&moved, &moved_id, cx),
                        ));
                    }
                    if open {
                        actions = actions.child(self.note_action(
                            format!("viewer-note-reply-{key}-{}", annotation.id),
                            "Reply",
                            false,
                            cx,
                            move |this, window, cx| {
                                this.viewer_reply_to(&reply, Some(reply_id.clone()), window, cx)
                            },
                        ));
                    }
                    actions = actions
                        .child(self.note_action(
                            format!("viewer-note-resolve-{key}-{}", annotation.id),
                            if open { "Resolve" } else { "Reopen" },
                            false,
                            cx,
                            move |this, _, cx| this.viewer_resolve(&resolve, &resolve_id, cx),
                        ))
                        .child(self.note_action(
                            format!("viewer-note-remove-{key}-{}", annotation.id),
                            "Delete",
                            false,
                            cx,
                            move |this, _, cx| {
                                this.viewer_annotate(
                                    &remove,
                                    AnnotationOp::Remove(vec![remove_id.clone()]),
                                    cx,
                                )
                            },
                        ));
                    list = list.child(card.child(actions));
                }
            }
        }
        if total == 0 {
            list = list.child(
                div()
                    .p(chrome_rems(10.))
                    .text_color(rgb(colors.muted))
                    .child("No comments on this document yet."),
            );
        }
        panel = panel.child(list);
        if total > 0 {
            let (marks, resolved) = (key.clone(), key.clone());
            panel = panel.child(
                div()
                    .flex()
                    .flex_none()
                    .gap(chrome_rems(2.))
                    .px(chrome_rems(6.))
                    .py(chrome_rems(3.))
                    .border_t_1()
                    .border_color(rgb(colors.subtle_border))
                    .child(self.note_action(
                        format!("viewer-marks-{key}"),
                        if entry.marks_hidden {
                            "Show marks"
                        } else {
                            "Hide marks"
                        },
                        false,
                        cx,
                        move |this, _, cx| this.viewer_toggle_marks(&marks, false, cx),
                    ))
                    .when(!entry.marks_hidden, |row| {
                        row.child(self.note_action(
                            format!("viewer-resolved-marks-{key}"),
                            if entry.resolved_marks {
                                "Unmark resolved"
                            } else {
                                "Mark resolved too"
                            },
                            false,
                            cx,
                            move |this, _, cx| this.viewer_toggle_marks(&resolved, true, cx),
                        ))
                    }),
            );
        }
        if self.viewer_open_notes(key) > 0 {
            if entry.sending {
                // Every way a review leaves the window, for each terminal.
                let targets = self.viewer_review_targets(key);
                let mut ways = div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .px(chrome_rems(6.))
                    .py(chrome_rems(4.))
                    .border_t_1()
                    .border_color(rgb(colors.subtle_border));
                if targets.is_empty() {
                    ways = ways.child(
                        div()
                            .p(chrome_rems(4.))
                            .text_color(rgb(colors.muted))
                            .child("No terminal on this Screen takes the review."),
                    );
                }
                for (index, (pane, title)) in targets.into_iter().enumerate() {
                    let title = excerpt(&title, 28);
                    for (how, label) in [
                        (Handover::Paste, format!("Paste into {title}")),
                        (Handover::File, format!("As a file for {title}")),
                        (
                            Handover::Scratchpad,
                            format!("To the scratchpad of {title}"),
                        ),
                    ] {
                        if how == Handover::Scratchpad
                            && self.session_key_for_pane(pane.pane_id()).is_none()
                        {
                            continue;
                        }
                        let (pane_key, target) = (key.clone(), pane.clone());
                        ways = ways.child(self.note_action(
                            format!("viewer-way-{key}-{index}-{}", how as u8),
                            label,
                            false,
                            cx,
                            move |this, window, cx| {
                                this.viewer_send_review(
                                    &pane_key,
                                    Some(target.clone()),
                                    how,
                                    window,
                                    cx,
                                )
                            },
                        ));
                    }
                }
                panel = panel.child(ways);
            }
            let (send, copy, more) = (key.clone(), key.clone(), key.clone());
            panel = panel.child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .gap(chrome_rems(6.))
                    .p(chrome_rems(8.))
                    .border_t_1()
                    .border_color(rgb(colors.subtle_border))
                    .child(self.note_action(
                        format!("viewer-review-copy-{key}"),
                        "Copy review",
                        false,
                        cx,
                        move |this, _, cx| this.viewer_copy_review(&copy, cx),
                    ))
                    .child(self.note_action(
                        format!("viewer-review-more-{key}"),
                        if entry.sending {
                            "Fewer ways"
                        } else {
                            "Send to…"
                        },
                        false,
                        cx,
                        move |this, _, cx| this.viewer_toggle_sending(&more, cx),
                    ))
                    .child(self.note_action(
                        format!("viewer-review-send-{key}"),
                        "Send to terminal",
                        true,
                        cx,
                        move |this, window, cx| {
                            this.viewer_send_review(&send, None, Handover::Paste, window, cx)
                        },
                    )),
            );
        }
        panel.into_any_element()
    }
}
