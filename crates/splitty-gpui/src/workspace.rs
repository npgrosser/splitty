//! Native projection of one authoritative local screen; no independent dock tree.
mod floating;
mod shell_profiles;
mod templates;
mod transfer;

use crate::appearance::NativeAppearance;
use crate::terminal::{self, Terminal};
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::base::{ColorTokens, Popover, Theme, ThemeAppearance};
use gpui_kit::{prelude::*, *};
use splitty_client_core::appearance::{AppearanceSnapshot, ColorScheme};
use splitty_client_core::native_workspace::{Command, Update, Worker};
use splitty_client_core::shell_profiles::ShellProfilesSnapshot;
use splitty_client_core::workspace::{
    Axis, ClosePlan, CloseTarget, FloatingActionOutcome, LayoutNode, LocalWorkspace, MovePlacement,
    PaneActionTarget, PaneDrag, ScreenModel, ScreenTransferOutcome, ScreenTransferTarget, TabStep,
};
use splitty_client_core::zoom::{Zoom, ZoomAction, ZoomOutcome, ZoomResult};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use uuid::Uuid;

use self::floating::{FloatGesture, FloatGestureKind};
use self::shell_profiles::ProfilePicker;
use self::templates::TemplateDialog;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Tiled,
    Floating,
}

struct PendingFloat {
    connection_id: Uuid,
    request_id: Uuid,
    restore_focus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    Templates,
    NewTab,
    NewScreen,
    ScreenIndex(usize),
    ToggleMaximize,
    SplitRight,
    SplitDown,
    ClosePane,
    Focus(Direction),
    PreviousTab,
    NextTab,
}

fn chrome_rems(base_px: f32) -> Rems {
    rems(base_px / 16.)
}

fn tab_header_pixels(font_size: f32, window_scale: f32) -> Pixels {
    px((font_size + 19.).max(32.) * window_scale + 1.)
}

fn zoom_shortcut(key: &Keystroke, is_mac: bool) -> Option<ZoomAction> {
    let mods = key.modifiers;
    if mods.alt
        || mods.function
        || if is_mac {
            !mods.platform || mods.control
        } else {
            !mods.control || mods.platform
        }
    {
        return None;
    }
    match key.key.as_str() {
        "+" | "=" => Some(ZoomAction::In),
        "-" => Some(ZoomAction::Out),
        "0" => Some(ZoomAction::Reset),
        _ => None,
    }
}

fn shortcut(key: &Keystroke, is_mac: bool) -> Option<Shortcut> {
    let mods = key.modifiers;
    if mods.control
        && mods.shift
        && !mods.alt
        && !mods.platform
        && !mods.function
        && key.key == "space"
    {
        return Some(Shortcut::NewScreen);
    }
    if mods.control || mods.function || mods.platform != is_mac || mods.alt == is_mac {
        return None;
    }
    match (key.key.to_ascii_lowercase().as_str(), mods.shift) {
        ("p", false) => Some(Shortcut::Templates),
        ("t", false) => Some(Shortcut::NewTab),
        ("m", false) => Some(Shortcut::ToggleMaximize),
        ("d", false) => Some(Shortcut::SplitRight),
        ("d", true) => Some(Shortcut::SplitDown),
        ("w", false) => Some(Shortcut::ClosePane),
        ("pageup", false) => Some(Shortcut::PreviousTab),
        ("pagedown", false) => Some(Shortcut::NextTab),
        ("left", true) => Some(Shortcut::Focus(Direction::Left)),
        ("right", true) => Some(Shortcut::Focus(Direction::Right)),
        ("up", true) => Some(Shortcut::Focus(Direction::Up)),
        ("down", true) => Some(Shortcut::Focus(Direction::Down)),
        _ => None,
    }
}

// GPUI's XKB conversion clears Keystroke.modifiers.shift for punctuation.
// Window::modifiers retains the physical Shift state. Symbol -> digit is only
// known for these exact XKB layouts; an unknown layout passes through to PTY.
fn shifted_screen_index(key: &Keystroke, raw_shift: bool, layout: &str) -> Option<usize> {
    if !raw_shift {
        return None;
    }
    if let Ok(digit) = key.key.parse::<usize>() {
        if (1..=9).contains(&digit) {
            return Some(digit - 1);
        }
    }
    let symbols = match layout {
        "English (US)" => ["!", "@", "#", "$", "%", "^", "&", "*", "("],
        "German" => ["!", "\"", "§", "$", "%", "&", "/", "(", ")"],
        _ => return None,
    };
    let printed = key.key_char.as_deref().unwrap_or(&key.key);
    symbols.iter().position(|symbol| *symbol == printed)
}

fn shortcut_with_layout(
    key: &Keystroke,
    is_mac: bool,
    raw_shift: bool,
    layout: &str,
) -> Option<Shortcut> {
    let mods = key.modifiers;
    if !mods.control && !mods.function && mods.platform == is_mac && mods.alt != is_mac {
        if let Some(index) = shifted_screen_index(key, raw_shift, layout) {
            return Some(Shortcut::ScreenIndex(index));
        }
    }
    shortcut(key, is_mac)
}

struct VisibleSlot {
    id: String,
    pane: String,
    bounds: Bounds<Pixels>,
}

#[derive(Clone, Default)]
struct TabScroll {
    handle: ScrollHandle,
    last_width: Rc<Cell<Option<Pixels>>>,
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
#[derive(Clone, Debug, PartialEq)]
enum DropTarget {
    Body {
        slot: String,
        placement: MovePlacement,
    },
    Header {
        slot: String,
        before: Option<String>,
        marker_x: Pixels,
    },
}

fn slot_by_id<'a>(node: &'a LayoutNode, id: &str) -> Option<&'a LayoutNode> {
    match node {
        LayoutNode::Slot { id: slot, .. } => (slot.as_str() == id).then_some(node),
        LayoutNode::Split { children, .. } => {
            children.iter().find_map(|child| slot_by_id(child, id))
        }
    }
}

fn tab_insertion(
    viewport: Bounds<Pixels>,
    point: Point<Pixels>,
    count: usize,
    offset_x: Pixels,
    scale: f32,
    mut bounds_for: impl FnMut(usize) -> Option<Bounds<Pixels>>,
) -> Option<(usize, Pixels)> {
    if !viewport.contains(&point) || viewport.size.width <= px(4. * scale) || count == 0 {
        return None;
    }
    let marker = |x: Pixels| {
        let left = viewport.left() + px(scale);
        let right = viewport.right() - px(3. * scale);
        if x < left {
            left
        } else if x > right {
            right
        } else {
            x
        }
    };
    for index in 0..count {
        let bounds = bounds_for(index)?;
        let left = bounds.left() + offset_x;
        if point.x < left + bounds.size.width / 2. {
            return Some((index, marker(left)));
        }
    }
    let last = bounds_for(count - 1)?;
    Some((count, marker(last.right() + offset_x)))
}

