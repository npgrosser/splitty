//! Native projection of one authoritative local screen; no independent dock tree.
use crate::{
    terminal::{self, Terminal},
    workspace_session::{Command, Update, Worker},
};
use gpui_kit::{prelude::*, *};
use splitty_client_core::workspace::{Axis, LayoutNode, LocalWorkspace};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

// The worker queue is the only admission boundary. A successful terminal send
// already owns one of its bounded slots, with the pane/session route captured.
fn terminal_commands(
    sender: mpsc::Sender<Command>,
    pane: String,
    session: String,
) -> impl Fn(terminal::Command) -> Result<(), String> {
    move |command| {
        let command = match command {
            terminal::Command::Input {
                generation: 0,
                text,
            } => Command::Input {
                pane: pane.clone(),
                session: session.clone(),
                text,
            },
            terminal::Command::Input { .. } => {
                return Err("Terminal generation changed".into());
            }
            terminal::Command::Resize(cols, rows) => Command::ResizePane {
                pane: pane.clone(),
                session: session.clone(),
                cols,
                rows,
            },
        };
        sender.try_send(command).map_err(|error| error.to_string())
    }
}

struct PaneView {
    session: String,
    terminal: Entity<Terminal>,
    marker: bool,
    tail: String,
}
struct ResizeDrag {
    node: String,
    axis: Axis,
    index: usize,
    start: Point<Pixels>,
    bounds: Bounds<Pixels>,
    sizes: Vec<f64>,
    last_sent: Instant,
    pending: Vec<f64>,
}
pub struct Workspace {
    model: Option<LocalWorkspace>,
    panes: HashMap<String, PaneView>,
    updates: mpsc::Receiver<Update>,
    commands: mpsc::Sender<Command>,
    stopping: Arc<AtomicBool>,
    split_bounds: HashMap<String, Bounds<Pixels>>,
    drag: Option<ResizeDrag>,
    visible: Vec<String>,
    status: String,
    _poll: Task<()>,
    started: Instant,
    smoke: bool,
    smoke_stage: u8,
    smoke_first: Option<String>,
    smoke_tab: Option<String>,
    smoke_initial: Option<Entity<Terminal>>,
    smoke_markers: bool,
    outcome: Arc<AtomicBool>,
}
impl Workspace {
    pub fn new(
        smoke: bool,
        outcome: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Self, Worker) {
        let (commands, receiver) = mpsc::channel(64);
        let (sender, updates) = mpsc::channel(64);
        let worker = Worker::start(receiver, sender);
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            if this
                .update_in(cx, |this, window, cx| this.poll(window, cx))
                .is_err()
            {
                break;
            }
        });
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.stop();
                cx.quit();
            });
            true
        });
        (
            Self {
                model: None,
                panes: HashMap::new(),
                updates,
                commands,
                stopping: worker.stop.clone(),
                split_bounds: HashMap::new(),
                drag: None,
                visible: Vec::new(),
                status: "Starting local workspace…".into(),
                _poll: poll,
                started: Instant::now(),
                smoke,
                smoke_stage: 0,
                smoke_first: None,
                smoke_tab: None,
                smoke_initial: None,
                smoke_markers: false,
                outcome,
            },
            worker,
        )
    }
    fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.updates.close();
        while self.updates.try_recv().is_ok() {}
    }
    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Err(error) = self.commands.try_send(command) {
            self.status = format!("Action not sent: {error}");
            cx.notify();
        }
    }
    fn ensure_pane(&mut self, pane: &str, session: &str, cx: &mut Context<Self>) {
        if self
            .panes
            .get(pane)
            .is_some_and(|view| view.session == session)
        {
            return;
        }
        let commands =
            terminal_commands(self.commands.clone(), pane.to_owned(), session.to_owned());
        let terminal = cx.new(|cx| Terminal::new(commands, cx));
        self.panes.insert(
            pane.to_owned(),
            PaneView {
                session: session.to_owned(),
                terminal,
                marker: false,
                tail: String::new(),
            },
        );
    }
    fn reconcile(&mut self, model: LocalWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(screen) = model.snapshot() {
            self.panes
                .retain(|pane, view| model.session_for(pane) == Some(view.session.as_str()));
            for pane in screen.panes.keys() {
                let id = pane.as_str();
                if let Some(session) = model.session_for(id) {
                    self.ensure_pane(id, session, cx);
                }
            }
            self.status.clear();
        }
        self.model = Some(model);
        self.sync_visible(cx);
        self.focus_current(window, cx);
        cx.notify();
    }
    fn sync_visible(&mut self, cx: &mut Context<Self>) {
        let mut visible = Vec::new();
        if let Some(model) = &self.model {
            if let Some(screen) = model.snapshot() {
                visible_panes(model, &screen.root, &mut visible);
            }
        }
        visible.sort();
        if visible != self.visible {
            self.visible = visible;
            self.send(Command::Visible(self.visible.clone()), cx);
            for pane in &self.visible {
                if let Some(view) = self.panes.get(pane) {
                    view.terminal
                        .update(cx, |terminal, _| terminal.report_size());
                }
            }
        }
    }
    fn focus_current(&self, window: &mut Window, cx: &mut App) {
        if let Some(pane) = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::focused_pane)
            .and_then(|pane| self.panes.get(pane))
        {
            pane.terminal
                .update(cx, |terminal, cx| terminal.focus(window, cx));
        }
    }
    fn select(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(model) = &mut self.model {
            model.select_tab(pane);
        }
        self.send(Command::Select(pane.to_owned()), cx);
        self.sync_visible(cx);
        self.focus_current(window, cx);
        cx.notify();
    }
    fn focus(&mut self, pane: &str, cx: &mut Context<Self>) {
        if self
            .model
            .as_mut()
            .is_some_and(|model| model.focus_pane(pane))
        {
            self.send(Command::Focus(pane.to_owned()), cx);
            cx.notify();
        }
    }
    fn split(&mut self, axis: Axis, cx: &mut Context<Self>) {
        if let Some(pane) = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::focused_pane)
            .map(str::to_owned)
        {
            self.send(Command::Split { pane, axis }, cx);
        }
    }
    fn tab(&mut self, cx: &mut Context<Self>) {
        if self
            .model
            .as_ref()
            .and_then(LocalWorkspace::snapshot)
            .is_some_and(|screen| screen.panes.is_empty())
        {
            self.send(Command::NewTerminal, cx);
            return;
        }
        if let Some(pane) = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::focused_pane)
            .map(str::to_owned)
        {
            self.send(Command::Tab(pane), cx);
        }
    }
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut bytes = 0;
        for _ in 0..64 {
            if bytes >= 256 * 1024 {
                break;
            }
            let Ok(update) = self.updates.try_recv() else {
                break;
            };
            match update {
                Update::Workspace(model) => self.reconcile(*model, window, cx),
                Update::Status(status) => {
                    self.status = status;
                    cx.notify();
                }
                Update::Attached(session) => {
                    for view in self.panes.values().filter(|view| view.session == session) {
                        view.terminal.update(cx, |terminal, cx| {
                            if terminal.engine_ready() {
                                terminal.status = "Attached".into();
                            }
                            terminal.report_size();
                            cx.notify();
                        });
                    }
                }
                Update::Event(event) => {
                    if let Some(output) = event.output() {
                        bytes += output.bytes.len();
                        let pane = self
                            .model
                            .as_ref()
                            .and_then(|model| model.pane_for(output.session_id))
                            .map(str::to_owned);
                        if let Some(pane) = pane {
                            // A known pending spawn can stream before its layout snapshot.
                            // Retain its emulator immediately rather than discarding replay/prompt bytes.
                            self.ensure_pane(&pane, output.session_id, cx);
                            if let Some(view) = self.panes.get_mut(&pane) {
                                if view.session != output.session_id {
                                    continue;
                                }
                                view.terminal.update(cx, |terminal, cx| {
                                    terminal.feed(output.bytes, output.replay, cx)
                                });
                                if self.smoke {
                                    view.tail.push_str(&String::from_utf8_lossy(output.bytes));
                                    view.marker |=
                                        view.tail.contains(&format!("NATIVE_WORKSPACE_{pane}_OK"));
                                    if view.tail.len() > 4096 {
                                        let start = view
                                            .tail
                                            .char_indices()
                                            .rev()
                                            .nth(2048)
                                            .map(|(i, _)| i)
                                            .unwrap_or(0);
                                        view.tail.drain(..start);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if self.smoke {
            self.smoke_tick(window, cx);
        }
    }
    fn smoke_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.started.elapsed() > Duration::from_secs(40) {
            self.finish_smoke(false, cx);
            return;
        }
        let Some(model) = &self.model else {
            return;
        };
        let Some(screen) = model.snapshot() else {
            return;
        };
        let ready = screen.panes.keys().all(|pane| {
            model
                .session_for(pane.as_str())
                .is_some_and(|session| model.session_ready(session))
        });
        if !ready {
            return;
        }
        let count = screen.panes.len();
        match self.smoke_stage {
            0 if count == 1 => {
                let Some(first) = model.focused_pane().map(str::to_owned) else {
                    return;
                };
                self.smoke_initial = self.panes.get(&first).map(|view| view.terminal.clone());
                self.smoke_first = Some(first.clone());
                self.send(
                    Command::Split {
                        pane: first,
                        axis: Axis::Horizontal,
                    },
                    cx,
                );
                self.smoke_stage = 1;
            }
            1 if count == 2 => {
                self.split(Axis::Vertical, cx);
                self.smoke_stage = 2;
            }
            2 if count == 3 => {
                if let Some(first) = self.smoke_first.clone() {
                    self.send(Command::Tab(first), cx);
                }
                self.smoke_stage = 3;
            }
            3 if count == 4 => {
                self.smoke_tab = model.focused_pane().map(str::to_owned);
                let commands: Vec<_> = self
                    .panes
                    .iter()
                    .map(|(pane, view)| Command::Input {
                        pane: pane.clone(),
                        session: view.session.clone(),
                        text: format!("printf 'NATIVE_WORKSPACE_%s_OK\\n' '{pane}'\r"),
                    })
                    .collect();
                for command in commands {
                    self.send(command, cx);
                }
                self.smoke_stage = 4;
            }
            4 if count == 4 && self.panes.values().all(|view| view.marker) => {
                self.smoke_markers = true;
                let resize = match &screen.root {
                    LayoutNode::Split { id, children, .. } if children.len() == 2 => {
                        Some(id.to_string())
                    }
                    _ => None,
                };
                if let Some(first) = self.smoke_first.clone() {
                    self.select(&first, window, cx);
                }
                if let Some(node) = resize {
                    self.send(
                        Command::ResizeSplit {
                            node,
                            sizes: vec![0.65, 0.35],
                        },
                        cx,
                    );
                    self.smoke_stage = 5;
                }
            }
            5 if matches!(&screen.root,LayoutNode::Split{sizes,..} if (sizes[0]-0.65).abs()<0.001) =>
            {
                if let Some(tab) = self.smoke_tab.clone() {
                    self.send(Command::Close(tab), cx);
                }
                self.smoke_stage = 6;
            }
            6 if count == 3
                && self.visible.iter().all(|pane| {
                    self.panes
                        .get(pane)
                        .is_some_and(|view| view.terminal.read(cx).painted_current())
                }) =>
            {
                let stable = self
                    .smoke_first
                    .as_ref()
                    .and_then(|pane| self.panes.get(pane))
                    .zip(self.smoke_initial.as_ref())
                    .is_some_and(|(view, original)| {
                        view.terminal.entity_id() == original.entity_id()
                    });
                self.finish_smoke(stable && self.smoke_markers, cx);
            }
            _ => {}
        }
    }
    fn finish_smoke(&mut self, passed: bool, cx: &mut Context<Self>) {
        self.outcome.store(passed, Ordering::Release);
        eprintln!(
            "{}",
            serde_json::json!({"probe":"splitty-local-workspace","passed":passed,"stage":self.smoke_stage,"panes":self.panes.len(),"distinctSessionMarkers":self.smoke_markers,"visiblePanes":self.visible.len(),"elapsedMs":self.started.elapsed().as_millis(),"status":self.status})
        );
        self.stop();
        cx.quit();
    }
    fn begin_resize(
        &mut self,
        node: String,
        axis: Axis,
        index: usize,
        sizes: Vec<f64>,
        position: Point<Pixels>,
    ) {
        if let Some(bounds) = self.split_bounds.get(&node).copied() {
            self.drag = Some(ResizeDrag {
                node,
                axis,
                index,
                start: position,
                bounds,
                pending: sizes.clone(),
                sizes,
                last_sent: Instant::now() - Duration::from_secs(1),
            });
        }
    }
    fn drag_resize(&mut self, position: Point<Pixels>, force: bool, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let (delta, extent) = match drag.axis {
            Axis::Horizontal => (position.x - drag.start.x, drag.bounds.size.width),
            Axis::Vertical => (position.y - drag.start.y, drag.bounds.size.height),
        };
        if let Some(sizes) = resized_pair(
            &drag.sizes,
            drag.index,
            f64::from(f32::from(delta)) / f64::from(f32::from(extent).max(1.)),
        ) {
            drag.pending = sizes;
        }
        if force || drag.last_sent.elapsed() >= Duration::from_millis(32) {
            drag.last_sent = Instant::now();
            let command = Command::ResizeSplit {
                node: drag.node.clone(),
                sizes: drag.pending.clone(),
            };
            self.send(command, cx);
        }
    }
    fn render_node(&self, node: &LayoutNode, cx: &mut Context<Self>) -> AnyElement {
        match node {
            LayoutNode::Slot { id, pane_ids, .. } => {
                let selected = self
                    .model
                    .as_ref()
                    .and_then(|model| model.selected_pane(node));
                let mut tabs = div()
                    .flex()
                    .h(px(32.))
                    .flex_none()
                    .gap_1()
                    .bg(rgb(0x17212c))
                    .overflow_hidden();
                for (index, pane) in pane_ids.iter().enumerate() {
                    let key = pane.to_string();
                    let close = key.clone();
                    let active = selected == Some(pane.as_str());
                    let title = self
                        .model
                        .as_ref()
                        .and_then(LocalWorkspace::snapshot)
                        .and_then(|screen| screen.panes.get(pane))
                        .and_then(|pane| pane.title.clone())
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| format!("Terminal {}", index + 1));
                    tabs =
                        tabs.child(
                            div()
                                .id(SharedString::from(format!("tab-{pane}")))
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_3()
                                .cursor_pointer()
                                .bg(rgb(if active { 0x293b4d } else { 0x17212c }))
                                .text_color(rgb(if active { 0xe8eff6 } else { 0x93a6b9 }))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select(&key, window, cx)
                                }))
                                .child(div().max_w(px(180.)).overflow_hidden().child(title))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("close-{pane}")))
                                        .px_1()
                                        .child("×")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.send(Command::Close(close.clone()), cx);
                                        })),
                                ),
                        );
                }
                let new_tab = selected.map(str::to_owned);
                tabs = tabs.child(
                    div()
                        .id(SharedString::from(format!("new-tab-{id}")))
                        .px_3()
                        .cursor_pointer()
                        .child("+")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(pane) = &new_tab {
                                this.send(Command::Tab(pane.clone()), cx);
                            }
                        })),
                );
                let focused = self
                    .model
                    .as_ref()
                    .and_then(LocalWorkspace::focused_pane)
                    .is_some_and(|pane| pane_ids.iter().any(|id| id.as_str() == pane));
                let mut slot = div()
                    .id(SharedString::from(format!("slot-{id}")))
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .min_h_0()
                    .border_1()
                    .border_color(rgb(if focused { 0x416783 } else { 0x263544 }))
                    .child(tabs);
                if let Some(pane) = selected {
                    if let Some(view) = self.panes.get(pane) {
                        let focus = pane.to_owned();
                        slot = slot.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .overflow_hidden()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| this.focus(&focus, cx)),
                                )
                                .child(view.terminal.clone()),
                        );
                    } else {
                        slot = slot.child(div().p_3().child("Starting terminal…"));
                    }
                }
                slot.into_any_element()
            }
            LayoutNode::Split {
                id,
                axis,
                children,
                sizes,
            } => {
                let horizontal = *axis == Axis::Horizontal;
                let bounds_id = id.to_string();
                let entity = cx.entity();
                let mut split = div()
                    .id(SharedString::from(format!("split-{id}")))
                    .relative()
                    .size_full()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .when(!horizontal, |view| view.flex_col())
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                entity.update(cx, |this, _| {
                                    this.split_bounds.insert(bounds_id.clone(), bounds);
                                })
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    );
                for (index, child) in children.iter().enumerate() {
                    let ratio = sizes
                        .get(index)
                        .copied()
                        .filter(|n| n.is_finite() && *n > 0.)
                        .unwrap_or(1.);
                    split = split.child(
                        div()
                            .flex_basis(px(0.))
                            .flex_grow(ratio as f32)
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(self.render_node(child, cx)),
                    );
                    if index + 1 < children.len() {
                        let node = id.to_string();
                        let axis = axis.clone();
                        let sizes = sizes.clone();
                        split = split.child(
                            div()
                                .id(SharedString::from(format!("divider-{id}-{index}")))
                                .flex_none()
                                .bg(rgb(0x0d141d))
                                .when(horizontal, |view| {
                                    view.w(px(5.)).h_full().cursor(CursorStyle::ResizeColumn)
                                })
                                .when(!horizontal, |view| {
                                    view.h(px(5.)).w_full().cursor(CursorStyle::ResizeRow)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        this.begin_resize(
                                            node.clone(),
                                            axis.clone(),
                                            index,
                                            sizes.clone(),
                                            event.position,
                                        );
                                        cx.stop_propagation();
                                    }),
                                ),
                        );
                    }
                }
                split.into_any_element()
            }
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty_screen = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::snapshot)
            .is_some_and(|screen| screen.panes.is_empty());
        let mut view = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x10151c))
            .text_color(rgb(0xd9e2ef))
            .text_size(px(13.))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() && event.dragging() {
                    this.drag_resize(event.position, false, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.drag_resize(event.position, true, cx);
                    this.drag = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.drag_resize(event.position, true, cx);
                    this.drag = None;
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(42.))
                    .px_3()
                    .flex_none()
                    .bg(rgb(0x18232f))
                    .child(div().font_weight(FontWeight::BOLD).child("SPLITTY"))
                    .child(div().text_color(rgb(0x91a8bc)).child("Local / Screen 1"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("split-horizontal")
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(0x293b4d))
                            .child("Split right")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.split(Axis::Horizontal, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("split-vertical")
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(0x293b4d))
                            .child("Split below")
                            .on_click(cx.listener(|this, _, _, cx| this.split(Axis::Vertical, cx))),
                    )
                    .child(
                        div()
                            .id("new-tab")
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(0x293b4d))
                            .child(if empty_screen {
                                "New terminal"
                            } else {
                                "New tab"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.tab(cx))),
                    ),
            );
        if let Some(model) = &self.model {
            if let Some(screen) = model.snapshot() {
                if screen.panes.is_empty() {
                    view = view.child(
                        div().flex_1().flex().items_center().justify_center().child(
                            div()
                                .id("new-terminal-empty")
                                .px_4()
                                .py_2()
                                .cursor_pointer()
                                .bg(rgb(0x293b4d))
                                .child("New terminal")
                                .on_click(cx.listener(|this, _, _, cx| this.tab(cx))),
                        ),
                    );
                } else {
                    view = view.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .p_1()
                            .child(self.render_node(&screen.root, cx)),
                    );
                }
            } else {
                view = view.child(div().flex_1().p_4().child("Starting local terminal…"));
            }
        } else {
            view = view.child(div().flex_1().p_4().child("Connecting…"));
        }
        view.child(
            div()
                .h(px(25.))
                .px_3()
                .flex_none()
                .text_size(px(11.))
                .text_color(rgb(0x8ea3b7))
                .child(if self.status.is_empty() {
                    format!(
                        "{} terminals · Ctrl+Shift+C/V copy/paste · native preview",
                        self.panes.len()
                    )
                } else {
                    self.status.clone()
                }),
        )
    }
}
fn visible_panes(model: &LocalWorkspace, node: &LayoutNode, out: &mut Vec<String>) {
    match node {
        LayoutNode::Slot { .. } => {
            if let Some(pane) = model.selected_pane(node) {
                out.push(pane.to_owned());
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                visible_panes(model, child, out);
            }
        }
    }
}
fn resized_pair(sizes: &[f64], index: usize, delta: f64) -> Option<Vec<f64>> {
    if index + 1 >= sizes.len()
        || !delta.is_finite()
        || sizes.iter().any(|n| !n.is_finite() || *n <= 0.)
    {
        return None;
    }
    let total = sizes[index] + sizes[index + 1];
    let minimum = (total * 0.1).min(0.05);
    let left = (sizes[index] + delta).clamp(minimum, total - minimum);
    let mut next = sizes.to_vec();
    next[index] = left;
    next[index + 1] = total - left;
    Some(next)
}
#[cfg(test)]
mod tests {
    use super::{resized_pair, terminal_commands, Command};
    use crate::terminal;

