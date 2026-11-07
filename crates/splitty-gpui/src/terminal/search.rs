//! Terminal-local find editor and bounded incremental search scheduling.
use super::*;
use crate::terminal_engine::{SearchProgress, SearchSnapshot};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use std::time::Duration;

pub(super) enum SearchStop {
    Limited,
    Error(String),
}

pub(super) struct SearchUi {
    pub(super) input: Entity<InputState>,
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) snapshot: Option<SearchSnapshot>,
    pub(super) stopped: Option<SearchStop>,
    pub(super) epoch: u64,
    pub(super) pumping: bool,
    pub(super) task: Option<Task<()>>,
    pub(super) _subscription: Subscription,
}

impl Terminal {
    pub fn set_search_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.search_active == active {
            return;
        }
        self.search_active = active;
        self.stop_search_index(cx);
        if active {
            self.start_search_query(cx);
        }
    }

    pub fn search_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.search_active
            && self.search.as_ref().is_some_and(|search| {
                search.open && search.input.read(cx).focus_handle(cx).is_focused(window)
            })
    }

    pub(super) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_none() {
            let selected = self
                .engine
                .as_mut()
                .and_then(|engine| engine.selection_text().ok().flatten())
                .filter(|text| text.len() <= 1024 && !text.contains(['\n', '\r']))
                .unwrap_or_default();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Search…")
                    .default_value(selected.clone())
            });
            let subscription = cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _window, cx| match event {
                    InputEvent::Change => this.search_query_changed(cx),
                    InputEvent::PressEnter { shift, .. } => this.navigate_search(!*shift, cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            );
            self.search = Some(SearchUi {
                input,
                open: true,
                query: selected,
                snapshot: None,
                stopped: None,
                epoch: 0,
                pumping: false,
                task: None,
                _subscription: subscription,
            });
            self.start_search_query(cx);
        } else if let Some(search) = self.search.as_mut() {
            if !search.open {
                search.open = true;
                self.start_search_query(cx);
            }
        }
        if let Some(search) = self.search.as_ref() {
            search.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = self.search.as_mut() {
            search.open = false;
            // Reopening is an explicit retry after a limit or engine failure.
            search.stopped = None;
        }
        self.stop_search_index(cx);
        self.focus_pty(window, cx);
        cx.notify();
    }

    pub(super) fn search_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        let query = search.input.read(cx).value().to_string();
        if query == search.query {
            return;
        }
        search.query = query;
        search.stopped = None;
        self.stop_search_index(cx);
        self.start_search_query(cx);
    }

    pub(super) fn search_reset(&mut self, cx: &mut Context<Self>) {
        self.stop_search_index(cx);
        self.start_search_query(cx);
    }

    pub(super) fn search_wake(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        if !self.search_active
            || !search.open
            || search.query.is_empty()
            || search.stopped.is_some()
        {
            return;
        }
        if search.pumping {
            return;
        }
        let epoch = search.epoch;
        if let Some(search) = self.search.as_mut() {
            search.pumping = true;
        }
        let task = cx.spawn(async move |this, cx| loop {
            let pending = this
                .update(cx, |terminal, cx| terminal.search_advance(epoch, cx))
                .unwrap_or(false);
            if !pending {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
        });
        if let Some(search) = self.search.as_mut().filter(|search| search.epoch == epoch) {
            search.task = Some(task);
        }
    }

    fn search_advance(&mut self, epoch: u64, cx: &mut Context<Self>) -> bool {
        if !self.search_active
            || !self
                .search
                .as_ref()
                .is_some_and(|search| search.epoch == epoch && search.open)
        {
            return false;
        }
        let result = self.engine.as_mut().map(Engine::search_step);
        let pending = match result {
            Some(Ok(snapshot)) => {
                if let Some(search) = self.search.as_mut() {
                    if snapshot.progress == SearchProgress::Limited {
                        search.stopped = Some(SearchStop::Limited);
                    }
                    search.snapshot = Some(snapshot);
                }
                self.engine.as_ref().is_some_and(Engine::search_pending)
            }
            Some(Err(error)) => {
                if let Some(search) = self.search.as_mut() {
                    search.stopped = Some(SearchStop::Error(error.to_string()));
                    search.snapshot = None;
                }
                false
            }
            None => false,
        };
        if !pending {
            if let Some(search) = self.search.as_mut() {
                search.pumping = false;
            }
        }
        self.content_revision += 1;
        cx.notify();
        pending
    }

    fn start_search_query(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        if !self.search_active
            || !search.open
            || search.query.is_empty()
            || search.stopped.is_some()
        {
            cx.notify();
            return;
        }
        let query = search.query.clone();
        let result = self
            .engine
            .as_mut()
            .map(|engine| engine.set_search_query(&query));
        match result {
            Some(Ok(())) => self.search_wake(cx),
            Some(Err(error)) => {
                self.engine.as_mut().map(Engine::clear_search);
                if let Some(search) = self.search.as_mut() {
                    search.stopped = Some(SearchStop::Error(error.to_string()));
                }
            }
            None => {}
        }
        self.content_revision += 1;
        cx.notify();
    }

    fn stop_search_index(&mut self, cx: &mut Context<Self>) {
        if let Some(search) = self.search.as_mut() {
            search.epoch = search.epoch.wrapping_add(1);
            search.task = None;
            search.pumping = false;
            search.snapshot = None;
        }
        self.engine.as_mut().map(Engine::clear_search);
        self.content_revision += 1;
        cx.notify();
    }

    fn navigate_search(&mut self, older: bool, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_ref() else {
            return;
        };
        if !self.search_active
            || !search.open
            || search.query.is_empty()
            || search.stopped.is_some()
        {
            return;
        }
        match self
            .engine
            .as_mut()
            .map(|engine| engine.search_navigate(older))
        {
            Some(Ok(snapshot)) => {
                if let Some(search) = self.search.as_mut() {
                    search.snapshot = Some(snapshot);
                }
                self.search_wake(cx);
            }
            Some(Err(error)) => {
                if let Some(search) = self.search.as_mut() {
                    search.stopped = Some(SearchStop::Error(error.to_string()));
                    search.snapshot = None;
                }
            }
            None => {}
        }
        self.content_revision += 1;
        cx.notify();
    }

    pub(super) fn search_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.search.as_ref().filter(|search| search.open)?;
        let label = if let Some(stopped) = &search.stopped {
            match stopped {
                SearchStop::Limited => "Limit: refine".to_owned(),
                SearchStop::Error(reason) => format!(
                    "Search error: {}",
                    reason.chars().take(24).collect::<String>()
                ),
            }
        } else {
            match search.snapshot.as_ref().map(|snapshot| snapshot.progress) {
                Some(SearchProgress::Limited) => "Limit: refine".to_owned(),
                Some(SearchProgress::Searching) => {
                    format!("Searching… {}", search.snapshot.as_ref()?.total)
                }
                Some(SearchProgress::Complete) => {
                    let snapshot = search.snapshot.as_ref()?;
                    if snapshot.total == 0 {
                        "No results".to_owned()
                    } else if let Some(index) = snapshot.selected {
                        format!("{} of {}", index + 1, snapshot.total)
                    } else {
                        format!("{} found", snapshot.total)
                    }
                }
                _ => String::new(),
            }
        };
        let label_width = if matches!(search.stopped, Some(SearchStop::Error(_))) {
            132.
        } else {
            84.
        };
        let colors = self.colors;
        Some(
            div()
                .id("terminal-search-bar")
                .absolute()
                .top_0()
                .right_0()
                .m_1()
                .w(px(330.))
                .max_w(relative(0.96))
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .p_1()
                .border_1()
                .border_color(rgb(colors.ansi[8]))
                .rounded_sm()
                .bg(rgb(colors.background))
                .text_color(rgb(colors.foreground))
                .text_size(px(12.))
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("terminal-search-input")
                        .flex_1()
                        .min_w_0()
                        .h(px(26.))
                        .child(Input::new(&search.input)),
                )
                .child(
                    div()
                        .w(px(label_width))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .child(
                    div()
                        .id("terminal-search-older")
                        .cursor_pointer()
                        .px_1()
                        .child("↑")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.navigate_search(true, cx);
                            this.focus(window, cx);
                        })),
                )
                .child(
                    div()
                        .id("terminal-search-newer")
                        .cursor_pointer()
                        .px_1()
                        .child("↓")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.navigate_search(false, cx);
                            this.focus(window, cx);
                        })),
                )
                .child(
                    div()
                        .id("terminal-search-close")
                        .cursor_pointer()
                        .px_1()
                        .child("×")
                        .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
                )
                .into_any_element(),
        )
    }
}