fn body_drop_zone(
    slot: Bounds<Pixels>,
    header_height: Pixels,
    position: Point<Pixels>,
) -> Option<MovePlacement> {
    if !slot.contains(&position) || slot.size.height <= header_height {
        return None;
    }
    let body = Bounds::new(
        point(slot.left(), slot.top() + header_height),
        size(slot.size.width, slot.size.height - header_height),
    );
    drop_zone(body, position)
}
struct DragPreview;
impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = Theme::global(cx).tokens.colors;
        div()
            .px_3()
            .py_1()
            .bg(colors.secondary)
            .text_color(colors.secondary_foreground)
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
    route_generation: Arc<AtomicU64>,
) -> impl Fn(terminal::Command) -> Result<(), String> {
    move |command| {
        let resize = match &command {
            terminal::Command::Resize(cols, rows) => Some((*cols, *rows)),
            _ => None,
        };
        let command = match command {
            terminal::Command::Input {
                generation: 0,
                bytes,
            } => Command::Input {
                pane: pane.clone(),
                session: session.clone(),
                bytes,
            },
            terminal::Command::Input { .. } => {
                return Err("Terminal generation changed".into());
            }
            terminal::Command::Resize(cols, rows) => Command::ResizePane {
                route_generation: route_generation.load(Ordering::Acquire),
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
    route_generation: &AtomicU64,
) -> Result<(), String> {
    let size = pending.lock().map_err(|error| error.to_string())?.take();
    let Some((cols, rows)) = size else {
        return Ok(());
    };
    if let Err(error) = sender.try_send(Command::ResizePane {
        route_generation: route_generation.load(Ordering::Acquire),
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

#[derive(Clone)]
struct TransferMenu {
    source: PaneActionTarget,
    selected: usize,
    scroll: ScrollHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingTransfer {
    connection_id: Uuid,
    request_id: Uuid,
}

impl PendingTransfer {
    fn matches(self, connection_id: Uuid, request_id: Uuid) -> bool {
        self.connection_id == connection_id && self.request_id == request_id
    }
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
    focus: FocusHandle,
    panes: HashMap<String, PaneView>,
    commands: mpsc::Sender<Command>,
    stopping: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    split_bounds: HashMap<String, Bounds<Pixels>>,
    slot_bounds: HashMap<String, Bounds<Pixels>>,
    tab_scrolls: HashMap<String, TabScroll>,
    drop_target: Option<DropTarget>,
    drag: Option<ResizeDrag>,
    visible: Vec<String>,
    visible_screen: Option<String>,
    visible_owner: Option<Uuid>,
    visible_route_generation: u64,
    route_generation: Arc<AtomicU64>,
    search_owner: Option<Entity<Terminal>>,
    visible_pending: bool,
    status: String,
    appearance: NativeAppearance,
    appearance_error: Option<String>,
    appearance_generation: u64,
    pending_zoom: HashSet<Uuid>,
    zoom_feedback: Option<(String, Instant)>,
    profiles: Option<Arc<ShellProfilesSnapshot>>,
    profiles_error: Option<String>,
    profiles_generation: u64,
    profile_picker: Option<ProfilePicker>,
    template_dialog: Option<TemplateDialog>,
    pending_template_apply: Option<(Uuid, Uuid)>,
    transfer_menu: Option<TransferMenu>,
    pending_transfer: Option<PendingTransfer>,
    float_order: HashMap<String, Vec<String>>,
    float_viewport: Option<Bounds<Pixels>>,
    float_gesture: Option<FloatGesture>,
    pending_float: Option<PendingFloat>,
    desired_scheme: ColorScheme,
    sent_scheme: ColorScheme,
    scheme_pending: bool,
    _appearance_subscription: Subscription,
    _activation_subscription: Subscription,
    ui_family: String,
    mono_family: String,
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
        window.set_rem_size(px(16.));
        let (commands, receiver) = mpsc::channel(64);
        let (sender, mut updates) = mpsc::channel(64);
        let initial_scheme = Self::window_scheme(window);
        let worker = Worker::start_with_scheme(receiver, sender, initial_scheme);
        let stopping = worker.stop.clone();
        let poll = cx.spawn_in(window, async move |this, cx| {
            let maintenance = Duration::from_millis(16);
            let mut next_tick = Instant::now() + maintenance;
            let mut updates_open = true;
            let mut work_batches = 0_usize;
            let mut work_updates = 0_usize;
            loop {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                let now = Instant::now();
                let first = if now >= next_tick {
                    next_tick = now + maintenance;
                    None
                } else {
                    let timer = cx.background_executor().timer(next_tick - now);
                    if updates_open {
                        tokio::select! {
                            update = updates.recv() => {
                                if update.is_none() {
                                    updates_open = false;
                                }
                                update
                            }
                            _ = timer => {
                                next_tick = Instant::now() + maintenance;
                                None
                            }
                        }
                    } else {
                        timer.await;
                        next_tick = Instant::now() + maintenance;
                        None
                    }
                };
                let (paced, processed) = match this.update_in(cx, |this, window, cx| {
                    this.poll(first, &mut updates, window, cx)
                }) {
                    Ok(result) => result,
                    Err(_) => break,
                };
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                if paced {
                    // Keep the existing 64-update/256 KiB output pacing under
                    // sustained traffic while small batches wake immediately.
                    cx.background_executor().timer(maintenance).await;
                    work_batches = 0;
                    work_updates = 0;
                } else if processed != 0 {
                    work_batches += 1;
                    work_updates += processed;
                    if work_batches >= 4 || work_updates >= 64 {
                        // Many small ready batches must still yield to GPUI.
                        cx.background_executor()
                            .timer(Duration::from_millis(1))
                            .await;
                        work_batches = 0;
                        work_updates = 0;
                    }
                }
            }
        });
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.request_close(CloseTarget::Window, window, cx);
            });
            false
        });
        let appearance_subscription = cx.observe_window_appearance(window, |this, window, _cx| {
            this.desired_scheme = Self::window_scheme(window);
            this.scheme_pending = this.desired_scheme != this.sent_scheme;
            this.flush_scheme();
        });
        let activation_subscription = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() && this.float_gesture.take().is_some() {
                cx.notify();
            }
        });
        let typography = Theme::global(cx).tokens.typography;
        let ui_family = typography.sans.to_string();
        let mono_family = typography.mono.to_string();
        (
            Self {
                model: None,
                focus: cx.focus_handle(),
                panes: HashMap::new(),
                commands,
                stopping: worker.stop.clone(),
                cleanup_complete: worker.cleanup_complete.clone(),
                split_bounds: HashMap::new(),
                slot_bounds: HashMap::new(),
                tab_scrolls: HashMap::new(),
                drop_target: None,
                drag: None,
                visible: Vec::new(),
                visible_screen: None,
                visible_owner: None,
                visible_route_generation: 0,
                route_generation: Arc::new(AtomicU64::new(0)),
                search_owner: None,
                visible_pending: false,
                status: "Starting local workspace…".into(),
                appearance: NativeAppearance::fallback(&ui_family),
                appearance_error: None,
                appearance_generation: 0,
                pending_zoom: HashSet::new(),
                zoom_feedback: None,
                profiles: None,
                profiles_error: None,
                profiles_generation: 0,
                profile_picker: None,
                template_dialog: None,
                pending_template_apply: None,
                transfer_menu: None,
                pending_transfer: None,
                float_order: HashMap::new(),
                float_viewport: None,
                float_gesture: None,
                pending_float: None,
                desired_scheme: initial_scheme,
                sent_scheme: initial_scheme,
                scheme_pending: false,
                _appearance_subscription: appearance_subscription,
                _activation_subscription: activation_subscription,
                ui_family,
                mono_family,
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
    }
    fn window_scheme(window: &Window) -> ColorScheme {
        match window.appearance() {
            WindowAppearance::Light | WindowAppearance::VibrantLight => ColorScheme::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ColorScheme::Dark,
        }
    }
    fn flush_scheme(&mut self) {
        if !self.scheme_pending {
            return;
        }
        let Some(model) = &self.model else {
            return;
        };
        if self
            .commands
            .try_send(Command::SetColorScheme {
                connection: model.connection_id(),
                scheme: self.desired_scheme,
            })
            .is_ok()
        {
            self.sent_scheme = self.desired_scheme;
            self.scheme_pending = false;
        }
    }
    fn apply_appearance(
        &mut self,
        snapshot: &AppearanceSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.connection_id() == snapshot.connection_id())
        {
            return;
        }
        if snapshot.generation() <= self.appearance_generation {
            return;
        }
        let available_fonts = window.text_system().all_font_names();
        let next = NativeAppearance::from_snapshot(
            snapshot,
            &available_fonts,
            &self.ui_family,
            &self.mono_family,
        );
        self.apply_zoom(next.zoom, window, cx);
        let theme = Theme::global_mut(cx);
        theme.tokens.typography.sans = next.font_family.clone().into();
        match snapshot.scheme() {
            ColorScheme::Light => {
                theme.appearance = ThemeAppearance::Light;
                theme.tokens.colors = ColorTokens::light();
            }
            ColorScheme::Dark => {
                theme.appearance = ThemeAppearance::Dark;
                theme.tokens.colors = ColorTokens::dark();
            }
        }
        let mut failure = None;
        for view in self.panes.values() {
            let applied = view.terminal.update(cx, |terminal, cx| {
                terminal.apply_colors(
                    next.terminal,
                    next.selection_foreground,
                    next.selection_background,
                    cx,
                )
            });
            if failure.is_none() {
                failure = applied.err();
            }
        }
        self.appearance = next;
        self.appearance_generation = snapshot.generation();
        self.appearance_error = failure;
        cx.notify();
    }

    fn apply_zoom(&mut self, zoom: Zoom, window: &mut Window, cx: &mut Context<Self>) {
        if self.appearance.zoom == zoom {
            return;
        }
        if cx.has_active_drag() {
            cx.stop_active_drag(window);
        }
        self.drag = None;
        self.float_gesture = None;
        self.drop_target = None;
        self.split_bounds.clear();
        self.slot_bounds.clear();
        window.set_rem_size(px(16. * zoom.window_scale()));
        for view in self.panes.values() {
            view.terminal.update(cx, |terminal, cx| {
                terminal.set_zoom(zoom.terminal_scale(), cx)
            });
        }
        self.appearance.zoom = zoom;
        cx.notify();
    }

    fn submit_zoom(&mut self, action: ZoomAction, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            self.status = "Zoom is unavailable while the workspace starts".into();
            cx.notify();
            return;
        };
        let request_id = Uuid::new_v4();
        match self.commands.try_send(Command::Zoom {
            connection: model.connection_id(),
            request_id,
            action,
        }) {
            Ok(()) => {
                self.pending_zoom.insert(request_id);
            }
            Err(error) => {
                self.status = format!("Zoom request not sent: {error}");
                cx.notify();
            }
        }
    }

    fn apply_zoom_result(
        &mut self,
        result: ZoomResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.connection_id() == result.connection_id)
            || !self.pending_zoom.remove(&result.request_id)
        {
            return;
        }
        match result.outcome {
            ZoomOutcome::Confirmed(zoom) => {
                self.apply_zoom(zoom, window, cx);
                self.zoom_feedback = Some((
                    format!("Zoom {} %", (zoom.level() * 100.).round() as u32),
                    Instant::now() + Duration::from_millis(1200),
                ));
            }
            ZoomOutcome::Rejected { reason } => self.status = format!("Zoom: {reason}"),
            ZoomOutcome::Uncertain { reason } => {
                self.status = format!("Zoom outcome uncertain: {reason}")
            }
        }
        cx.notify();
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
        self.float_gesture = None;
        self.transfer_menu = None;
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
        self.transfer_menu = None;
        self.close_profile_picker(window, cx);
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
        self.float_gesture = None;
        self.transfer_menu = None;
        self.close_profile_picker(window, cx);
        self.dismiss_templates(window, cx);
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
        if let Some(previous) = self.panes.get(pane) {
            previous
                .terminal
                .update(cx, |terminal, _| terminal.cancel_pointer());
        }
        let pending_resize = Arc::new(Mutex::new(None));
        let route = terminal_commands(
            self.commands.clone(),
            pane.to_owned(),
            session.to_owned(),
            pending_resize.clone(),
            self.route_generation.clone(),
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
        let colors = self.appearance.terminal;
        let terminal_scale = self.appearance.zoom.terminal_scale();
        let selection_foreground = self.appearance.selection_foreground;
        let selection_background = self.appearance.selection_background;
        let terminal = cx.new(|cx| Terminal::new(commands, cx));
        if let Err(error) = terminal.update(cx, |terminal, cx| {
            terminal.set_search_active(false, cx);
            terminal.set_zoom(terminal_scale, cx);
            terminal.apply_colors(colors, selection_foreground, selection_background, cx)
        }) {
            self.appearance_error = Some(error);
        }
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
        let old_preview = self
            .model
            .as_ref()
            .is_some_and(|old| old.pending_template().is_some());
        let new_preview = model.pending_template().is_some();
        if self
            .model
            .as_ref()
            .is_some_and(|old| old.connection_id() != model.connection_id())
        {
            self.apply_zoom(Zoom::from_values(1.0, "window"), window, cx);
            self.appearance_generation = 0;
            self.appearance_error = None;
            self.appearance = NativeAppearance::fallback(&self.ui_family);
            self.pending_zoom.clear();
            self.zoom_feedback = None;
            self.scheme_pending = true;
            self.profiles = None;
            self.profiles_error = None;
            self.profiles_generation = 0;
            self.profile_picker = None;
            self.template_dialog = None;
            self.pending_template_apply = None;
            self.transfer_menu = None;
            self.pending_transfer = None;
            self.pending_float = None;
            self.float_order.clear();
            self.float_gesture = None;
            self.float_viewport = None;
        }
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
        if let Some(root) = model.presentation_root() {
            let roots = std::iter::once(root)
                .chain(model.presented_floats().iter().map(|entry| &entry.root));
            let slots = prune_layout_bounds(roots, &mut self.split_bounds, &mut self.slot_bounds);
            self.tab_scrolls.retain(|id, _| slots.contains(id.as_str()));
            for slot in slots {
                self.tab_scrolls.entry(slot.to_owned()).or_default();
            }
            reveal_changed_tabs(self.model.as_ref(), &model, &self.tab_scrolls);
        } else {
            self.split_bounds.clear();
            self.slot_bounds.clear();
            self.tab_scrolls.clear();
        }
        self.panes.retain(|pane, view| {
            let retained = model.session_for(pane) == Some(view.session.as_str());
            if !retained {
                view.terminal
                    .update(cx, |terminal, _| terminal.cancel_pointer());
            }
            retained
        });
        for route in model.routes() {
            self.ensure_pane(&route.pane_id, &route.session_id, cx);
        }
        let routes_changed = self.model.as_ref().is_none_or(|previous| {
            previous.connection_id() != model.connection_id()
                || previous.route_generation() != model.route_generation()
        });
        if routes_changed {
            for view in self.panes.values() {
                if let Ok(mut pending) = view.pending_resize.lock() {
                    *pending = None;
                }
                view.terminal.update(cx, |terminal, cx| {
                    terminal.invalidate_size_report(cx);
                });
            }
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
        let picker_cancelled = self
            .profile_picker
            .as_ref()
            .is_some_and(|picker| !picker.valid_for(&model));
        let transfer_cancelled = self.transfer_menu.as_ref().is_some_and(|menu| {
            model.validate_pane_action(&menu.source).is_err()
                || !model.can_transfer_pane(&menu.source)
        });
        if picker_cancelled {
            self.profile_picker = None;
        }
        if transfer_cancelled {
            self.transfer_menu = None;
        }
        if editor_cancelled {
            self.rename = None;
            self.rename_subscription = None;
        }
        if old_screen != new_screen {
            self.drag = None;
            self.float_gesture = None;
            self.float_viewport = None;
        }
        self.reconcile_float_order(&model);
        if self.float_gesture.as_ref().is_some_and(|gesture| {
            model.connection_id() != gesture.connection
                || model.shown_screen_id() != Some(gesture.screen.as_str())
                || model.validate_float(&gesture.target).is_err()
        }) {
            self.float_gesture = None;
        }
        self.drop_target = None;
        self.route_generation
            .store(model.route_generation(), Ordering::Release);
        self.model = Some(model);
        self.flush_scheme();
        self.sync_visible(cx);
        if self.rename.is_none()
            && !self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
            && (editor_cancelled
                || picker_cancelled
                || transfer_cancelled
                || old_screen != new_screen
                || (old_preview && !new_preview)
                || old_focus != new_focus
                || old_entity != new_entity)
        {
            self.focus_current(window, cx);
        }
        cx.notify();
    }
    fn sync_visible(&mut self, cx: &mut Context<Self>) {
        let mut visible = self
            .model
            .as_ref()
            .map_or_else(Vec::new, LocalWorkspace::displayed_panes);
        if self.pending_template_apply.is_some()
            || self
                .model
                .as_ref()
                .is_some_and(|model| model.pending_template().is_some())
        {
            visible.clear();
        }
        visible.sort();
        let screen = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::shown_screen_id)
            .map(str::to_owned);
        let owner = self.model.as_ref().map(LocalWorkspace::connection_id);
        let generation = self
            .model
            .as_ref()
            .map(LocalWorkspace::route_generation)
            .unwrap_or(0);
        let route_changed =
            owner != self.visible_owner || generation != self.visible_route_generation;
        if visible != self.visible || screen != self.visible_screen || route_changed {
            for pane in &self.visible {
                if !visible.contains(pane) {
                    if let Some(view) = self.panes.get(pane) {
                        view.terminal
                            .update(cx, |terminal, _| terminal.cancel_pointer());
                    }
                }
            }
            self.visible = visible;
            self.visible_screen = screen;
            self.visible_owner = owner;
            self.visible_route_generation = generation;
            self.visible_pending = true;
            for pane in &self.visible {
                if let Some(view) = self.panes.get(pane) {
                    if !route_changed {
                        view.terminal
                            .update(cx, |terminal, cx| terminal.invalidate_size_report(cx));
                    }
                }
            }
        }
        self.sync_search_owner(cx);
        self.flush_visible(cx);
    }
    fn sync_search_owner(&mut self, cx: &mut Context<Self>) {
        let next = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::focused_pane)
            .filter(|pane| self.visible.iter().any(|visible| visible.as_str() == *pane))
            .and_then(|pane| self.panes.get(pane))
            .map(|view| view.terminal.clone());
        if self.search_owner.as_ref().map(Entity::entity_id) == next.as_ref().map(Entity::entity_id)
        {
            return;
        }
        // Release the previous index before activating the next one. Pane
        // replacement can keep the same pane ID while changing its entity.
        if let Some(previous) = self.search_owner.take() {
            previous.update(cx, |terminal, cx| terminal.set_search_active(false, cx));
        }
        if let Some(next) = &next {
            next.update(cx, |terminal, cx| terminal.set_search_active(true, cx));
        }
        self.search_owner = next;
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
            if let Err(error) = retry_pending_resize(
                &self.commands,
                pane,
                &view.session,
                &view.pending_resize,
                &self.route_generation,
            ) {
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
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.transfer_menu.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
        {
            return;
        }
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.pending_template().is_some())
        {
            self.focus.focus(window, cx);
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
        } else {
            self.focus.focus(window, cx);
        }
    }
    fn select(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(model) = &mut self.model {
            model.select_tab(pane);
        }
        self.reveal_pane(pane);
        self.send(Command::Select(pane.to_owned()), cx);
        self.sync_visible(cx);
        self.focus_current(window, cx);
        cx.notify();
    }
    fn reveal_pane(&self, pane: &str) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(slot) = model.focused_slot_id() else {
            return;
        };
        let Some(LayoutNode::Slot { pane_ids, .. }) = presented_slot_by_id(model, slot) else {
            return;
        };
        if let (Some(handle), Some(index)) = (
            self.tab_scrolls.get(slot),
            pane_ids.iter().position(|id| id.as_str() == pane),
        ) {
            handle.handle.scroll_to_item(index);
        }
    }
    fn reveal_selected_slot(&self, slot: &str) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(node @ LayoutNode::Slot { pane_ids, .. }) = presented_slot_by_id(model, slot)
        else {
            return;
        };
        let Some(selected) = model.selected_pane(node) else {
            return;
        };
        if let (Some(scroll), Some(index)) = (
            self.tab_scrolls.get(slot),
            pane_ids.iter().position(|pane| pane.as_str() == selected),
        ) {
            scroll.handle.scroll_to_item(index);
        }
    }
    fn focus(&mut self, pane: &str, cx: &mut Context<Self>) {
        if self
            .model
            .as_mut()
            .is_some_and(|model| model.focus_pane(pane))
        {
            self.send(Command::Focus(pane.to_owned()), cx);
            self.sync_visible(cx);
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
        let Some(root) = model.presentation_root() else {
            return;
        };
        let Some(focused) = model.focused_pane() else {
            return;
        };
        let mut slots = Vec::new();
        let Some(shown) = model.shown() else {
            return;
        };
        visible_slots(shown, root, &self.slot_bounds, &mut slots);
        for entry in model.presented_floats() {
            visible_slots(shown, &entry.root, &self.slot_bounds, &mut slots);
        }
        let Some(current) = slots.iter().find(|slot| slot.pane == focused) else {
            return;
        };
        if let Some(next) = nearest_slot(&slots, &current.id, direction) {
            let pane = next.pane.clone();
            self.focus(&pane, cx);
            self.focus_current(window, cx);
        }
    }
    fn cycle_tab(&mut self, step: TabStep, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(screen) = model.shown_screen_id() else {
            return;
        };
        let Some(slot) = model.focused_slot_id() else {
            return;
        };
        self.send(
            Command::CycleTab {
                connection: model.connection_id(),
                screen: screen.to_owned(),
                slot: slot.to_owned(),
                step,
            },
            cx,
        );
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .pending_close
            .as_ref()
            .is_some_and(|pending| pending.prompting)
        {
            // A native confirmation owns keyboard input until it settles.
            cx.stop_propagation();
            return;
        }
        if self.template_dialog.is_some() {
            self.template_key(&event.keystroke, window, cx);
            return;
        }
        if self.pending_template_apply.is_some()
            || self
                .model
                .as_ref()
                .is_some_and(|model| model.pending_template().is_some())
        {
            // Placeholder panes have no admitted session routes. Screen
            // navigation remains available; every other key stays local.
            if let Some(Shortcut::ScreenIndex(index)) = shortcut_with_layout(
                &event.keystroke,
                cx.platform_info().is_mac(),
                window.modifiers().shift,
                cx.keyboard_layout().id(),
            ) {
                if let Some(id) = self
                    .model
                    .as_ref()
                    .and_then(|model| model.screens().get(index))
                    .map(|info| info.id.to_string())
                {
                    self.switch_screen(&id, window, cx);
                }
            }
            cx.stop_propagation();
            return;
        }
        if self.transfer_menu.is_some() {
            self.transfer_menu_key(&event.keystroke, cx);
            return;
        }
        if let Some(action) = zoom_shortcut(&event.keystroke, cx.platform_info().is_mac()) {
            cx.stop_propagation();
            self.submit_zoom(action, cx);
            return;
        }
        if self.rename.is_some() {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                self.finish_rename(false, window, cx);
            }
            return;
        }
        if self.profile_picker.is_some() {
            self.profile_key(&event.keystroke, window, cx);
            return;
        }
        if event.keystroke.key == "escape" && self.float_gesture.take().is_some() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.keystroke.key == "escape" && cx.has_active_drag() {
            cx.stop_active_drag(window);
            self.drop_target = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self
            .search_owner
            .as_ref()
            .is_some_and(|terminal| terminal.read(cx).search_input_focused(window, cx))
        {
            return;
        }
        let Some(action) = shortcut_with_layout(
            &event.keystroke,
            cx.platform_info().is_mac(),
            window.modifiers().shift,
            cx.keyboard_layout().id(),
        ) else {
            return;
        };
        cx.stop_propagation();
        match action {
            Shortcut::Templates => self.toggle_templates(window, cx),
            Shortcut::NewTab => self.tab(cx),
            Shortcut::NewScreen => self.create_screen(cx),
            Shortcut::ScreenIndex(index) => {
                if let Some(id) = self
                    .model
                    .as_ref()
                    .and_then(|model| model.screens().get(index))
                    .map(|info| info.id.to_string())
                {
                    self.switch_screen(&id, window, cx);
                }
            }
            Shortcut::ToggleMaximize => self.toggle_maximize(window, cx),
            Shortcut::PreviousTab => self.cycle_tab(TabStep::Previous, cx),
            Shortcut::NextTab => self.cycle_tab(TabStep::Next, cx),
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
            if self
                .model
                .as_ref()
                .is_some_and(|model| !model.is_root_pane(&pane))
            {
                self.status = "Split is available for tiled groups".into();
                cx.notify();
                return;
            }
            self.send(Command::Split { pane, axis }, cx);
        }
    }
    fn toggle_maximize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(model) = &self.model {
            if let Some(screen) = model.shown_screen_id() {
                let (slot, pane) = if model.maximized_slot_id().is_some() {
                    (None, None)
                } else {
                    (
                        model.focused_slot_id().map(str::to_owned),
                        model.focused_pane().map(str::to_owned),
                    )
                };
                if slot.is_none() && model.maximized_slot_id().is_none() {
                    return;
                }
                match self.commands.try_send(Command::SetMaximized {
                    screen: screen.to_owned(),
                    connection: model.connection_id(),
                    slot,
                    pane,
                }) {
                    Ok(()) => self.focus_current(window, cx),
                    Err(error) => {
                        self.status = format!("Action not sent: {error}");
                        cx.notify();
                    }
                }
            }
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
    fn poll(
        &mut self,
        first: Option<Update>,
        updates: &mut mpsc::Receiver<Update>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (bool, usize) {
        let mut bytes = 0;
        let mut first = first;
        let mut processed = 0;
        for _ in 0..64 {
            if bytes >= 256 * 1024 {
                break;
            }
            let update = if let Some(update) = first.take() {
                update
            } else {
                let Ok(update) = updates.try_recv() else {
                    break;
                };
                update
            };
            processed += 1;
            match update {
                Update::Workspace(model) => self.reconcile(*model, window, cx),
                Update::TemplateApi(result) => self.template_result(result, window, cx),
                Update::TemplateApply(result) => self.template_apply_result(result, window, cx),
                Update::Appearance(snapshot) => self.apply_appearance(&snapshot, window, cx),
                Update::Zoom(result) => self.apply_zoom_result(result, window, cx),
                Update::ScreenTransfer(result) => {
                    if self.pending_transfer.is_some_and(|pending| {
                        pending.matches(result.connection_id, result.request_id)
                    }) {
                        self.pending_transfer = None;
                        match result.outcome {
                            ScreenTransferOutcome::Confirmed => {
                                self.status = "Terminal moved to Screen".into();
                            }
                            ScreenTransferOutcome::Rejected(reason) => {
                                self.status = format!("Move rejected: {reason}");
                            }
                            ScreenTransferOutcome::Unverified(reason) => {
                                self.status = format!("Move unverified: {reason}");
                            }
                        }
                        cx.notify();
                    }
                }
                Update::FloatingAction(result) => {
                    if self.pending_float.as_ref().is_some_and(|pending| {
                        pending.connection_id == result.connection_id
                            && pending.request_id == result.request_id
                    }) {
                        let restore_focus = self
                            .pending_float
                            .take()
                            .is_some_and(|pending| pending.restore_focus);
                        self.status = match result.outcome {
                            FloatingActionOutcome::Confirmed => "Floating group updated".into(),
                            FloatingActionOutcome::Rejected(reason) => {
                                format!("Floating action rejected: {reason}")
                            }
                            FloatingActionOutcome::Unverified(reason) => {
                                format!("Floating action unverified: {reason}")
                            }
                        };
                        if restore_focus {
                            self.focus_current(window, cx);
                        }
                        cx.notify();
                    }
                }
                Update::ShellProfiles(snapshot) => self.apply_profiles(snapshot, cx),
                Update::ShellProfilesError { connection, reason } => {
                    self.profile_error(&connection.to_string(), reason, cx);
                }
                Update::AppearanceError { connection, reason } => {
                    if self
                        .model
                        .as_ref()
                        .is_some_and(|model| model.connection_id() == connection)
                    {
                        self.appearance_error = Some(reason);
                        cx.notify();
                    }
                }
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
            if self.stopping.load(Ordering::Acquire) {
                break;
            }
        }
        if self.smoke {
            self.smoke_tick(window, cx);
        }
        if self
            .zoom_feedback
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            self.zoom_feedback = None;
            cx.notify();
        }
        self.flush_visible(cx);
        self.flush_resizes(cx);
        self.flush_scheme();
        if updates.is_closed() && self.cleanup_complete.load(Ordering::Acquire) {
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
        (processed == 64 || bytes >= 256 * 1024, processed)
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
                        bytes: format!("printf 'NATIVE_WORKSPACE_%s_OK\\n' '{pane}'\r")
                            .into_bytes(),
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
                let Some(tab) = self.smoke_tab.clone() else {
                    self.finish_smoke(false, cx);
                    return;
                };
                let Some(screen_id) = model.shown_screen_id() else {
                    return;
                };
                let target = CloseTarget::Pane {
                    screen: screen_id.to_owned(),
                    pane: tab.clone(),
                };
                let Ok(close) = model.assess_close(0, target) else {
                    return;
                };
                if close.needs_confirmation() {
                    // The marker can arrive before the shell's later idle
                    // snapshot. This smoke exercises the no-prompt shortcut.
                    return;
                }
                self.select(&tab, window, cx);
                self.smoke_key(window, cx, "cmd-w", "alt-w", None);
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
                        bytes: b"printf 'NATIVE_HIDDEN_SCREEN_OK\\n'\r".into(),
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
            self.drop_target_at(&drag.origin, event.event.position)
        } else {
            None
        };
        if self.drop_target != target {
            self.drop_target = target;
            cx.notify();
        }
    }

    fn tab_header_height(&self) -> Pixels {
        tab_header_pixels(
            self.appearance.font_size,
            self.appearance.zoom.window_scale(),
        )
    }

    fn drop_target_at(&self, drag: &PaneDrag, position: Point<Pixels>) -> Option<DropTarget> {
        let model = self.model.as_ref()?;
        let root = model.presentation_root()?;
        if let Some(viewport) = self.float_viewport {
            if model.presented_floats().iter().any(|entry| {
                let projected = self
                    .float_gesture
                    .as_ref()
                    .filter(|gesture| gesture.target.floating_id() == entry.id.as_str())
                    .map_or_else(
                        || {
                            floating::project(
                                &entry.bounds,
                                viewport,
                                self.appearance.zoom.window_scale(),
                            )
                        },
                        |gesture| gesture.preview,
                    );
                projected.contains(&position)
            }) {
                return None;
            }
        }
        self.slot_bounds.iter().find_map(|(slot, bounds)| {
            slot_by_id(root, slot)?;
            if !bounds.contains(&position) {
                return None;
            }
            if position.y < bounds.top() + self.tab_header_height() {
                let LayoutNode::Slot { pane_ids, .. } = slot_by_id(root, slot)? else {
                    return None;
                };
                let scroll = self.tab_scrolls.get(slot)?;
                let (index, marker) = tab_insertion(
                    scroll.handle.bounds(),
                    position,
                    pane_ids.len(),
                    scroll.handle.offset().x,
                    self.appearance.zoom.window_scale(),
                    |index| scroll.handle.bounds_for_item(index),
                )?;
                let before = pane_ids.get(index).map(ToString::to_string);
                model
                    .can_place_tab(drag, slot, before.as_deref())
                    .then(|| DropTarget::Header {
                        slot: slot.clone(),
                        before,
                        marker_x: marker - bounds.left(),
                    })
            } else {
                let placement = body_drop_zone(*bounds, self.tab_header_height(), position)?;
                model
                    .can_move(drag, slot, placement)
                    .then(|| DropTarget::Body {
                        slot: slot.clone(),
                        placement,
                    })
            }
        })
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
        let intent = self.drop_target_at(&drag.origin, window.mouse_position());
        if let (Some(model), Some(intent)) = (&self.model, intent) {
            match intent {
                DropTarget::Body { slot, placement } if slot == target => {
                    match model.check_move(&drag.origin, &slot, placement) {
                        Ok(true) => self.send(
                            Command::MovePane {
                                drag: drag.origin.clone(),
                                target: slot,
                                placement,
                            },
                            cx,
                        ),
                        Ok(false) => {}
                        Err(error) => self.status = error.to_string(),
                    }
                }
                DropTarget::Header { slot, before, .. } if slot == target => {
                    match model.check_place_tab(&drag.origin, &slot, before.as_deref()) {
                        Ok(true) => self.send(
                            Command::PlaceTab {
                                drag: drag.origin.clone(),
                                target: slot,
                                before,
                            },
                            cx,
                        ),
                        Ok(false) => {}
                        Err(error) => self.status = error.to_string(),
                    }
                }
                _ => {}
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
    fn reconcile_float_order(&mut self, model: &LocalWorkspace) {
        let known_screens: HashSet<_> = model
            .screens()
            .iter()
            .map(|screen| screen.id.as_str())
            .collect();
        self.float_order
            .retain(|screen, _| known_screens.contains(screen.as_str()));
        let Some(screen) = model.shown_screen_id() else {
            return;
        };
        let canonical = model
            .snapshot()
            .map_or(&[][..], |snapshot| snapshot.floating.as_slice());
        let order = self.float_order.entry(screen.to_owned()).or_default();
        let ids: Vec<_> = canonical.iter().map(|entry| entry.id.to_string()).collect();
        floating::reconcile_order(order, &ids);
    }
    fn raise_float(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(screen) = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::shown_screen_id)
        else {
            return;
        };
        let order = self.float_order.entry(screen.to_owned()).or_default();
        if let Some(index) = order.iter().position(|item| item == id) {
            if index + 1 != order.len() {
                let id = order.remove(index);
                order.push(id);
                cx.notify();
            }
        }
    }
    fn float_at(&self, position: Point<Pixels>) -> Option<String> {
        let model = self.model.as_ref()?;
        let viewport = self.float_viewport?;
        let order = self.float_order.get(model.shown_screen_id()?)?;
        order.iter().rev().find_map(|id| {
            let entry = model
                .presented_floats()
                .iter()
                .find(|entry| entry.id.as_str() == id)?;
            let bounds = self
                .float_gesture
                .as_ref()
                .filter(|gesture| gesture.target.floating_id() == id)
                .map_or_else(
                    || {
                        floating::project(
                            &entry.bounds,
                            viewport,
                            self.appearance.zoom.window_scale(),
                        )
                    },
                    |gesture| gesture.preview,
                );
            bounds.contains(&position).then(|| id.clone())
        })
    }
    fn submit_float_action(
        &mut self,
        request_id: Uuid,
        command: Command,
        restore_focus: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(connection) = self.model.as_ref().map(LocalWorkspace::connection_id) else {
            return;
        };
        if self.pending_float.is_some() {
            return;
        }
        match self.commands.try_send(command) {
            Ok(()) => {
                self.pending_float = Some(PendingFloat {
                    connection_id: connection,
                    request_id,
                    restore_focus,
                })
            }
            Err(error) => {
                self.status = format!("Floating action not sent: {error}");
                cx.notify();
            }
        }
    }
    fn float_tab(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(pane) = model.focused_pane() else {
            return;
        };
        match model.capture_pane_action(pane) {
            Ok(source) if model.can_float_pane(&source) => {
                let request_id = Uuid::new_v4();
                self.submit_float_action(
                    request_id,
                    Command::FloatPane { request_id, source },
                    true,
                    cx,
                );
            }
            Ok(_) => {
                self.status = "Float Tab needs another tiled terminal".into();
                cx.notify();
            }
            Err(error) => {
                self.status = format!("Float Tab: {error}");
                cx.notify();
            }
        }
    }
    fn dock_float(&mut self, id: &str, cx: &mut Context<Self>) {
        let target = self
            .model
            .as_ref()
            .and_then(|model| model.capture_float(id).ok());
        if let Some(target) = target {
            let request_id = Uuid::new_v4();
            self.submit_float_action(
                request_id,
                Command::DockFloat { request_id, target },
                true,
                cx,
            );
        } else {
            self.status = "Floating group changed; Dock was not sent".into();
            cx.notify();
        }
    }
    fn maximize_float(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(entry) = model.floating(id) else {
            return;
        };
        let selected = model
            .focused_slot_id()
            .and_then(|slot| slot_by_id(&entry.root, slot))
            .and_then(|node| {
                model
                    .selected_pane(node)
                    .map(|pane| (node.id().to_string(), pane.to_owned()))
            })
            .or_else(|| first_selected_slot(model, &entry.root));
        let Some((slot, pane)) = selected else {
            return;
        };
        let Some(screen) = model.shown_screen_id() else {
            return;
        };
        match self.commands.try_send(Command::SetMaximized {
            screen: screen.to_owned(),
            connection: model.connection_id(),
            slot: Some(slot),
            pane: Some(pane),
        }) {
            Ok(()) => self.focus_current(window, cx),
            Err(error) => {
                self.status = format!("Action not sent: {error}");
                cx.notify();
            }
        }
    }
    fn begin_float_gesture(
        &mut self,
        id: &str,
        kind: FloatGestureKind,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.float_viewport else {
            return;
        };
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let Some(entry) = model.floating(id) else {
            return;
        };
        let Ok(target) = model.capture_float(id) else {
            return;
        };
        let zoom = self.appearance.zoom.window_scale();
        let initial = floating::project(&entry.bounds, viewport, zoom);
        self.float_gesture = Some(FloatGesture {
            target,
            connection: model.connection_id(),
            screen: model.shown_screen_id().unwrap_or_default().to_owned(),
            kind,
            origin: position,
            viewport,
            zoom,
            initial,
            preview: initial,
        });
        self.raise_float(id, cx);
    }
    fn move_float_gesture(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(gesture) = self.float_gesture.as_ref() else {
            return;
        };
        let valid = self.float_viewport == Some(gesture.viewport)
            && self.appearance.zoom.window_scale() == gesture.zoom
            && self
                .model
                .as_ref()
                .is_some_and(|model| model.validate_float(&gesture.target).is_ok());
        if !valid {
            self.float_gesture = None;
            cx.notify();
            return;
        }
        let next = floating::moved(gesture, position);
        if let Some(gesture) = self.float_gesture.as_mut() {
            if gesture.preview != next {
                gesture.preview = next;
                cx.notify();
            }
        }
    }
    fn finish_float_gesture(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.move_float_gesture(position, cx);
        let Some(gesture) = self.float_gesture.take() else {
            return;
        };
        if !floating::changed(gesture.initial, gesture.preview) {
            return;
        }
        let request_id = Uuid::new_v4();
        let bounds = floating::normalized(gesture.preview, gesture.viewport);
        self.submit_float_action(
            request_id,
            Command::MoveFloat {
                request_id,
                target: gesture.target,
                bounds,
            },
            false,
            cx,
        );
        cx.notify();
    }
    fn render_screen_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut screens = div()
            .id("screen-strip")
            .flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .min_w(chrome_rems(200.))
            .max_w(chrome_rems(390.))
            .overflow_x_scroll();
        if let Some(model) = &self.model {
            for (index, info) in model.screens().iter().enumerate() {
                let id = info.id.to_string();
                let selected = model.shown_screen_id() == Some(id.as_str());
                let mut label = info
                    .name
                    .as_deref()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("Screen {}", index + 1));
                if model
                    .pending_template()
                    .is_some_and(|preview| preview.screen_id() == id)
                {
                    label.push_str(" · Applying…");
                }
                let mut chip = div()
                    .id(SharedString::from(format!("screen-{id}")))
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(chrome_rems((self.appearance.font_size + 15.).max(28.)))
                    .bg(rgb(if selected {
                        self.appearance.active
                    } else {
                        self.appearance.raised
                    }));
                if let Some(editor) = self.rename.as_ref().filter(|editor| editor.screen == id) {
                    chip = chip.child(
                        div()
                            .w(chrome_rems(145.))
                            .h(chrome_rems((self.appearance.font_size + 14.).max(27.)))
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
                                .max_w(chrome_rems(120.))
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
            if let Some(preview) = model.pending_template().filter(|preview| {
                !model
                    .screens()
                    .iter()
                    .any(|info| info.id.as_str() == preview.screen_id())
            }) {
                screens = screens.child(
                    div()
                        .id("template-pending-screen")
                        .flex_none()
                        .px_2()
                        .py_1()
                        .bg(rgb(self.appearance.active))
                        .child(format!("{} · Applying…", preview.display_name())),
                );
            }
            if model.pending_template().is_none() && model.screens().is_empty() {
                screens = screens.child(
                    div()
                        .text_color(rgb(self.appearance.secondary))
                        .child("No screens"),
                );
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
                    .h(chrome_rems((self.appearance.font_size + 15.).max(28.)))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .bg(rgb(if pending {
                        self.appearance.raised
                    } else {
                        self.appearance.active
                    }))
                    .child("+ Screen")
                    .on_click(cx.listener(|this, _, _, cx| this.create_screen(cx))),
            )
            .into_any_element()
    }
    fn render_node(
        &self,
        node: &LayoutNode,
        surface: Surface,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            LayoutNode::Slot { id, pane_ids, .. } => {
                let selected = self
                    .model
                    .as_ref()
                    .and_then(|model| model.selected_pane(node));
                let scroll = self
                    .tab_scrolls
                    .get(id.as_str())
                    .cloned()
                    .unwrap_or_default();
                let tracked = scroll.clone();
                let weak = cx.entity().downgrade();
                let slot_for_width = id.to_string();
                let identity = self.model.as_ref().and_then(|model| {
                    Some((model.connection_id(), model.shown_screen_id()?.to_owned()))
                });
                let mut tabs = div()
                    .on_children_prepainted(move |_, window, _| {
                        let width = tracked.handle.bounds().size.width;
                        if width <= px(0.) {
                            return;
                        }
                        let previous = tracked.last_width.replace(Some(width));
                        if previous.is_some_and(|old| width >= old) {
                            return;
                        }
                        let Some((owner, screen)) = identity.clone() else {
                            return;
                        };
                        let weak = weak.clone();
                        let slot = slot_for_width.clone();
                        let tracked = tracked.clone();
                        window.on_next_frame(move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                let same_handle =
                                    this.tab_scrolls.get(&slot).is_some_and(|current| {
                                        Rc::ptr_eq(&current.last_width, &tracked.last_width)
                                    });
                                let same_screen = this.model.as_ref().is_some_and(|model| {
                                    model.connection_id() == owner
                                        && model.shown_screen_id() == Some(screen.as_str())
                                });
                                if same_handle && same_screen {
                                    this.reveal_selected_slot(&slot);
                                    cx.notify();
                                }
                            });
                        });
                    })
                    .id(SharedString::from(format!("tab-strip-{id}")))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h(chrome_rems((self.appearance.font_size + 19.).max(32.)))
                    .gap_1()
                    .bg(rgb(self.appearance.raised))
                    .overflow_x_scroll()
                    .track_scroll(&scroll.handle);
                for (index, pane) in pane_ids.iter().enumerate() {
                    let key = pane.to_string();
                    let close = key.clone();
                    let right_click = key.clone();
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
                                .flex_none()
                                .items_center()
                                .gap_3()
                                .px_3()
                                .cursor_pointer()
                                .bg(rgb(if active {
                                    self.appearance.active
                                } else {
                                    self.appearance.raised
                                }))
                                .text_color(rgb(if active {
                                    self.appearance.text
                                } else {
                                    self.appearance.secondary
                                }))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select(&key, window, cx)
                                }))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.open_transfer_menu(&right_click, window, cx);
                                    }),
                                )
                                .when_some(
                                    (surface == Surface::Tiled)
                                        .then(|| {
                                            self.model
                                                .as_ref()
                                                .and_then(|model| model.capture_drag(pane.as_str()))
                                        })
                                        .flatten(),
                                    |view, origin| {
                                        view.on_drag(
                                            TabDrag {
                                                origin,
                                                owner: cx.entity().entity_id(),
                                            },
                                            |_, _, _, cx| cx.new(|_| DragPreview),
                                        )
                                    },
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("drag-tab-{pane}")))
                                        .max_w(chrome_rems(180.))
                                        .overflow_hidden()
                                        .child(title),
                                )
                                .child(self.render_transfer_menu(pane.as_str(), cx))
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
                let plain_tab = new_tab.clone();
                let mut header = div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(chrome_rems((self.appearance.font_size + 19.).max(32.)))
                    .bg(rgb(self.appearance.raised))
                    .child(tabs)
                    .child(
                        div()
                            .id(SharedString::from(format!("new-tab-{id}")))
                            .flex_none()
                            .px_3()
                            .cursor_pointer()
                            .child("+")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(pane) = &plain_tab {
                                    this.send(Command::Tab(pane.clone()), cx);
                                }
                            })),
                    );
                if let Some(pane) = new_tab {
                    let slot = id.to_string();
                    header = header.child(
                        div()
                            .id(SharedString::from(format!("shell-profile-picker-{id}")))
                            .flex_none()
                            .px_2()
                            .cursor_pointer()
                            .child("▾")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.toggle_profile_picker(&slot, &pane, window, cx);
                            })),
                    );
                }
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
                    .when(surface == Surface::Tiled, |slot| {
                        slot.on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                            this.drop_pane(drag, &target, window, cx)
                        }))
                    })
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .min_h_0()
                    .border_1()
                    .border_color(rgb(if focused {
                        self.appearance.border
                    } else {
                        self.appearance.subtle_border
                    }))
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
                    .child(header);
                if let Some(pane) = selected {
                    if let Some(view) = self.panes.get(pane) {
                        let focus = pane.to_owned();
                        slot = slot.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .overflow_hidden()
                                .capture_any_mouse_down(cx.listener(
                                    move |this, event: &MouseDownEvent, _, cx| {
                                        if event.button == MouseButton::Left {
                                            this.focus(&focus, cx);
                                        }
                                    },
                                ))
                                .child(view.terminal.clone()),
                        );
                    } else {
                        let message = self
                            .model
                            .as_ref()
                            .and_then(|model| model.template_launch_error(pane))
                            .map(|reason| format!("Terminal failed to start: {reason}"))
                            .unwrap_or_else(|| "Starting terminal…".to_owned());
                        slot = slot.child(div().p_3().child(message));
                    }
                }
                match if surface == Surface::Tiled {
                    self.drop_target.as_ref()
                } else {
                    None
                } {
                    Some(DropTarget::Body {
                        slot: target,
                        placement,
                    }) if target == id.as_str() => {
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
                                    .border_color(rgb(self.appearance.accent))
                                    .bg(rgba((self.appearance.accent << 8) | 0x44)),
                            );
                        }
                    }
                    Some(DropTarget::Header {
                        slot: target,
                        marker_x,
                        ..
                    }) if target == id.as_str() => {
                        slot = slot.child(
                            div()
                                .absolute()
                                .left(*marker_x)
                                .top_0()
                                .w(chrome_rems(3.))
                                .h(self.tab_header_height())
                                .bg(rgb(self.appearance.accent)),
                        );
                    }
                    _ => {}
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
                            .child(self.render_node(child, surface, cx)),
                    );
                    if index + 1 < children.len() {
                        let node = id.to_string();
                        let axis = axis.clone();
                        let sizes = sizes.clone();
                        split = split.child(
                            div()
                                .id(SharedString::from(format!("divider-{id}-{index}")))
                                .flex_none()
                                .bg(rgb(self.appearance.border))
                                .when(horizontal, |view| {
                                    view.w(chrome_rems(5.))
                                        .h_full()
                                        .cursor(CursorStyle::ResizeColumn)
                                })
                                .when(!horizontal, |view| {
                                    view.h(chrome_rems(5.))
                                        .w_full()
                                        .cursor(CursorStyle::ResizeRow)
                                })
                                .when(surface == Surface::Tiled, |divider| {
                                    divider.on_mouse_down(
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
                                    )
                                }),
                        );
                    }
                }
                split.into_any_element()
            }
        }
    }
    fn render_float(
        &self,
        entry: &splitty_client_core::workspace::FloatingEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(viewport) = self.float_viewport else {
            return div().into_any_element();
        };
        let id = entry.id.to_string();
        let projected = self
            .float_gesture
            .as_ref()
            .filter(|gesture| gesture.target.floating_id() == id)
            .map_or_else(
                || floating::project(&entry.bounds, viewport, self.appearance.zoom.window_scale()),
                |gesture| gesture.preview,
            );
        let left = projected.left() - viewport.left();
        let top = projected.top() - viewport.top();
        let header_height = chrome_rems((self.appearance.font_size + 19.).max(32.));
        let move_id = id.clone();
        let dock_id = id.clone();
        let maximize_id = id.clone();
        let resize_id = id.clone();
        let raise_id = id.clone();
        let mut group = div()
            .id(SharedString::from(format!("float-{id}")))
            .absolute()
            .left(left)
            .top(top)
            .w(projected.size.width)
            .h(projected.size.height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .border_2()
            .border_color(rgb(self.appearance.accent))
            .bg(rgb(self.appearance.surface))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.raise_float(&raise_id, cx)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("float-drag-{id}")))
                    .flex_none()
                    .h(header_height)
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .cursor(CursorStyle::OpenHand)
                    .bg(rgb(self.appearance.raised))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.begin_float_gesture(
                                &move_id,
                                FloatGestureKind::Move,
                                event.position,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child("Floating group"),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("float-dock-{id}")))
                            .flex_none()
                            .px_2()
                            .cursor_pointer()
                            .child("Dock")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.dock_float(&dock_id, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("float-maximize-{id}")))
                            .flex_none()
                            .px_2()
                            .cursor_pointer()
                            .child("Maximize")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.maximize_float(&maximize_id, window, cx);
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(self.render_node(
                &entry.root,
                Surface::Floating,
                cx,
            )));
        group = group.child(
            div()
                .id(SharedString::from(format!("float-resize-{id}")))
                .absolute()
                .right_0()
                .bottom_0()
                .w(chrome_rems(18.))
                .h(chrome_rems(18.))
                .bg(rgb(self.appearance.accent))
                .cursor(CursorStyle::ResizeUpLeftDownRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.begin_float_gesture(
                            &resize_id,
                            FloatGestureKind::Resize,
                            event.position,
                            cx,
                        );
                        cx.stop_propagation();
                    }),
                ),
        );
        group.into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let floating_focus = self
            .model
            .as_ref()
            .and_then(LocalWorkspace::focused_pane)
            .is_some_and(|pane| {
                self.model
                    .as_ref()
                    .is_some_and(|model| !model.is_root_pane(pane))
            });
        let can_float = self
            .model
            .as_ref()
            .and_then(|model| {
                model.focused_pane().and_then(|pane| {
                    model
                        .capture_pane_action(pane)
                        .ok()
                        .map(|source| model.can_float_pane(&source))
                })
            })
            .unwrap_or(false);
        let appearance_message = self
            .appearance_error
            .as_ref()
            .map(|error| format!("Appearance: {error}"))
            .or_else(|| self.appearance.notice.map(str::to_owned));
        let profile_message = self
            .profiles_error
            .as_ref()
            .map(|error| format!("Profiles: {error}"));
        let zoom_message = self
            .zoom_feedback
            .as_ref()
            .map(|(message, _)| message.clone());
        let extra_lines =
            usize::from(appearance_message.is_some()) + usize::from(profile_message.is_some());
        let primary_status = if self.status.is_empty() {
            format!(
                "{} terminals · {modifier}+T tab · {modifier}+D split · {modifier}+Shift+arrows focus · Ctrl+Shift+C/V copy/paste",
                self.panes.len(),
            )
        } else {
            self.status.clone()
        };
        let primary_status = if let Some(message) = zoom_message {
            format!("{message} · {primary_status}")
        } else {
            primary_status
        };
        let footer_height =
            (self.appearance.font_size * (extra_lines + 1) as f32 + 14. + 2. * extra_lines as f32)
                .max(25. + 16. * extra_lines as f32);
        let picker_overlay = self.profile_picker_overlay(window, cx);
        let template_overlay = self.template_overlay(window, cx);
        let mut view = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(self.appearance.surface))
            .track_focus(&self.focus)
            .on_drag_move(cx.listener(Self::drag_hover))
            .text_color(rgb(self.appearance.text))
            .font(font(self.appearance.font_family.clone()))
            .text_size(chrome_rems(self.appearance.font_size))
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
                    .id("workspace-toolbar")
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(chrome_rems((self.appearance.font_size + 29.).max(42.)))
                    .px_3()
                    .flex_none()
                    .min_w_0()
                    .overflow_x_scroll()
                    .bg(rgb(self.appearance.raised))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .child("SPLITTY"),
                    )
                    .child(self.render_screen_controls(cx))
                    .child(
                        div()
                            .id("templates-open")
                            .flex_none()
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(self.appearance.overlay))
                            .child("Templates")
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.toggle_templates(window, cx)
                                }),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("maximize-group")
                            .flex_none()
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(self.appearance.overlay))
                            .child(
                                if self
                                    .model
                                    .as_ref()
                                    .and_then(LocalWorkspace::maximized_slot_id)
                                    .is_some()
                                {
                                    "Restore"
                                } else {
                                    "Maximize"
                                },
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_maximize(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("float-tab")
                            .flex_none()
                            .px_3()
                            .py_1()
                            .cursor(if can_float {
                                CursorStyle::PointingHand
                            } else {
                                CursorStyle::OperationNotAllowed
                            })
                            .text_color(rgb(if can_float {
                                self.appearance.text
                            } else {
                                self.appearance.muted
                            }))
                            .bg(rgb(self.appearance.overlay))
                            .child("Float Tab")
                            .on_click(cx.listener(|this, _, _, cx| this.float_tab(cx))),
                    )
                    .child(
                        div()
                            .id("split-horizontal")
                            .flex_none()
                            .px_3()
                            .py_1()
                            .cursor(if floating_focus {
                                CursorStyle::OperationNotAllowed
                            } else {
                                CursorStyle::PointingHand
                            })
                            .text_color(rgb(if floating_focus {
                                self.appearance.muted
                            } else {
                                self.appearance.text
                            }))
                            .bg(rgb(self.appearance.overlay))
                            .child("Split right")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.split(Axis::Horizontal, cx)),
                            ),
                    )
                    .child(
                        div()
                            .id("split-vertical")
                            .flex_none()
                            .px_3()
                            .py_1()
                            .cursor(if floating_focus {
                                CursorStyle::OperationNotAllowed
                            } else {
                                CursorStyle::PointingHand
                            })
                            .text_color(rgb(if floating_focus {
                                self.appearance.muted
                            } else {
                                self.appearance.text
                            }))
                            .bg(rgb(self.appearance.overlay))
                            .child("Split below")
                            .on_click(cx.listener(|this, _, _, cx| this.split(Axis::Vertical, cx))),
                    )
                    .child(
                        div()
                            .id("new-tab")
                            .flex_none()
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(self.appearance.overlay))
                            .child(if empty_screen {
                                "New terminal"
                            } else {
                                "New tab"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.tab(cx))),
                    ),
            );
        if let Some(model) = &self.model {
            if let Some(preview) = model.pending_template() {
                view = view.child(self.render_template_preview(preview, cx));
            } else if model.screens().is_empty() {
                view = view.child(
                    div().flex_1().flex().items_center().justify_center().child(
                        div()
                            .id("new-screen-empty")
                            .px_4()
                            .py_2()
                            .cursor_pointer()
                            .bg(rgb(self.appearance.overlay))
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
                                .bg(rgb(self.appearance.overlay))
                                .child("New terminal")
                                .on_click(cx.listener(|this, _, _, cx| this.tab(cx))),
                        ),
                    );
                } else {
                    let entity = cx.entity();
                    let weak_move = entity.downgrade();
                    let weak_up = entity.downgrade();
                    let weak_down = entity.downgrade();
                    let mut viewport = div()
                        .id("workspace-viewport")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .p_1()
                        .child(
                            canvas(
                                move |bounds, _, cx| {
                                    entity.update(cx, |this, cx| {
                                        if this.float_viewport != Some(bounds) {
                                            this.float_viewport = Some(bounds);
                                            if this.float_gesture.take().is_some() {
                                                cx.notify();
                                            }
                                            cx.notify();
                                        }
                                    })
                                },
                                move |_, (), window, _| {
                                    let weak = weak_down.clone();
                                    window.on_mouse_event(
                                        move |event: &MouseDownEvent, phase, _, cx| {
                                            if phase == DispatchPhase::Capture {
                                                let _ = weak.update(cx, |this, cx| {
                                                    if let Some(id) = this.float_at(event.position)
                                                    {
                                                        this.raise_float(&id, cx);
                                                    }
                                                });
                                            }
                                        },
                                    );
                                    let weak = weak_move.clone();
                                    window.on_mouse_event(
                                        move |event: &MouseMoveEvent, phase, _, cx| {
                                            if phase == DispatchPhase::Capture {
                                                let active = weak
                                                    .update(cx, |this, cx| {
                                                        let active = this.float_gesture.is_some();
                                                        this.move_float_gesture(event.position, cx);
                                                        active
                                                    })
                                                    .unwrap_or(false);
                                                if active {
                                                    cx.stop_propagation();
                                                }
                                            }
                                        },
                                    );
                                    let weak = weak_up.clone();
                                    window.on_mouse_event(
                                        move |event: &MouseUpEvent, phase, _, cx| {
                                            if phase == DispatchPhase::Capture
                                                && event.button == MouseButton::Left
                                            {
                                                let active = weak
                                                    .update(cx, |this, cx| {
                                                        let active = this.float_gesture.is_some();
                                                        this.finish_float_gesture(
                                                            event.position,
                                                            cx,
                                                        );
                                                        active
                                                    })
                                                    .unwrap_or(false);
                                                if active {
                                                    cx.stop_propagation();
                                                }
                                            }
                                        },
                                    );
                                },
                            )
                            .absolute()
                            .size_full(),
                        );
                    let root = model.presentation_root().unwrap_or(&screen.root);
                    let root_surface = if model.maximized_slot_id().is_some()
                        && slot_by_id(&screen.root, model.maximized_slot_id().unwrap_or_default())
                            .is_none()
                    {
                        Surface::Floating
                    } else {
                        Surface::Tiled
                    };
                    viewport = viewport.child(self.render_node(root, root_surface, cx));
                    let mut floats: Vec<_> = model.presented_floats().iter().collect();
                    if let Some(order) = model
                        .shown_screen_id()
                        .and_then(|screen| self.float_order.get(screen))
                    {
                        floats.sort_by_key(|entry| {
                            order
                                .iter()
                                .position(|id| id == entry.id.as_str())
                                .unwrap_or(usize::MAX)
                        });
                    }
                    for entry in floats {
                        viewport = viewport.child(self.render_float(entry, cx));
                    }
                    view = view.child(viewport);
                }
            } else {
                view = view.child(div().flex_1().p_4().child("Loading local Screen…"));
            }
        } else {
            view = view.child(div().flex_1().p_4().child("Connecting…"));
        }
        view.child(
            div()
                .h(chrome_rems(footer_height))
                .px_3()
                .flex_none()
                .flex()
                .flex_col()
                .justify_center()
                .text_size(chrome_rems((self.appearance.font_size - 2.).max(11.)))
                .text_color(rgb(self.appearance.muted))
                .child(
                    div()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(primary_status),
                )
                .when_some(appearance_message, |view, message| {
                    view.child(
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(rgb(self.appearance.secondary))
                            .child(message),
                    )
                })
                .when_some(profile_message, |view, message| {
                    view.child(
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(rgb(self.appearance.secondary))
                            .child(message),
                    )
                }),
        )
        .when_some(picker_overlay, |view, picker| view.child(picker))
        .when_some(template_overlay, |view, overlay| view.child(overlay))
    }
}
fn first_selected_slot(model: &LocalWorkspace, node: &LayoutNode) -> Option<(String, String)> {
    match node {
        LayoutNode::Slot { id, .. } => {
            Some((id.to_string(), model.selected_pane(node)?.to_owned()))
        }
        LayoutNode::Split { children, .. } => children
            .iter()
            .find_map(|child| first_selected_slot(model, child)),
    }
}
fn presented_slot_by_id<'a>(model: &'a LocalWorkspace, id: &str) -> Option<&'a LayoutNode> {
    presented_slot_in(model.presentation_root(), model.presented_floats(), id)
}
fn presented_slot_in<'a>(
    root: Option<&'a LayoutNode>,
    floats: &'a [splitty_client_core::workspace::FloatingEntry],
    id: &str,
) -> Option<&'a LayoutNode> {
    root.and_then(|root| slot_by_id(root, id))
        .or_else(|| floats.iter().find_map(|entry| slot_by_id(&entry.root, id)))
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
fn prune_layout_bounds<'a>(
    roots: impl IntoIterator<Item = &'a LayoutNode>,
    split_bounds: &mut HashMap<String, Bounds<Pixels>>,
    slot_bounds: &mut HashMap<String, Bounds<Pixels>>,
) -> HashSet<&'a str> {
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
    for root in roots {
        collect(root, &mut splits, &mut slots);
    }
    split_bounds.retain(|id, _| splits.contains(id.as_str()));
    slot_bounds.retain(|id, _| slots.contains(id.as_str()));
    slots
}
fn reveal_changed_tabs(
    previous: Option<&LocalWorkspace>,
    current: &LocalWorkspace,
    handles: &HashMap<String, TabScroll>,
) {
    fn visit(
        node: &LayoutNode,
        previous: Option<&LocalWorkspace>,
        current: &LocalWorkspace,
        handles: &HashMap<String, TabScroll>,
    ) {
        match node {
            LayoutNode::Slot { id, pane_ids, .. } => {
                let old_slot = previous.and_then(|model| presented_slot_by_id(model, id.as_str()));
                let changed = match old_slot {
                    Some(
                        old_slot @ LayoutNode::Slot {
                            pane_ids: old_ids, ..
                        },
                    ) => {
                        old_ids != pane_ids
                            || previous.and_then(|model| model.selected_pane(old_slot))
                                != current.selected_pane(node)
                    }
                    _ => true,
                };
                if changed {
                    if let Some(index) = current.selected_pane(node).and_then(|selected| {
                        pane_ids.iter().position(|pane| pane.as_str() == selected)
                    }) {
                        if let Some(handle) = handles.get(id.as_str()) {
                            handle.handle.scroll_to_item(index);
                        }
                    }
                }
            }
            LayoutNode::Split { children, .. } => {
                for child in children {
                    visit(child, previous, current, handles);
                }
            }
        }
    }
    if let Some(root) = current.presentation_root() {
        visit(root, previous, current, handles);
    }
    for entry in current.presented_floats() {
        visit(&entry.root, previous, current, handles);
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
    use super::{
        admit_close, nearest_slot, prune_layout_bounds, resized_pair, retry_pending_resize,
        shortcut, shortcut_with_layout, tab_header_pixels, terminal_commands, visible_slots,
        zoom_shortcut, Command, Direction, PendingClose, Shortcut, Update, VisibleSlot,
    };
    use crate::terminal;
    use gpui_kit::{point, px, size, Bounds, Keystroke, Modifiers, Pixels};
    use splitty_client_core::{
        workspace::{ClosePlan, CloseTarget, LayoutNode, LocalWorkspace, ScreenModel},
        zoom::ZoomAction,
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
    fn zoom_shortcuts_use_logical_keys_and_do_not_claim_other_modifiers() {
        for (mac, base) in [
            (
                false,
                Modifiers {
                    control: true,
                    ..Default::default()
                },
            ),
            (
                true,
                Modifiers {
                    platform: true,
                    ..Default::default()
                },
            ),
        ] {
            for (name, action) in [
                ("+", ZoomAction::In),
                ("=", ZoomAction::In),
                ("-", ZoomAction::Out),
                ("0", ZoomAction::Reset),
            ] {
                assert_eq!(zoom_shortcut(&key(name, base), mac), Some(action));
            }
            assert_eq!(
                zoom_shortcut(
                    &key(
                        "+",
                        Modifiers {
                            shift: true,
                            ..base
                        }
                    ),
                    mac
                ),
                Some(ZoomAction::In)
            );
            for modifiers in [
                Modifiers { alt: true, ..base },
                Modifiers {
                    function: true,
                    ..base
                },
                Modifiers {
                    control: true,
                    platform: true,
                    ..base
                },
            ] {
                assert_eq!(zoom_shortcut(&key("=", modifiers), mac), None);
            }
            assert_eq!(zoom_shortcut(&key("f", base), mac), None);
        }
    }

    #[test]
    fn scaled_tab_header_and_insertion_marker_match_visible_geometry() {
        assert_eq!(tab_header_pixels(13., 0.5), px(17.));
        assert_eq!(tab_header_pixels(13., 1.25), px(41.));
        assert_eq!(tab_header_pixels(13., 3.), px(97.));
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(540.), px(97.)));
        let tab = |index| {
            Some(Bounds::new(
                point(px(index as f32 * 180.), px(0.)),
                size(px(180.), px(97.)),
            ))
        };
        assert_eq!(
            super::tab_insertion(viewport, point(px(530.), px(48.)), 3, px(0.), 3., tab),
            Some((3, px(531.)))
        );
        let slot = Bounds::new(point(px(0.), px(0.)), size(px(540.), px(400.)));
        assert_eq!(
            super::body_drop_zone(slot, tab_header_pixels(13., 3.), point(px(200.), px(96.))),
            None
        );
        assert!(
            super::body_drop_zone(slot, tab_header_pixels(13., 3.), point(px(200.), px(150.)))
                .is_some()
        );
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
            assert_eq!(
                shortcut(&key("pageup", base), is_mac),
                Some(Shortcut::PreviousTab)
            );
            assert_eq!(
                shortcut(&key("pagedown", base), is_mac),
                Some(Shortcut::NextTab)
            );
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
            assert_eq!(shortcut(&key("pageup", shifted), is_mac), None);
            assert_eq!(shortcut(&key("pagedown", shifted), is_mac), None);
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
            assert_eq!(shortcut(&key("pageup", base), !is_mac), None);
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

    #[test]
    fn navigation_chords_require_actual_shift_and_known_symbol_layout() {
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        let shifted_alt = Modifiers { shift: true, ..alt };
        let ctrl_shift = Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            shortcut(&key("m", alt), false),
            Some(Shortcut::ToggleMaximize)
        );
        assert_eq!(
            shortcut(&key("space", ctrl_shift), false),
            Some(Shortcut::NewScreen)
        );
        assert_eq!(
            shortcut(
                &key(
                    "space",
                    Modifiers {
                        alt: true,
                        ..ctrl_shift
                    }
                ),
                false
            ),
            None
        );
        assert_eq!(
            shortcut_with_layout(&key("1", shifted_alt), false, true, "unknown"),
            Some(Shortcut::ScreenIndex(0))
        );
        assert_eq!(
            shortcut_with_layout(&key("&", alt), false, true, "English (US)"),
            Some(Shortcut::ScreenIndex(6))
        );
        assert_eq!(
            shortcut_with_layout(&key("&", alt), false, true, "German"),
            Some(Shortcut::ScreenIndex(5))
        );
        assert_eq!(
            shortcut_with_layout(&key("/", alt), false, true, "German"),
            Some(Shortcut::ScreenIndex(6))
        );
        assert_eq!(
            shortcut_with_layout(&key("(", alt), false, true, "German"),
            Some(Shortcut::ScreenIndex(7))
        );
        assert_eq!(
            shortcut_with_layout(&key("(", alt), false, true, "English (US)"),
            Some(Shortcut::ScreenIndex(8))
        );
        assert_eq!(
            shortcut_with_layout(&key("&", alt), false, false, "German"),
            None
        );
        assert_eq!(
            shortcut_with_layout(&key("&", alt), false, true, "unknown"),
            None
        );
        assert_eq!(
            shortcut_with_layout(
                &key(
                    "&",
                    Modifiers {
                        control: true,
                        ..alt
                    }
                ),
                false,
                true,
                "German"
            ),
            None
        );
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
        prune_layout_bounds(std::iter::once(&root), &mut splits, &mut slots);
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
    fn tab_header_insertion_clips_to_viewport_and_body_remains_a_distinct_target() {
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(180.), px(32.)));
        let tab = |index| {
            Some(Bounds::new(
                point(px(index as f32 * 60.), px(0.)),
                size(px(60.), px(32.)),
            ))
        };
        for (x, expected_index, expected_marker) in
            [(10., 0, 1.), (40., 1, 60.), (90., 2, 120.), (170., 3, 177.)]
        {
            assert_eq!(
                super::tab_insertion(viewport, point(px(x), px(16.)), 3, px(0.), 1., tab),
                Some((expected_index, px(expected_marker))),
            );
        }
        assert_eq!(
            super::tab_insertion(viewport, point(px(190.), px(16.)), 3, px(0.), 1., tab),
            None
        );
        let clipped = Bounds::new(point(px(100.), px(0.)), size(px(120.), px(32.)));
        let shifted = |index| {
            Some(Bounds::new(
                point(px(80. + index as f32 * 60.), px(0.)),
                size(px(60.), px(32.)),
            ))
        };
        assert_eq!(
            super::tab_insertion(clipped, point(px(105.), px(16.)), 3, px(-20.), 1., shifted),
            Some((1, px(120.))),
        );
        assert_eq!(
            super::tab_insertion(clipped, point(px(99.), px(16.)), 3, px(-20.), 1., shifted),
            None
        );
        let slot = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(120.)));
        assert_eq!(
            super::body_drop_zone(slot, px(33.), point(px(100.), px(20.))),
            None
        );
        assert_eq!(
            super::body_drop_zone(slot, px(33.), point(px(100.), px(60.))),
            Some(super::MovePlacement::Tab)
        );
        assert_eq!(
            super::body_drop_zone(slot, px(33.), point(px(100.), px(34.))),
            Some(super::MovePlacement::Above)
        );
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
            Arc::new(std::sync::atomic::AtomicU64::new(4)),
        );
        let second = terminal_commands(
            sender.clone(),
            "pane-b".into(),
            "session-b".into(),
            second_pending.clone(),
            Arc::new(std::sync::atomic::AtomicU64::new(4)),
        );
        let input = |generation, text: &str| terminal::Command::Input {
            generation,
            bytes: text.as_bytes().to_vec(),
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
                bytes,
            } => {
                assert_eq!(
                    (pane.as_str(), session.as_str(), bytes.as_slice()),
                    ("pane-a", "session-a", paste.as_bytes())
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
                bytes,
            } => {
                assert_eq!(
                    (pane.as_str(), session.as_str(), bytes.as_slice()),
                    ("pane-b", "session-b", b"next".as_slice())
                );
            }
            _ => panic!("expected input"),
        }
        match receiver.try_recv().expect("newly accepted input") {
            Command::Input { bytes, .. } => assert_eq!(bytes, b"after drain"),
            _ => panic!("expected input"),
        }
        assert!(receiver.try_recv().is_err());
        assert!(first(terminal::Command::Input {
            generation: 1,
            bytes: vec![0x80]
        })
        .is_err());
        first(terminal::Command::Input {
            generation: 0,
            bytes: vec![0, 0x80, 0xff],
        })
        .unwrap();
        assert!(
            matches!(receiver.try_recv(), Ok(Command::Input { pane, session, bytes })
            if pane == "pane-a" && session == "session-a" && bytes == [0, 0x80, 0xff])
        );
        retry_pending_resize(
            &sender,
            "pane-b",
            "session-b",
            &second_pending,
            &std::sync::atomic::AtomicU64::new(5),
        )
        .expect("resize retry admitted");
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::ResizePane {
                route_generation: 5,
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
