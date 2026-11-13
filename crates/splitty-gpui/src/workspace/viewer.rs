//! Viewer panes: a file of the pane's host or a text the service keeps, shown
//! in place of a terminal. The document is read while its pane is shown and
//! read again when it changes; nothing in it is run, loaded or followed.
mod notes;
mod syntax;
use super::*;
use gpui_kit::base::text::{TextView, TextViewState, TextViewStyle};
use splitty_client_core::native_workspace::{ViewerContent, ViewerUpdate};
use splitty_client_core::workspace::ViewerSource;

/// How often a shown file is asked whether it changed.
const REREAD: Duration = Duration::from_secs(2);
/// After this long without an answer a read is asked again.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest document that is rendered instead of shown as its source.
const RENDER_LIMIT: usize = 2 * 1024 * 1024;
/// A document whose first bytes hold a NUL is not shown as text.
const TEXT_PROBE: usize = 8 * 1024;
const HEADER_HEIGHT: f32 = 26.;
const SOURCE_TEXT: f32 = 12.;
const SOURCE_LINE: f32 = 18.;

enum Shown {
    /// Rendered Markdown, with its source lines for the other view.
    Markdown {
        view: Entity<TextViewState>,
        lines: Arc<Vec<SharedString>>,
    },
    Text(Arc<Vec<SharedString>>),
    Image(Arc<Image>),
    /// Neither text nor a known image: its size only.
    Binary(usize),
}

pub(super) struct ViewerPane {
    source: SourceToken,
    /// The record's revision the last read was asked for.
    revision: u64,
    version: Option<String>,
    shown: Option<Shown>,
    failure: Option<String>,
    reading: bool,
    last_read: Option<Instant>,
    show_source: bool,
    scroll: UniformListScrollHandle,
    /// The document's text, when it is one: what annotations are anchored in.
    text: Option<Arc<str>>,
    notes_open: bool,
    input: Option<Entity<gpui_kit::component::input::TextareaState>>,
    /// The text a suggested change puts in place of its passage.
    replacement: Option<Entity<gpui_kit::component::input::TextareaState>>,
    suggesting: bool,
    /// The thread chosen in the list or by its mark.
    chosen: Option<String>,
    _input_events: Option<Subscription>,
    _view_events: Option<Subscription>,
    /// The syntax colors of the source lines, once they are worked out.
    syntax: Option<Arc<Vec<syntax::LineSpans>>>,
    syntax_task: Option<gpui::Task<()>>,
    /// The longest source line: it gives the source view its width.
    widest: usize,
    /// The pending read of the selection the rendered view has painted.
    passage_task: Option<gpui::Task<()>>,
    /// The thread the field replies to.
    reply_to: Option<String>,
    /// The chosen lines of the source view, from 0, both included.
    lines: Option<(usize, usize)>,
    /// The passage last selected in the rendered view.
    passage: Option<splitty_client_core::annotations::AnnotationTarget>,
    anchors: Option<notes::AnchorCache>,
    /// The service's anchors for a version of the document, and what they
    /// were last asked for.
    answered: Option<notes::Answered>,
    asked: Option<notes::Asked>,
    /// A review asked of the service, for what it is to do.
    reviewing: Option<(u64, notes::ReviewPurpose)>,
    marks_dirty: bool,
    /// The marks in the document are off, but for the chosen thread.
    marks_hidden: bool,
    /// Resolved threads are marked too.
    resolved_marks: bool,
    /// The ways to hand the review over are listed.
    sending: bool,
    /// The wait for a review seen first, with the document version shown then.
    wait_seen: Option<(String, Option<String>)>,
    answering: Option<notes::Answering>,
}
impl ViewerPane {
    fn new(source: SourceToken) -> Self {
        Self {
            source,
            revision: 0,
            version: None,
            shown: None,
            failure: None,
            reading: false,
            last_read: None,
            show_source: false,
            scroll: UniformListScrollHandle::new(),
            text: None,
            notes_open: false,
            input: None,
            replacement: None,
            suggesting: false,
            chosen: None,
            _input_events: None,
            _view_events: None,
            syntax: None,
            syntax_task: None,
            widest: 0,
            passage_task: None,
            reply_to: None,
            lines: None,
            passage: None,
            anchors: None,
            answered: None,
            asked: None,
            reviewing: None,
            marks_dirty: false,
            marks_hidden: false,
            resolved_marks: false,
            sending: false,
            wait_seen: None,
            answering: None,
        }
    }
}

