//! Native projection of one authoritative local screen; no independent dock tree.
use crate::terminal::{self, Terminal};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{prelude::*, *};
use splitty_client_core::native_workspace::{Command, Update, Worker};
use splitty_client_core::workspace::{
    Axis, ClosePlan, CloseTarget, LayoutNode, LocalWorkspace, MovePlacement, PaneDrag, ScreenModel,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    NewTab,
    SplitRight,
    SplitDown,
    ClosePane,
    Focus(Direction),
}

fn shortcut(key: &Keystroke, is_mac: bool) -> Option<Shortcut> {
    let mods = key.modifiers;
    if mods.control || mods.function || mods.platform != is_mac || mods.alt == is_mac {
        return None;
    }
    match (key.key.to_ascii_lowercase().as_str(), mods.shift) {
        ("t", false) => Some(Shortcut::NewTab),
        ("d", false) => Some(Shortcut::SplitRight),
        ("d", true) => Some(Shortcut::SplitDown),
        ("w", false) => Some(Shortcut::ClosePane),
        ("left", true) => Some(Shortcut::Focus(Direction::Left)),
        ("right", true) => Some(Shortcut::Focus(Direction::Right)),
        ("up", true) => Some(Shortcut::Focus(Direction::Up)),
        ("down", true) => Some(Shortcut::Focus(Direction::Down)),
        _ => None,
    }
}

struct VisibleSlot {
    id: String,
    pane: String,
    bounds: Bounds<Pixels>,
}

fn nearest_slot<'a>(
    slots: &'a [VisibleSlot],
    current: &str,
    direction: Direction,
) -> Option<&'a VisibleSlot> {
    let source = slots
        .iter()
        .find(|slot| slot.id == current)?
        .bounds
        .center();
    let mut best: Option<(f32, &VisibleSlot)> = None;
    for slot in slots.iter().filter(|slot| slot.id != current) {
        let center = slot.bounds.center();
        let dx = f32::from(center.x - source.x);
        let dy = f32::from(center.y - source.y);
        let matching = match direction {
            Direction::Right => dx > 0. && dy.abs() <= dx,
            Direction::Left => dx < 0. && dy.abs() <= -dx,
            Direction::Down => dy > 0. && dx.abs() <= dy,
            Direction::Up => dy < 0. && dx.abs() <= -dy,
        };
        if matching {
            let distance = dx * dx + dy * dy;
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, slot));
            }
        }
    }
    best.map(|(_, slot)| slot)
}

#[derive(Clone)]
struct TabDrag {
    origin: PaneDrag,
    owner: EntityId,
}
struct DragPreview;
impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .bg(rgb(0x293b4d))
            .text_color(rgb(0xe8eff6))
            .child("Move terminal")
    }
}

fn drop_zone(bounds: Bounds<Pixels>, point: Point<Pixels>) -> Option<MovePlacement> {
    if !bounds.contains(&point) || bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return None;
    }
    let x = (point.x - bounds.left()) / bounds.size.width;
    let y = (point.y - bounds.top()) / bounds.size.height;
    let edges = [
        (x, MovePlacement::Left),
        (1. - x, MovePlacement::Right),
        (y, MovePlacement::Above),
        (1. - y, MovePlacement::Below),
    ];
    edges
        .into_iter()
        .filter(|(distance, _)| *distance < 0.22)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, placement)| placement)
        .or(Some(MovePlacement::Tab))
}

fn drop_highlight(bounds: Bounds<Pixels>, placement: MovePlacement) -> Bounds<Pixels> {
    let mut result = Bounds {
        origin: point(px(0.), px(0.)),
        size: bounds.size,
    };
    match placement {
        MovePlacement::Left => result.size.width /= 2.,
        MovePlacement::Right => {
            result.size.width /= 2.;
            result.origin.x = result.size.width;
        }
        MovePlacement::Above => result.size.height /= 2.,
        MovePlacement::Below => {
            result.size.height /= 2.;
            result.origin.y = result.size.height;
        }
        MovePlacement::Tab => {}
    }
    result
}

// The worker queue is the only admission boundary. A successful terminal send
// already owns one of its bounded slots, with the pane/session route captured.
fn terminal_commands(
    sender: mpsc::Sender<Command>,
    pane: String,
    session: String,
    pending_resize: Arc<Mutex<Option<(u16, u16)>>>,
) -> impl Fn(terminal::Command) -> Result<(), String> {
    move |command| {
        let resize = match &command {
            terminal::Command::Resize(cols, rows) => Some((*cols, *rows)),
            _ => None,
        };
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
        match sender.try_send(command) {
            Ok(()) => {
                if resize.is_some() {
                    if let Ok(mut pending) = pending_resize.lock() {
                        *pending = None;
                    }
                }
                Ok(())
            }
            Err(error) => {
                if let Some(size) = resize {
                    if let Ok(mut pending) = pending_resize.lock() {
                        *pending = Some(size);
                    }
                }
                Err(error.to_string())
            }
        }
    }
}

fn retry_pending_resize(
    sender: &mpsc::Sender<Command>,
    pane: &str,
    session: &str,
    pending: &Arc<Mutex<Option<(u16, u16)>>>,
) -> Result<(), String> {
    let size = pending.lock().map_err(|error| error.to_string())?.take();
    let Some((cols, rows)) = size else {
        return Ok(());
    };
    if let Err(error) = sender.try_send(Command::ResizePane {
        pane: pane.to_owned(),
        session: session.to_owned(),
        cols,
        rows,
    }) {
        let mut current = pending.lock().map_err(|error| error.to_string())?;
        if current.is_none() {
            *current = Some((cols, rows));
        }
        return Err(error.to_string());
    }
    Ok(())
}

