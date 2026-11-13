//! Session scratchpads in terminal panes (`session-scratchpad`).
use super::*;
use crate::terminal::{pasted, Pasted};
use gpui_kit::component::input::{InputEvent, Paste, Textarea, TextareaState};

/// Maximum UTF-8 byte length of a session scratchpad (`64 KiB`).
pub(super) const SCRATCHPAD_LIMIT: usize = 64 * 1024;
const SAVE_DELAY: Duration = Duration::from_millis(500);
/// How long a saved text outlasts snapshots, on a service without versions.
const SETTLE_DURATION: Duration = Duration::from_millis(2000);
const DEFAULT_SHARE: f32 = 1. / 3.;
const MIN_SHARE: f32 = 0.1;
const MAX_SHARE: f32 = 0.8;

/// Cuts `text` to [`SCRATCHPAD_LIMIT`] bytes at a UTF-8 character boundary.
pub(super) fn within_limit(text: &str) -> &str {
    if text.len() <= SCRATCHPAD_LIMIT {
        return text;
    }
    let mut end = SCRATCHPAD_LIMIT;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn session_scratchpad<'a>(
    screen: &'a splitty_client_core::workspace::ScreenState,
    session_id: &str,
) -> Option<&'a str> {
    scratchpad_of(screen, session_id).map(|(text, _)| text)
}

/// A session's scratchpad text and its version on the service.
fn scratchpad_of<'a>(
    screen: &'a splitty_client_core::workspace::ScreenState,
    session_id: &str,
) -> Option<(&'a str, u64)> {
    screen.sessions.iter().find_map(|(id, session)| {
        (id.as_str() == session_id)
            .then_some((session.scratchpad.as_str(), session.scratchpad_version))
    })
}

/// What an editor knows of its text's version on a service that versions
/// scratchpads (`session-scratchpad`, Concurrent Edits): the version the
/// shown text is based on, or the one its own last save is expected to
/// become. It belongs to one connection; another one starts unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ScratchpadVersion {
    known: Option<(Uuid, u64)>,
}

impl ScratchpadVersion {
    fn on(&self, connection: Uuid) -> Option<u64> {
        self.known
            .and_then(|(owner, version)| (owner == connection).then_some(version))
    }

    /// Whether the version of this connection is known.
    pub(super) fn knows(&self, connection: Uuid) -> bool {
        self.on(connection).is_some()
    }

    /// A snapshot with `version` arrived while an edit waits: the edit is
    /// saved on top of it.
    pub(super) fn waiting(&mut self, connection: Uuid, version: u64) {
        let known = self.on(connection).unwrap_or(0).max(version);
        self.known = Some((connection, known));
    }

    /// A snapshot with `version` arrived and no edit waits: whether its text
    /// replaces the shown one. A lower version is from before this editor's
    /// own save.
    pub(super) fn snapshot(&mut self, connection: Uuid, version: u64) -> bool {
        if self.on(connection).is_some_and(|known| version < known) {
            return false;
        }
        self.known = Some((connection, version));
        true
    }

