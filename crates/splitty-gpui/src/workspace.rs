//! Native projection of one authoritative local screen; no independent dock tree.
mod chrome;
mod drawer;
mod floating;
mod host_update;
mod hosts;
mod ports;
mod screen_motion;
mod settings;
mod shell_profiles;
#[cfg(test)]
mod source_routing_tests;
mod templates;
mod terminal_menu;
mod transfer;
mod unread;

use crate::appearance::NativeAppearance;
use crate::terminal::{self, Terminal};
use gpui_kit::assets::IconName;
use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::base::{transition, Easing, GlobalState, Popover, Theme, Transition};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    ActiveTheme as _, Disableable, Icon, Sizable, WindowExt as _,
};
use gpui_kit::{prelude::*, *};
use splitty_client_core::appearance::{
    AppearanceSnapshot, ColorScheme, DrawerVisibilityOutcome, DrawerVisibilityResult,
};
use splitty_client_core::native_workspace::{
    Command as WorkerCommand, CommandKind as Command, Update, Worker,
};
use splitty_client_core::notifications::NotificationCenter;
use splitty_client_core::shell_profiles::ShellProfilesSnapshot;
use splitty_client_core::workspace::{
    Axis, CloseIdentity, ClosePlan, CloseTarget, FloatingActionOutcome, LayoutKey, LayoutNode,
    MovePlacement, PaneActionTarget, PaneDrag, PaneKey, ScreenKey, ScreenModel,
    ScreenTransferOutcome, ScreenTransferTarget, SourceId, SourceToken, SourceWorkspace, TabStep,
    TerminalToken, WindowWorkspace,
};
use splitty_client_core::zoom::{Zoom, ZoomAction, ZoomOutcome, ZoomResult};
use std::{
    cell::Cell,
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use uuid::Uuid;

use self::drawer::{ScreenDrop, WheelNavigation, WheelTarget};
use self::floating::{FloatGesture, FloatGestureKind};
use self::hosts::{HostKey, HostsView};
use self::ports::PortsView;
use self::screen_motion::{ScreenMotion, ScreenProjection};
use self::settings::{SettingsEvent, SettingsView};
use self::shell_profiles::ProfilePicker;
use self::templates::TemplateDialog;
use self::terminal_menu::TerminalMenu;

/// The local appearance read has produced either a usable theme or a visible
/// failure. The window shell may now reveal its first painted frame.
pub(crate) struct AppearanceSettled;

impl EventEmitter<AppearanceSettled> for Workspace {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Destination {
    Screens,
    Hosts,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Tiled,
    Floating,
}

struct PendingFloat {
    source: SourceToken,
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
    ToggleDrawer,
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

fn drawer_transition() -> Transition {
    Transition::new(Duration::from_millis(220)).easing(Easing::CubicBezier {
        x1: 0.25,
        y1: 0.1,
        x2: 0.25,
        y2: 1.,
    })
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
        && !mods.shift
        && !mods.alt
        && !mods.platform
        && !mods.function
        && key.key.eq_ignore_ascii_case("b")
    {
        return Some(Shortcut::ToggleDrawer);
    }
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
        slot: LayoutKey,
        placement: MovePlacement,
    },
    Header {
        slot: LayoutKey,
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
fn layout_node_by_id<'a>(node: &'a LayoutNode, id: &str) -> Option<&'a LayoutNode> {
    if node.id().as_str() == id {
        return Some(node);
    }
    match node {
        LayoutNode::Split { children, .. } => children
            .iter()
            .find_map(|child| layout_node_by_id(child, id)),
        LayoutNode::Slot { .. } => None,
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
struct DragPreview {
    title: String,
    active: bool,
    appearance: NativeAppearance,
}
impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_1()
            .h(chrome_rems((self.appearance.font_size + 19.).max(32.)))
            .font(font(self.appearance.font_family.clone()))
            .text_size(chrome_rems(self.appearance.font_size))
            .bg(rgb(if self.active {
                self.appearance.active
            } else {
                self.appearance.raised
            }))
            .text_color(rgb(if self.active {
                self.appearance.text
            } else {
                self.appearance.secondary
            }))
            .border_1()
            .border_color(rgb(self.appearance.border))
            .child(Icon::new(IconName::SquareTerminal).xsmall())
            .child(
                div()
                    .max_w(chrome_rems(180.))
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.title.clone()),
            )
            .child(Icon::new(IconName::X).xsmall())
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
#[derive(Clone, Debug, PartialEq, Eq)]
struct ResizeCapture {
    route: TerminalToken,
    screen: ScreenKey,
    route_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingResize {
    capture: ResizeCapture,
    cols: u16,
    rows: u16,
}

fn terminal_commands(
    sender: mpsc::Sender<WorkerCommand>,
    capture: ResizeCapture,
    pending_resize: Arc<Mutex<Option<PendingResize>>>,
) -> impl Fn(terminal::Command) -> Result<(), String> {
    move |command| {
        let resize = match &command {
            terminal::Command::Resize(cols, rows) => Some(PendingResize {
                capture: capture.clone(),
                cols: *cols,
                rows: *rows,
            }),
            _ => None,
        };
        let command = match command {
            terminal::Command::Input {
                generation: 0,
                bytes,
            } => Command::Input {
                route: capture.route.clone(),
                bytes,
            },
            terminal::Command::Input { .. } => {
                return Err("Terminal generation changed".into());
            }
            terminal::Command::Resize(cols, rows) => Command::ResizePane {
                route: capture.route.clone(),
                screen: capture.screen.clone(),
                route_generation: capture.route_generation,
                cols,
                rows,
            },
        };
        match sender.try_send(WorkerCommand::new(capture.route.source_token(), command)) {
            Ok(()) => {
                if resize.is_some() {
                    if let Ok(mut pending) = pending_resize.lock() {
                        if pending
                            .as_ref()
                            .is_some_and(|pending| pending.capture == capture)
                        {
                            *pending = None;
                        }
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

fn workspace_terminal_commands(
    sender: mpsc::Sender<WorkerCommand>,
    capture: ResizeCapture,
    pending_resize: Arc<Mutex<Option<PendingResize>>>,
    inputs: Option<Arc<AtomicU64>>,
    last_pane: Option<Arc<Mutex<Option<String>>>>,
) -> impl Fn(terminal::Command) -> Result<(), String> {
    let input_pane = capture.route.pane_id().to_owned();
    let route = terminal_commands(sender, capture, pending_resize);
    move |command| {
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
    }
}

fn retry_pending_resize(
    sender: &mpsc::Sender<WorkerCommand>,
    expected: &ResizeCapture,
    pending: &Arc<Mutex<Option<PendingResize>>>,
) -> Result<(), String> {
    let size = pending.lock().map_err(|error| error.to_string())?.take();
    let Some(size) = size else {
        return Ok(());
    };
    if size.capture != *expected {
        return Ok(());
    }
    if let Err(error) = sender.try_send(WorkerCommand::new(
        size.capture.route.source_token(),
        Command::ResizePane {
            route: size.capture.route.clone(),
            screen: size.capture.screen.clone(),
            route_generation: size.capture.route_generation,
            cols: size.cols,
            rows: size.rows,
        },
    )) {
        let mut current = pending.lock().map_err(|error| error.to_string())?;
        if current.is_none() {
            *current = Some(size);
        }
        return Err(error.to_string());
    }
    Ok(())
}

struct PaneView {
    route: TerminalToken,
    capture: ResizeCapture,
    terminal: Entity<Terminal>,
    _link_subscription: Subscription,
    _menu_subscription: Subscription,
    pending_resize: Arc<Mutex<Option<PendingResize>>>,
    marker: bool,
    tail: String,
}
#[cfg(test)]
fn dispatch_owned_update<T>(
    model: Option<&SourceWorkspace>,
    source: SourceToken,
    connection: Option<Uuid>,
    consume: impl FnOnce(&SourceWorkspace) -> T,
) -> Option<T> {
    let model = model.filter(|model| model.source_token() == source)?;
    if connection.is_some_and(|connection| connection != source.connection_id()) {
        return None;
    }
    Some(consume(model))
}
struct ResizeDrag {
    node: LayoutKey,
    source: SourceToken,
    revision: u64,
    axis: Axis,
    index: usize,
    start: Point<Pixels>,
    last_position: Point<Pixels>,
    bounds: Bounds<Pixels>,
    children: Vec<String>,
    sizes: Vec<f64>,
    last_sent: Instant,
    pending: Vec<f64>,
}
fn resize_split<'a>(
    model: &'a SourceWorkspace,
    node: &LayoutKey,
) -> Option<(u64, Axis, Vec<String>, &'a [f64])> {
    if model.screen_key(model.shown_screen_id()?) != *node.screen() {
        return None;
    }
    let revision = model.snapshot()?.revision;
    let LayoutNode::Split {
        axis,
        children,
        sizes,
        ..
    } = layout_node_by_id(model.presentation_root()?, node.node_id())?
    else {
        return None;
    };
    Some((
        revision,
        axis.clone(),
        children
            .iter()
            .map(|child| child.id().to_string())
            .collect(),
        sizes,
    ))
}
fn refresh_resize_drag(drag: &mut ResizeDrag, model: &SourceWorkspace) -> bool {
    if model.source_token() != drag.source {
        return false;
    }
    let Some((revision, axis, children, sizes)) = resize_split(model, &drag.node) else {
        return false;
    };
    if axis != drag.axis || children != drag.children || drag.index + 1 >= sizes.len() {
        return false;
    }
    if revision != drag.revision {
        drag.revision = revision;
        drag.sizes = sizes.to_vec();
        drag.pending = drag.sizes.clone();
        drag.start = drag.last_position;
    }
    true
}
struct RenameEditor {
    screen: ScreenKey,
    source: SourceToken,
    input: Entity<InputState>,
}

#[derive(Clone, Copy)]
struct PendingDrawerPin {
    owner: SourceToken,
    request_id: Uuid,
    desired: bool,
}

#[derive(Clone)]
struct TabContextMenu {
    source: PaneActionTarget,
    selected: usize,
    scroll: ScrollHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingTransfer {
    source: SourceToken,
    request_id: Uuid,
}

impl PendingTransfer {
    fn matches(self, connection_id: Uuid, request_id: Uuid) -> bool {
        self.source.connection_id() == connection_id && self.request_id == request_id
    }
}
struct PendingClose {
    plan: ClosePlan,
    prompting: bool,
}
impl PendingClose {
    fn matches_response(
        &self,
        request_id: u64,
        source: Option<SourceToken>,
        model: Option<&SourceWorkspace>,
    ) -> bool {
        self.plan.request_id == request_id
            && self.plan.source_token() == source
            && source.is_none_or(|source| model.is_some_and(|model| model.source_token() == source))
    }
    fn is_prompt_for(&self, request_id: u64) -> bool {
        self.prompting && self.plan.request_id == request_id
    }
    fn resolve_prompt(
        &self,
        model: Option<&SourceWorkspace>,
        confirmed: bool,
    ) -> Result<Option<ClosePlan>, String> {
        if !self.prompting {
            return Err("Close prompt is no longer pending".into());
        }
        if !confirmed {
            return Ok(None);
        }
        if self.plan == ClosePlan::startup_window(self.plan.request_id) {
            return Ok(Some(self.plan.clone()));
        }
        match model {
            Some(model) => model
                .validate_close(&self.plan, true)
                .map_err(|error| error.to_string())?,
            None => return Err("Close target is gone".into()),
        }
        Ok(Some(self.plan.clone()))
    }
    fn matches_window_approval(&self, update: &Update, model: Option<&SourceWorkspace>) -> bool {
        matches!(update, Update::WindowCloseApproved { request_id, source }
            if !self.prompting
                && self.matches_response(*request_id, *source, model)
                && self.plan.target == CloseTarget::Window)
    }
}
fn admit_close(
    commands: &mpsc::Sender<WorkerCommand>,
    plan: ClosePlan,
    confirmed: bool,
) -> Result<PendingClose, String> {
    commands
        .try_send(WorkerCommand::close(plan.clone(), confirmed))
        .map_err(|error| format!("Close not sent: {error}"))?;
    Ok(PendingClose {
        plan,
        prompting: false,
    })
}
pub struct Workspace {
    model: Option<WindowWorkspace>,
    focus: FocusHandle,
    hosts: HostsView,
    ports: PortsView,
    settings: Entity<SettingsView>,
    destination: Destination,
    host_connect_trigger: FocusHandle,
    port_trigger: FocusHandle,
    settings_trigger: FocusHandle,
    _settings_subscription: Subscription,
    panes: HashMap<PaneKey, PaneView>,
    commands: mpsc::Sender<WorkerCommand>,
    stopping: Arc<AtomicBool>,
    cleanup_complete: Arc<AtomicBool>,
    worker_finished: Arc<AtomicBool>,
    split_bounds: HashMap<LayoutKey, Bounds<Pixels>>,
    slot_bounds: HashMap<LayoutKey, Bounds<Pixels>>,
    tab_scrolls: HashMap<LayoutKey, TabScroll>,
    drawer_scroll: ScrollHandle,
    drop_target: Option<DropTarget>,
    drop_motion_generation: u64,
    drop_motion_duration: Duration,
    drag: Option<ResizeDrag>,
    visible: Vec<PaneKey>,
    visible_screen: Option<ScreenKey>,
    visible_owner: Option<SourceToken>,
    visible_route_generation: u64,
    search_owner: Option<(PaneKey, Entity<Terminal>)>,
    visible_pending: bool,
    status: String,
    appearance: NativeAppearance,
    appearance_error: Option<String>,
    appearance_generation: u64,
    drawer_visible: bool,
    drawer_authoritative_pinned: bool,
    drawer_width: i32,
    drawer_edge_reveal: bool,
    drawer_overlay_open: bool,
    drawer_edge_last: Option<(Point<Pixels>, Instant)>,
    drawer_edge_task: Option<Task<()>>,
    drawer_collapsed: HashSet<ScreenKey>,
    drawer_hovered: Option<ScreenKey>,
    drawer_pane_hovered: Option<(ScreenKey, String)>,
    drawer_drop: Option<ScreenDrop>,
    drawer_pending: Option<PendingDrawerPin>,
    wheel_navigation: WheelNavigation,
    pending_zoom: HashSet<Uuid>,
    zoom_feedback: Option<(String, Instant)>,
    profiles: Option<Arc<ShellProfilesSnapshot>>,
    profiles_error: Option<String>,
    profiles_generation: u64,
    profile_picker: Option<ProfilePicker>,
    template_dialog: Option<TemplateDialog>,
    pending_template_apply: Option<(Uuid, Uuid)>,
    tab_context_menu: Option<TabContextMenu>,
    terminal_menu: Option<TerminalMenu>,
    pending_transfer: Option<PendingTransfer>,
    float_order: HashMap<ScreenKey, Vec<String>>,
    float_viewport: Option<Bounds<Pixels>>,
    screen_viewport: Option<Bounds<Pixels>>,
    screen_motion: ScreenMotion,
    float_gesture: Option<FloatGesture>,
    pending_float: Option<PendingFloat>,
    desired_scheme: ColorScheme,
    sent_scheme: ColorScheme,
    scheme_pending: bool,
    _appearance_subscription: Subscription,
    _activation_subscription: Subscription,
    _notification_visibility_subscription: Subscription,
    notifications: NotificationCenter,
    notification_settings_generation: u64,
    notification_settings_error: Option<String>,
    notification_window_active: bool,
    notification_window_visible: bool,
    _drawer_bounds_subscription: Subscription,
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
    fn selected_model(&self) -> Option<&SourceWorkspace> {
        let window = self.model.as_ref()?;
        window.source(window.shown_key()?.source_id())
    }

    fn local_model(&self) -> Option<&SourceWorkspace> {
        Some(self.model.as_ref()?.local_source())
    }

    fn model_for_source(&self, id: SourceId) -> Option<&SourceWorkspace> {
        self.model.as_ref()?.source(id)
    }

    fn is_remote_selected(&self) -> bool {
        self.current_screen_key().is_some_and(|screen| {
            self.model
                .as_ref()
                .is_some_and(|window| screen.source_id() != window.local_source().source_id())
        })
    }

    fn lifecycle_token(&self) -> Option<SourceToken> {
        Some(self.model.as_ref()?.lifecycle_token())
    }

    fn execution_token(&self, id: SourceId) -> Option<SourceToken> {
        self.model.as_ref()?.execution_token(id)
    }

    fn close_model(&self, plan: &ClosePlan) -> Option<&SourceWorkspace> {
        if plan.target == CloseTarget::Window {
            self.local_model()
        } else {
            self.model_for_source(plan.source_token()?.source_id())
        }
    }

    fn current_screen_key(&self) -> Option<ScreenKey> {
        self.model.as_ref()?.shown_key()
    }

    fn current_layout_key(&self, node: &str) -> Option<LayoutKey> {
        Some(self.current_screen_key()?.layout_key(node))
    }
    fn owns_surface(&self, source: SourceToken, screen: &ScreenKey) -> bool {
        self.model
            .as_ref()
            .is_some_and(|window| window.shown_key() == Some(screen.clone()))
            && self.selected_model().is_some_and(|model| {
                model.source_token() == source
                    && model.shown_screen_id() == Some(screen.screen_id())
                    && model.screen_key(screen.screen_id()) == *screen
            })
    }

    pub fn new(
        smoke: bool,
        outcome: Arc<AtomicBool>,
        backend: splitty_client_core::owned_service::LocalBackend,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Self, Worker) {
        Self::new_with_worker(smoke, outcome, window, cx, |receiver, sender, scheme| {
            Worker::start_with_backend(receiver, sender, scheme, backend)
        })
    }

    fn new_with_worker(
        smoke: bool,
        outcome: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
        start_worker: impl FnOnce(
            mpsc::Receiver<WorkerCommand>,
            mpsc::Sender<Update>,
            ColorScheme,
        ) -> Worker,
    ) -> (Self, Worker) {
        let (commands, receiver) = mpsc::channel(64);
        let (sender, mut updates) = mpsc::channel(64);
        let initial_scheme = Self::window_scheme(window);
        let worker = start_worker(receiver, sender, initial_scheme);
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
            this.notification_window_active = window.is_window_active();
            this.refresh_notification_watch(cx);
            if !window.is_window_active() && this.drawer_drop.take().is_some() {
                cx.stop_active_drag(window);
                cx.notify();
            }
            if !window.is_window_active() {
                this.cancel_terminal_menu(window, cx);
                let changed = this.float_gesture.take().is_some() | this.screen_motion.clear();
                if changed {
                    cx.notify();
                }
            }
        });
        let notification_visibility_subscription =
            cx.observe_window_visibility(window, |this, _, window, cx| {
                this.notification_window_visible = window.is_visible();
                this.refresh_notification_watch(cx);
            });
        let drawer_bounds_subscription = cx.observe_window_bounds(window, |this, window, cx| {
            if this.drawer_drop.take().is_some() {
                cx.stop_active_drag(window);
                cx.notify();
            }
        });
        let typography = Theme::global(cx).tokens.typography;
        let ui_family = typography.sans.to_string();
        let mono_family = typography.mono.to_string();
        let available_fonts = window.text_system().all_font_names();
        let settings = cx.new(|cx| SettingsView::new(available_fonts, window, cx));
        let settings_subscription = cx.subscribe_in(
            &settings,
            window,
            |this, _, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::Close => this.close_settings(window, cx),
                SettingsEvent::Update {
                    connection_id,
                    request_id,
                    edit,
                } => {
                    let result = this
                        .local_model()
                        .filter(|model| model.connection_id() == *connection_id)
                        .ok_or_else(|| "Settings connection changed".to_owned())
                        .and_then(|model| {
                            this.commands
                                .try_send(WorkerCommand::new(
                                    model.source_token(),
                                    Command::UpdateSettings {
                                        connection: *connection_id,
                                        request_id: *request_id,
                                        edit: edit.clone(),
                                    },
                                ))
                                .map_err(|_| "Could not send settings change".to_owned())
                        });
                    if let Err(reason) = result {
                        this.settings
                            .update(cx, |view, cx| view.reject_update(*request_id, reason, cx));
                    }
                }
            },
        );
        (
            Self {
                model: None,
                focus: cx.focus_handle(),
                hosts: HostsView::default(),
                ports: PortsView::default(),
                settings,
                destination: Destination::Screens,
                host_connect_trigger: cx.focus_handle(),
                port_trigger: cx.focus_handle(),
                settings_trigger: cx.focus_handle(),
                _settings_subscription: settings_subscription,
                panes: HashMap::new(),
                commands,
                stopping: worker.stop.clone(),
                cleanup_complete: worker.cleanup_complete.clone(),
                worker_finished: worker.finished.clone(),
                split_bounds: HashMap::new(),
                slot_bounds: HashMap::new(),
                tab_scrolls: HashMap::new(),
                drawer_scroll: ScrollHandle::new(),
                drop_target: None,
                drop_motion_generation: 0,
                drop_motion_duration: Duration::from_millis(70),
                drag: None,
                visible: Vec::new(),
                visible_screen: None,
                visible_owner: None,
                visible_route_generation: 0,
                search_owner: None,
                visible_pending: false,
                status: "Starting local workspace…".into(),
                appearance: NativeAppearance::fallback(&ui_family),
                appearance_error: None,
                appearance_generation: 0,
                drawer_visible: true,
                drawer_authoritative_pinned: true,
                drawer_width: 200,
                drawer_edge_reveal: true,
                drawer_overlay_open: false,
                drawer_edge_last: None,
                drawer_edge_task: None,
                drawer_collapsed: HashSet::new(),
                drawer_hovered: None,
                drawer_pane_hovered: None,
                drawer_drop: None,
                drawer_pending: None,
                wheel_navigation: WheelNavigation::default(),
                pending_zoom: HashSet::new(),
                zoom_feedback: None,
                profiles: None,
                profiles_error: None,
                profiles_generation: 0,
                profile_picker: None,
                template_dialog: None,
                pending_template_apply: None,
                tab_context_menu: None,
                terminal_menu: None,
                pending_transfer: None,
                float_order: HashMap::new(),
                float_viewport: None,
                screen_viewport: None,
                screen_motion: ScreenMotion::default(),
                float_gesture: None,
                pending_float: None,
                desired_scheme: initial_scheme,
                sent_scheme: initial_scheme,
                scheme_pending: false,
                _appearance_subscription: appearance_subscription,
                _activation_subscription: activation_subscription,
                _notification_visibility_subscription: notification_visibility_subscription,
                notifications: NotificationCenter::default(),
                notification_settings_generation: 0,
                notification_settings_error: None,
                notification_window_active: window.is_window_active(),
                notification_window_visible: window.is_visible(),
                _drawer_bounds_subscription: drawer_bounds_subscription,
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
        let Some(model) = self.local_model() else {
            return;
        };
        if self
            .commands
            .try_send(WorkerCommand::new(
                model.source_token(),
                Command::SetColorScheme {
                    connection: model.connection_id(),
                    scheme: self.desired_scheme,
                },
            ))
            .is_ok()
        {
            self.sent_scheme = self.desired_scheme;
            self.scheme_pending = false;
        }
    }
    fn apply_appearance(
        &mut self,
        snapshot: &Arc<AppearanceSnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .local_model()
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
        self.drawer_authoritative_pinned = snapshot.drawer().pinned;
        // A read already in flight can arrive after the click but before its
        // identified write result. Keep the submitted presentation until that
        // result settles; the local reader starts later reads after the write.
        if self.drawer_pending.is_none() {
            if self.drawer_visible != self.drawer_authoritative_pinned {
                self.wheel_navigation.reset();
            }
            self.drawer_visible = self.drawer_authoritative_pinned;
        }
        self.drawer_width = snapshot.drawer().sidebar_width.clamp(150, 400);
        self.drawer_drop = None;
        self.drawer_edge_reveal = snapshot.drawer().edge_reveal;
        if self.drawer_visible {
            self.drawer_overlay_open = false;
        }
        next.apply_component_theme(snapshot.scheme(), cx);
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
        self.screen_motion.clear();
        self.appearance = next;
        self.appearance_generation = snapshot.generation();
        self.appearance_error =
            failure.or_else(|| snapshot.terminal_palette_error().map(str::to_owned));
        self.settings.update(cx, |view, cx| {
            view.set_snapshot(snapshot.clone(), window, cx)
        });
        cx.emit(AppearanceSettled);
        cx.notify();
    }

    fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination == Destination::Settings
            || self.pending_close.is_some()
            || self.rename.is_some()
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.tab_context_menu.is_some()
            || self.pending_template_apply.is_some()
            || self.drag.is_some()
            || self.float_gesture.is_some()
            || cx.has_active_drag()
        {
            return;
        }
        if self.destination == Destination::Hosts {
            self.show_screens(window, cx);
            if self.destination == Destination::Hosts {
                return;
            }
        }
        self.destination = Destination::Settings;
        self.wheel_navigation.reset();
        self.sync_visible(cx);
        self.settings.update(cx, |view, cx| view.focus(window, cx));
        cx.notify();
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination != Destination::Settings {
            return;
        }
        self.destination = Destination::Screens;
        self.sync_visible(cx);
        if self.drawer_visible || self.drawer_overlay_open {
            self.settings_trigger.focus(window, cx);
        } else {
            self.focus_current(window, cx);
        }
        cx.notify();
    }

    fn present_drawer_pin(&mut self, pinned: bool) {
        self.drawer_visible = pinned;
        self.drawer_overlay_open = !pinned;
        self.wheel_navigation.reset();
    }

    fn toggle_drawer(&mut self, cx: &mut Context<Self>) {
        let Some(model) = self.local_model() else {
            return;
        };
        if self.drawer_pending.is_some() {
            return;
        }
        let connection = model.connection_id();
        let owner = model.source_token();
        let desired = !self.drawer_visible;
        let request_id = Uuid::new_v4();
        if self
            .commands
            .try_send(WorkerCommand::new(
                owner,
                Command::SetDrawerVisibility {
                    connection,
                    request_id,
                    pinned: desired,
                },
            ))
            .is_ok()
        {
            self.drawer_pending = Some(PendingDrawerPin {
                owner,
                request_id,
                desired,
            });
            self.present_drawer_pin(desired);
        } else {
            self.status = "Could not send Drawer visibility change".into();
        }
        cx.notify();
    }

    fn apply_drawer_visibility(&mut self, result: DrawerVisibilityResult, cx: &mut Context<Self>) {
        let Some(pending) = self.drawer_pending else {
            return;
        };
        if pending.owner.connection_id() != result.connection_id
            || pending.request_id != result.request_id
            || self.lifecycle_token() != Some(pending.owner)
        {
            return;
        }
        self.drawer_pending = None;
        match result.outcome {
            DrawerVisibilityOutcome::Confirmed(pinned) => {
                self.drawer_authoritative_pinned = pinned;
                self.present_drawer_pin(pinned);
            }
            DrawerVisibilityOutcome::Rejected { reason } => {
                self.present_drawer_pin(self.drawer_authoritative_pinned);
                self.status = format!("Drawer update rejected: {reason}")
            }
            DrawerVisibilityOutcome::Uncertain { reason } => {
                self.present_drawer_pin(self.drawer_authoritative_pinned);
                self.status = format!("Drawer update uncertain: {reason}")
            }
        }
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
        gpui_kit::component::theme::Theme::update(cx, |theme| {
            theme.font_size = px(16. * zoom.window_scale());
        });
        for view in self.panes.values() {
            view.terminal.update(cx, |terminal, cx| {
                terminal.set_zoom(zoom.terminal_scale(), cx)
            });
        }
        self.appearance.zoom = zoom;
        cx.notify();
    }

    fn submit_zoom(&mut self, action: ZoomAction, cx: &mut Context<Self>) {
        let Some(model) = self.local_model() else {
            self.status = "Zoom is unavailable while the workspace starts".into();
            cx.notify();
            return;
        };
        let request_id = Uuid::new_v4();
        match self.commands.try_send(WorkerCommand::new(
            model.source_token(),
            Command::Zoom {
                connection: model.connection_id(),
                request_id,
                action,
            },
        )) {
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
            .local_model()
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
        let Some(source) = self.selected_model().map(SourceWorkspace::source_token) else {
            return;
        };
        if self.execution_token(source.source_id()) != Some(source) {
            self.status =
                "Host is offline. Retry the connection before changing this Screen.".into();
            cx.notify();
            return;
        }
        self.send_captured(source, command, cx);
    }
    fn send_captured(&mut self, source: SourceToken, command: Command, cx: &mut Context<Self>) {
        if let Err(error) = self.commands.try_send(WorkerCommand::new(source, command)) {
            self.status = format!("Action not sent: {error}");
            cx.notify();
        }
    }
    fn create_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_screens(window, cx);
        if self.destination != Destination::Screens {
            return;
        }
        let Some(model) = self.local_model() else {
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
        self.send_captured(model.source_token(), Command::CreateScreen(name), cx);
    }
    fn switch_screen_key(
        &mut self,
        screen: ScreenKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.opened().iter().any(|entry| entry.key() == &screen))
        {
            return;
        }
        self.show_screens(window, cx);
        if self.destination != Destination::Screens {
            return;
        }
        self.float_gesture = None;
        self.tab_context_menu = None;
        if self.rename.is_some() {
            self.finish_rename(true, window, cx);
        }
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.opened().iter().any(|entry| entry.key() == &screen))
        {
            if let Some(owner) = self.lifecycle_token() {
                self.send_captured(owner, Command::SwitchScreen(screen), cx);
            }
        }
    }
    fn start_captured_rename(
        &mut self,
        owner: SourceToken,
        screen: &ScreenKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lifecycle_token() != Some(owner)
            || !self.local_model().is_some_and(|model| {
                model.source_token() == owner
                    && model.screen_key(screen.screen_id()) == *screen
                    && model.screen(screen.screen_id()).is_some()
            })
        {
            return;
        }
        self.start_rename(screen.screen_id(), window, cx);
    }

    fn start_rename(&mut self, screen: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.wheel_navigation.reset();
        self.tab_context_menu = None;
        self.close_profile_picker(window, cx);
        let Some((name, source, screen_key)) = self.local_model().and_then(|model| {
            model
                .screens()
                .iter()
                .find(|info| info.id.as_str() == screen)
                .map(|info| {
                    (
                        info.name.as_deref().unwrap_or("Screen").to_owned(),
                        model.source_token(),
                        model.screen_key(screen),
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
            screen: screen_key,
            source,
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
                && self.local_model().is_some_and(|model| {
                    model.source_token() == editor.source
                        && model.screen_key(editor.screen.screen_id()) == editor.screen
                        && model.screen(editor.screen.screen_id()).is_some()
                })
            {
                self.send_captured(
                    editor.source,
                    Command::RenameScreen {
                        screen: editor.screen.screen_id().to_owned(),
                        connection: editor.source.connection_id(),
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
        let source = if target == CloseTarget::Window {
            None
        } else {
            self.current_screen_key().map(|key| key.source_id())
        };
        self.request_close_from(target, source, window, cx);
    }
    fn request_close_from(
        &mut self,
        target: CloseTarget,
        source: Option<SourceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.wheel_navigation.reset();
        self.float_gesture = None;
        self.tab_context_menu = None;
        self.close_profile_picker(window, cx);
        self.dismiss_templates(window, cx);
        if target == CloseTarget::Window
            && (self.cleanup_complete.load(Ordering::Acquire)
                || self.worker_finished.load(Ordering::Acquire))
        {
            self.stop();
            cx.quit();
            return;
        }
        if self.pending_close.is_some() {
            return;
        }
        let request_id = self.next_close_id.saturating_add(1);
        let plan = match if target == CloseTarget::Window {
            self.local_model()
        } else {
            source.and_then(|source| self.model_for_source(source))
        } {
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
        let remote_terminate = matches!(&target, CloseTarget::Screen { .. })
            && source.is_some_and(|source| {
                self.model
                    .as_ref()
                    .is_some_and(|model| source != model.local_source().source_id())
            });
        if !plan.needs_confirmation() && !remote_terminate {
            self.submit_close(plan, false, cx);
            return;
        }
        let title = match &target {
            CloseTarget::Pane { .. } => "Close terminal pane?".to_owned(),
            CloseTarget::Screen { screen } if remote_terminate => {
                format!("Terminate Screen {}?", hosts::display(screen))
            }
            CloseTarget::Screen { .. } => "Close Screen?".to_owned(),
            CloseTarget::Window => "End this local workspace?".to_owned(),
        };
        let detail = if remote_terminate {
            let host = source
                .and_then(|source| self.model.as_ref()?.source_target(source))
                .unwrap_or("host");
            let sessions = plan
                .affected
                .iter()
                .filter_map(|affected| match &affected.identity {
                    splitty_client_core::workspace::CloseIdentity::Session { session, .. } => {
                        Some(session.as_str())
                    }
                    splitty_client_core::workspace::CloseIdentity::Pane {
                        session: Some(session),
                        ..
                    } => Some(session.as_str()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            let examples = sessions
                .iter()
                .take(3)
                .map(|session| hosts::display(session))
                .collect::<Vec<_>>();
            format!(
                "Ends {} sessions on {}. Running: {}. Unresolved: {}. {}",
                sessions.len(),
                hosts::display(host),
                plan.busy_count,
                plan.unresolved_count,
                if sessions.is_empty() {
                    "No session IDs were reported.".to_owned()
                } else {
                    format!("Sessions: {}", examples.join(", "))
                }
            )
        } else if target == CloseTarget::Window {
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
            &title,
            Some(&detail),
            if remote_terminate {
                &["Cancel", "Terminate"]
            } else {
                &["Keep open", "Close"]
            },
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = prompt.await.ok() == Some(1);
            let _ = this.update_in(cx, |this, window, cx| {
                let decision = this
                    .pending_close
                    .as_ref()
                    .filter(|pending| pending.is_prompt_for(request_id))
                    .map(|pending| {
                        pending.resolve_prompt(this.close_model(&pending.plan), confirmed)
                    });
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

    fn request_close_captured(
        &mut self,
        source: SourceToken,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .model_for_source(source.source_id())
            .is_some_and(|model| model.source_token() == source)
        {
            return;
        }
        let screen = match &target {
            CloseTarget::Pane { screen, .. } | CloseTarget::Screen { screen } => {
                Some(screen.as_str())
            }
            CloseTarget::Window => None,
        };
        if screen.is_some_and(|screen| {
            !self.owns_surface(
                source,
                &self
                    .model_for_source(source.source_id())
                    .expect("checked source")
                    .screen_key(screen),
            )
        }) {
            return;
        }
        self.request_close(target, window, cx);
    }
    fn submit_close(&mut self, plan: ClosePlan, confirmed: bool, cx: &mut Context<Self>) {
        match admit_close(&self.commands, plan, confirmed) {
            Ok(pending) => {
                self.revoke_close_links(&pending.plan, cx);
                self.pending_close = Some(pending);
            }
            Err(error) => {
                self.pending_close = None;
                self.status = error;
            }
        }
        cx.notify();
    }
    fn request_close_screen(
        &mut self,
        screen: ScreenKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.opened().iter().any(|entry| entry.key() == &screen))
        {
            return;
        }
        let source = screen.source_id();
        if self
            .model
            .as_ref()
            .is_some_and(|model| source != model.local_source().source_id())
        {
            if let Some(owner) = self.lifecycle_token() {
                match self.commands.try_send(WorkerCommand::new(
                    owner,
                    Command::DetachRemoteScreen {
                        screen: screen.clone(),
                    },
                )) {
                    Ok(()) => self.revoke_screen_links(&screen, cx),
                    Err(error) => {
                        self.status = format!("Detach was not sent: {error}");
                        cx.notify();
                    }
                }
            }
        } else {
            self.request_close_from(
                CloseTarget::Screen {
                    screen: screen.screen_id().to_owned(),
                },
                Some(source),
                window,
                cx,
            );
        }
    }
    fn request_terminate_screen(
        &mut self,
        captured: SourceToken,
        screen: ScreenKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.execution_token(screen.source_id()) != Some(captured) {
            return;
        }
        let source = screen.source_id();
        self.request_close_from(
            CloseTarget::Screen {
                screen: screen.screen_id().to_owned(),
            },
            Some(source),
            window,
            cx,
        );
    }
    fn ensure_pane(
        &mut self,
        route: TerminalToken,
        screen: ScreenKey,
        generation: u64,
        origin: terminal::LinkOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = route.pane_key();
        let capture = ResizeCapture {
            route: route.clone(),
            screen,
            route_generation: generation,
        };
        if let Some(view) = self.panes.get_mut(&key) {
            if view.route == route {
                if view.capture != capture {
                    let pending_resize = Arc::new(Mutex::new(None));
                    let commands = workspace_terminal_commands(
                        self.commands.clone(),
                        capture.clone(),
                        pending_resize.clone(),
                        self.smoke_terminal_inputs.clone(),
                        self.smoke_last_input_pane.clone(),
                    );
                    view.terminal.update(cx, |terminal, cx| {
                        terminal.replace_commands(commands);
                        terminal.invalidate_size_report(cx);
                    });
                    view.capture = capture;
                    view.pending_resize = pending_resize;
                }
                return;
            }
            view.terminal
                .update(cx, |terminal, _| terminal.cancel_pointer());
        }
        let pending_resize = Arc::new(Mutex::new(None));
        let commands = workspace_terminal_commands(
            self.commands.clone(),
            capture.clone(),
            pending_resize.clone(),
            self.smoke_terminal_inputs.clone(),
            self.smoke_last_input_pane.clone(),
        );
        let colors = self.appearance.terminal;
        let terminal_scale = self.appearance.zoom.terminal_scale();
        let selection_foreground = self.appearance.selection_foreground;
        let selection_background = self.appearance.selection_background;
        let terminal = cx.new(|cx| Terminal::new(commands, origin, cx));
        let link_route = route.clone();
        let link_subscription = cx.subscribe(
            &terminal,
            move |this, _, event: &terminal::LinkEvent, cx| {
                let terminal::LinkEvent::RemoteLoopback(url) = event;
                this.remote_link(link_route.clone(), url.clone(), cx);
            },
        );
        let menu_route = route.clone();
        let menu_subscription = cx.subscribe_in(
            &terminal,
            window,
            move |this, terminal, request: &terminal::ContextMenuRequest, window, cx| {
                this.open_terminal_menu(
                    menu_route.clone(),
                    terminal.clone(),
                    request.clone(),
                    window,
                    cx,
                );
            },
        );
        if let Err(error) = terminal.update(cx, |terminal, cx| {
            terminal.set_search_active(false, cx);
            terminal.set_zoom(terminal_scale, cx);
            terminal.apply_colors(colors, selection_foreground, selection_background, cx)
        }) {
            self.appearance_error = Some(error);
        }
        self.panes.insert(
            key,
            PaneView {
                route,
                capture,
                terminal,
                _link_subscription: link_subscription,
                _menu_subscription: menu_subscription,
                pending_resize,
                marker: false,
                tail: String::new(),
            },
        );
    }
    fn reconcile(
        &mut self,
        window_model: WindowWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notifications.reconcile(&window_model);
        self.prepare_screen_motion(&window_model, window, cx);
        self.ports_owner_changed(window_model.local_source().source_token(), window, cx);
        self.revoke_missing_terminal_links(&window_model);
        self.settings.update(cx, |view, cx| {
            view.set_owner(window_model.local_source().connection_id(), window, cx);
        });
        let model = window_model
            .shown_key()
            .and_then(|key| window_model.source(key.source_id()))
            .unwrap_or_else(|| window_model.local_source());
        let old_preview = self
            .local_model()
            .is_some_and(|old| old.pending_template().is_some());
        let new_preview = window_model.local_source().pending_template().is_some();
        let opened_keys: HashSet<_> = window_model
            .opened()
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        self.drawer_collapsed
            .retain(|key| opened_keys.contains(key));
        if self
            .drawer_hovered
            .as_ref()
            .is_some_and(|key| !opened_keys.contains(key))
        {
            self.drawer_hovered = None;
        }
        if self
            .drawer_pane_hovered
            .as_ref()
            .is_some_and(|(screen, pane)| {
                !opened_keys.contains(screen)
                    || window_model
                        .source(screen.source_id())
                        .and_then(|source| source.screen(screen.screen_id()))
                        .and_then(ScreenModel::snapshot)
                        .is_none_or(|snapshot| !snapshot.panes.keys().any(|id| id.as_str() == pane))
            })
        {
            self.drawer_pane_hovered = None;
        }
        if self
            .local_model()
            .is_some_and(|old| old.source_token() != window_model.local_source().source_token())
        {
            if self.hosts.has_open_form() {
                window.close_dialog(cx);
            }
            self.hosts
                .set_owner(window_model.local_source().connection_id());
            self.apply_zoom(Zoom::from_values(1.0, "window"), window, cx);
            self.appearance_generation = 0;
            self.notification_settings_generation = 0;
            self.notification_settings_error = None;
            self.drawer_visible = true;
            self.drawer_authoritative_pinned = true;
            self.drawer_width = 200;
            self.drawer_edge_reveal = true;
            self.drawer_overlay_open = false;
            self.drawer_edge_last = None;
            self.drawer_edge_task = None;
            self.drawer_collapsed.clear();
            self.drawer_hovered = None;
            self.drawer_pane_hovered = None;
            self.drawer_drop = None;
            self.drawer_pending = None;
            self.wheel_navigation.reset();
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
            self.tab_context_menu = None;
            self.pending_transfer = None;
            self.pending_float = None;
            self.float_order.clear();
            self.float_gesture = None;
            self.float_viewport = None;
            self.pending_close = None;
            self.drag = None;
            self.drop_target = None;
        }
        if self.model.is_none() {
            self.hosts
                .set_owner(window_model.local_source().connection_id());
        }
        if self.rename.is_some() {
            self.smoke_editor_reconciles += u64::from(self.smoke);
        }
        let old_screen = self.current_screen_key();
        let new_screen = window_model.shown_key();
        let catalog_changed = self.model.as_ref().is_none_or(|old| {
            old.opened()
                .iter()
                .map(|entry| entry.key().clone())
                .collect::<Vec<_>>()
                != window_model
                    .opened()
                    .iter()
                    .map(|entry| entry.key().clone())
                    .collect::<Vec<_>>()
        });
        let old_focus = self
            .selected_model()
            .and_then(SourceWorkspace::focused_pane)
            .map(str::to_owned);
        let old_entity = old_focus
            .as_deref()
            .and_then(|pane| {
                self.selected_model()
                    .and_then(|old| self.panes.get(&old.pane_key(pane)))
            })
            .map(|view| view.terminal.entity_id());
        if let (Some(root), Some(screen)) = (model.presentation_root(), model.shown_screen_id()) {
            let screen = model.screen_key(screen);
            let roots = std::iter::once(root)
                .chain(model.presented_floats().iter().map(|entry| &entry.root));
            let slots = prune_layout_bounds(
                &screen,
                roots,
                &mut self.split_bounds,
                &mut self.slot_bounds,
            );
            self.tab_scrolls.retain(|id, _| slots.contains(id));
            for slot in slots {
                self.tab_scrolls.entry(slot).or_default();
            }
            reveal_changed_tabs(self.selected_model(), model, &self.tab_scrolls);
        } else {
            self.split_bounds.clear();
            self.slot_bounds.clear();
            self.tab_scrolls.clear();
        }
        self.panes.retain(|pane, view| {
            let retained = *pane == view.route.pane_key()
                && window_model.contains_terminal_binding(&view.route);
            if !retained {
                view.terminal
                    .update(cx, |terminal, _| terminal.cancel_pointer());
            }
            retained
        });
        for route in window_model.routes() {
            let origin = if route.terminal().source_token().source_id()
                == window_model.local_source().source_id()
            {
                terminal::LinkOrigin::Local
            } else {
                terminal::LinkOrigin::Remote
            };
            self.ensure_pane(
                route.terminal().clone(),
                route.screen().clone(),
                route.generation(),
                origin,
                window,
                cx,
            );
        }
        let routes_changed = self.selected_model().is_none_or(|previous| {
            previous.connection_id() != model.connection_id()
                || previous.route_generation() != model.route_generation()
        });
        if routes_changed {
            let displayed: HashSet<_> = model
                .displayed_panes()
                .into_iter()
                .map(|pane| model.pane_key(&pane))
                .collect();
            for (pane, view) in &self.panes {
                if let Ok(mut pending) = view.pending_resize.lock() {
                    *pending = None;
                }
                if displayed.contains(pane) {
                    view.terminal.update(cx, |terminal, cx| {
                        terminal.invalidate_size_report(cx);
                    });
                }
            }
        }
        if model.snapshot().is_some() {
            self.status.clear();
        }
        if self.pending_close.as_ref().is_some_and(|pending| {
            pending.plan.target != CloseTarget::Window
                && pending.plan.source_token().is_some_and(|source| {
                    source.source_id() != window_model.local_source().source_id()
                        && window_model.execution_token(source.source_id()) != Some(source)
                })
        }) {
            self.pending_close = None;
            self.status = "Remote close decision expired after the connection changed".into();
        }
        let new_focus = model.focused_pane().map(str::to_owned);
        let new_entity = new_focus
            .as_deref()
            .and_then(|pane| self.panes.get(&model.pane_key(pane)))
            .map(|view| view.terminal.entity_id());
        let editor_cancelled = self.rename.as_ref().is_some_and(|editor| {
            let local = window_model.local_source();
            local.source_token() != editor.source
                || local.screen_key(editor.screen.screen_id()) != editor.screen
                || local.screen(editor.screen.screen_id()).is_none()
        });
        let picker_cancelled = self
            .profile_picker
            .as_ref()
            .is_some_and(|picker| !picker.valid_for(model));
        let transfer_cancelled = self
            .tab_context_menu
            .as_ref()
            .is_some_and(|menu| model.validate_pane_action(&menu.source).is_err());
        if picker_cancelled {
            self.profile_picker = None;
        }
        if transfer_cancelled {
            self.tab_context_menu = None;
        }
        if editor_cancelled {
            self.rename = None;
            self.rename_subscription = None;
        }
        if old_screen != new_screen || catalog_changed {
            self.wheel_navigation.screen_changed();
            let mut group = None;
            let mut row = 0;
            for entry in window_model.opened() {
                if group != Some(entry.key().source_id()) {
                    group = Some(entry.key().source_id());
                    if entry.key().source_id() != window_model.local_source().source_id() {
                        row += 1;
                    }
                }
                if new_screen.as_ref() == Some(entry.key()) {
                    self.drawer_scroll.scroll_to_item(row);
                    break;
                }
                row += 1;
            }
        }
        if old_screen != new_screen {
            self.drag = None;
            self.float_gesture = None;
            self.float_viewport = None;
        }
        if self
            .drag
            .as_mut()
            .is_some_and(|drag| !refresh_resize_drag(drag, model))
        {
            self.drag = None;
        }
        self.reconcile_float_order(&window_model, model);
        if self.float_gesture.as_ref().is_some_and(|gesture| {
            model.source_token() != gesture.source
                || model.shown_screen_id() != Some(gesture.screen.screen_id())
                || model.screen_key(gesture.screen.screen_id()) != gesture.screen
                || model.validate_float(&gesture.target).is_err()
        }) {
            self.float_gesture = None;
        }
        self.drop_target = None;
        self.drawer_drop = None;
        self.model = Some(window_model);
        self.reconcile_terminal_menu(window, cx);
        self.host_update_reconcile(window, cx);
        self.flush_scheme();
        self.sync_visible(cx);
        if self.destination == Destination::Screens
            && self.rename.is_none()
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
        if self.destination != Destination::Screens {
            self.screen_motion.clear();
            self.terminal_menu = None;
        }
        let mut visible = self.selected_model().map_or_else(Vec::new, |model| {
            model
                .displayed_panes()
                .into_iter()
                .map(|pane| model.pane_key(&pane))
                .collect()
        });
        if self.pending_template_apply.is_some()
            || self.destination != Destination::Screens
            || (!self.is_remote_selected()
                && self
                    .local_model()
                    .is_some_and(|model| model.pending_template().is_some()))
            || self
                .current_screen_key()
                .is_some_and(|key| self.execution_token(key.source_id()).is_none())
        {
            visible.clear();
        }
        visible.sort_by(|a, b| a.pane_id().cmp(b.pane_id()));
        let screen = self.current_screen_key();
        let owner = screen
            .as_ref()
            .and_then(|key| self.execution_token(key.source_id()));
        let generation = self
            .selected_model()
            .map(SourceWorkspace::route_generation)
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
        self.refresh_notification_watch(cx);
    }
    fn sync_search_owner(&mut self, cx: &mut Context<Self>) {
        let next = self
            .selected_model()
            .and_then(SourceWorkspace::focused_pane)
            .and_then(|pane| self.selected_model().map(|model| model.pane_key(pane)))
            .filter(|pane| {
                self.destination == Destination::Screens
                    && self.pending_template_apply.is_none()
                    && self.selected_model().is_some_and(|source| {
                        source.pending_template().is_none()
                            && source.terminal_token(pane.pane_id()).is_some_and(|route| {
                                self.model.as_ref().is_some_and(|model| {
                                    model.shown_key().is_some_and(|screen| {
                                        model.validate_terminal_focus(&screen, &route).is_ok()
                                    })
                                })
                            })
                    })
            })
            .and_then(|pane| {
                self.panes
                    .get(&pane)
                    .map(|view| (pane, view.terminal.clone()))
            });
        if self
            .search_owner
            .as_ref()
            .map(|(_, entity)| entity.entity_id())
            == next.as_ref().map(|(_, entity)| entity.entity_id())
        {
            return;
        }
        // Release the previous index before activating the next one. Pane
        // replacement can keep the same pane ID while changing its entity.
        if let Some((_, previous)) = self.search_owner.take() {
            previous.update(cx, |terminal, cx| terminal.set_search_active(false, cx));
        }
        if let Some((_, next)) = &next {
            next.update(cx, |terminal, cx| terminal.set_search_active(true, cx));
        }
        self.search_owner = next;
    }
    fn flush_visible(&mut self, cx: &mut Context<Self>) {
        if self.visible_pending {
            let Some(source) = self.visible_owner else {
                return;
            };
            match self.commands.try_send(WorkerCommand::new(
                source,
                Command::Visible {
                    screen: self.visible_screen.clone(),
                    route_generation: self.visible_route_generation,
                    panes: self
                        .visible
                        .iter()
                        .map(|pane| pane.pane_id().to_owned())
                        .collect(),
                },
            )) {
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
                retry_pending_resize(&self.commands, &view.capture, &view.pending_resize)
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
        if self.destination == Destination::Settings {
            self.settings.update(cx, |view, cx| view.focus(window, cx));
            return;
        }
        if self.destination == Destination::Hosts {
            self.focus.focus(window, cx);
            return;
        }
        if self.rename.is_some()
            || self.template_dialog.is_some()
            || self.profile_picker.is_some()
            || self.tab_context_menu.is_some()
            || self.terminal_menu.is_some()
            || self
                .pending_close
                .as_ref()
                .is_some_and(|pending| pending.prompting)
        {
            return;
        }
        if self
            .selected_model()
            .is_some_and(|model| model.pending_template().is_some())
        {
            self.focus.focus(window, cx);
            return;
        }
        if let Some(pane) = self
            .selected_model()
            .and_then(SourceWorkspace::focused_pane)
            .and_then(|pane| {
                self.selected_model()
                    .and_then(|model| self.panes.get(&model.pane_key(pane)))
            })
        {
            pane.terminal
                .update(cx, |terminal, cx| terminal.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }
    fn select(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        let screen = self.selected_model().and_then(|model| {
            model
                .shown_screen_id()
                .map(|screen| model.screen_key(screen))
        });
        if let Some(source) = screen.as_ref().map(ScreenKey::source_id) {
            if let Some(mut model) = self
                .model
                .as_mut()
                .and_then(|window| window.source_mut(source))
            {
                model.select_tab(pane);
            }
        }
        self.reveal_pane(pane);
        if let Some(screen) = screen {
            self.send(
                Command::Select {
                    screen,
                    pane: pane.to_owned(),
                },
                cx,
            );
        }
        self.sync_visible(cx);
        self.focus_current(window, cx);
        cx.notify();
    }

    fn select_captured(
        &mut self,
        source: SourceToken,
        screen: &ScreenKey,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selected_model().is_some_and(|model| {
            model.source_token() == source
                && model.shown_screen_id() == Some(screen.screen_id())
                && model.screen_key(screen.screen_id()) == *screen
                && model.screen_for(pane) == Some(screen.screen_id())
        }) {
            return;
        }
        self.select(pane, window, cx);
    }
    fn reveal_pane(&self, pane: &str) {
        let Some(model) = self.selected_model() else {
            return;
        };
        let Some(slot) = model.focused_slot_id() else {
            return;
        };
        let Some(LayoutNode::Slot { pane_ids, .. }) = presented_slot_by_id(model, slot) else {
            return;
        };
        if let (Some(handle), Some(index)) = (
            self.current_layout_key(slot)
                .and_then(|key| self.tab_scrolls.get(&key)),
            pane_ids.iter().position(|id| id.as_str() == pane),
        ) {
            handle.handle.scroll_to_item(index);
        }
    }
    fn reveal_selected_slot(&self, slot: &str) {
        let Some(model) = self.selected_model() else {
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
            self.current_layout_key(slot)
                .and_then(|key| self.tab_scrolls.get(&key)),
            pane_ids.iter().position(|pane| pane.as_str() == selected),
        ) {
            scroll.handle.scroll_to_item(index);
        }
    }
    fn focus(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some((screen, route)) = self.selected_model().and_then(|model| {
            Some((
                model.screen_key(model.shown_screen_id()?),
                model.terminal_token(pane)?,
            ))
        }) else {
            return;
        };
        let Some(model) = self.model.as_mut() else {
            return;
        };
        if model.validate_terminal_focus(&screen, &route).is_err() {
            return;
        }
        if model
            .source(screen.source_id())
            .and_then(|source| source.focused_pane())
            == Some(pane)
        {
            return;
        }
        // Admit the captured worker intent before optimistic presentation. The
        // same shared validation owns focus when later workspace updates arrive.
        if let Err(error) = self.commands.try_send(WorkerCommand::new(
            route.source_token(),
            Command::Focus {
                screen: screen.clone(),
                route: route.clone(),
            },
        )) {
            self.status = format!("Action not sent: {error}");
            cx.notify();
            return;
        }
        let _ = model.focus_terminal(&screen, &route);
        self.sync_visible(cx);
        cx.notify();
    }
    fn focus_captured(
        &mut self,
        source: SourceToken,
        screen: &ScreenKey,
        pane: &str,
        cx: &mut Context<Self>,
    ) {
        if self.selected_model().is_some_and(|model| {
            model.source_token() == source
                && model.shown_screen_id() == Some(screen.screen_id())
                && model.screen_key(screen.screen_id()) == *screen
                && model.screen_for(pane) == Some(screen.screen_id())
        }) {
            self.focus(pane, cx);
        }
    }
    fn focus_neighbor(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self.selected_model() else {
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
        let Some(screen) = self.current_screen_key() else {
            return;
        };
        visible_slots(shown, &screen, root, &self.slot_bounds, &mut slots);
        for entry in model.presented_floats() {
            visible_slots(shown, &screen, &entry.root, &self.slot_bounds, &mut slots);
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
        let Some(model) = self.selected_model() else {
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
        if self.terminal_menu.is_some() {
            // Kit owns the focused popup's keys; unmatched keys never become
            // workspace shortcuts while its captured action is pending.
            cx.stop_propagation();
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
        if self.template_dialog.is_some() {
            self.template_key(&event.keystroke, window, cx);
            return;
        }
        if self.pending_template_apply.is_some()
            || self
                .selected_model()
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
                if let Some(key) = self
                    .model
                    .as_ref()
                    .and_then(|model| model.opened().get(index).map(|entry| entry.key().clone()))
                {
                    self.switch_screen_key(key, window, cx);
                }
            }
            cx.stop_propagation();
            return;
        }
        if self.tab_context_menu.is_some() {
            self.tab_context_menu_key(&event.keystroke, window, cx);
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
            self.drawer_drop = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self
            .search_owner
            .as_ref()
            .is_some_and(|(_, terminal)| terminal.read(cx).search_input_focused(window, cx))
        {
            self.wheel_navigation.reset();
            return;
        }
        if GlobalState::is_in_deferred_context(cx) {
            return;
        }
        if self.destination == Destination::Settings {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                if self.drawer_overlay_open {
                    self.close_drawer_overlay(cx);
                    self.settings.update(cx, |view, cx| view.focus(window, cx));
                } else {
                    self.close_settings(window, cx);
                }
            } else if shortcut(&event.keystroke, cx.platform_info().is_mac())
                == Some(Shortcut::ToggleDrawer)
            {
                cx.stop_propagation();
                self.toggle_drawer_overlay(cx);
            }
            return;
        }
        if self.destination == Destination::Hosts {
            if self.host_alias_key(&event.keystroke, window, cx) {
                return;
            }
            if let Some(action) = HostKey::from_keystroke(&event.keystroke) {
                cx.stop_propagation();
                match action {
                    HostKey::Back => self.show_screens(window, cx),
                    HostKey::Refresh => self.refresh_hosts(cx),
                }
                return;
            }
            if let Some(action) = zoom_shortcut(&event.keystroke, cx.platform_info().is_mac()) {
                cx.stop_propagation();
                self.submit_zoom(action, cx);
                return;
            }
        }
        if shortcut(&event.keystroke, cx.platform_info().is_mac()) == Some(Shortcut::ToggleDrawer) {
            cx.stop_propagation();
            self.toggle_drawer_overlay(cx);
            return;
        }
        if event.keystroke.key == "escape"
            && self.drawer_overlay_open
            && self.destination != Destination::Hosts
        {
            cx.stop_propagation();
            self.close_drawer_overlay(cx);
            self.focus_current(window, cx);
            return;
        }
        if self.destination == Destination::Hosts {
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
            Shortcut::ToggleDrawer => self.toggle_drawer_overlay(cx),
            Shortcut::Templates => {
                self.wheel_navigation.reset();
                self.toggle_templates(window, cx)
            }
            Shortcut::NewTab => self.tab(cx),
            Shortcut::NewScreen => self.create_screen(window, cx),
            Shortcut::ScreenIndex(index) => {
                if let Some(key) = self
                    .model
                    .as_ref()
                    .and_then(|model| model.opened().get(index).map(|entry| entry.key().clone()))
                {
                    self.switch_screen_key(key, window, cx);
                }
            }
            Shortcut::ToggleMaximize if !self.is_remote_selected() => {
                self.toggle_maximize(window, cx)
            }
            Shortcut::ToggleMaximize => {}
            Shortcut::PreviousTab => self.cycle_tab(TabStep::Previous, cx),
            Shortcut::NextTab => self.cycle_tab(TabStep::Next, cx),
            Shortcut::SplitRight => self.split(Axis::Horizontal, cx),
            Shortcut::SplitDown => self.split(Axis::Vertical, cx),
            Shortcut::ClosePane => {
                if let Some(pane) = self
                    .selected_model()
                    .and_then(SourceWorkspace::focused_pane)
                    .map(str::to_owned)
                {
                    if let Some(screen) = self
                        .selected_model()
                        .and_then(SourceWorkspace::shown_screen_id)
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
            .selected_model()
            .and_then(SourceWorkspace::focused_pane)
            .map(str::to_owned)
        {
            if self
                .selected_model()
                .is_some_and(|model| !model.is_root_pane(&pane))
            {
                self.status = "Split is available for tiled groups".into();
                cx.notify();
                return;
            }
            if let Some(screen) = self.selected_model().and_then(|model| {
                model
                    .shown_screen_id()
                    .map(|screen| model.screen_key(screen))
            }) {
                self.send(Command::Split { screen, pane, axis }, cx);
            }
        }
    }
    fn toggle_maximize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(model) = self.selected_model() {
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
                match self.commands.try_send(WorkerCommand::new(
                    model.source_token(),
                    Command::SetMaximized {
                        screen: screen.to_owned(),
                        connection: model.connection_id(),
                        slot,
                        pane,
                    },
                )) {
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
            .current_screen_key()
            .is_some_and(|key| self.execution_token(key.source_id()).is_none())
        {
            return;
        }
        if self
            .selected_model()
            .and_then(SourceWorkspace::snapshot)
            .is_some_and(|screen| screen.panes.is_empty())
        {
            if let Some(model) = self.selected_model().filter(|_| !self.is_remote_selected()) {
                if let Some(screen) = model.shown_screen_id() {
                    self.send(
                        Command::NewTerminal {
                            screen: screen.to_owned(),
                            connection: model.connection_id(),
                        },
                        cx,
                    );
                }
            }
            return;
        }
        if let Some(pane) = self
            .selected_model()
            .and_then(SourceWorkspace::focused_pane)
            .map(str::to_owned)
        {
            if let Some(screen) = self.selected_model().and_then(|model| {
                model
                    .shown_screen_id()
                    .map(|screen| model.screen_key(screen))
            }) {
                self.send(Command::Tab { screen, pane }, cx);
            }
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
                Update::HostDirectory(update) => {
                    if self.hosts.accept(update) {
                        cx.notify();
                    }
                }
                Update::HostManagement(result) => {
                    self.host_management_result(result, window, cx);
                }
                Update::HostUpdatePrepared(plan) => {
                    self.host_update_prepared(plan, window, cx);
                }
                Update::HostUpdateRejected {
                    owner,
                    request_id,
                    reason,
                } => self.host_update_rejected(owner, request_id, reason, window, cx),
                Update::Forwards(snapshot) => {
                    if self.ports.accept(snapshot) {
                        cx.notify();
                    }
                }
                Update::Forward(result) => self.forward_result(result, window, cx),
                Update::TemplateApi(result) => self.template_result(result, window, cx),
                Update::TemplateApply(result) => self.template_apply_result(result, window, cx),
                Update::Appearance(snapshot) => self.apply_appearance(&snapshot, window, cx),
                Update::SettingsWrite(result) => {
                    if self
                        .local_model()
                        .is_some_and(|model| model.connection_id() == result.connection_id())
                    {
                        self.settings
                            .update(cx, |view, cx| view.apply_write_result(&result, window, cx));
                    }
                }
                Update::SettingsReconciled(result) => {
                    if self
                        .local_model()
                        .is_some_and(|model| model.connection_id() == result.connection_id())
                    {
                        self.settings
                            .update(cx, |view, cx| view.apply_reconciled(&result, cx));
                    }
                }
                Update::DrawerVisibility(result) => self.apply_drawer_visibility(result, cx),
                Update::Zoom(result) => self.apply_zoom_result(result, window, cx),
                Update::ScreenTransfer(result) => {
                    if self.pending_transfer.is_some_and(|pending| {
                        self.selected_model()
                            .is_some_and(|model| model.source_token() == pending.source)
                            && pending.matches(result.connection_id, result.request_id)
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
                        self.selected_model()
                            .is_some_and(|model| model.source_token() == pending.source)
                            && pending.source.connection_id() == result.connection_id
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
                        .local_model()
                        .is_some_and(|model| model.connection_id() == connection)
                    {
                        self.appearance_error = Some(reason);
                        cx.emit(AppearanceSettled);
                        cx.notify();
                    }
                }
                Update::Status(status) => {
                    self.status = status;
                    cx.notify();
                }
                Update::CloseSubmitted { request_id, source } => {
                    if !self.pending_close.as_ref().is_some_and(|pending| {
                        pending.matches_response(
                            request_id,
                            source,
                            self.close_model(&pending.plan),
                        )
                    }) {
                        self.status = "Close submission belonged to an earlier request".into();
                    }
                }
                Update::CloseRejected {
                    source,
                    request_id,
                    reason,
                } => {
                    if self.pending_close.as_ref().is_some_and(|pending| {
                        pending.matches_response(
                            request_id,
                            source,
                            self.close_model(&pending.plan),
                        )
                    }) {
                        self.pending_close = None;
                        self.status = reason;
                        self.focus_current(window, cx);
                        cx.notify();
                    }
                }
                Update::CloseSettled { source, request_id } => {
                    if self.pending_close.as_ref().is_some_and(|pending| {
                        pending.matches_response(
                            request_id,
                            source,
                            self.close_model(&pending.plan),
                        )
                    }) {
                        self.pending_close = None;
                        self.focus_current(window, cx);
                        cx.notify();
                    }
                }
                update @ Update::WindowCloseApproved { .. } => {
                    if self.pending_close.as_ref().is_some_and(|pending| {
                        pending.matches_window_approval(&update, self.close_model(&pending.plan))
                    }) {
                        self.stop();
                        cx.quit();
                    }
                }
                Update::Attached { source, session } => {
                    if self.execution_token(source.source_id()) != Some(source) {
                        continue;
                    }
                    let views = self
                        .panes
                        .iter()
                        .filter(|(_, view)| {
                            view.route.source_token() == source
                                && view.route.session_id() == session
                        })
                        .map(|(pane, view)| (view.terminal.clone(), self.visible.contains(pane)))
                        .collect::<Vec<_>>();
                    for (terminal_view, visible) in views {
                        terminal_view.update(cx, |terminal, cx| {
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
                Update::NotificationSettings { owner, read } => {
                    self.apply_notification_settings(owner, read, cx);
                }
                Update::Event { source, event } => {
                    self.observe_notification(source, &event, cx);
                    if event.connection_id != source.connection_id()
                        || self.execution_token(source.source_id()) != Some(source)
                    {
                        continue;
                    }
                    if let Some(output) = event.output() {
                        let route = self.model.as_ref().and_then(|model| {
                            model
                                .route_for_session(source, output.session_id)
                                .map(|route| {
                                    let origin = if route.terminal().source_token().source_id()
                                        == model.local_source().source_id()
                                    {
                                        terminal::LinkOrigin::Local
                                    } else {
                                        terminal::LinkOrigin::Remote
                                    };
                                    (route, origin)
                                })
                        });
                        bytes += output.bytes.len();
                        if let Some((route, origin)) = route {
                            // A known pending spawn can stream before its layout snapshot.
                            // Retain its emulator immediately rather than discarding replay/prompt bytes.
                            let pane = route.terminal().pane_key();
                            self.ensure_pane(
                                route.terminal().clone(),
                                route.screen().clone(),
                                route.generation(),
                                origin,
                                window,
                                cx,
                            );
                            if let Some(view) = self.panes.get_mut(&pane) {
                                if view.route != *route.terminal() {
                                    continue;
                                }
                                view.terminal.update(cx, |terminal, cx| {
                                    terminal.feed(output.bytes, output.replay, cx)
                                });
                                if self.smoke {
                                    view.tail.push_str(&String::from_utf8_lossy(output.bytes));
                                    view.marker |= view.tail.contains(&format!(
                                        "NATIVE_WORKSPACE_{}_OK",
                                        pane.pane_id()
                                    ));
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
        let Some(model) = self.model.as_ref().map(WindowWorkspace::local_source) else {
            return;
        };
        if self.smoke_stage == 17 && model.catalog_initialized() && model.screens().is_empty() {
            if !self.panes.is_empty() || !self.visible.is_empty() {
                self.finish_smoke(false, cx);
                return;
            }
            self.create_screen(window, cx);
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
                self.smoke_initial = self
                    .panes
                    .get(&model.pane_key(&first))
                    .map(|view| view.terminal.clone());
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
                let Some(screen) = model.shown_screen_id() else {
                    return;
                };
                // Attachment can finish while a shell still reads terminal
                // queries during startup. Wait for its observed idle state
                // before injecting commands into all four real terminals.
                if !model
                    .assess_close(
                        0,
                        CloseTarget::Screen {
                            screen: screen.to_owned(),
                        },
                    )
                    .is_ok_and(|close| !close.needs_confirmation())
                {
                    return;
                }
                self.smoke_tab = model.focused_pane().map(str::to_owned);
                let commands: Vec<_> = self
                    .panes
                    .iter()
                    .map(|(pane, view)| Command::Input {
                        route: view.route.clone(),
                        bytes: format!("printf 'NATIVE_WORKSPACE_%s_OK\\n' '{}'\r", pane.pane_id())
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
                                screen: model
                                    .screen_key(model.shown_screen_id().unwrap_or_default()),
                                revision: screen.revision,
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
                    .and_then(|pane| self.panes.get(&model.pane_key(pane)))
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
                let Some(view) = self.panes.get(&model.pane_key(first)) else {
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
                        route: view.route.clone(),
                        bytes: b"printf 'NATIVE_HIDDEN_SCREEN_OK\\n'\r".into(),
                    },
                    cx,
                );
                self.smoke_stage = 11;
            }
            11 if self
                .smoke_first
                .as_deref()
                .and_then(|first| self.panes.get(&model.pane_key(first)))
                .is_some_and(|view| view.tail.contains("NATIVE_HIDDEN_SCREEN_OK")) =>
            {
                self.smoke_hidden_output = true;
                if let Some(old) = self.smoke_screen.clone() {
                    self.send_captured(
                        model.source_token(),
                        Command::SwitchScreen(model.screen_key(&old)),
                        cx,
                    );
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
                    if let Some(screen) = self.current_screen_key() {
                        if let Some(route) = self
                            .selected_model()
                            .and_then(|model| model.terminal_token(&pane))
                        {
                            self.send_captured(
                                route.source_token(),
                                Command::Focus { screen, route },
                                cx,
                            );
                        }
                    }
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
                    self.send_captured(
                        model.source_token(),
                        Command::SwitchScreen(model.screen_key(&second)),
                        cx,
                    );
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
        if GlobalState::is_in_deferred_context(cx) {
            if self.drop_target.take().is_some() {
                cx.notify();
            }
            return;
        }
        let drag = event.drag(cx);
        let target = if drag.owner == cx.entity().entity_id() {
            self.drop_target_at(&drag.origin, event.event.position)
        } else {
            None
        };
        self.set_drop_target(target, cx);
    }

    fn set_drop_target(&mut self, target: Option<DropTarget>, cx: &mut Context<Self>) {
        if self.drop_target != target {
            let previous_body = match &self.drop_target {
                Some(DropTarget::Body { slot, .. }) => Some(slot),
                _ => None,
            };
            let next_body = match &target {
                Some(DropTarget::Body { slot, .. }) => Some(slot),
                _ => None,
            };
            if next_body.is_some() && previous_body.is_none() {
                self.drop_motion_generation = self.drop_motion_generation.wrapping_add(1);
            }
            self.drop_motion_duration = if previous_body
                .zip(next_body)
                .is_some_and(|(old, new)| old != new)
            {
                Duration::from_millis(300)
            } else {
                Duration::from_millis(70)
            };
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
        let model = self.selected_model()?;
        let root = model.presentation_root()?;
        let screen = self.current_screen_key()?;
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
            if slot.screen() != &screen {
                return None;
            }
            slot_by_id(root, slot.node_id())?;
            if !bounds.contains(&position) {
                return None;
            }
            if position.y < bounds.top() + self.tab_header_height() {
                let LayoutNode::Slot { pane_ids, .. } = slot_by_id(root, slot.node_id())? else {
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
                    .can_place_tab(drag, slot.node_id(), before.as_deref())
                    .then(|| DropTarget::Header {
                        slot: slot.clone(),
                        before,
                        marker_x: marker - bounds.left(),
                    })
            } else {
                let placement = body_drop_zone(*bounds, self.tab_header_height(), position)?;
                model
                    .check_move(drag, slot.node_id(), placement)
                    .is_ok()
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
        if drag.owner != cx.entity().entity_id() || GlobalState::is_in_deferred_context(cx) {
            return;
        }
        let intent = self.drop_target_at(&drag.origin, window.mouse_position());
        if let (Some(model), Some(intent)) = (self.selected_model(), intent) {
            match intent {
                DropTarget::Body { slot, placement } if slot.node_id() == target => {
                    match model.check_move(&drag.origin, slot.node_id(), placement) {
                        Ok(true) => self.send_captured(
                            drag.origin.source_token(),
                            Command::MovePane {
                                drag: drag.origin.clone(),
                                target: slot.node_id().to_owned(),
                                placement,
                            },
                            cx,
                        ),
                        Ok(false) => {}
                        Err(error) => self.status = error.to_string(),
                    }
                }
                DropTarget::Header { slot, before, .. } if slot.node_id() == target => {
                    match model.check_place_tab(&drag.origin, slot.node_id(), before.as_deref()) {
                        Ok(true) => self.send_captured(
                            drag.origin.source_token(),
                            Command::PlaceTab {
                                drag: drag.origin.clone(),
                                target: slot.node_id().to_owned(),
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
        source: SourceToken,
        node: LayoutKey,
        revision: u64,
        split: (Axis, usize, Vec<f64>),
        position: Point<Pixels>,
    ) {
        let (axis, index, sizes) = split;
        let Some(children) = self.selected_model().and_then(|model| {
            (model.source_token() == source)
                .then(|| resize_split(model, &node))
                .flatten()
                .and_then(
                    |(current_revision, current_axis, children, current_sizes)| {
                        (current_revision == revision
                            && current_axis == axis
                            && current_sizes == sizes)
                            .then_some(children)
                    },
                )
        }) else {
            return;
        };
        if let Some(bounds) = self.split_bounds.get(&node).copied() {
            self.drag = Some(ResizeDrag {
                node,
                source,
                revision,
                axis,
                index,
                start: position,
                last_position: position,
                bounds,
                children,
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
        if !self
            .model
            .as_ref()
            .and_then(|window| {
                window
                    .shown_key()
                    .and_then(|key| window.source(key.source_id()))
            })
            .is_some_and(|model| {
                model.source_token() == drag.source
                    && resize_split(model, &drag.node).is_some_and(
                        |(revision, axis, children, _)| {
                            revision == drag.revision
                                && axis == drag.axis
                                && children == drag.children
                        },
                    )
            })
        {
            self.drag = None;
            return;
        }
        drag.last_position = position;
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
                screen: drag.node.screen().clone(),
                revision: drag.revision,
                node: drag.node.node_id().to_owned(),
                sizes: drag.pending.clone(),
            };
            let source = drag.source;
            self.send_captured(source, command, cx);
        }
    }
    fn reconcile_float_order(&mut self, window: &WindowWorkspace, model: &SourceWorkspace) {
        let known_screens: HashSet<_> = window
            .opened()
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        self.float_order
            .retain(|screen, _| known_screens.contains(screen));
        let Some(screen) = model.shown_screen_id() else {
            return;
        };
        let canonical = model
            .snapshot()
            .map_or(&[][..], |snapshot| snapshot.floating.as_slice());
        let order = self
            .float_order
            .entry(model.screen_key(screen))
            .or_default();
        let ids: Vec<_> = canonical.iter().map(|entry| entry.id.to_string()).collect();
        floating::reconcile_order(order, &ids);
    }
    fn raise_float(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(screen) = self.selected_model().and_then(|model| {
            model
                .shown_screen_id()
                .map(|screen| model.screen_key(screen))
        }) else {
            return;
        };
        let order = self.float_order.entry(screen).or_default();
        if let Some(index) = order.iter().position(|item| item == id) {
            if index + 1 != order.len() {
                let id = order.remove(index);
                order.push(id);
                cx.notify();
            }
        }
    }
    fn float_at(&self, position: Point<Pixels>) -> Option<String> {
        let model = self.selected_model()?;
        let viewport = self.float_viewport?;
        let order = self
            .float_order
            .get(&model.screen_key(model.shown_screen_id()?))?;
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
        source: SourceToken,
        request_id: Uuid,
        command: Command,
        restore_focus: bool,
        cx: &mut Context<Self>,
    ) {
        if self.pending_float.is_some() {
            return;
        }
        match self.commands.try_send(WorkerCommand::new(source, command)) {
            Ok(()) => {
                self.pending_float = Some(PendingFloat {
                    source,
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
    fn dock_float(&mut self, id: &str, cx: &mut Context<Self>) {
        let target = self
            .selected_model()
            .and_then(|model| model.capture_float(id).ok());
        if let Some(target) = target {
            let request_id = Uuid::new_v4();
            self.submit_float_action(
                target.source_token(),
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
        let Some(model) = self.selected_model() else {
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
        match self.commands.try_send(WorkerCommand::new(
            model.source_token(),
            Command::SetMaximized {
                screen: screen.to_owned(),
                connection: model.connection_id(),
                slot: Some(slot),
                pane: Some(pane),
            },
        )) {
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
        if GlobalState::is_in_deferred_context(cx) {
            return;
        }
        let Some(viewport) = self.float_viewport else {
            return;
        };
        let Some(model) = self.selected_model() else {
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
            source: model.source_token(),
            screen: model.screen_key(model.shown_screen_id().unwrap_or_default()),
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
                .selected_model()
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
            gesture.source,
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
    fn render_drop_highlight(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (Some(DropTarget::Body { slot, placement }), Some(viewport)) =
            (&self.drop_target, self.float_viewport)
        else {
            return None;
        };
        let bounds = self.slot_bounds.get(slot)?;
        let local = drop_highlight(*bounds, *placement);
        let target = Bounds {
            origin: point(
                bounds.origin.x + local.origin.x - viewport.origin.x,
                bounds.origin.y + local.origin.y - viewport.origin.y,
            ),
            size: local.size,
        };
        let bounds = transition(
            format!("drop-highlight-{}", self.drop_motion_generation),
            target,
            Transition::new(self.drop_motion_duration).easing(Easing::EaseOut),
            window,
            cx,
        );
        Some(
            div()
                .id("workspace-drop-highlight")
                .when(cfg!(all(test, feature = "ui-test-support")), |highlight| {
                    highlight.debug_selector(|| "workspace-drop-highlight".into())
                })
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .border_2()
                .border_color(rgb(self.appearance.accent))
                .bg(rgba((self.appearance.accent << 8) | 0x44))
                .into_any_element(),
        )
    }

    fn render_node(
        &self,
        node: &LayoutNode,
        surface: Surface,
        projection: &ScreenProjection<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let interactive = projection.is_interactive();
        match node {
            LayoutNode::Slot { id, pane_ids, .. } => {
                if let Some(slot) = self.render_presented_slot(node, surface, projection, cx) {
                    return slot;
                }
                let slot_key = projection.key().layout_key(id.as_str());
                let selected = self
                    .selected_model()
                    .and_then(|model| model.selected_pane(node));
                let scroll = self.tab_scrolls.get(&slot_key).cloned().unwrap_or_default();
                let tracked = scroll.clone();
                let weak = cx.entity().downgrade();
                let slot_for_width = slot_key.clone();
                let identity = self.selected_model().map(SourceWorkspace::source_token);
                let remote = self.is_remote_selected();
                let ready = identity
                    .is_some_and(|source| self.execution_token(source.source_id()) == Some(source));
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
                        let Some(owner) = identity else {
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
                                let same_screen = this.selected_model().is_some_and(|model| {
                                    model.source_token() == owner
                                        && model.shown_screen_id()
                                            == Some(slot.screen().screen_id())
                                        && model.screen_key(slot.screen().screen_id())
                                            == *slot.screen()
                                });
                                if same_handle && same_screen {
                                    this.reveal_selected_slot(slot.node_id());
                                    cx.notify();
                                }
                            });
                        });
                    })
                    .id(SharedString::from(format!("tab-strip-{slot_key}")))
                    .flex()
                    .flex_initial()
                    .min_w_0()
                    .h(chrome_rems((self.appearance.font_size + 19.).max(32.)))
                    .gap_1()
                    .bg(rgb(self.appearance.raised))
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .track_scroll(&scroll.handle)
                    .on_scroll({
                        let slot = id.to_string();
                        let screen = self
                            .selected_model()
                            .and_then(SourceWorkspace::shown_screen_id)
                            .unwrap_or_default()
                            .to_owned();
                        let source = identity;
                        cx.listener(move |this, event: &ScrollEvent, window, cx| {
                            if let Some(source) = source {
                                this.chrome_wheel(
                                    source,
                                    WheelTarget::Slot {
                                        screen: screen.clone(),
                                        slot: slot.clone(),
                                    },
                                    event,
                                    window,
                                    cx,
                                );
                            }
                        })
                    });
                for (index, pane) in pane_ids.iter().enumerate() {
                    let Some(pane_key) = self
                        .selected_model()
                        .map(|model| model.pane_key(pane.as_str()))
                    else {
                        continue;
                    };
                    let key = pane.to_string();
                    let close = key.clone();
                    let middle_close = key.clone();
                    let right_click = key.clone();
                    let close_weak = cx.entity().downgrade();
                    let tab_source = identity;
                    let tab_screen = slot_key.screen().clone();
                    let right_screen = slot_key.screen().clone();
                    let close_source = identity;
                    let close_screen = slot_key.screen().clone();
                    let middle_source = identity;
                    let middle_screen = slot_key.screen().clone();
                    let active = selected == Some(pane.as_str());
                    let title = self
                        .selected_model()
                        .and_then(SourceWorkspace::snapshot)
                        .and_then(|screen| screen.panes.get(pane))
                        .and_then(|pane| pane.title.clone())
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| format!("Terminal {}", index + 1));
                    let drag_title = title.clone();
                    let drag_appearance = self.appearance.clone();
                    tabs = tabs.child(
                        screen_motion::tab_surface(&self.appearance, active)
                            .id(SharedString::from(format!("tab-{slot_key}-{pane_key}")))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if ready && !GlobalState::is_in_deferred_context(cx) {
                                    if let Some(source) = tab_source {
                                        this.select_captured(source, &tab_screen, &key, window, cx);
                                    }
                                }
                            }))
                            .on_aux_click(cx.listener(
                                move |this, event: &ClickEvent, window, cx| {
                                    if !matches!(event, ClickEvent::Mouse(click) if click.up.button == MouseButton::Middle)
                                    {
                                        return;
                                    }
                                    cx.stop_propagation();
                                    if ready && !GlobalState::is_in_deferred_context(cx) {
                                        if let Some(source) = middle_source {
                                            this.request_close_captured(
                                                source,
                                                CloseTarget::Pane {
                                                    screen: middle_screen.screen_id().to_owned(),
                                                    pane: middle_close.clone(),
                                                },
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                },
                            ))
                            .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        if let Some(source) = tab_source {
                                            this.open_tab_context_menu_captured(
                                                source,
                                                &right_screen,
                                                &right_click,
                                                window,
                                                cx,
                                            );
                                        }
                                    }),
                                )
                            .when_some(
                                (!remote
                                    && ready
                                    && surface == Surface::Tiled
                                    && !GlobalState::is_in_deferred_context(cx))
                                .then(|| {
                                    self.selected_model()
                                        .and_then(|model| model.capture_drag(pane.as_str()))
                                })
                                .flatten(),
                                |view, origin| {
                                    view.on_drag(
                                        TabDrag {
                                            origin,
                                            owner: cx.entity().entity_id(),
                                        },
                                        move |_, _, _, cx| {
                                            cx.new(|_| DragPreview {
                                                title: drag_title.clone(),
                                                active,
                                                appearance: drag_appearance.clone(),
                                            })
                                        },
                                    )
                                },
                            )
                            .child(Icon::new(IconName::SquareTerminal).xsmall())
                            .child(
                                screen_motion::tab_label(title.clone())
                                    .id(SharedString::from(format!("drag-tab-{slot_key}-{pane_key}")))
                                    .tooltip({
                                        let title = title.clone();
                                        move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                title.clone(),
                                            )
                                            .build(window, cx)
                                        }
                                    })
                                    .when(cfg!(all(test, feature = "ui-test-support")), |label| {
                                        label.debug_selector(|| {
                                            format!("tab-title-{}-{title}", pane.as_str())
                                        })
                                    }),
                            )
                            .child(self.render_tab_context_menu(pane.as_str(), cx))
                            .child(
                                Button::new(SharedString::from(format!(
                                    "close-{slot_key}-{pane_key}"
                                )))
                                .accessibility_label(format!("Close {title}"))
                                .tooltip(format!("Close {title}"))
                                .icon(Icon::new(IconName::X))
                                .ghost()
                                .compact()
                                .xsmall()
                                .disabled(!ready)
                                .on_click(
                                    move |_, window, cx| {
                                        cx.stop_propagation();
                                        let _ = close_weak.update(cx, |this, cx| {
                                            if let Some(source) = close_source {
                                                this.request_close_captured(
                                                    source,
                                                    CloseTarget::Pane {
                                                        screen: close_screen.screen_id().to_owned(),
                                                        pane: close.clone(),
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            }
                                        });
                                    },
                                ),
                            ),
                    );
                }
                let header = div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .h(chrome_rems((self.appearance.font_size + 19.).max(32.)))
                    .bg(rgb(self.appearance.raised))
                    .child(tabs)
                    .child(self.render_group_new_tab_actions(id.as_str(), selected, window, cx))
                    .child(div().flex_1().min_w_0())
                    .child(self.render_group_actions(id.as_str(), selected, surface, cx));
                let focused = self
                    .selected_model()
                    .and_then(SourceWorkspace::focused_pane)
                    .is_some_and(|pane| pane_ids.iter().any(|id| id.as_str() == pane));
                let bounds_id = slot_key.clone();
                let bounds_source = identity;
                let target = id.to_string();
                let entity = cx.entity();
                let chrome_scale = self.appearance.zoom.window_scale();
                let mut slot = div()
                    .id(SharedString::from(format!("slot-{slot_key}")))
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
                                entity.update(cx, |this, cx| {
                                    if !this.selected_model().is_some_and(|model| {
                                        Some(model.source_token()) == bounds_source
                                            && model.shown_screen_id()
                                                == Some(bounds_id.screen().screen_id())
                                            && model.screen_key(bounds_id.screen().screen_id())
                                                == *bounds_id.screen()
                                    }) {
                                        return;
                                    }
                                    let previous =
                                        this.slot_bounds.insert(bounds_id.clone(), bounds);
                                    if chrome::compact_group_actions(
                                        previous.map(|bounds| bounds.size.width),
                                        chrome_scale,
                                    ) != chrome::compact_group_actions(
                                        Some(bounds.size.width),
                                        chrome_scale,
                                    ) {
                                        cx.notify();
                                    }
                                })
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(header);
                if let Some(pane) = selected {
                    if let Some(view) = self
                        .selected_model()
                        .and_then(|model| self.panes.get(&model.pane_key(pane)))
                    {
                        let focus = pane.to_owned();
                        let focus_source = identity;
                        let focus_screen = slot_key.screen().clone();
                        slot = slot.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .overflow_hidden()
                                .capture_any_mouse_down(cx.listener(
                                    move |this, event: &MouseDownEvent, _, cx| {
                                        if event.button == MouseButton::Left {
                                            if let Some(source) = focus_source {
                                                this.focus_captured(
                                                    source,
                                                    &focus_screen,
                                                    &focus,
                                                    cx,
                                                );
                                            }
                                        }
                                    },
                                ))
                                .child(view.terminal.clone()),
                        );
                    } else {
                        let message = self
                            .selected_model()
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
                    Some(DropTarget::Header {
                        slot: target,
                        marker_x,
                        ..
                    }) if target == &slot_key => {
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
                let split_key = projection.key().layout_key(id.as_str());
                let horizontal = *axis == Axis::Horizontal;
                let bounds_id = split_key.clone();
                let bounds_source = Some(projection.owner());
                let entity = cx.entity();
                let mut split = div()
                    .id(SharedString::from(format!("split-{split_key}")))
                    .relative()
                    .size_full()
                    .flex()
                    .min_w_0()
                    .min_h_0()
                    .when(!horizontal, |view| view.flex_col())
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                if !interactive {
                                    return;
                                }
                                entity.update(cx, |this, _| {
                                    if !this.selected_model().is_some_and(|model| {
                                        Some(model.source_token()) == bounds_source
                                            && model.shown_screen_id()
                                                == Some(bounds_id.screen().screen_id())
                                            && model.screen_key(bounds_id.screen().screen_id())
                                                == *bounds_id.screen()
                                    }) {
                                        return;
                                    }
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
                            .child(self.render_node(child, surface, projection, window, cx)),
                    );
                    if index + 1 < children.len() {
                        let node = split_key.clone();
                        let source = self.selected_model().map(SourceWorkspace::source_token);
                        let revision = self
                            .selected_model()
                            .and_then(SourceWorkspace::snapshot)
                            .map(|screen| screen.revision);
                        let axis = axis.clone();
                        let sizes = sizes.clone();
                        split = split.child(
                            div()
                                .id(SharedString::from(format!("divider-{split_key}-{index}")))
                                .flex_none()
                                .bg(rgb(self.appearance.border))
                                .when(horizontal, |view| view.w(chrome_rems(5.)).h_full())
                                .when(!horizontal, |view| view.h(chrome_rems(5.)).w_full())
                                .when(interactive, |view| {
                                    view.cursor(if horizontal {
                                        CursorStyle::ResizeColumn
                                    } else {
                                        CursorStyle::ResizeRow
                                    })
                                })
                                .when(interactive && surface == Surface::Tiled, |divider| {
                                    divider.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            if GlobalState::is_in_deferred_context(cx) {
                                                return;
                                            }
                                            if let (Some(source), Some(revision)) =
                                                (source, revision)
                                            {
                                                this.begin_resize(
                                                    source,
                                                    node.clone(),
                                                    revision,
                                                    (axis.clone(), index, sizes.clone()),
                                                    event.position,
                                                );
                                            }
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
        projection: &ScreenProjection<'_>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(group) = self.render_presented_float(entry, projection, window, cx) {
            return group;
        }
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
        let float_owner = self.selected_model().and_then(|model| {
            Some((
                model.source_token(),
                model.screen_key(model.shown_screen_id()?),
            ))
        });
        let raise_owner = float_owner.clone();
        let move_owner = float_owner.clone();
        let dock_owner = float_owner.clone();
        let maximize_owner = float_owner.clone();
        let resize_owner = float_owner;
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
                cx.listener(move |this, _, _, cx| {
                    if raise_owner
                        .as_ref()
                        .is_some_and(|(source, screen)| this.owns_surface(*source, screen))
                    {
                        this.raise_float(&raise_id, cx);
                    }
                }),
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
                            if move_owner
                                .as_ref()
                                .is_some_and(|(source, screen)| this.owns_surface(*source, screen))
                            {
                                this.begin_float_gesture(
                                    &move_id,
                                    FloatGestureKind::Move,
                                    event.position,
                                    cx,
                                );
                            }
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
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .child(
                                Button::new(SharedString::from(format!("dock-button-{id}")))
                                    .accessibility_label("Dock this floating group")
                                    .tooltip("Dock this floating group")
                                    .icon(Icon::new(IconName::PanelsTopLeft))
                                    .ghost()
                                    .compact()
                                    .small()
                                    .on_click({
                                        let weak = cx.entity().downgrade();
                                        move |_, _, cx| {
                                            cx.stop_propagation();
                                            let _ = weak.update(cx, |this, cx| {
                                                if dock_owner.as_ref().is_some_and(
                                                    |(source, screen)| {
                                                        this.owns_surface(*source, screen)
                                                    },
                                                ) {
                                                    this.dock_float(&dock_id, cx);
                                                }
                                            });
                                        }
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("float-maximize-{id}")))
                            .flex_none()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "float-maximize-button-{id}"
                                )))
                                .accessibility_label("Maximize this floating group")
                                .tooltip("Maximize this floating group")
                                .icon(Icon::new(IconName::Maximize))
                                .ghost()
                                .compact()
                                .small()
                                .on_click({
                                    let weak = cx.entity().downgrade();
                                    move |_, window, cx| {
                                        cx.stop_propagation();
                                        let _ = weak.update(cx, |this, cx| {
                                            if maximize_owner.as_ref().is_some_and(
                                                |(source, screen)| {
                                                    this.owns_surface(*source, screen)
                                                },
                                            ) {
                                                this.maximize_float(&maximize_id, window, cx);
                                            }
                                        });
                                    }
                                }),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(self.render_node(
                &entry.root,
                Surface::Floating,
                projection,
                window,
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
                        if resize_owner
                            .as_ref()
                            .is_some_and(|(source, screen)| this.owns_surface(*source, screen))
                        {
                            this.begin_float_gesture(
                                &resize_id,
                                FloatGestureKind::Resize,
                                event.position,
                                cx,
                            );
                        }
                        cx.stop_propagation();
                    }),
                ),
        );
        group.into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sample_screen_motion(window, cx);
        let mut messages = Vec::new();
        if !self.status.is_empty() {
            messages.push(self.status.clone());
        }
        if self.destination == Destination::Screens {
            if let Some(notice) = &self.ports.link_notice {
                messages.push(notice.clone());
            }
        }
        if let Some((message, _)) = &self.zoom_feedback {
            messages.push(message.clone());
        }
        if let Some(error) = &self.appearance_error {
            messages.push(format!("Appearance: {error}"));
        } else if let Some(notice) = self.appearance.notice {
            messages.push(notice.to_owned());
        }
        if let Some(error) = &self.profiles_error {
            messages.push(format!("Profiles: {error}"));
        }
        if let Some(error) = &self.notification_settings_error {
            messages.push(format!("Notifications: {error}"));
        }
        let drawer_fraction = transition(
            "drawer-visibility",
            if self.drawer_visible || self.drawer_overlay_open {
                1.
            } else {
                0.
            },
            drawer_transition(),
            window,
            cx,
        );
        let reserved_width = transition(
            "drawer-reserved-width",
            if self.drawer_visible {
                self.drawer_width as f32
            } else {
                0.
            },
            drawer_transition(),
            window,
            cx,
        );
        let template_overlay = self.template_overlay(window, cx);
        let toolbar_source = self.lifecycle_token();
        let view = div()
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
            .capture_any_mouse_down(cx.listener(Self::preserve_screen_motion_focus))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.drawer_edge_hover(event, cx);
                if this.drag.is_some() && event.dragging() {
                    this.drag_resize(event.position, false, cx);
                }
            }))
            .on_mouse_exit(cx.listener(|this, _: &MouseExitEvent, _, _| {
                this.drawer_edge_last = None;
                this.drawer_edge_task = None;
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
            );
        let mut content = div().flex().flex_1().min_h_0().min_w_0();
        let mut main_content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .ml(chrome_rems(reserved_width));
        if self.destination == Destination::Settings {
            main_content = main_content.child(self.settings.clone());
        } else if self.destination == Destination::Hosts {
            main_content = main_content.child(self.render_hosts(cx));
        } else if let Some(model) = self.selected_model() {
            if self.is_remote_selected() {
                if let Some(key) = self.current_screen_key() {
                    if let Some(entry) = self.model.as_ref().and_then(|model| {
                        model.opened().into_iter().find(|entry| entry.key() == &key)
                    }) {
                        let message = match entry.state() {
                            splitty_client_core::workspace::OpenedScreenState::Ready => None,
                            splitty_client_core::workspace::OpenedScreenState::Loading => {
                                Some("Opening remote Screen…")
                            }
                            splitty_client_core::workspace::OpenedScreenState::Missing => {
                                Some("This Screen is missing on its host. Detach it from Screens.")
                            }
                            splitty_client_core::workspace::OpenedScreenState::Offline => Some(
                                "Host offline. Terminal input is unavailable until reconnection.",
                            ),
                        };
                        if let Some(message) = message {
                            main_content = main_content.child(
                                div()
                                    .px_4()
                                    .py_2()
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().foreground)
                                    .child(message),
                            );
                        }
                    }
                }
            }
            if let Some(preview) = self
                .selected_model()
                .filter(|_| !self.is_remote_selected())
                .and_then(SourceWorkspace::pending_template)
            {
                main_content = main_content.child(self.render_template_preview(preview, cx));
            } else if model.screens().is_empty() {
                if self.is_remote_selected() {
                    main_content = main_content.child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("Remote Screen content unavailable"),
                    );
                } else {
                    main_content = main_content.child(
                        div().flex_1().flex().items_center().justify_center().child(
                            div()
                                .id("new-screen-empty")
                                .px_4()
                                .py_2()
                                .cursor_pointer()
                                .bg(rgb(self.appearance.overlay))
                                .child("New screen")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if toolbar_source.is_some_and(|source| {
                                        this.lifecycle_token() == Some(source)
                                            && this
                                                .local_model()
                                                .is_some_and(|model| model.screens().is_empty())
                                    }) {
                                        this.create_screen(window, cx);
                                    }
                                })),
                        ),
                    );
                }
            } else if let Some(screen) = model.snapshot() {
                if screen.panes.is_empty() && !self.is_remote_selected() {
                    main_content = main_content.child(
                        div().flex_1().flex().items_center().justify_center().child(
                            div()
                                .id("new-terminal-empty")
                                .px_4()
                                .py_2()
                                .cursor_pointer()
                                .bg(rgb(self.appearance.overlay))
                                .child("New terminal")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if toolbar_source.is_some_and(|source| {
                                        this.lifecycle_token() == Some(source)
                                            && this.local_model().is_some_and(|model| {
                                                model
                                                    .snapshot()
                                                    .is_some_and(|screen| screen.panes.is_empty())
                                            })
                                    }) {
                                        this.tab(cx);
                                    }
                                })),
                        ),
                    );
                } else if screen.panes.is_empty() {
                    main_content = main_content.child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("No terminals in this Screen"),
                    );
                } else {
                    let entity = cx.entity();
                    let weak_move = entity.downgrade();
                    let weak_up = entity.downgrade();
                    let weak_down = entity.downgrade();
                    let mut viewport = div()
                        .id("workspace-viewport")
                        .when(cfg!(all(test, feature = "ui-test-support")), |viewport| {
                            viewport.debug_selector(|| "workspace-viewport".into())
                        })
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .p_1()
                        .pt_0()
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
                    let projection = ScreenProjection::Interactive(model);
                    viewport = viewport.child(self.render_node(
                        root,
                        root_surface,
                        &projection,
                        window,
                        cx,
                    ));
                    let mut floats: Vec<_> = model.presented_floats().iter().collect();
                    if let Some(order) = model
                        .shown_screen_id()
                        .and_then(|screen| self.float_order.get(&model.screen_key(screen)))
                    {
                        floats.sort_by_key(|entry| {
                            order
                                .iter()
                                .position(|id| id == entry.id.as_str())
                                .unwrap_or(usize::MAX)
                        });
                    }
                    for entry in floats {
                        viewport =
                            viewport.child(self.render_float(entry, &projection, window, cx));
                    }
                    if let Some(highlight) = self.render_drop_highlight(window, cx) {
                        viewport = viewport.child(highlight);
                    }
                    let entity = cx.entity();
                    let mut layers = div()
                        .id("screen-viewport")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .child(
                            canvas(
                                move |bounds, _, cx| {
                                    entity.update(cx, |this, cx| {
                                        if this.screen_viewport != Some(bounds) {
                                            this.screen_viewport = Some(bounds);
                                            cx.notify();
                                        }
                                    });
                                },
                                |_, (), _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        );
                    if let Some(outgoing) = self.render_outgoing_screen(window, cx) {
                        layers = layers.child(outgoing);
                    }
                    main_content = main_content.child(
                        layers.child(
                            viewport
                                .absolute()
                                .left_0()
                                .top(self.screen_offset())
                                .w_full()
                                .h_full(),
                        ),
                    );
                }
            } else {
                main_content =
                    main_content.child(div().flex_1().p_4().child(if self.is_remote_selected() {
                        "Loading remote Screen…"
                    } else {
                        "Loading local Screen…"
                    }));
            }
        } else {
            main_content = main_content.child(div().flex_1().p_4().child("Connecting…"));
        }
        content = content.child(main_content);
        view.child(content)
            .when(!messages.is_empty(), |view| {
                view.child(
                    div()
                        .id("workspace-status")
                        .px_3()
                        .py_1()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .text_size(chrome_rems((self.appearance.font_size - 2.).max(11.)))
                        .text_color(rgb(self.appearance.secondary))
                        .children(messages.into_iter().map(|message| {
                            div()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(message)
                        })),
                )
            })
            .when(drawer_fraction > 0. || reserved_width > 0., |view| {
                let owner = self.lifecycle_token();
                let shade =
                    (drawer_fraction * (1. - reserved_width / self.drawer_width as f32) * 68.)
                        .round()
                        .clamp(0., 68.) as u32;
                view.when(shade > 0, |view| {
                    view.child(
                        div()
                            .id("drawer-backdrop")
                            .absolute()
                            .inset_0()
                            .bg(rgba((self.appearance.overlay << 8) | shade))
                            .occlude()
                            .on_scroll(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.lifecycle_token() == owner
                                    && !GlobalState::is_in_deferred_context(cx)
                                {
                                    this.close_drawer_overlay(cx);
                                }
                            })),
                    )
                })
                .child(
                    div()
                        .id("drawer-slide")
                        .when(cfg!(all(test, feature = "ui-test-support")), |drawer| {
                            drawer.debug_selector(|| "drawer-slide".into())
                        })
                        .absolute()
                        .top_0()
                        .left(chrome_rems(
                            (drawer_fraction - 1.) * self.drawer_width as f32,
                        ))
                        .h_full()
                        .occlude()
                        .child(self.render_drawer(window, cx)),
                )
            })
            .when_some(template_overlay, |view, overlay| view.child(overlay))
            .children(self.render_terminal_menu(window))
    }
}
fn first_selected_slot(model: &SourceWorkspace, node: &LayoutNode) -> Option<(String, String)> {
    match node {
        LayoutNode::Slot { id, .. } => {
            Some((id.to_string(), model.selected_pane(node)?.to_owned()))
        }
        LayoutNode::Split { children, .. } => children
            .iter()
            .find_map(|child| first_selected_slot(model, child)),
    }
}
fn presented_slot_by_id<'a>(model: &'a SourceWorkspace, id: &str) -> Option<&'a LayoutNode> {
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
    screen: &ScreenKey,
    node: &LayoutNode,
    bounds: &HashMap<LayoutKey, Bounds<Pixels>>,
    out: &mut Vec<VisibleSlot>,
) {
    match node {
        LayoutNode::Slot { id, .. } => {
            if let (Some(pane), Some(bounds)) = (
                model.selected_pane(node),
                bounds.get(&screen.layout_key(id.as_str())).copied(),
            ) {
                out.push(VisibleSlot {
                    id: id.to_string(),
                    pane: pane.to_owned(),
                    bounds,
                });
            }
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                visible_slots(model, screen, child, bounds, out);
            }
        }
    }
}
fn prune_layout_bounds<'a>(
    screen: &ScreenKey,
    roots: impl IntoIterator<Item = &'a LayoutNode>,
    split_bounds: &mut HashMap<LayoutKey, Bounds<Pixels>>,
    slot_bounds: &mut HashMap<LayoutKey, Bounds<Pixels>>,
) -> HashSet<LayoutKey> {
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
    split_bounds.retain(|id, _| id.screen() == screen && splits.contains(id.node_id()));
    slot_bounds.retain(|id, _| id.screen() == screen && slots.contains(id.node_id()));
    slots
        .into_iter()
        .map(|slot| screen.layout_key(slot))
        .collect()
}
fn reveal_changed_tabs(
    previous: Option<&SourceWorkspace>,
    current: &SourceWorkspace,
    handles: &HashMap<LayoutKey, TabScroll>,
) {
    fn visit(
        node: &LayoutNode,
        previous: Option<&SourceWorkspace>,
        current: &SourceWorkspace,
        screen: &ScreenKey,
        handles: &HashMap<LayoutKey, TabScroll>,
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
                        if let Some(handle) = handles.get(&screen.layout_key(id.as_str())) {
                            handle.handle.scroll_to_item(index);
                        }
                    }
                }
            }
            LayoutNode::Split { children, .. } => {
                for child in children {
                    visit(child, previous, current, screen, handles);
                }
            }
        }
    }
    let Some(screen_id) = current.shown_screen_id() else {
        return;
    };
    let screen = current.screen_key(screen_id);
    let previous = previous.filter(|previous| {
        previous.source_id() == current.source_id() && previous.shown_screen_id() == Some(screen_id)
    });
    if let Some(root) = current.presentation_root() {
        visit(root, previous, current, &screen, handles);
    }
    for entry in current.presented_floats() {
        visit(&entry.root, previous, current, &screen, handles);
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
        admit_close, dispatch_owned_update, nearest_slot, prune_layout_bounds, refresh_resize_drag,
        resized_pair, shortcut, shortcut_with_layout, tab_header_pixels, visible_slots,
        zoom_shortcut, Command, Direction, PendingClose, ResizeDrag, Shortcut, Update, VisibleSlot,
        WorkerCommand,
    };
    use gpui_kit::{point, px, size, Bounds, Keystroke, Modifiers, Pixels};
    use splitty_client_core::{
        workspace::{ClosePlan, CloseTarget, LayoutNode, ScreenModel, SourceWorkspace},
        zoom::ZoomAction,
        EmbeddedService, ScreenIds,
    };
    use std::{
        cell::Cell,
        collections::HashMap,
        time::{Duration, Instant},
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
        let model = SourceWorkspace::new(connection);
        let plan = model.assess_close(9, CloseTarget::Window).unwrap();
        let pending = PendingClose {
            plan: plan.clone(),
            prompting: false,
        };
        let approval = |request_id, source| Update::WindowCloseApproved { request_id, source };
        let foreign = SourceWorkspace::new("00000000-0000-4000-8000-000000000002".parse().unwrap());
        assert!(pending.matches_response(9, Some(model.source_token()), Some(&model)));
        assert!(!pending.matches_response(9, Some(foreign.source_token()), Some(&model)));
        assert!(
            pending.matches_window_approval(&approval(9, Some(model.source_token())), Some(&model))
        );
        assert!(!pending
            .matches_window_approval(&approval(8, Some(model.source_token())), Some(&model)));
        assert!(!pending
            .matches_window_approval(&approval(9, Some(foreign.source_token())), Some(&model)));
        assert!(!pending
            .matches_window_approval(&approval(9, Some(model.source_token())), Some(&foreign)));
        assert!(!PendingClose {
            plan,
            prompting: true
        }
        .matches_window_approval(&approval(9, Some(model.source_token())), Some(&model)));
        let pane = PendingClose {
            plan: ClosePlan::startup_window(9),
            prompting: false,
        };
        assert!(pane.matches_window_approval(&approval(9, None), None));
        assert!(
            !pane.matches_window_approval(&approval(9, Some(model.source_token())), Some(&model))
        );
    }

    #[test]
    fn stale_output_and_attach_callbacks_never_enter_the_view_consumer() {
        let old_connection = uuid::Uuid::new_v4();
        let new_connection = uuid::Uuid::new_v4();
        let old = SourceWorkspace::new(old_connection);
        let mut current = SourceWorkspace::new(new_connection);
        let old_source = old.source_token();
        let current_source = current.source_token();
        let delivered = Cell::new(0);
        let consume = |model: &SourceWorkspace| {
            // Identical raw IDs are deliberate; only source ownership may admit them.
            assert_eq!(model.pane_key("pane").pane_id(), "pane");
            delivered.set(delivered.get() + 1);
        };
        assert!(
            dispatch_owned_update(Some(&current), old_source, Some(old_connection), consume)
                .is_none()
        );
        assert!(dispatch_owned_update(Some(&current), old_source, None, consume).is_none());
        assert_eq!(
            delivered.get(),
            0,
            "neither stale output nor Attached reached its consumer"
        );
        assert!(dispatch_owned_update(
            Some(&current),
            current_source,
            Some(old_connection),
            consume
        )
        .is_none());
        assert_eq!(
            delivered.get(),
            0,
            "underlying Event connection also gates output access"
        );
        assert!(dispatch_owned_update(
            Some(&current),
            current_source,
            Some(new_connection),
            consume
        )
        .is_some());
        assert!(dispatch_owned_update(Some(&current), current_source, None, consume).is_some());
        assert_eq!(
            delivered.get(),
            2,
            "current output and Attached reach the same consumer seam"
        );
        current.reset_connection(uuid::Uuid::new_v4());
        assert_eq!(current.source_id(), current_source.source_id());
        assert!(dispatch_owned_update(Some(&current), current_source, None, consume).is_none());
        assert!(dispatch_owned_update(
            Some(&current),
            current_source,
            Some(new_connection),
            consume
        )
        .is_none());
        assert_eq!(
            delivered.get(),
            2,
            "same-source connection replacement revokes old callbacks"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn divider_drag_uses_confirmed_revision_only_while_split_structure_survives() {
        let service = EmbeddedService::start();
        let mut client = service.connect().await.unwrap();
        let connection = client.connection_id();
        let mut model = SourceWorkspace::new(connection);
        model.set_catalog_supported(true);
        model.mark_catalog_dirty();
        model.flush_catalog(&client).unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
            {
                model
                    .apply_catalog(connection, event.json().unwrap())
                    .unwrap();
                break;
            }
        }
        let ids = model.create_screen(&client, "Drag").unwrap();
        model.flush_catalog(&client).unwrap();
        loop {
            let event = client.next_event().await.unwrap().unwrap();
            if event
                .json()
                .is_some_and(|json| json["type"] == "mux.workspace_snapshot")
            {
                model
                    .apply_catalog(connection, event.json().unwrap())
                    .unwrap();
                break;
            }
        }
        assert_eq!(model.shown_screen_id(), Some(ids.screen_id.as_str()));
        let snapshot = |revision, right: &str, sizes: [f64; 2]| {
            serde_json::json!({
                "type":"mux.screen_snapshot",
                "screen":{"id":ids.screen_id,"revision":revision,
                    "root":{"type":"split","id":"root","axis":"horizontal","sizes":sizes,
                        "children":[{"type":"slot","id":"left","paneIds":[ids.pane_id]},
                            {"type":"slot","id":right,"paneIds":[]}]},
                    "panes":{ids.pane_id.clone():{"id":ids.pane_id}},"sessions":{}}
            })
        };
        model
            .apply_snapshot(connection, &snapshot(1, "right", [0.5, 0.5]))
            .unwrap();
        let source = model.source_token();
        let mut drag = ResizeDrag {
            node: model.screen_key(&ids.screen_id).layout_key("root"),
            source,
            revision: 1,
            axis: super::Axis::Horizontal,
            index: 0,
            start: point(px(10.), px(10.)),
            last_position: point(px(60.), px(10.)),
            bounds: rectangle(0., 0., 200., 100.),
            children: vec!["left".into(), "right".into()],
            sizes: vec![0.5, 0.5],
            pending: vec![0.6, 0.4],
            last_sent: Instant::now() - Duration::from_millis(40),
        };
        assert!(refresh_resize_drag(&mut drag, &model));
        model
            .apply_snapshot(connection, &snapshot(2, "right", [0.7, 0.3]))
            .unwrap();
        assert!(refresh_resize_drag(&mut drag, &model));
        assert_eq!(
            drag.revision, 2,
            "only a confirmed matching snapshot refreshes authority"
        );
        assert_eq!(drag.sizes, [0.7, 0.3]);
        assert_eq!(
            drag.start, drag.last_position,
            "next pointer delta starts at the confirmed position"
        );
        assert!(!refresh_resize_drag(
            &mut drag,
            &SourceWorkspace::new(uuid::Uuid::new_v4())
        ));
        model
            .apply_snapshot(connection, &snapshot(3, "replacement", [0.7, 0.3]))
            .unwrap();
        assert!(
            !refresh_resize_drag(&mut drag, &model),
            "reused split ID with new children cancels the old drag"
        );
        client.finish().await.unwrap();
        service.close().await.unwrap();
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
        let model = SourceWorkspace::new(connection);
        assert_eq!(
            startup.resolve_prompt(Some(&model), true).unwrap(),
            Some(ClosePlan::startup_window(4)),
            "an exact confirmed startup plan remains valid after readiness publication"
        );
        let removed = PendingClose {
            plan: model.assess_close(5, CloseTarget::Window).unwrap(),
            prompting: true,
        };
        let replacement =
            SourceWorkspace::new("00000000-0000-4000-8000-000000000002".parse().unwrap());
        assert!(removed.resolve_prompt(Some(&replacement), true).is_err());
        assert!(removed
            .resolve_prompt(Some(&replacement), false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn full_close_command_queue_leaves_no_pending_close_and_recovers() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        sender
            .try_send(WorkerCommand::close(ClosePlan::startup_window(1), false))
            .unwrap();
        let plan = ClosePlan::startup_window(2);
        assert!(admit_close(&sender, plan.clone(), true).is_err());
        assert!(
            matches!(receiver.try_recv(), Ok(command) if matches!(command.kind(), Command::RequestClose { .. }))
        );
        let pending = admit_close(&sender, plan, true).unwrap();
        assert_eq!(pending.plan.request_id, 2);
        assert!(matches!(
            receiver.try_recv(),
            Ok(command) if matches!(command.kind(), Command::RequestClose { .. })
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
        let screen_key = SourceWorkspace::new(connection).screen_key("screen");
        assert!(model.select_tab("b"));
        let bounds = HashMap::from([
            (screen_key.layout_key("left"), rectangle(0., 0., 100., 200.)),
            (
                screen_key.layout_key("upper"),
                rectangle(100., 0., 100., 100.),
            ),
            (
                screen_key.layout_key("lower"),
                rectangle(100., 100., 100., 100.),
            ),
            (
                screen_key.layout_key("stale"),
                rectangle(200., 0., 100., 100.),
            ),
        ]);
        let mut slots = Vec::new();
        visible_slots(
            &model,
            &screen_key,
            &model.snapshot().unwrap().root,
            &bounds,
            &mut slots,
        );
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
        missing.remove(&screen_key.layout_key("left"));
        let mut slots = Vec::new();
        visible_slots(
            &model,
            &screen_key,
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
        let screen = SourceWorkspace::new(uuid::Uuid::new_v4()).screen_key("screen");
        let mut splits = HashMap::from([
            (
                screen.layout_key("old-split"),
                rectangle(0., 0., 100., 100.),
            ),
            (
                screen.layout_key("current-split"),
                rectangle(0., 0., 200., 100.),
            ),
        ]);
        let mut slots = HashMap::from([
            (
                screen.layout_key("closed-slot"),
                rectangle(0., 0., 100., 100.),
            ),
            (screen.layout_key("left"), rectangle(0., 0., 100., 100.)),
            (screen.layout_key("right"), rectangle(100., 0., 100., 100.)),
        ]);
        prune_layout_bounds(&screen, std::iter::once(&root), &mut splits, &mut slots);
        assert_eq!(
            splits.keys().map(|key| key.node_id()).collect::<Vec<_>>(),
            ["current-split"]
        );
        assert_eq!(slots.len(), 2);
        assert!(
            slots.contains_key(&screen.layout_key("left"))
                && slots.contains_key(&screen.layout_key("right"))
        );
        assert!(!slots.contains_key(&screen.layout_key("closed-slot")));
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