struct PaneView {
    session: String,
    terminal: Entity<Terminal>,
    pending_resize: Arc<Mutex<Option<(u16, u16)>>>,
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
struct RenameEditor {
    screen: String,
    connection: String,
    input: Entity<InputState>,
}
struct PendingClose {
    plan: ClosePlan,
    prompting: bool,
}
impl PendingClose {
    fn is_prompt_for(&self, request_id: u64) -> bool {
        self.prompting && self.plan.request_id == request_id
    }
    fn resolve_prompt(
        &self,
        model: Option<&LocalWorkspace>,
        confirmed: bool,
    ) -> Result<Option<ClosePlan>, String> {
        if !self.prompting {
            return Err("Close prompt is no longer pending".into());
        }
        if !confirmed {
            return Ok(None);
        }
        match model {
            Some(model) => model
                .validate_close(&self.plan, true)
                .map_err(|error| error.to_string())?,
            None if self.plan == ClosePlan::startup_window(self.plan.request_id) => {}
            None => return Err("Close target is gone".into()),
        }
        Ok(Some(self.plan.clone()))
    }
    fn matches_window_approval(&self, update: &Update) -> bool {
        matches!(update, Update::WindowCloseApproved { request_id, connection_id }
            if !self.prompting
                && self.plan.request_id == *request_id
                && self.plan.connection_id == *connection_id
                && self.plan.target == CloseTarget::Window)
    }
}
fn admit_close(
    commands: &mpsc::Sender<Command>,
    plan: ClosePlan,
    confirmed: bool,
) -> Result<PendingClose, String> {
    commands
        .try_send(Command::RequestClose {
            plan: plan.clone(),
            confirmed,
        })
        .map_err(|error| format!("Close not sent: {error}"))?;
    Ok(PendingClose {
        plan,
        prompting: false,
    })
}
pub struct Workspace {
    model: Option<LocalWorkspace>,
    panes: HashMap<String, PaneView>,
    updates: mpsc::Receiver<Update>,
    commands: mpsc::Sender<Command>,
    stopping: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    split_bounds: HashMap<String, Bounds<Pixels>>,
    slot_bounds: HashMap<String, Bounds<Pixels>>,
    drop_target: Option<(String, MovePlacement)>,
    drag: Option<ResizeDrag>,
    visible: Vec<String>,
    visible_screen: Option<String>,
    visible_pending: bool,
    status: String,
    rename: Option<RenameEditor>,
    rename_subscription: Option<Subscription>,
    pending_close: Option<PendingClose>,
    next_close_id: u64,
    _poll: Task<()>,
    started: Instant,
    smoke: bool,
    smoke_stage: u8,
    smoke_first: Option<String>,
    smoke_tab: Option<String>,
    smoke_screen: Option<String>,
    smoke_second_screen: Option<String>,
    smoke_initial: Option<Entity<Terminal>>,
    smoke_markers: bool,
    smoke_hidden_output: bool,
    smoke_editor_reconciles: u64,
    smoke_editor_clean: Arc<AtomicBool>,
    smoke_terminal_inputs: Option<Arc<AtomicU64>>,
    smoke_last_input_pane: Option<Arc<Mutex<Option<String>>>>,
    smoke_shortcut_checks: Arc<AtomicU64>,
    smoke_shortcuts_clean: Arc<AtomicBool>,
    smoke_normal_input: Arc<AtomicBool>,
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
        window.on_window_should_close(cx, move |window, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.request_close(CloseTarget::Window, window, cx);
            });
            false
        });
        (
            Self {
                model: None,
                panes: HashMap::new(),
                updates,
                commands,
                stopping: worker.stop.clone(),
                cleanup_complete: worker.cleanup_complete.clone(),
                split_bounds: HashMap::new(),
                slot_bounds: HashMap::new(),
                drop_target: None,
                drag: None,
                visible: Vec::new(),
                visible_screen: None,
                visible_pending: false,
                status: "Starting local workspace…".into(),
                rename: None,
                rename_subscription: None,
                pending_close: None,
                next_close_id: 0,
                _poll: poll,
                started: Instant::now(),
                smoke,
                smoke_stage: 0,
                smoke_first: None,
                smoke_tab: None,
                smoke_screen: None,
                smoke_second_screen: None,
                smoke_initial: None,
                smoke_markers: false,
                smoke_hidden_output: false,
                smoke_editor_reconciles: 0,
                smoke_editor_clean: Arc::new(AtomicBool::new(false)),
                smoke_terminal_inputs: smoke.then(|| Arc::new(AtomicU64::new(0))),
                smoke_last_input_pane: smoke.then(|| Arc::new(Mutex::new(None))),
                smoke_shortcut_checks: Arc::new(AtomicU64::new(0)),
                smoke_shortcuts_clean: Arc::new(AtomicBool::new(true)),
                smoke_normal_input: Arc::new(AtomicBool::new(false)),
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
    fn create_screen(&mut self, cx: &mut Context<Self>) {
        let Some(model) = &self.model else {
            return;
        };
        if model.has_pending_create() {
            return;
        }
        let used: HashSet<_> = model
            .screens()
            .iter()
            .filter_map(|screen| screen.name.as_deref())
            .collect();
        let name = (1..=model.screens().len() + 1)
            .map(|number| format!("Screen {number}"))
            .find(|name| !used.contains(name.as_str()))
            .unwrap_or_else(|| "Screen".into());
        self.send(Command::CreateScreen(name), cx);
    }
    fn switch_screen(&mut self, screen: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.is_some() {
            self.finish_rename(true, window, cx);
        }
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.screen(screen).is_some())
        {
            self.send(Command::SwitchScreen(screen.to_owned()), cx);
        }
    }
    fn start_rename(&mut self, screen: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((name, connection)) = self.model.as_ref().and_then(|model| {
            model
                .screens()
                .iter()
                .find(|info| info.id.as_str() == screen)
                .map(|info| {
                    (
                        info.name.as_deref().unwrap_or("Screen").to_owned(),
                        model.connection_id().to_string(),
                    )
                })
        }) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        self.rename_subscription = Some(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.finish_rename(true, window, cx);
                }
                _ => {}
            },
        ));
        self.rename = Some(RenameEditor {
            screen: screen.to_owned(),
            connection,
            input: input.clone(),
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        cx.notify();
    }
    fn finish_rename(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.rename.take() else {
            return;
        };
        self.rename_subscription = None;
        if save {
            let name = editor.input.read(cx).value().trim().to_owned();
            if !name.is_empty()
                && self.model.as_ref().is_some_and(|model| {
                    model.connection_id().to_string() == editor.connection
                        && model.screen(&editor.screen).is_some()
                })
            {
                self.send(
                    Command::RenameScreen {
                        screen: editor.screen,
                        connection: editor.connection,
                        name,
                    },
                    cx,
                );
            }
        }
        self.focus_current(window, cx);
        cx.notify();
    }
    fn request_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        if target == CloseTarget::Window && self.cleanup_complete.load(Ordering::Acquire) {
            self.stop();
            cx.quit();
            return;
        }
        if self.pending_close.is_some() {
            return;
        }
        let request_id = self.next_close_id.saturating_add(1);
        let plan = match &self.model {
            Some(model) => model.assess_close(request_id, target.clone()),
            None if target == CloseTarget::Window => Ok(ClosePlan::startup_window(request_id)),
            None => return,
        };
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
                return;
            }
        };
        self.next_close_id = request_id;
        if !plan.needs_confirmation() {
            self.submit_close(plan, false, cx);
            return;
        }
        let title = match &target {
            CloseTarget::Pane { .. } => "Close terminal pane?",
            CloseTarget::Screen { .. } => "Close Screen?",
            CloseTarget::Window => "End this local workspace?",
        };
        let detail = if target == CloseTarget::Window {
            "Ends all local terminal sessions.".to_owned()
        } else if plan.unresolved_count != 0 {
            "Session state is still loading.".to_owned()
        } else {
            format!("Running sessions: {}", plan.busy_count)
        };
        self.pending_close = Some(PendingClose {
            plan: plan.clone(),
            prompting: true,
        });
        let prompt = window.prompt(
            PromptLevel::Warning,
            title,
            Some(&detail),
            &["Keep open", "Close"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = prompt.await.ok() == Some(1);
            let _ = this.update_in(cx, |this, window, cx| {
                let decision = this
                    .pending_close
                    .as_ref()
                    .filter(|pending| pending.is_prompt_for(request_id))
                    .map(|pending| pending.resolve_prompt(this.model.as_ref(), confirmed));
                let Some(decision) = decision else {
                    return;
                };
                this.pending_close = None;
                match decision {
                    Ok(Some(plan)) => this.submit_close(plan, true, cx),
                    Ok(None) => {}
                    Err(error) => this.status = error,
                }
                this.focus_current(window, cx);
                cx.notify();
            });
        })
        .detach();
    }
    fn submit_close(&mut self, plan: ClosePlan, confirmed: bool, cx: &mut Context<Self>) {
        match admit_close(&self.commands, plan, confirmed) {
            Ok(pending) => self.pending_close = Some(pending),
            Err(error) => {
                self.pending_close = None;
                self.status = error;
            }
        }
        cx.notify();
    }
    fn request_close_screen(&mut self, screen: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(
            CloseTarget::Screen {
                screen: screen.to_owned(),
            },
            window,
            cx,
        );
    }
    fn ensure_pane(&mut self, pane: &str, session: &str, cx: &mut Context<Self>) {
        if self
            .panes
            .get(pane)
            .is_some_and(|view| view.session == session)
        {
            return;
        }
        let pending_resize = Arc::new(Mutex::new(None));
        let route = terminal_commands(
            self.commands.clone(),
            pane.to_owned(),
            session.to_owned(),
            pending_resize.clone(),
        );
        let inputs = self.smoke_terminal_inputs.clone();
        let last_pane = self.smoke_last_input_pane.clone();
        let input_pane = pane.to_owned();
        let commands = move |command| {
            if matches!(&command, terminal::Command::Input { .. }) {
                if let Some(inputs) = &inputs {
                    inputs.fetch_add(1, Ordering::Relaxed);
                }
                if let Some(last_pane) = &last_pane {
                    if let Ok(mut last) = last_pane.lock() {
                        *last = Some(input_pane.clone());
                    }
                }
            }
            route(command)
        };
        let terminal = cx.new(|cx| Terminal::new(commands, cx));
        self.panes.insert(
            pane.to_owned(),
            PaneView {
                session: session.to_owned(),
                terminal,
                pending_resize,
                marker: false,
                tail: String::new(),
            },
        );
    }
    fn reconcile(&mut self, model: LocalWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.is_some() {
            self.smoke_editor_reconciles += u64::from(self.smoke);
        }
        let old_screen = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::shown_screen_id)
            .map(str::to_owned);
        let old_focus = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::focused_pane)
            .map(str::to_owned);
        let old_entity = old_focus
            .as_deref()
            .and_then(|pane| self.panes.get(pane))
            .map(|view| view.terminal.entity_id());
        if let Some(screen) = model.snapshot() {
            prune_layout_bounds(&screen.root, &mut self.split_bounds, &mut self.slot_bounds);
        } else {
            self.split_bounds.clear();
            self.slot_bounds.clear();
        }
        self.panes
            .retain(|pane, view| model.session_for(pane) == Some(view.session.as_str()));
        for route in model.routes() {
            self.ensure_pane(&route.pane_id, &route.session_id, cx);
        }
        if model.snapshot().is_some() {
            self.status.clear();
        }
        let new_screen = model.shown_screen_id().map(str::to_owned);
        let new_focus = model.focused_pane().map(str::to_owned);
        let new_entity = new_focus
            .as_deref()
            .and_then(|pane| self.panes.get(pane))
            .map(|view| view.terminal.entity_id());
        let editor_cancelled = self.rename.as_ref().is_some_and(|editor| {
            model.connection_id().to_string() != editor.connection
                || model.screen(&editor.screen).is_none()
        });
        if editor_cancelled {
            self.rename = None;
            self.rename_subscription = None;
        }
        if old_screen != new_screen {
            self.drag = None;
        }
        self.drop_target = None;
        self.model = Some(model);
        self.sync_visible(cx);
        if self.rename.is_none()
            && !self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
            && (editor_cancelled
                || old_screen != new_screen
                || old_focus != new_focus
                || old_entity != new_entity)
        {
            self.focus_current(window, cx);
        }
        cx.notify();
    }
    fn sync_visible(&mut self, cx: &mut Context<Self>) {
        let mut visible = Vec::new();
        if let Some(model) = &self.model {
            if let Some(screen) = model.snapshot() {
                if let Some(shown) = model.shown() {
                    visible_panes(shown, &screen.root, &mut visible);
                }
            }
        }
        visible.sort();
        let screen = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::shown_screen_id)
            .map(str::to_owned);
        if visible != self.visible || screen != self.visible_screen {
            self.visible = visible;
            self.visible_screen = screen;
            self.visible_pending = true;
            for pane in &self.visible {
                if let Some(view) = self.panes.get(pane) {
                    view.terminal
                        .update(cx, |terminal, _| terminal.report_size());
                }
            }
        }
        self.flush_visible(cx);
    }
    fn flush_visible(&mut self, cx: &mut Context<Self>) {
        if self.visible_pending {
            match self.commands.try_send(Command::Visible {
                screen: self.visible_screen.clone(),
                panes: self.visible.clone(),
            }) {
                Ok(()) => self.visible_pending = false,
                Err(error) => {
                    self.status = format!("Visibility pending: {error}");
                    cx.notify();
                }
            }
        }
    }
    fn flush_resizes(&mut self, cx: &mut Context<Self>) {
        for pane in &self.visible {
            let Some(view) = self.panes.get(pane) else {
                continue;
            };
            if let Err(error) =
                retry_pending_resize(&self.commands, pane, &view.session, &view.pending_resize)
            {
                let status = format!("Resize pending: {error}");
                if self.status != status {
                    self.status = status;
                    cx.notify();
                }
            }
        }
    }
    fn focus_current(&self, window: &mut Window, cx: &mut App) {
        if self.rename.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
        {
            return;
        }
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
    fn focus_neighbor(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(screen) = model.snapshot() else {
            return;
        };
        let Some(focused) = model.focused_pane() else {
            return;
        };
        let mut slots = Vec::new();
        let Some(shown) = model.shown() else {
            return;
        };
        visible_slots(shown, &screen.root, &self.slot_bounds, &mut slots);
        let Some(current) = slots.iter().find(|slot| slot.pane == focused) else {
            return;
        };
        if let Some(next) = nearest_slot(&slots, &current.id, direction) {
            let pane = next.pane.clone();
            self.focus(&pane, cx);
            self.focus_current(window, cx);
        }
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.is_some() {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                self.finish_rename(false, window, cx);
            }
            return;
        }
        if self
            .pending_close
            .as_ref()
            .is_some_and(|pending| pending.prompting)
        {
            // A native confirmation owns keyboard input until it settles.
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" && cx.has_active_drag() {
            cx.stop_active_drag(window);
            self.drop_target = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some(action) = shortcut(&event.keystroke, cx.platform_info().is_mac()) else {
            return;
        };
        cx.stop_propagation();
        match action {
            Shortcut::NewTab => self.tab(cx),
            Shortcut::SplitRight => self.split(Axis::Horizontal, cx),
            Shortcut::SplitDown => self.split(Axis::Vertical, cx),
            Shortcut::ClosePane => {
                if let Some(pane) = self
                    .model
                    .as_ref()
                    .and_then(LocalWorkspace::focused_pane)
                    .map(str::to_owned)
                {
                    if let Some(screen) = self
                        .model
                        .as_ref()
                        .and_then(LocalWorkspace::shown_screen_id)
                    {
                        self.request_close(
                            CloseTarget::Pane {
                                screen: screen.to_owned(),
                                pane,
                            },
                            window,
                            cx,
                        );
                    }
                }
            }
            Shortcut::Focus(direction) => self.focus_neighbor(direction, window, cx),
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
            if let Some(model) = &self.model {
                if let Some(screen) = model.shown_screen_id() {
                    self.send(
                        Command::NewTerminal {
                            screen: screen.to_owned(),
                            connection: model.connection_id().to_string(),
                        },
                        cx,
                    );
                }
            }
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
                Update::CloseSubmitted {
                    request_id,
                    connection_id,
                } => {
                    if !self.pending_close.as_ref().is_some_and(|pending| {
                        pending.plan.request_id == request_id
                            && pending.plan.connection_id == connection_id
                    }) {
                        self.status = "Close submission belonged to an earlier request".into();
                    }
                }
                Update::CloseRejected { request_id, reason } => {
                    if self
                        .pending_close
                        .as_ref()
                        .is_some_and(|pending| pending.plan.request_id == request_id)
                    {
                        self.pending_close = None;
                        self.status = reason;
                        self.focus_current(window, cx);
                        cx.notify();
                    }
                }
                Update::CloseSettled { request_id } => {
                    if self
                        .pending_close
                        .as_ref()
                        .is_some_and(|pending| pending.plan.request_id == request_id)
                    {
                        self.pending_close = None;
                        self.focus_current(window, cx);
                        cx.notify();
                    }
                }
                update @ Update::WindowCloseApproved { .. } => {
                    if self
                        .pending_close
                        .as_ref()
                        .is_some_and(|pending| pending.matches_window_approval(&update))
                    {
                        self.stop();
                        cx.quit();
                    }
                }
                Update::Attached(session) => {
                    for (pane, view) in self
                        .panes
                        .iter()
                        .filter(|(_, view)| view.session == session)
                    {
                        let visible = self.visible.contains(pane);
                        view.terminal.update(cx, |terminal, cx| {
                            if terminal.engine_ready() {
                                terminal.status = "Attached".into();
                            }
                            if visible {
                                terminal.report_size();
                            }
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
        self.flush_visible(cx);
        self.flush_resizes(cx);
        if self.updates.is_closed() && self.cleanup_complete.load(Ordering::Acquire) {
            if let Some(pending) = self.pending_close.take() {
                if pending.plan.target == CloseTarget::Window {
                    self.stop();
                    cx.quit();
                } else {
                    self.status = "Local service ended after close cleanup".into();
                    self.focus_current(window, cx);
                    cx.notify();
                }
            }
        }
    }
    fn smoke_key(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        mac: &'static str,
        other: &'static str,
        expected_input_pane: Option<String>,
    ) {
        let chord = if cx.platform_info().is_mac() {
            mac
        } else {
            other
        };
        let key = Keystroke::parse(chord).expect("fixed smoke keystroke");
        let inputs = self
            .smoke_terminal_inputs
            .as_ref()
            .expect("smoke input counter")
            .clone();
        let checks = self.smoke_shortcut_checks.clone();
        let clean = self.smoke_shortcuts_clean.clone();
        let normal = self.smoke_normal_input.clone();
        let last_input = self
            .smoke_last_input_pane
            .as_ref()
            .expect("smoke last input pane")
            .clone();
        // Dispatch after this Workspace update returns. The capture callback
        // updates the same entity, so synchronous dispatch would reborrow it.
        window.defer(cx, move |window, cx| {
            let before = inputs.load(Ordering::Relaxed);
            let handled = window.dispatch_keystroke(key, cx);
            let after = inputs.load(Ordering::Relaxed);
            if let Some(expected) = expected_input_pane {
                let routed_to_focused = last_input
                    .lock()
                    .is_ok_and(|last| last.as_deref() == Some(expected.as_str()));
                normal.store(
                    handled && after > before && routed_to_focused,
                    Ordering::Release,
                );
            } else if handled && after == before {
                checks.fetch_add(1, Ordering::Release);
            } else {
                clean.store(false, Ordering::Release);
            }
        });
    }
    fn smoke_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.started.elapsed() > Duration::from_secs(40) {
            self.finish_smoke(false, cx);
            return;
        }
        let Some(model) = &self.model else {
            return;
        };
        if self.smoke_stage == 17 && model.catalog_initialized() && model.screens().is_empty() {
            if !self.panes.is_empty() || !self.visible.is_empty() {
                self.finish_smoke(false, cx);
                return;
            }
            self.create_screen(cx);
            self.smoke_stage = 18;
            return;
        }
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
                self.smoke_screen = model.shown_screen_id().map(str::to_owned);
                self.smoke_initial = self.panes.get(&first).map(|view| view.terminal.clone());
                self.smoke_first = Some(first.clone());
                self.focus_current(window, cx);
                self.smoke_key(window, cx, "cmd-d", "alt-d", None);
                self.smoke_stage = 1;
            }
            1 if count == 2 && self.smoke_shortcut_checks.load(Ordering::Acquire) >= 1 => {
                self.smoke_key(window, cx, "cmd-shift-d", "alt-shift-d", None);
                self.smoke_stage = 2;
            }
            2 if count == 3 && self.smoke_shortcut_checks.load(Ordering::Acquire) >= 2 => {
                self.smoke_key(window, cx, "cmd-t", "alt-t", None);
                self.smoke_stage = 3;
            }
            3 if count == 4 && self.smoke_shortcut_checks.load(Ordering::Acquire) >= 3 => {
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
                if let Some(first) = self.smoke_first.clone() {
                    self.select(&first, window, cx);
                }
                self.smoke_key(window, cx, "cmd-shift-right", "alt-shift-right", None);
                self.smoke_stage = 8;
            }
            8 if self.smoke_shortcut_checks.load(Ordering::Acquire) >= 4
                && model.focused_pane() != self.smoke_first.as_deref() =>
            {
                self.smoke_key(
                    window,
                    cx,
                    "ctrl-c",
                    "ctrl-c",
                    model.focused_pane().map(str::to_owned),
                );
                self.smoke_stage = 9;
            }
            9 if self.smoke_normal_input.load(Ordering::Acquire) => {
                if let LayoutNode::Split { id, children, .. } = &screen.root {
                    if children.len() == 2 {
                        self.send(
                            Command::ResizeSplit {
                                node: id.to_string(),
                                sizes: vec![0.65, 0.35],
                            },
                            cx,
                        );
                        self.smoke_stage = 5;
                    }
                }
            }
            5 if matches!(&screen.root,LayoutNode::Split{sizes,..} if (sizes[0]-0.65).abs()<0.001) =>
            {
                if let Some(tab) = self.smoke_tab.clone() {
                    self.select(&tab, window, cx);
                    self.smoke_key(window, cx, "cmd-w", "alt-w", None);
                }
                self.smoke_stage = 6;
            }
            6 if count == 3
                && self.smoke_shortcut_checks.load(Ordering::Acquire) >= 5
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
                if !stable
                    || !self.smoke_markers
                    || !self.smoke_shortcuts_clean.load(Ordering::Acquire)
                    || !self.smoke_normal_input.load(Ordering::Acquire)
                {
                    self.finish_smoke(false, cx);
                    return;
                }
                self.send(Command::CreateScreen("Native Smoke 2".into()), cx);
                self.smoke_stage = 10;
            }
            10 if model.screens().len() == 2
                && model.shown_screen_id() != self.smoke_screen.as_deref()
                && count == 1 =>
            {
                self.smoke_second_screen = model.shown_screen_id().map(str::to_owned);
                let Some(first) = self.smoke_first.as_ref() else {
                    return;
                };
                let Some(view) = self.panes.get(first) else {
                    return;
                };
                if self
                    .smoke_initial
                    .as_ref()
                    .is_none_or(|old| old.entity_id() != view.terminal.entity_id())
                {
                    self.finish_smoke(false, cx);
                    return;
                }
                self.send(
                    Command::Input {
                        pane: first.clone(),
                        session: view.session.clone(),
                        text: "printf 'NATIVE_HIDDEN_SCREEN_OK\\n'\r".into(),
                    },
                    cx,
                );
                self.smoke_stage = 11;
            }
            11 if self
                .smoke_first
                .as_deref()
                .and_then(|first| self.panes.get(first))
                .is_some_and(|view| view.tail.contains("NATIVE_HIDDEN_SCREEN_OK")) =>
            {
                self.smoke_hidden_output = true;
                if let Some(old) = self.smoke_screen.clone() {
                    self.send(Command::SwitchScreen(old), cx);
                    self.smoke_stage = 12;
                }
            }
            12 if model.shown_screen_id() == self.smoke_screen.as_deref() && count == 3 => {
                let Some(old) = self.smoke_screen.clone() else {
                    return;
                };
                let focused = model.focused_pane().map(str::to_owned);
                self.start_rename(&old, window, cx);
                self.smoke_editor_reconciles = 0;
                if let Some(pane) = focused {
                    self.send(Command::Focus(pane), cx);
                }
                let Some(editor) = self.rename.as_ref() else {
                    return;
                };
                let input = editor.input.clone();
                let counter = self.smoke_terminal_inputs.as_ref().unwrap().clone();
                let clean = self.smoke_editor_clean.clone();
                let key = Keystroke::parse(if cx.platform_info().is_mac() {
                    "cmd-d"
                } else {
                    "alt-d"
                })
                .expect("fixed editor smoke keystroke");
                window.defer(cx, move |window, cx| {
                    let before = counter.load(Ordering::Relaxed);
                    window.dispatch_keystroke(key, cx);
                    let focused = input.read(cx).focus_handle(cx).is_focused(window);
                    clean.store(
                        focused && counter.load(Ordering::Relaxed) == before,
                        Ordering::Release,
                    );
                });
                self.smoke_stage = 13;
            }
            13 if count == 3
                && self.smoke_editor_reconciles > 0
                && self.smoke_editor_clean.load(Ordering::Acquire) =>
            {
                let Some(editor) = self.rename.as_ref() else {
                    return;
                };
                if !editor.input.read(cx).focus_handle(cx).is_focused(window) {
                    self.finish_smoke(false, cx);
                    return;
                }
                editor.input.update(cx, |input, cx| {
                    input.set_value("Renamed Native", window, cx)
                });
                self.finish_rename(true, window, cx);
                self.smoke_stage = 14;
            }
            14 if model.screens().iter().any(|info| {
                info.id.as_str() == self.smoke_screen.as_deref().unwrap_or("")
                    && info.name.as_deref() == Some("Renamed Native")
            }) =>
            {
                if let Some(second) = self.smoke_second_screen.clone() {
                    self.send(Command::SwitchScreen(second), cx);
                    self.smoke_stage = 15;
                }
            }
            15 if model.shown_screen_id() == self.smoke_second_screen.as_deref() => {
                if let Some(second) = self.smoke_second_screen.clone() {
                    let request_id = self.next_close_id.saturating_add(1);
                    if let Ok(plan) =
                        model.assess_close(request_id, CloseTarget::Screen { screen: second })
                    {
                        self.next_close_id = request_id;
                        self.submit_close(plan, true, cx);
                    }
                    self.smoke_stage = 16;
                }
            }
            16 if model.screens().len() == 1
                && model.shown_screen_id() == self.smoke_screen.as_deref() =>
            {
                if let Some(old) = self.smoke_screen.clone() {
                    let request_id = self.next_close_id.saturating_add(1);
                    if let Ok(plan) =
                        model.assess_close(request_id, CloseTarget::Screen { screen: old })
                    {
                        self.next_close_id = request_id;
                        self.submit_close(plan, true, cx);
                    }
                    self.smoke_stage = 17;
                }
            }
            18 if model.screens().len() == 1
                && count == 1
                && model.shown_screen_id() != self.smoke_screen.as_deref() =>
            {
                self.finish_smoke(
                    self.smoke_hidden_output && self.smoke_editor_clean.load(Ordering::Acquire),
                    cx,
                );
            }
            _ => {}
        }
    }
    fn finish_smoke(&mut self, passed: bool, cx: &mut Context<Self>) {
        self.outcome.store(passed, Ordering::Release);
        eprintln!(
            "{}",
            serde_json::json!({"probe":"splitty-local-workspace","passed":passed,"stage":self.smoke_stage,"panes":self.panes.len(),"distinctSessionMarkers":self.smoke_markers,"shortcutChecks":self.smoke_shortcut_checks.load(Ordering::Acquire),"shortcutsNoInput":self.smoke_shortcuts_clean.load(Ordering::Acquire),"normalInput":self.smoke_normal_input.load(Ordering::Acquire),"hiddenOutput":self.smoke_hidden_output,"renameEditorKeptFocus":self.smoke_editor_reconciles > 0 && self.smoke_editor_clean.load(Ordering::Acquire),"visiblePanes":self.visible.len(),"elapsedMs":self.started.elapsed().as_millis(),"status":self.status})
        );
        self.stop();
        cx.quit();
    }
    fn drag_hover(
        &mut self,
        event: &DragMoveEvent<TabDrag>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let drag = event.drag(cx);
        let target = if drag.owner == cx.entity().entity_id() {
            self.model.as_ref().and_then(|model| {
                self.slot_bounds.iter().find_map(|(slot, bounds)| {
                    let placement = drop_zone(*bounds, event.event.position)?;
                    model
                        .can_move(&drag.origin, slot, placement)
                        .then(|| (slot.clone(), placement))
                })
            })
        } else {
            None
        };
        if self.drop_target != target {
            self.drop_target = target;
            cx.notify();
        }
    }

    fn drop_pane(
        &mut self,
        drag: &TabDrag,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if drag.owner != cx.entity().entity_id() {
            return;
        }
        let placement = self
            .slot_bounds
            .get(target)
            .and_then(|bounds| drop_zone(*bounds, window.mouse_position()));
        if let (Some(model), Some(placement)) = (&self.model, placement) {
            match model.check_move(&drag.origin, target, placement) {
                Ok(true) => self.send(
                    Command::MovePane {
                        drag: drag.origin.clone(),
                        target: target.to_owned(),
                        placement,
                    },
                    cx,
                ),
                Ok(false) => {}
                Err(error) => self.status = error.to_string(),
            }
        }
        cx.notify();
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
    fn render_screen_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut screens = div()
            .id("screen-strip")
            .flex()
            .items_center()
            .gap_1()
            .max_w(px(390.))
            .min_w_0()
            .overflow_x_scroll();
        if let Some(model) = &self.model {
            for (index, info) in model.screens().iter().enumerate() {
                let id = info.id.to_string();
                let selected = model.shown_screen_id() == Some(id.as_str());
                let label = info
                    .name
                    .as_deref()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Screen {}", index + 1));
                let mut chip = div()
                    .id(SharedString::from(format!("screen-{id}")))
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(px(28.))
                    .bg(rgb(if selected { 0x30475b } else { 0x243342 }));
                if let Some(editor) = self.rename.as_ref().filter(|editor| editor.screen == id) {
                    chip = chip.child(
                        div()
                            .w(px(145.))
                            .h(px(27.))
                            .px_1()
                            .overflow_hidden()
                            .child(Input::new(&editor.input)),
                    );
                    chip = chip.child(
                        div()
                            .id(SharedString::from(format!("screen-rename-save-{id}")))
                            .px_1()
                            .cursor_pointer()
                            .child("✓")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.finish_rename(true, window, cx)
                            })),
                    );
                } else {
                    let switch = id.clone();
                    let rename = id.clone();
                    chip = chip
                        .child(
                            div()
                                .id(SharedString::from(format!("screen-select-{id}")))
                                .max_w(px(120.))
                                .overflow_hidden()
                                .px_2()
                                .cursor_pointer()
                                .child(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.switch_screen(&switch, window, cx)
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("screen-rename-{id}")))
                                .px_1()
                                .cursor_pointer()
                                .child("✎")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_rename(&rename, window, cx)
                                })),
                        );
                }
                let close = id.clone();
                screens = screens.child(
                    chip.child(
                        div()
                            .id(SharedString::from(format!("screen-close-{id}")))
                            .px_1()
                            .cursor_pointer()
                            .child("×")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_close_screen(&close, window, cx)
                            })),
                    ),
                );
            }
            if model.screens().is_empty() {
                screens = screens.child(div().text_color(rgb(0x91a8bc)).child("No screens"));
            }
        }
        let pending = self
            .model
            .as_ref()
            .is_some_and(LocalWorkspace::has_pending_create);
        screens
            .child(
                div()
                    .id("new-screen")
                    .flex_none()
                    .px_2()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if pending { 0x243342 } else { 0x30475b }))
                    .child("+ Screen")
                    .on_click(cx.listener(|this, _, _, cx| this.create_screen(cx))),
            )
            .into_any_element()
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
                                .child(
                                    div()
                                        .id(SharedString::from(format!("drag-tab-{pane}")))
                                        .max_w(px(180.))
                                        .overflow_hidden()
                                        .child(title)
                                        .when_some(
                                            self.model.as_ref().and_then(|model| {
                                                model.capture_drag(pane.as_str())
                                            }),
                                            |view, origin| {
                                                view.on_drag(
                                                    TabDrag {
                                                        origin,
                                                        owner: cx.entity().entity_id(),
                                                    },
                                                    |_, _, _, cx| cx.new(|_| DragPreview),
                                                )
                                            },
                                        ),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("close-{pane}")))
                                        .px_1()
                                        .child("×")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            if let Some(screen) = this
                                                .model
                                                .as_ref()
                                                .and_then(LocalWorkspace::shown_screen_id)
                                            {
                                                this.request_close(
                                                    CloseTarget::Pane {
                                                        screen: screen.to_owned(),
                                                        pane: close.clone(),
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            }
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
                let bounds_id = id.to_string();
                let target = id.to_string();
                let entity = cx.entity();
                let mut slot = div()
                    .id(SharedString::from(format!("slot-{id}")))
                    .relative()
                    .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                        this.drop_pane(drag, &target, window, cx)
                    }))
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .min_h_0()
                    .border_1()
                    .border_color(rgb(if focused { 0x416783 } else { 0x263544 }))
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                entity.update(cx, |this, _| {
                                    this.slot_bounds.insert(bounds_id.clone(), bounds);
                                })
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
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
                if let Some((target, placement)) = &self.drop_target {
                    if target == id.as_str() {
                        if let Some(bounds) = self.slot_bounds.get(target) {
                            let highlight = drop_highlight(*bounds, *placement);
                            slot = slot.child(
                                div()
                                    .absolute()
                                    .left(highlight.origin.x)
                                    .top(highlight.origin.y)
                                    .w(highlight.size.width)
                                    .h(highlight.size.height)
                                    .border_2()
                                    .border_color(rgb(0x76b8e8))
                                    .bg(rgba(0x4094cc44)),
                            );
                        }
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
        let modifier = if cx.platform_info().is_mac() {
            "Cmd"
        } else {
            "Alt"
        };
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
            .on_drag_move(cx.listener(Self::drag_hover))
            .text_color(rgb(0xd9e2ef))
            .text_size(px(13.))
            .capture_key_down(cx.listener(Self::key))
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
                    if this.drop_target.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.drag_resize(event.position, true, cx);
                    this.drag = None;
                    if this.drop_target.take().is_some() {
                        cx.notify();
                    }
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
                    .child(self.render_screen_controls(cx))
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
            if model.screens().is_empty() {
                view = view.child(
                    div().flex_1().flex().items_center().justify_center().child(
                        div()
                            .id("new-screen-empty")
                            .px_4()
                            .py_2()
                            .cursor_pointer()
                            .bg(rgb(0x293b4d))
                            .child("New screen")
                            .on_click(cx.listener(|this, _, _, cx| this.create_screen(cx))),
                    ),
                );
            } else if let Some(screen) = model.snapshot() {
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
                view = view.child(div().flex_1().p_4().child("Loading local Screen…"));
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
                        "{} terminals · {modifier}+T tab · {modifier}+D split · {modifier}+Shift+arrows focus · Ctrl+Shift+C/V copy/paste",
                        self.panes.len(),
                    )
                } else {
                    self.status.clone()
                }),
        )
    }
}
fn visible_panes(model: &ScreenModel, node: &LayoutNode, out: &mut Vec<String>) {
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
fn visible_slots(
    model: &ScreenModel,
    node: &LayoutNode,
    bounds: &HashMap<String, Bounds<Pixels>>,
    out: &mut Vec<VisibleSlot>,
) {
    match node {
        LayoutNode::Slot { id, .. } => {
            if let (Some(pane), Some(bounds)) =
                (model.selected_pane(node), bounds.get(id.as_str()).copied())
            {
                out.push(VisibleSlot {
                    id: id.to_string(),
                    pane: pane.to_owned(),
                    bounds,
                });
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                visible_slots(model, child, bounds, out);
            }
        }
    }
}
fn prune_layout_bounds(
    root: &LayoutNode,
    split_bounds: &mut HashMap<String, Bounds<Pixels>>,
    slot_bounds: &mut HashMap<String, Bounds<Pixels>>,
) {
    fn collect<'a>(
        node: &'a LayoutNode,
        splits: &mut HashSet<&'a str>,
        slots: &mut HashSet<&'a str>,
    ) {
        match node {
            LayoutNode::Slot { id, .. } => {
                slots.insert(id.as_str());
            }
            LayoutNode::Split { id, children, .. } => {
                splits.insert(id.as_str());
                for child in children {
                    collect(child, splits, slots);
                }
            }
        }
    }
    let mut splits = HashSet::new();
    let mut slots = HashSet::new();
    collect(root, &mut splits, &mut slots);
    split_bounds.retain(|id, _| splits.contains(id.as_str()));
    slot_bounds.retain(|id, _| slots.contains(id.as_str()));
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
    use super::{
        admit_close, nearest_slot, prune_layout_bounds, resized_pair, retry_pending_resize,
        shortcut, terminal_commands, visible_slots, Command, Direction, PendingClose, Shortcut,
        Update, VisibleSlot,
    };
    use crate::terminal;
    use gpui_kit::{point, px, size, Bounds, Keystroke, Modifiers, Pixels};
    use splitty_client_core::{
        workspace::{ClosePlan, CloseTarget, LayoutNode, LocalWorkspace, ScreenModel},
        ScreenIds,
    };
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    fn key(name: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            key: name.into(),
            modifiers,
            key_char: None,
        }
    }

    #[test]
    fn only_matching_submitted_window_close_can_approve_exit() {
        let connection = "00000000-0000-4000-8000-000000000001".parse().unwrap();
        let mut plan = ClosePlan::startup_window(9);
        plan.connection_id = Some(connection);
        let pending = PendingClose {
            plan: plan.clone(),
            prompting: false,
        };
        let approval = |request_id, connection_id| Update::WindowCloseApproved {
            request_id,
            connection_id,
        };
        assert!(pending.matches_window_approval(&approval(9, Some(connection))));
        assert!(!pending.matches_window_approval(&approval(8, Some(connection))));
        assert!(!pending.matches_window_approval(&approval(
            9,
            Some("00000000-0000-4000-8000-000000000002".parse().unwrap())
        )));
        assert!(!PendingClose {
            plan,
            prompting: true
        }
        .matches_window_approval(&approval(9, Some(connection))));
        let pane = PendingClose {
            plan: ClosePlan {
                target: CloseTarget::Pane {
                    screen: "s".into(),
                    pane: "p".into(),
                },
                ..ClosePlan::startup_window(9)
            },
            prompting: false,
        };
        assert!(!pane.matches_window_approval(&approval(9, None)));
    }

    #[test]
    fn cancelled_and_removed_prompt_targets_do_not_submit_close() {
        let startup = PendingClose {
            plan: ClosePlan::startup_window(4),
            prompting: true,
        };
        assert!(startup.is_prompt_for(4));
        assert!(
            !startup.is_prompt_for(3),
            "an old prompt cannot act on a new request"
        );
        assert!(startup.resolve_prompt(None, false).unwrap().is_none());
        assert!(startup.resolve_prompt(None, true).unwrap().is_some());
        let connection = "00000000-0000-4000-8000-000000000001".parse().unwrap();
        let model = LocalWorkspace::new(connection);
        assert!(
            startup.resolve_prompt(Some(&model), true).is_err(),
            "discovery requires a fresh decision"
        );
        let removed = PendingClose {
            plan: ClosePlan {
                connection_id: Some(connection),
                target: CloseTarget::Screen {
                    screen: "removed".into(),
                },
                ..ClosePlan::startup_window(5)
            },
            prompting: true,
        };
        assert!(removed.resolve_prompt(Some(&model), true).is_err());
        assert!(removed
            .resolve_prompt(Some(&model), false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn full_close_command_queue_leaves_no_pending_close_and_recovers() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        sender.try_send(Command::Focus("existing".into())).unwrap();
        let plan = ClosePlan::startup_window(2);
        assert!(admit_close(&sender, plan.clone(), true).is_err());
        assert!(matches!(receiver.try_recv(), Ok(Command::Focus(_))));
        let pending = admit_close(&sender, plan, true).unwrap();
        assert_eq!(pending.plan.request_id, 2);
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::RequestClose { .. })
        ));
    }

    #[test]
    fn shortcut_chords_do_not_claim_terminal_control_or_clipboard_keys() {
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        for (is_mac, base) in [(false, alt), (true, cmd)] {
            assert_eq!(shortcut(&key("t", base), is_mac), Some(Shortcut::NewTab));
            assert_eq!(
                shortcut(&key("d", base), is_mac),
                Some(Shortcut::SplitRight)
            );
            assert_eq!(shortcut(&key("w", base), is_mac), Some(Shortcut::ClosePane));
            let shifted = Modifiers {
                shift: true,
                ..base
            };
            assert_eq!(
                shortcut(&key("d", shifted), is_mac),
                Some(Shortcut::SplitDown)
            );
            for (name, direction) in [
                ("left", Direction::Left),
                ("right", Direction::Right),
                ("up", Direction::Up),
                ("down", Direction::Down),
            ] {
                assert_eq!(
                    shortcut(&key(name, shifted), is_mac),
                    Some(Shortcut::Focus(direction))
                );
            }
            assert_eq!(shortcut(&key("t", shifted), is_mac), None);
            assert_eq!(
                shortcut(
                    &key(
                        "d",
                        Modifiers {
                            control: true,
                            ..base
                        }
                    ),
                    is_mac
                ),
                None
            );
            assert_eq!(
                shortcut(
                    &key(
                        "d",
                        Modifiers {
                            function: true,
                            ..base
                        }
                    ),
                    is_mac
                ),
                None
            );
            assert_eq!(shortcut(&key("t", base), !is_mac), None);
        }
        assert_eq!(
            shortcut(
                &key(
                    "c",
                    Modifiers {
                        control: true,
                        ..Default::default()
                    }
                ),
                false
            ),
            None
        );
        for name in ["c", "v"] {
            assert_eq!(
                shortcut(
                    &key(
                        name,
                        Modifiers {
                            control: true,
                            shift: true,
                            ..Default::default()
                        }
                    ),
                    false
                ),
                None
            );
        }
    }

    fn rectangle(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(width), px(height)),
        }
    }

    #[test]
    fn visible_tab_and_nearest_split_neighbor_follow_screen_geometry() {
        let connection = Default::default();
        let mut model = ScreenModel::new(
            ScreenIds {
                screen_id: "screen".into(),
                pane_id: "a".into(),
            },
            connection,
        );
        let snapshot = serde_json::json!({
            "type":"mux.screen_snapshot",
            "screen":{"id":"screen","revision":1,
                "root":{"type":"split","id":"root","axis":"horizontal","sizes":[0.5,0.5],"children":[
                    {"type":"slot","id":"left","paneIds":["a","b"]},
                    {"type":"split","id":"right","axis":"vertical","sizes":[0.5,0.5],"children":[
                        {"type":"slot","id":"upper","paneIds":["c"]},
                        {"type":"slot","id":"lower","paneIds":["d"]}
                    ]}
                ]},
                "panes":{"a":{"id":"a"},"b":{"id":"b"},"c":{"id":"c"},"d":{"id":"d"}},
                "sessions":{}}
        });
        model
            .apply_snapshot(connection, &snapshot)
            .expect("snapshot");
        assert!(model.select_tab("b"));
        let bounds = HashMap::from([
            ("left".into(), rectangle(0., 0., 100., 200.)),
            ("upper".into(), rectangle(100., 0., 100., 100.)),
            ("lower".into(), rectangle(100., 100., 100., 100.)),
            ("stale".into(), rectangle(200., 0., 100., 100.)),
        ]);
        let mut slots = Vec::new();
        visible_slots(&model, &model.snapshot().unwrap().root, &bounds, &mut slots);
        assert_eq!(slots.len(), 3, "stale slot bounds are excluded");
        assert_eq!(slots[0].pane, "b", "window-local selected tab is visible");
        assert_eq!(
            nearest_slot(&slots, "left", Direction::Right).map(|slot| slot.pane.as_str()),
            Some("c"),
            "equal distance chooses canonical tree order"
        );
        assert_eq!(
            nearest_slot(&slots, "upper", Direction::Down).map(|slot| slot.pane.as_str()),
            Some("d")
        );
        assert!(nearest_slot(&slots, "left", Direction::Left).is_none());
        assert!(nearest_slot(&slots, "stale", Direction::Right).is_none());
        let mut missing = bounds;
        missing.remove("left");
        let mut slots = Vec::new();
        visible_slots(
            &model,
            &model.snapshot().unwrap().root,
            &missing,
            &mut slots,
        );
        assert!(nearest_slot(&slots, "left", Direction::Right).is_none());
    }

    #[test]
    fn nearest_slot_rejects_diagonal_outside_cone() {
        let slots = [
            VisibleSlot {
                id: "current".into(),
                pane: "a".into(),
                bounds: rectangle(0., 0., 100., 100.),
            },
            VisibleSlot {
                id: "diagonal".into(),
                pane: "b".into(),
                bounds: rectangle(100., 300., 100., 100.),
            },
        ];
        assert!(nearest_slot(&slots, "current", Direction::Right).is_none());
    }

    #[test]
    fn authoritative_layout_releases_closed_slot_and_replaced_split_bounds() {
        let root: LayoutNode = serde_json::from_value(serde_json::json!({
            "type":"split","id":"current-split","axis":"horizontal","sizes":[0.5,0.5],
            "children":[
                {"type":"slot","id":"left","paneIds":["a"]},
                {"type":"slot","id":"right","paneIds":["b"]}
            ]
        }))
        .expect("valid authoritative layout");
        let mut splits = HashMap::from([
            ("old-split".into(), rectangle(0., 0., 100., 100.)),
            ("current-split".into(), rectangle(0., 0., 200., 100.)),
        ]);
        let mut slots = HashMap::from([
            ("closed-slot".into(), rectangle(0., 0., 100., 100.)),
            ("left".into(), rectangle(0., 0., 100., 100.)),
            ("right".into(), rectangle(100., 0., 100., 100.)),
        ]);
        prune_layout_bounds(&root, &mut splits, &mut slots);
        assert_eq!(
            splits.keys().map(String::as_str).collect::<Vec<_>>(),
            ["current-split"]
        );
        assert_eq!(slots.len(), 2);
        assert!(slots.contains_key("left") && slots.contains_key("right"));
        assert!(!slots.contains_key("closed-slot"));
    }

    #[test]
    fn drop_zones_are_bounded_and_cover_center_and_four_edges() {
        use gpui_kit::{point, px, size, Bounds};
        use splitty_client_core::workspace::MovePlacement::*;
        let bounds = Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(100.), px(200.)),
        };
        for (x, y, expected) in [
            (60., 120., Tab),
            (12., 120., Left),
            (108., 120., Right),
            (60., 22., Above),
            (60., 218., Below),
            (11., 21., Above),
        ] {
            assert_eq!(
                super::drop_zone(bounds, point(px(x), px(y))),
                Some(expected)
            );
        }
        assert_eq!(super::drop_zone(bounds, point(px(0.), px(120.))), None);
        assert_eq!(
            super::drop_zone(Bounds::default(), point(px(0.), px(0.))),
            None
        );
        let right = super::drop_highlight(bounds, Right);
        assert_eq!(right.origin.x, px(50.));
        assert_eq!(right.size.width, px(50.));
    }

    #[test]
    fn terminal_input_has_one_shared_admission_boundary_and_keeps_atomic_payloads() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        let first_pending = Arc::new(Mutex::new(None));
        let second_pending = Arc::new(Mutex::new(None));
        let first = terminal_commands(
            sender.clone(),
            "pane-a".into(),
            "session-a".into(),
            first_pending,
        );
        let second = terminal_commands(
            sender.clone(),
            "pane-b".into(),
            "session-b".into(),
            second_pending.clone(),
        );
        let input = |generation, text: &str| terminal::Command::Input {
            generation,
            text: text.to_owned(),
        };
        let paste = "\x1b[200~one\ntwo\x1b[201~";
        first(input(0, paste)).expect("accepted atomic paste");
        second(input(0, "next")).expect("second pane uses the same queue");
        assert!(first(input(0, "rejected")).is_err());
        assert!(second(terminal::Command::Resize(120, 40)).is_err());
        assert!(second(terminal::Command::Resize(130, 41)).is_err());
        assert_eq!(
            *second_pending.lock().unwrap(),
            Some((130, 41)),
            "latest rejected size survives queue pressure"
        );
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
        retry_pending_resize(&sender, "pane-b", "session-b", &second_pending)
            .expect("resize retry admitted");
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::ResizePane {
                cols: 130,
                rows: 41,
                ..
            })
        ));
        assert_eq!(*second_pending.lock().unwrap(), None);
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