/// What a document is named and how its record asks to show it.
struct Described {
    name: String,
    file: bool,
    markdown_text: bool,
}

fn describe(source: &ViewerSource) -> Described {
    match source {
        ViewerSource::File { path } => Described {
            name: path.clone(),
            file: true,
            markdown_text: false,
        },
        ViewerSource::Text { title, format, .. } => Described {
            name: title.clone(),
            file: false,
            // Markdown is the only format a text names.
            markdown_text: format.is_some(),
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Markdown,
    /// HTML, rendered as inertly as Markdown.
    Html,
    Text,
    Image(ImageFormat),
    Binary,
}

/// How a document is shown, by its name and its first bytes.
fn classify(name: &str, file: bool, markdown_text: bool, bytes: &[u8]) -> Kind {
    let extension = name
        .rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if file {
        let image = match extension.as_str() {
            "png" => Some(ImageFormat::Png),
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "gif" => Some(ImageFormat::Gif),
            "webp" => Some(ImageFormat::Webp),
            "svg" => Some(ImageFormat::Svg),
            _ => None,
        };
        if let Some(format) = image {
            return Kind::Image(format);
        }
    }
    if bytes[..bytes.len().min(TEXT_PROBE)].contains(&0) {
        return Kind::Binary;
    }
    let rendered = if file {
        match extension.as_str() {
            "md" | "markdown" | "mdown" => Some(Kind::Markdown),
            "html" | "htm" => Some(Kind::Html),
            _ => None,
        }
    } else {
        markdown_text.then_some(Kind::Markdown)
    };
    rendered
        .filter(|_| bytes.len() <= RENDER_LIMIT)
        .unwrap_or(Kind::Text)
}

fn source_lines(text: &str) -> Arc<Vec<SharedString>> {
    Arc::new(
        text.lines()
            .map(|line| SharedString::from(line.replace('\t', "    ")))
            .collect(),
    )
}

impl Workspace {
    /// The viewer record of a pane of the shown Screen.
    fn viewer_source(&self, pane: &str) -> Option<ViewerSource> {
        self.selected_model()?
            .snapshot()?
            .panes
            .values()
            .find(|entry| entry.id.as_str() == pane)?
            .viewer
            .as_ref()
            .map(|viewer| viewer.source.clone())
    }

    pub(super) fn is_viewer_pane(&self, pane: &str) -> bool {
        self.viewer_source(pane).is_some()
    }

    /// Ask for the documents of the shown viewer panes that are due: never
    /// read, reopened by a newer record, or a file last asked a while ago.
    pub(super) fn sync_viewers(&mut self, cx: &mut Context<Self>) {
        let visible = self.notification_window_visible;
        let mut due = Vec::new();
        let mut live = HashSet::new();
        if let Some(model) = self.selected_model() {
            let source = model.source_token();
            let ready = self.execution_token(source.source_id()) == Some(source);
            if let Some(screen) = model.snapshot() {
                for pane in model.displayed_panes() {
                    let Some(record) = screen
                        .panes
                        .values()
                        .find(|entry| entry.id.as_str() == pane)
                        .and_then(|entry| entry.viewer.as_ref())
                    else {
                        continue;
                    };
                    let key = model.pane_key(&pane);
                    live.insert(key.clone());
                    if ready && self.destination == Destination::Screens {
                        due.push((
                            key,
                            source,
                            record.revision,
                            matches!(record.source, ViewerSource::File { .. }),
                        ));
                    }
                }
            }
        }
        // A pane that left the window takes its document with it.
        let known: Vec<_> = self.viewers.keys().cloned().collect();
        for key in known {
            let kept = live.contains(&key)
                || self.model_for_source(key.source_id()).is_some_and(|model| {
                    model.screen_for(key.pane_id()).is_some()
                        && self.viewers[&key].source == model.source_token()
                });
            if !kept {
                self.viewers.remove(&key);
            }
        }
        for key in &live {
            self.refresh_viewer_notes(key, cx);
        }
        let now = Instant::now();
        for (key, source, revision, file) in due {
            let entry = self
                .viewers
                .entry(key.clone())
                .or_insert_with(|| ViewerPane::new(source));
            if entry.source != source {
                *entry = ViewerPane::new(source);
            }
            let waited = entry.last_read.map(|at| now.saturating_duration_since(at));
            let wanted = match waited {
                None => true,
                Some(waited) if entry.reading => waited >= ANSWER_TIMEOUT,
                Some(waited) => {
                    revision > entry.revision
                        || (file && visible && entry.failure.is_none() && waited >= REREAD)
                }
            };
            if !wanted {
                continue;
            }
            let command = Command::ReadViewer {
                pane: key.pane_id().to_owned(),
                // A newer record is read whatever version the file has.
                if_version: entry
                    .version
                    .clone()
                    .filter(|_| revision == entry.revision && entry.shown.is_some()),
            };
            if self
                .commands
                .try_send(WorkerCommand::new(source, command))
                .is_ok()
            {
                let entry = self.viewers.get_mut(&key).expect("entry inserted");
                entry.reading = true;
                entry.revision = revision;
                entry.last_read = Some(now);
                cx.notify();
            }
        }
    }

    pub(super) fn viewer_update(&mut self, update: ViewerUpdate, cx: &mut Context<Self>) {
        let Some(model) = self.model_for_source(update.source.source_id()) else {
            return;
        };
        if model.source_token() != update.source {
            return;
        }
        let key = model.pane_key(&update.pane);
        let described = model
            .screen_for(&update.pane)
            .and_then(|screen| model.screen(screen))
            .and_then(|screen| screen.snapshot())
            .and_then(|screen| {
                screen
                    .panes
                    .values()
                    .find(|entry| entry.id.as_str() == update.pane)
            })
            .and_then(|entry| entry.viewer.as_ref())
            .map(|viewer| describe(&viewer.source));
        let Some(entry) = self
            .viewers
            .get_mut(&key)
            .filter(|entry| entry.source == update.source)
        else {
            return;
        };
        entry.reading = false;
        match update.content {
            ViewerContent::Unchanged => {}
            ViewerContent::Failed { code, error } => {
                entry.shown = None;
                entry.version = None;
                entry.text = None;
                entry.failure = Some(match code.as_str() {
                    "too_large" => "This file is larger than 8 MiB and is not shown.".into(),
                    "not_a_file" => "This is not a regular file.".into(),
                    "not_found" => "This file no longer exists.".into(),
                    _ if error.is_empty() => "The document could not be read.".into(),
                    _ => super::hosts::display(&error),
                });
            }
            ViewerContent::Document { version, bytes } => {
                let Some(described) = described else {
                    return;
                };
                entry.version = Some(version);
                entry.failure = None;
                // The text the service anchors annotations in: its ranges
                // index this text.
                let text = || splitty_client_core::annotations::viewer_source_text(&bytes);
                entry.lines = None;
                entry.passage = None;
                entry.text = None;
                entry.shown = Some(
                    match classify(
                        &described.name,
                        described.file,
                        described.markdown_text,
                        &bytes,
                    ) {
                        Kind::Binary => Shown::Binary(bytes.len()),
                        Kind::Image(format) => {
                            Shown::Image(Arc::new(Image::from_bytes(format, bytes.to_vec())))
                        }
                        Kind::Text => {
                            let text = text();
                            let lines = source_lines(&text);
                            entry.text = Some(Arc::from(text));
                            Shown::Text(lines)
                        }
                        kind @ (Kind::Markdown | Kind::Html) => {
                            let text = text();
                            let lines = source_lines(&text);
                            entry.text = Some(Arc::from(text.as_str()));
                            // The same view keeps its reading position.
                            let view = match entry.shown.take() {
                                Some(Shown::Markdown { view, .. }) => {
                                    view.update(cx, |state, cx| state.set_text(&text, cx));
                                    view
                                }
                                _ => {
                                    let view = cx.new(|cx| {
                                        if kind == Kind::Html {
                                            TextViewState::html(&text, cx)
                                        } else {
                                            TextViewState::markdown(&text, cx)
                                        }
                                        .selectable(true)
                                        .scrollable(true)
                                    });
                                    let passage = key.clone();
                                    entry._view_events =
                                        Some(cx.observe(&view, move |this, _, cx| {
                                            this.viewer_selection_changed(&passage, cx)
                                        }));
                                    view
                                }
                            };
                            Shown::Markdown { view, lines }
                        }
                    },
                );
                // Colors follow on a worker; the plain lines show meanwhile.
                entry.syntax = None;
                entry.syntax_task = None;
                let lines = match &entry.shown {
                    Some(Shown::Text(lines)) | Some(Shown::Markdown { lines, .. }) => {
                        Some(lines.clone())
                    }
                    _ => None,
                };
                if let Some(lines) = lines {
                    entry.widest = lines
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, line)| line.len())
                        .map_or(0, |(index, _)| index);
                    let (name, version, pane) =
                        (described.name.clone(), entry.version.clone(), key.clone());
                    entry.syntax_task = Some(cx.spawn(async move |this, cx| {
                        let spans = cx
                            .background_executor()
                            .spawn(async move { syntax::highlight(&name, &lines) })
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(entry) = this
                                .viewers
                                .get_mut(&pane)
                                .filter(|entry| entry.version == version)
                            {
                                entry.syntax = spans.map(Arc::new);
                                cx.notify();
                            }
                        });
                    }));
                }
            }
        }
        self.refresh_viewer_notes(&key, cx);
        cx.notify();
    }

    fn viewer_read_again(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        if let Some(entry) = self.viewers.get_mut(key) {
            entry.failure = None;
            entry.reading = false;
            entry.last_read = None;
        }
        self.sync_viewers(cx);
    }

    fn viewer_toggle_source(&mut self, key: &PaneKey, cx: &mut Context<Self>) {
        if let Some(entry) = self.viewers.get_mut(key) {
            entry.show_source = !entry.show_source;
            cx.notify();
        }
    }

    fn render_source_lines(
        &self,
        key: &PaneKey,
        lines: &Arc<Vec<SharedString>>,
        syntax: Option<&Arc<Vec<syntax::LineSpans>>>,
        widest: usize,
        scroll: &UniformListScrollHandle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &self.appearance;
        let ansi = colors.terminal.ansi;
        let syntax = syntax.cloned();
        let (muted, text, chosen_bg, mark) = (
            colors.muted,
            colors.text,
            colors.hover_overlay,
            colors.warning,
        );
        let lines = lines.clone();
        let digits = lines.len().max(1).to_string().len() as f32;
        let family = crate::terminal::terminal_family(cx);
        let row_family = family.clone();
        let weak = cx.entity().downgrade();
        let key = key.clone();
        uniform_list(
            SharedString::from(format!("viewer-source-{key}")),
            lines.len(),
            move |range, _, cx| {
                let Some(view) = weak.upgrade() else {
                    return Vec::new();
                };
                view.update(cx, |this, cx| {
                    let chosen = this.viewer_lines(&key);
                    let marked = this.viewer_marked_lines(&key);
                    range
                        .map(|index| {
                            let line = u32::try_from(index + 1).unwrap_or(u32::MAX);
                            let is_chosen =
                                chosen.is_some_and(|(first, last)| (first..=last).contains(&index));
                            let is_marked = marked
                                .iter()
                                .any(|(first, last)| (*first..=*last).contains(&line));
                            let choose = key.clone();
                            div()
                                .id(("viewer-line", index))
                                .flex()
                                .flex_none()
                                // The list measures a row outside its own text
                                // style: the row names its font itself.
                                .font_family(row_family.clone())
                                .text_size(chrome_rems(SOURCE_TEXT))
                                .h(chrome_rems(SOURCE_LINE))
                                .items_center()
                                .whitespace_nowrap()
                                .when(is_chosen, |row| row.bg(rgba(chosen_bg)))
                                .child(
                                    // Choosing a number chooses its line to comment on.
                                    div()
                                        .id(("viewer-line-number", index))
                                        .flex_none()
                                        .w(chrome_rems(digits * 8. + 20.))
                                        .pr(chrome_rems(10.))
                                        .mr(chrome_rems(2.))
                                        .border_r_2()
                                        .border_color(if is_marked {
                                            rgb(mark)
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .text_right()
                                        .text_color(rgb(muted))
                                        .cursor_pointer()
                                        .child((index + 1).to_string())
                                        .on_click(cx.listener(
                                            move |this, event: &ClickEvent, _, cx| {
                                                this.viewer_choose_line(
                                                    &choose,
                                                    index,
                                                    event.modifiers().shift,
                                                    cx,
                                                )
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .pl(chrome_rems(8.))
                                        .pr(chrome_rems(16.))
                                        .text_color(rgb(text))
                                        .child(
                                            match syntax
                                                .as_ref()
                                                .and_then(|spans| spans.get(index))
                                                .filter(|spans| !spans.is_empty())
                                            {
                                                Some(spans) => {
                                                    StyledText::new(lines[index].clone())
                                                        .with_highlights(spans.iter().map(
                                                            |(range, token)| {
                                                                (
                                                                    range.clone(),
                                                                    HighlightStyle {
                                                                        color: Some(
                                                                            rgb(token
                                                                                .ansi()
                                                                                .map_or(
                                                                                    muted,
                                                                                    |i| ansi[i],
                                                                                ))
                                                                            .into(),
                                                                        ),
                                                                        ..Default::default()
                                                                    },
                                                                )
                                                            },
                                                        ))
                                                        .into_any_element()
                                                }
                                                None => lines[index].clone().into_any_element(),
                                            },
                                        ),
                                )
                        })
                        .collect::<Vec<_>>()
                })
            },
        )
        .track_scroll(scroll)
        // Long lines are not wrapped; the view scrolls sideways to them.
        .with_width_from_item(Some(widest))
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .size_full()
        .font_family(family)
        .text_size(chrome_rems(SOURCE_TEXT))
        .into_any_element()
    }

    /// A viewer pane's body: its header and its document, a failure with the
    /// way to read again, or the wait for the first answer.
    pub(super) fn render_viewer(&self, pane: &str, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.appearance;
        let Some(key) = self.selected_model().map(|model| model.pane_key(pane)) else {
            return div().into_any_element();
        };
        let name = self
            .viewer_source(pane)
            .map(|source| describe(&source).name)
            .unwrap_or_default();
        let entry = self.viewers.get(&key);
        let rendered = matches!(
            entry.and_then(|entry| entry.shown.as_ref()),
            Some(Shown::Markdown { .. })
        );
        let show_source = entry.is_some_and(|entry| entry.show_source);
        let mut header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(chrome_rems(8.))
            .h(chrome_rems(HEADER_HEIGHT))
            .px(chrome_rems(10.))
            .border_b_1()
            .border_color(rgb(colors.subtle_border))
            .text_size(chrome_rems(11.))
            .text_color(rgb(colors.muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(super::hosts::display(&name)),
            );
        if rendered {
            let toggle_key = key.clone();
            header = header.child(
                div()
                    .id(SharedString::from(format!("viewer-mode-{key}")))
                    .flex_none()
                    .px(chrome_rems(6.))
                    .py(chrome_rems(2.))
                    .rounded(chrome_rems(4.))
                    .cursor_pointer()
                    .text_color(rgb(colors.secondary))
                    .hover(|style| style.bg(rgba(colors.hover_overlay)))
                    .child(if show_source { "Rendered" } else { "Source" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.viewer_toggle_source(&toggle_key, cx)
                    })),
            );
        }
        {
            let notes_key = key.clone();
            let open = self.viewer_open_notes(&key);
            let waiting = self.viewer_wait(&key).is_some();
            let notes_open = entry.is_some_and(|entry| entry.notes_open);
            header = header.child(
                div()
                    .id(SharedString::from(format!("viewer-notes-toggle-{key}")))
                    .flex_none()
                    .px(chrome_rems(6.))
                    .py(chrome_rems(2.))
                    .rounded(chrome_rems(4.))
                    .cursor_pointer()
                    .text_color(rgb(if open > 0 || waiting {
                        colors.warning
                    } else {
                        colors.secondary
                    }))
                    .when(notes_open, |toggle| toggle.bg(rgba(colors.hover_overlay)))
                    .hover(|style| style.bg(rgba(colors.hover_overlay)))
                    // A process waiting for the verdict is said on the pane itself.
                    .child(match (waiting, open) {
                        (true, 0) => "Review waiting".to_owned(),
                        (true, open) => format!("Review waiting · {open}"),
                        (false, 0) => "Comments".to_owned(),
                        (false, open) => format!("Comments {open}"),
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_viewer_notes(&notes_key, window, cx)
                    })),
            );
        }
        let notice = |text: SharedString| {
            div()
                .p(chrome_rems(14.))
                .text_size(chrome_rems(12.))
                .text_color(rgb(colors.secondary))
                .child(text)
        };
        let body: AnyElement = match entry {
            None => notice("Reading document…".into()).into_any_element(),
            Some(entry) => match (&entry.shown, &entry.failure) {
                (_, Some(failure)) => {
                    let again = key.clone();
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(notice(failure.clone().into()))
                        .child(
                            div()
                                .id(SharedString::from(format!("viewer-again-{key}")))
                                .ml(chrome_rems(14.))
                                .px(chrome_rems(8.))
                                .py(chrome_rems(4.))
                                .rounded(chrome_rems(4.))
                                .border_1()
                                .border_color(rgb(colors.border))
                                .text_size(chrome_rems(12.))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgba(colors.hover_overlay)))
                                .child("Read again")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.viewer_read_again(&again, cx)
                                })),
                        )
                        .into_any_element()
                }
                (None, None) => notice("Reading document…".into()).into_any_element(),
                (Some(Shown::Binary(size)), None) => notice(
                    format!(
                        "This document is not text and is not shown ({}).",
                        super::file_transfer::format_bytes(*size as u64)
                    )
                    .into(),
                )
                .into_any_element(),
                (Some(Shown::Image(image)), None) => div()
                    .size_full()
                    .p(chrome_rems(12.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(image.clone())
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element(),
                (Some(Shown::Text(lines)), None) => self.render_source_lines(
                    &key,
                    lines,
                    entry.syntax.as_ref(),
                    entry.widest,
                    &entry.scroll,
                    cx,
                ),
                (Some(Shown::Markdown { lines, .. }), None) if show_source => self
                    .render_source_lines(
                        &key,
                        lines,
                        entry.syntax.as_ref(),
                        entry.widest,
                        &entry.scroll,
                        cx,
                    ),
                (Some(Shown::Markdown { view, .. }), None) => div()
                    .size_full()
                    .px(chrome_rems(18.))
                    .py(chrome_rems(10.))
                    // A press in the document starts over: a click drops the
                    // passage, a drag selects the next one.
                    .capture_any_mouse_down({
                        let passage = key.clone();
                        cx.listener(move |this, _, _, cx| this.viewer_drop_passage(&passage, cx))
                    })
                    .child(
                        TextView::new(view)
                            .style(
                                TextViewStyle::default()
                                    .with_foreground(rgb(colors.text).into())
                                    .with_muted_foreground(rgb(colors.muted).into())
                                    .with_link(rgb(colors.accent).into())
                                    .with_selection(
                                        rgba(colors.selection_background << 8 | 0x88).into(),
                                    )
                                    .with_code_background(rgb(colors.raised).into())
                                    .with_border(rgb(colors.border).into())
                                    .with_inline_code(HighlightStyle {
                                        background_color: Some(rgb(colors.raised).into()),
                                        ..Default::default()
                                    }),
                            )
                            .selectable(true)
                            .scrollable(true)
                            // Rendered documents are inert: a link opens nothing
                            // and an image loads nothing.
                            .on_link_click(|_, _, _, _| {})
                            .image_source(|_| ImageSource::Custom(Arc::new(|_, _| None)))
                            .size_full(),
                    )
                    .into_any_element(),
            },
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(rgb(colors.surface))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_hidden()
                            .child(body),
                    )
                    .when(entry.is_some_and(|entry| entry.notes_open), |row| {
                        row.child(self.render_viewer_notes(&key, cx))
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{classify, source_lines, Kind};
    use gpui_kit::ImageFormat;

    #[test]
    fn documents_are_shown_by_their_name_and_first_bytes() {
        let text = b"# Plan\n\nDo it.";
        assert_eq!(classify("/w/PLAN.md", true, false, text), Kind::Markdown);
        assert_eq!(
            classify("/w/notes.MARKDOWN", true, false, text),
            Kind::Markdown
        );
        assert_eq!(classify("/w/main.rs", true, false, text), Kind::Text);
        assert_eq!(classify("/w/report.HTML", true, false, text), Kind::Html);
        assert_eq!(classify("/w/page.htm", true, false, text), Kind::Html);
        // A text from an agent is rendered only when its record says so.
        assert_eq!(classify("plan.md", false, false, text), Kind::Text);
        assert_eq!(classify("Plan", false, true, text), Kind::Markdown);
        assert_eq!(
            classify("/w/shot.PNG", true, false, &[0x89, b'P', 0, 0]),
            Kind::Image(ImageFormat::Png)
        );
        assert_eq!(classify("shot.png", false, false, b"text"), Kind::Text);
        assert_eq!(
            classify("/bin/ls", true, false, &[0x7f, b'E', 0]),
            Kind::Binary
        );
        let large = vec![b'a'; 2 * 1024 * 1024 + 1];
        assert_eq!(classify("/w/big.md", true, false, &large), Kind::Text);
        assert_eq!(
            source_lines("a\n\tb\r\nc")
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>(),
            ["a", "    b", "c"]
        );
    }
}