    /// A save goes out: the version it is based on, when known; the save is
    /// then expected to become the next one.
    pub(super) fn save(&mut self, connection: Uuid) -> Option<u64> {
        let base = self.on(connection)?;
        self.known = Some((connection, base + 1));
        Some(base)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ScratchpadSessionKey {
    pub source: SourceId,
    pub session_id: String,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ScratchpadPlacement {
    pub open: bool,
    pub share: f32,
}

impl Default for ScratchpadPlacement {
    fn default() -> Self {
        Self {
            open: false,
            share: DEFAULT_SHARE,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ScratchpadResizeDrag {
    pub session: ScratchpadSessionKey,
    pub body_bounds: Bounds<Pixels>,
}

pub(super) struct ScratchpadEditor {
    pub source: SourceToken,
    pub screen_id: String,
    pub session_id: String,
    pub input: Entity<TextareaState>,
    pub dirty: bool,
    pub full: bool,
    pub syncing: bool,
    pub settle_until: Option<Instant>,
    /// Whether the editor's service versions scratchpads.
    pub versioned: bool,
    pub version: ScratchpadVersion,
    pub debounce_task: Option<Task<()>>,
    pub _subscription: Subscription,
}

#[derive(Default)]
pub(super) struct ScratchpadState {
    pub placements: HashMap<ScratchpadSessionKey, ScratchpadPlacement>,
    pub editors: HashMap<ScratchpadSessionKey, ScratchpadEditor>,
    pub resize: Option<ScratchpadResizeDrag>,
}

impl Workspace {
    pub(super) fn session_key_for_pane(
        &self,
        pane: &str,
    ) -> Option<(SourceToken, String, ScratchpadSessionKey)> {
        let model = self.selected_model()?;
        let source = model.source_token();
        if self.execution_token(source.source_id()) != Some(source) {
            return None;
        }
        let screen_id = model.shown_screen_id()?.to_owned();
        let session_id = model.session_for(pane)?.to_owned();
        session_scratchpad(model.snapshot()?, session_id.as_str())?;
        Some((
            source,
            screen_id,
            ScratchpadSessionKey {
                source: source.source_id(),
                session_id,
            },
        ))
    }

    /// Returns `Some(true)` if the pane has an open scratchpad, `Some(false)` if
    /// it has a closed scratchpad, or `None` when the pane has no live session.
    pub(super) fn scratchpad_state_for_pane(&self, pane: &str) -> Option<bool> {
        let (_, _, key) = self.session_key_for_pane(pane)?;
        Some(
            self.scratchpads
                .placements
                .get(&key)
                .is_some_and(|placement| placement.open),
        )
    }

    /// Whether the pane's session holds non-empty scratchpad text.
    pub(super) fn pane_has_scratchpad_text(&self, pane: &str, cx: &App) -> bool {
        let Some(model) = self.selected_model() else {
            return false;
        };
        let Some(session_id) = model.session_for(pane) else {
            return false;
        };
        let key = ScratchpadSessionKey {
            source: model.source_id(),
            session_id: session_id.to_owned(),
        };
        if let Some(editor) = self.scratchpads.editors.get(&key) {
            return !editor.input.read(cx).value().is_empty();
        }
        model
            .snapshot()
            .and_then(|screen| session_scratchpad(screen, session_id))
            .is_some_and(|text| !text.is_empty())
    }

    pub(super) fn scratchpad_focused(&self, window: &Window, cx: &App) -> bool {
        self.scratchpads
            .editors
            .values()
            .any(|editor| editor.input.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn focused_pane_scratchpad_focused(&self, window: &Window, cx: &App) -> bool {
        let Some(pane) = self
            .selected_model()
            .and_then(SourceWorkspace::focused_pane)
        else {
            return false;
        };
        let Some((_, _, key)) = self.session_key_for_pane(pane) else {
            return false;
        };
        self.scratchpads
            .placements
            .get(&key)
            .is_some_and(|placement| placement.open)
            && self
                .scratchpads
                .editors
                .get(&key)
                .is_some_and(|editor| editor.input.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn focus_pane_terminal(&self, pane: &str, window: &mut Window, cx: &mut App) {
        if let Some(view) = self
            .selected_model()
            .and_then(|model| self.panes.get(&model.pane_key(pane)))
        {
            view.terminal
                .update(cx, |terminal, cx| terminal.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }

    fn ensure_scratchpad_editor(
        &mut self,
        source: SourceToken,
        screen_id: &str,
        key: &ScratchpadSessionKey,
        initial_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        if let Some(editor) = self.scratchpads.editors.get_mut(key) {
            editor.source = source;
            editor.screen_id = screen_id.to_owned();
            return editor.input.clone();
        }
        let shortcut = if cx.platform_info().is_mac() {
            "⌘J"
        } else {
            "Alt+J"
        };
        let placeholder = format!(
            "Scratchpad for this session: notes and prompts for later. Esc returns to the terminal, {shortcut} closes."
        );
        let clamped = within_limit(initial_text).to_owned();
        let full = clamped.len() >= SCRATCHPAD_LIMIT;
        // The shown text is the snapshot's, and so is its version.
        let mut version = ScratchpadVersion::default();
        let model = self
            .selected_model()
            .filter(|model| model.source_token() == source);
        let versioned = model.is_some_and(SourceWorkspace::scratchpad_versions_supported);
        if let Some((_, known)) = model
            .and_then(SourceWorkspace::snapshot)
            .and_then(|screen| scratchpad_of(screen, key.session_id.as_str()))
            .filter(|_| versioned)
        {
            version.snapshot(source.connection_id(), known);
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(placeholder)
                .default_value(clamped)
        });
        let session_key = key.clone();
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.on_scratchpad_change(&session_key, window, cx);
                }
                InputEvent::Blur => {
                    this.flush_scratchpad(&session_key, cx);
                }
                _ => {}
            },
        );
        self.scratchpads.editors.insert(
            key.clone(),
            ScratchpadEditor {
                source,
                screen_id: screen_id.to_owned(),
                session_id: key.session_id.clone(),
                input: input.clone(),
                dirty: false,
                full,
                syncing: false,
                settle_until: None,
                versioned,
                version,
                debounce_task: None,
                _subscription: subscription,
            },
        );
        input
    }

    fn on_scratchpad_change(
        &mut self,
        key: &ScratchpadSessionKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.scratchpads.editors.get_mut(key) else {
            return;
        };
        if editor.syncing {
            return;
        }
        let current = editor.input.read(cx).value().to_string();
        let kept = within_limit(&current).to_owned();
        editor.full = kept.len() >= SCRATCHPAD_LIMIT;
        if kept.len() < current.len() {
            let input = editor.input.clone();
            editor.syncing = true;
            input.update(cx, |state, cx| state.set_value(kept, window, cx));
            if let Some(editor) = self.scratchpads.editors.get_mut(key) {
                editor.syncing = false;
            }
        }
        let Some(editor) = self.scratchpads.editors.get_mut(key) else {
            return;
        };
        editor.dirty = true;
        let task_key = key.clone();
        editor.debounce_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.flush_scratchpad(&task_key, cx);
            });
        }));
        cx.notify();
    }

    pub(super) fn flush_scratchpad(&mut self, key: &ScratchpadSessionKey, cx: &mut Context<Self>) {
        let Some(editor) = self.scratchpads.editors.get_mut(key) else {
            return;
        };
        if !editor.dirty {
            return;
        }
        editor.dirty = false;
        editor.debounce_task = None;
        let source = editor.source;
        let if_version = editor
            .versioned
            .then(|| editor.version.save(source.connection_id()))
            .flatten();
        if if_version.is_none() {
            // No version to base the save on: hold the text for a moment.
            editor.settle_until = Some(Instant::now() + SETTLE_DURATION);
        }
        let text = within_limit(&editor.input.read(cx).value()).to_owned();
        let screen = editor.screen_id.clone();
        let session = editor.session_id.clone();
        self.send_captured(
            source,
            Command::SetScratchpad {
                screen,
                connection: source.connection_id(),
                session,
                text,
                if_version,
            },
            cx,
        );
    }

    pub(super) fn toggle_scratchpad(
        &mut self,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((source, screen_id, key)) = self.session_key_for_pane(pane) else {
            return;
        };
        let initial_text = self
            .selected_model()
            .and_then(SourceWorkspace::snapshot)
            .and_then(|screen| session_scratchpad(screen, key.session_id.as_str()))
            .map(str::to_owned)
            .unwrap_or_default();
        let input =
            self.ensure_scratchpad_editor(source, &screen_id, &key, &initial_text, window, cx);
        let placement = self.scratchpads.placements.entry(key.clone()).or_default();
        placement.open = !placement.open;
        let open = placement.open;
        if open {
            input.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.flush_scratchpad(&key, cx);
            self.focus_pane_terminal(pane, window, cx);
        }
        cx.notify();
    }

    /// Append a text to the scratchpad of a pane's session and open it.
    pub(super) fn append_scratchpad(
        &mut self,
        pane: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        let Some((source, screen_id, key)) = self.session_key_for_pane(pane) else {
            return Err("This terminal has no scratchpad");
        };
        let initial_text = self
            .selected_model()
            .and_then(SourceWorkspace::snapshot)
            .and_then(|screen| session_scratchpad(screen, key.session_id.as_str()))
            .map(str::to_owned)
            .unwrap_or_default();
        let input =
            self.ensure_scratchpad_editor(source, &screen_id, &key, &initial_text, window, cx);
        let mut next = input.read(cx).value().to_string();
        if !next.trim_end().is_empty() {
            next.truncate(next.trim_end().len());
            next.push_str("\n\n");
        }
        next.push_str(text);
        if next.len() > SCRATCHPAD_LIMIT {
            return Err("The review does not fit into the scratchpad");
        }
        input.update(cx, |state, cx| state.set_value(next, window, cx));
        self.on_scratchpad_change(&key, window, cx);
        self.scratchpads.placements.entry(key).or_default().open = true;
        cx.notify();
        Ok(())
    }

    pub(super) fn toggle_focused_scratchpad(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self
            .selected_model()
            .and_then(SourceWorkspace::focused_pane)
            .map(str::to_owned)
        {
            self.toggle_scratchpad(&pane, window, cx);
        }
    }

    pub(super) fn reconcile_scratchpads(
        &mut self,
        window_model: &WindowWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (key, editor) in &mut self.scratchpads.editors {
            let Some(source) = window_model.source(key.source) else {
                continue;
            };
            let found = source
                .shown_screen_id()
                .zip(
                    source
                        .snapshot()
                        .and_then(|snap| scratchpad_of(snap, key.session_id.as_str())),
                )
                .or_else(|| {
                    source.screens().iter().find_map(|info| {
                        source
                            .screen(info.id.as_str())
                            .and_then(ScreenModel::snapshot)
                            .and_then(|snap| scratchpad_of(snap, key.session_id.as_str()))
                            .map(|found| (info.id.as_str(), found))
                    })
                });
            let Some((screen_id, (scratchpad_text, version))) = found else {
                continue;
            };
            editor.source = source.source_token();
            editor.screen_id = screen_id.to_owned();
            editor.versioned = source.scratchpad_versions_supported();
            let connection = source.connection_id();
            if editor.dirty {
                if editor.versioned {
                    editor.version.waiting(connection, version);
                }
                continue;
            }
            if editor.versioned && editor.version.knows(connection) {
                // Versions tell a snapshot from before this editor's save.
                if !editor.version.snapshot(connection, version) {
                    continue;
                }
            } else {
                if editor
                    .settle_until
                    .is_some_and(|until| Instant::now() < until)
                {
                    continue;
                }
                if editor.versioned {
                    editor.version.snapshot(connection, version);
                }
            }
            editor.settle_until = None;
            let server_text = within_limit(scratchpad_text).to_owned();
            if editor.input.read(cx).value().as_ref() != server_text {
                editor.syncing = true;
                editor.full = server_text.len() >= SCRATCHPAD_LIMIT;
                let input = editor.input.clone();
                input.update(cx, |state, cx| state.set_value(server_text, window, cx));
                editor.syncing = false;
            }
        }
    }

    pub(super) fn begin_scratchpad_resize(
        &mut self,
        session: ScratchpadSessionKey,
        slot: LayoutKey,
    ) {
        let Some(slot_bounds) = self.slot_bounds.get(&slot).copied() else {
            return;
        };
        let Some(body_bounds) = slot_body(slot_bounds, self.tab_header_height()) else {
            return;
        };
        self.scratchpads.resize = Some(ScratchpadResizeDrag {
            session,
            body_bounds,
        });
    }

    pub(super) fn drag_scratchpad_resize(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &self.scratchpads.resize else {
            return;
        };
        let height = f32::from(drag.body_bounds.size.height).max(1.);
        let share = (f32::from(drag.body_bounds.bottom() - position.y) / height)
            .clamp(MIN_SHARE, MAX_SHARE);
        if let Some(placement) = self.scratchpads.placements.get_mut(&drag.session) {
            placement.share = share;
            cx.notify();
        }
    }

    pub(super) fn render_scratchpad(
        &self,
        pane: &str,
        slot_key: &LayoutKey,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (source, _, key) = self.session_key_for_pane(pane)?;
        let placement = self.scratchpads.placements.get(&key).copied()?;
        if !placement.open {
            return None;
        }
        let editor = self.scratchpads.editors.get(&key)?;
        let input = editor.input.clone();
        let full = editor.full;
        let resize_key = key.clone();
        let resize_slot = slot_key.clone();
        let paste_key = key.clone();
        let focus_source = source;
        let focus_screen = slot_key.screen().clone();
        let focus_pane = pane.to_owned();
        let selector = format!("scratchpad-{pane}");
        let handle_selector = format!("scratchpad-handle-{pane}");
        Some(
            div()
                .id(SharedString::from(format!("scratchpad-{slot_key}-{pane}")))
                .when(cfg!(all(test, feature = "ui-test-support")), |el| {
                    el.debug_selector(move || selector.clone())
                })
                .relative()
                .flex_none()
                .w_full()
                .h(relative(placement.share))
                .min_h(chrome_rems(56.))
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(rgb(self.appearance.border))
                .bg(rgb(self.appearance.raised))
                .font(font(self.mono_family.clone()))
                .drag_over::<ExternalPaths>(|pad, _, _, _| pad.opacity(0.7))
                .on_drop({
                    let (drop_key, drop_pane) = (key.clone(), pane.to_owned());
                    cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                        if !paths.paths().is_empty() {
                            this.drop_files_into_scratchpad(
                                source,
                                drop_key.clone(),
                                &drop_pane,
                                paths.paths().to_vec(),
                                window,
                                cx,
                            );
                        }
                    })
                })
                .capture_any_mouse_down(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.button == MouseButton::Left {
                        this.focus_captured(focus_source, &focus_screen, &focus_pane, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                // A paste without text carries images or files: they go to
                // the session's server and their paths into the scratchpad.
                .capture_action(cx.listener(move |this, _: &Paste, _, cx| {
                    if let Some(Pasted::Files(files)) =
                        cx.read_from_clipboard().map(|item| pasted(&item))
                    {
                        cx.stop_propagation();
                        this.paste_files_into_scratchpad(source, paste_key.clone(), files, cx);
                    }
                }))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "scratchpad-handle-{slot_key}-{pane}"
                        )))
                        .when(cfg!(all(test, feature = "ui-test-support")), |el| {
                            el.debug_selector(move || handle_selector.clone())
                        })
                        .absolute()
                        .top(px(-3.))
                        .left_0()
                        .right_0()
                        .h(px(6.))
                        .occlude()
                        .cursor(CursorStyle::ResizeRow)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                if !GlobalState::is_in_deferred_context(cx) {
                                    this.begin_scratchpad_resize(
                                        resize_key.clone(),
                                        resize_slot.clone(),
                                    );
                                }
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .px(chrome_rems(6.))
                        .py(chrome_rems(4.))
                        .child(
                            Textarea::new(&input)
                                .appearance(false)
                                .bordered(false)
                                .size_full()
                                .p_0()
                                .text_size(chrome_rems(12.))
                                .text_color(rgb(self.appearance.text)),
                        ),
                )
                .when(full, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .px(chrome_rems(10.))
                            .pb(chrome_rems(4.))
                            .text_size(chrome_rems(11.))
                            .text_color(cx.theme().warning)
                            .child("The scratchpad is full: it keeps at most 64 KiB."),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::ScratchpadVersion;
    use uuid::Uuid;

    /// session-scratchpad, Concurrent Edits (Snapshot from before the save,
    /// Another window saved first, Unsaved edit is kept): a save names its
    /// base, snapshots below its expected version change nothing, a waiting
    /// edit saves on top of newer versions, and another connection starts
    /// unknown.
    #[test]
    fn versions_tell_snapshots_from_before_the_save() {
        let connection = Uuid::new_v4();
        let mut version = ScratchpadVersion::default();
        assert_eq!(version.save(connection), None, "unknown: no ifVersion");
        assert!(version.snapshot(connection, 2));
        assert_eq!(version.save(connection), Some(2));
        assert!(!version.snapshot(connection, 2), "from before the save");
        assert!(version.snapshot(connection, 3), "the save or a first one");
        assert!(version.snapshot(connection, 5));
        version.waiting(connection, 7);
        assert_eq!(version.save(connection), Some(7));
        let reconnected = Uuid::new_v4();
        assert!(!version.knows(reconnected));
        assert!(version.snapshot(reconnected, 1), "a new connection");
        assert_eq!(version.save(reconnected), Some(1));
    }
}