    #[test]
    fn terminal_input_has_one_shared_admission_boundary_and_keeps_atomic_payloads() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        let first = terminal_commands(sender.clone(), "pane-a".into(), "session-a".into());
        let second = terminal_commands(sender, "pane-b".into(), "session-b".into());
        let input = |generation, text: &str| terminal::Command::Input {
            generation,
            text: text.to_owned(),
        };
        let paste = "\x1b[200~one\ntwo\x1b[201~";
        first(input(0, paste)).expect("accepted atomic paste");
        second(input(0, "next")).expect("second pane uses the same queue");
        assert!(first(input(0, "rejected")).is_err());
        assert!(second(terminal::Command::Resize(120, 40)).is_err());
        match receiver.try_recv().expect("accepted paste remains queued") {
            Command::Input {
                pane,
                session,
                text,
            } => {
                assert_eq!(
                    (pane.as_str(), session.as_str(), text.as_str()),
                    ("pane-a", "session-a", paste)
                );
            }
            _ => panic!("expected input"),
        }
        // Reject stale generations before they can consume the newly free slot.
        assert!(first(input(1, "stale")).is_err());
        first(input(0, "after drain")).expect("capacity returns only after worker drain");
        match receiver.try_recv().expect("other pane remains in order") {
            Command::Input {
                pane,
                session,
                text,
            } => {
                assert_eq!(
                    (pane.as_str(), session.as_str(), text.as_str()),
                    ("pane-b", "session-b", "next")
                );
            }
            _ => panic!("expected input"),
        }
        match receiver.try_recv().expect("newly accepted input") {
            Command::Input { text, .. } => assert_eq!(text, "after drain"),
            _ => panic!("expected input"),
        }
        assert!(receiver.try_recv().is_err());
        drop(receiver);
        assert!(first(input(0, "closed")).is_err());
    }

    #[test]
    fn divider_keeps_other_siblings_and_adjacent_total() {
        let result = resized_pair(&[0.2, 0.3, 0.5], 1, 0.1).unwrap();
        assert_eq!(result[0], 0.2);
        assert!((result[1] - 0.4).abs() < 1e-9);
        assert!((result.iter().sum::<f64>() - 1.).abs() < 1e-9);
        let edge = resized_pair(&[0.5, 0.5], 0, 10.).unwrap();
        assert_eq!(edge, vec![0.95, 0.050000000000000044]);
        assert!(resized_pair(&[0.5, 0.5], 1, 0.).is_none());
        assert!(resized_pair(&[0.5, 0.5], 0, f64::NAN).is_none());
    }
}
