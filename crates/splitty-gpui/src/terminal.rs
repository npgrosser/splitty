//! Native terminal presentation. The service owns the PTY; Ghostty owns VT state.
mod links;
mod search;
pub(crate) use links::LinkOrigin;
use splitty_client_core::external_links::WebUrl;

#[derive(Clone, Debug)]
pub(crate) enum LinkEvent {
    RemoteLoopback(WebUrl),
}

impl EventEmitter<LinkEvent> for Terminal {}

pub use crate::terminal_engine::ColorConfig as TerminalColors;
use crate::terminal_engine::{
    Cursor, CursorShape, Engine, MouseAction, MouseButton as EngineMouseButton, MouseInput,
    MouseModifiers, MousePolicy, MouseTracking, Row, SearchHighlight, MAX_INPUT,
};
use anyhow::{anyhow, Result};
use gpui_kit::{prelude::*, *};
use std::{collections::HashMap, ops::Range, time::Instant};

const BASE_FONT_SIZE: f32 = 15.;
const BASE_LINE_HEIGHT: f32 = 21.;
const DEFAULT_SELECTION_FOREGROUND: u32 = 0xffffff;
const DEFAULT_SELECTION_BACKGROUND: u32 = 0x345778;
const MAX_ALTERNATE_WHEEL_KEYS: i32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerRoute {
    Selection,
    Local,
    Link,
    Application,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelRoute {
    Application,
    Scrollback,
    AlternateKeys,
}

fn wheel_route(policy: MousePolicy, alternate: bool, shift: bool) -> WheelRoute {
    if shift {
        WheelRoute::Scrollback
    } else if policy.reports_wheel() {
        WheelRoute::Application
    } else if alternate {
        WheelRoute::AlternateKeys
    } else {
        WheelRoute::Scrollback
    }
}

#[derive(Default)]
struct PointerGestures {
    buttons: [Option<PointerRoute>; 5],
    last_position: Option<(f32, f32)>,
}

impl PointerGestures {
    fn index(button: MouseButton) -> Option<usize> {
        match button {
            MouseButton::Left => Some(0),
            MouseButton::Middle => Some(1),
            MouseButton::Right => Some(2),
            MouseButton::Navigate(NavigationDirection::Back) => Some(3),
            MouseButton::Navigate(NavigationDirection::Forward) => Some(4),
        }
    }

    fn begin(&mut self, button: MouseButton, shift: bool, reports_buttons: bool) -> PointerRoute {
        let route = if shift || !reports_buttons {
            if button == MouseButton::Left {
                PointerRoute::Selection
            } else {
                PointerRoute::Local
            }
        } else {
            PointerRoute::Application
        };
        if let Some(index) = Self::index(button) {
            self.buttons[index] = Some(route);
        }
        route
    }

    fn capture_link(&mut self) {
        self.buttons[0] = Some(PointerRoute::Link);
    }

    fn take(&mut self, button: MouseButton) -> Option<PointerRoute> {
        Self::index(button).and_then(|index| self.buttons[index].take())
    }

    fn reject_application(&mut self, button: MouseButton) {
        if let Some(index) = Self::index(button) {
            if self.buttons[index] == Some(PointerRoute::Application) {
                self.buttons[index] = Some(PointerRoute::Local);
            }
        }
    }

    fn route(&self, button: MouseButton) -> Option<PointerRoute> {
        Self::index(button).and_then(|index| self.buttons[index])
    }

    fn first_application_button(&self) -> Option<MouseButton> {
        pointer_buttons()
            .into_iter()
            .find(|button| self.route(*button) == Some(PointerRoute::Application))
    }

    fn any_application_pressed(&self) -> bool {
        self.buttons.contains(&Some(PointerRoute::Application))
    }

    fn any_local_pressed(&self) -> bool {
        self.buttons.iter().any(|route| {
            matches!(
                route,
                Some(PointerRoute::Selection | PointerRoute::Local | PointerRoute::Link)
            )
        })
    }

    fn unadmitted_button_pressed(&self, button: Option<MouseButton>) -> bool {
        button.is_some_and(|button| self.route(button).is_none())
    }
}

#[derive(Clone, Copy)]
struct SelectionColors {
    foreground: u32,
    background: u32,
}

struct SearchPaint<'a> {
    highlights: &'a [&'a SearchHighlight],
    passive: u32,
    active: u32,
}

#[derive(Clone, Copy)]
struct CursorGeometry {
    bounds: Bounds<Pixels>,
    metrics: TerminalMetrics,
}

/// One desired terminal scale. Cell width becomes authoritative only after a
/// visible layout measures the current font on the current display.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalMetrics {
    scale: f32,
    font_size: Pixels,
    line_height: Pixels,
    cell_width: Pixels,
}

impl TerminalMetrics {
    fn new(scale: f32) -> Self {
        let scale = if scale.is_finite() && (0.5..=3.).contains(&scale) {
            scale
        } else {
            1.
        };
        Self {
            scale,
            font_size: px(BASE_FONT_SIZE * scale),
            line_height: px(BASE_LINE_HEIGHT * scale),
            // This value is provisional until layout measures the actual face.
            cell_width: px(9. * scale),
        }
    }

    fn pixel_cell(self, display_scale: f32) -> (u32, u32) {
        (
            (f32::from(self.cell_width) * display_scale).round().max(1.) as u32,
            (f32::from(self.line_height) * display_scale)
                .round()
                .max(1.) as u32,
        )
    }

    fn grid_size(self, width: Pixels, height: Pixels) -> Size {
        Size {
            cols: (width / self.cell_width).floor().clamp(2., 400.) as u16,
            rows: (height / self.line_height).floor().clamp(1., 200.) as u16,
        }
    }

    fn hit_cell(self, x: f32, y: f32, size: Size) -> Option<links::Cell> {
        let cell_width = f32::from(self.cell_width);
        let row_height = f32::from(self.line_height);
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.
            || y < 0.
            || x >= cell_width * f32::from(size.cols)
            || y >= row_height * f32::from(size.rows)
        {
            return None;
        }
        Some(links::Cell {
            col: (x / cell_width).floor() as u16,
            row: (y / row_height).floor() as u16,
        })
    }
}

/// GPUI reports positive Y for upward wheel motion. Keep only the fractional
/// part across events; a large gesture must not become a long key backlog.
fn wheel_lines(remainder: &mut f32, delta: f32, alternate: bool) -> i32 {
    let total = *remainder + delta;
    if !total.is_finite() {
        *remainder = 0.;
        return 0;
    }
    let whole = total.trunc();
    *remainder = total - whole;
    let lines = whole as i32;
    if alternate {
        lines.clamp(-MAX_ALTERNATE_WHEEL_KEYS, MAX_ALTERNATE_WHEEL_KEYS)
    } else {
        lines
    }
}

fn alternate_wheel_input(engine: &mut Engine, lines: i32) -> Result<Vec<u8>> {
    let key = if lines > 0 { "up" } else { "down" };
    let mut input = Vec::new();
    for _ in 0..lines.unsigned_abs().min(MAX_ALTERNATE_WHEEL_KEYS as u32) {
        input.extend(
            engine.key(key, false, false, false)?.ok_or_else(|| {
                anyhow!("Ghostty did not encode {key} for alternate-screen wheel")
            })?,
        );
    }
    Ok(input)
}

fn wheel_button(lines: i32, horizontal: bool) -> EngineMouseButton {
    match (horizontal, lines > 0) {
        (false, true) => EngineMouseButton::WheelUp,
        (false, false) => EngineMouseButton::WheelDown,
        (true, true) => EngineMouseButton::WheelLeft,
        (true, false) => EngineMouseButton::WheelRight,
    }
}

fn link_modifier(modifiers: Modifiers, mac: bool) -> bool {
    !modifiers.shift
        && if mac {
            modifiers.platform
        } else {
            modifiers.control
        }
}

fn append_wheel_reports(
    engine: &mut Engine,
    mut input: MouseInput,
    lines: i32,
    horizontal: bool,
    bytes: &mut Vec<u8>,
) -> Result<()> {
    if lines != 0 {
        input.button = Some(wheel_button(lines, horizontal));
        for _ in 0..lines.unsigned_abs() {
            bytes.extend(engine.mouse(input)?);
        }
    }
    Ok(())
}

fn engine_mouse_button(button: MouseButton) -> EngineMouseButton {
    match button {
        MouseButton::Left => EngineMouseButton::Left,
        MouseButton::Middle => EngineMouseButton::Middle,
        MouseButton::Right => EngineMouseButton::Right,
        MouseButton::Navigate(NavigationDirection::Back) => EngineMouseButton::Back,
        MouseButton::Navigate(NavigationDirection::Forward) => EngineMouseButton::Forward,
    }
}

fn pointer_buttons() -> [MouseButton; 5] {
    [
        MouseButton::Left,
        MouseButton::Middle,
        MouseButton::Right,
        MouseButton::Navigate(NavigationDirection::Back),
        MouseButton::Navigate(NavigationDirection::Forward),
    ]
}

fn mouse_modifiers(modifiers: Modifiers) -> MouseModifiers {
    MouseModifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        super_key: modifiers.platform,
    }
}

#[derive(Debug)]
pub enum Command {
    Input { generation: u64, bytes: Vec<u8> },
    Resize(u16, u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Size {
    cols: u16,
    rows: u16,
}

struct BackgroundSpan {
    col: u16,
    width: u16,
    color: u32,
}

struct UnderlineSpan {
    col: u16,
    width: u16,
    color: u32,
}

struct ShapedRun {
    col: u16,
    line: ShapedLine,
}

struct CachedRow {
    last_frame: usize,
    backgrounds: Vec<BackgroundSpan>,
    underlines: Vec<UnderlineSpan>,
    runs: Vec<ShapedRun>,
    selected: Vec<Range<u16>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RunStyle {
    foreground: u32,
    bold: bool,
    italic: bool,
}

struct PendingAscii {
    col: u16,
    text: String,
    style: RunStyle,
}

pub struct Terminal {
    engine: Option<Engine>,
    commands: Box<dyn Fn(Command) -> std::result::Result<(), String>>,
    command_error: Option<String>,
    focus: FocusHandle,
    bounds: Bounds<Pixels>,
    metrics: TerminalMetrics,
    geometry_ready: bool,
    size_report_pending: bool,
    resized_pixel_cell: Option<(u32, u32)>,
    face: Font,
    size: Size,
    generation: u64,
    composition: String,
    pointer: PointerGestures,
    links: links::State,
    link_origin: LinkOrigin,
    scroll_remainder: f32,
    scroll_remainder_x: f32,
    scroll_route: Option<WheelRoute>,
    focus_out_subscription: Option<Subscription>,
    activation_subscription: Option<Subscription>,
    search: Option<search::SearchUi>,
    search_active: bool,
    row_cache: HashMap<[u64; 2], CachedRow>,
    colors: TerminalColors,
    selection_foreground: u32,
    selection_background: u32,
    pub status: String,
    pub bytes: usize,
    pub frames: usize,
    content_revision: u64,
    painted_revision: u64,
    pub saw_smoke: bool,
    smoke_tail: String,
    pub last_paint: Option<Instant>,
    pub shaped_rows: u64,
    pub shaped_runs: u64,
    pub parse_micros: u64,
    pub paint_micros: u64,
}

impl Terminal {
    pub(crate) fn new(
        commands: impl Fn(Command) -> std::result::Result<(), String> + 'static,
        link_origin: LinkOrigin,
        cx: &mut Context<Self>,
    ) -> Self {
        let size = Size { cols: 80, rows: 24 };
        let (engine, status) = match Engine::new(size.cols, size.rows) {
            Ok(engine) => (Some(engine), "Connecting…".into()),
            Err(error) => (None, format!("Terminal engine: {error:#}")),
        };
        Self {
            engine,
            commands: Box::new(commands),
            command_error: None,
            focus: cx.focus_handle(),
            bounds: Bounds::default(),
            metrics: TerminalMetrics::new(1.),
            geometry_ready: false,
            size_report_pending: false,
            resized_pixel_cell: None,
            // GPUI font lookup takes concrete family names, not CSS generic families.
            face: font(gpui_kit::base::Theme::global(cx).tokens.typography.mono),
            size,
            generation: 0,
            composition: String::new(),
            pointer: PointerGestures::default(),
            links: links::State::default(),
            link_origin,
            scroll_remainder: 0.,
            scroll_remainder_x: 0.,
            scroll_route: None,
            focus_out_subscription: None,
            activation_subscription: None,
            search: None,
            // The standalone probe has no workspace focus coordinator.
            search_active: true,
            row_cache: HashMap::new(),
            colors: TerminalColors::default(),
            selection_foreground: DEFAULT_SELECTION_FOREGROUND,
            selection_background: DEFAULT_SELECTION_BACKGROUND,
            status,
            bytes: 0,
            frames: 0,
            content_revision: 1,
            painted_revision: 0,
            saw_smoke: false,
            smoke_tail: String::new(),
            last_paint: None,
            shaped_rows: 0,
            shaped_runs: 0,
            parse_micros: 0,
            paint_micros: 0,
        }
    }

    /// Rebind the route of a surviving terminal after workspace geometry changes.
    /// Previously retained closures keep their original captured authority.
    pub(crate) fn replace_commands(
        &mut self,
        commands: impl Fn(Command) -> std::result::Result<(), String> + 'static,
    ) {
        self.commands = Box::new(commands);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(search) = self
            .search
            .as_ref()
            .filter(|search| self.search_active && search.open)
        {
            search.input.update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.focus_pty(window, cx);
        }
    }

    fn focus_pty(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    pub fn painted_current(&self) -> bool {
        self.engine.is_some() && self.frames > 0 && self.painted_revision == self.content_revision
    }

    pub fn engine_ready(&self) -> bool {
        self.engine.is_some()
    }

    /// Stage the confirmed scale. The next visible layout measures the font
    /// before any painting, hit testing or PTY size claim uses new geometry.
    pub fn set_zoom(&mut self, scale: f32, cx: &mut Context<Self>) {
        let metrics = TerminalMetrics::new(scale);
        if self.metrics.scale == metrics.scale {
            return;
        }
        self.cancel_pointer();
        self.metrics = metrics;
        self.geometry_ready = false;
        self.row_cache.clear();
        self.scroll_remainder = 0.;
        self.scroll_remainder_x = 0.;
        self.scroll_route = None;
        self.content_revision = self.content_revision.wrapping_add(1);
        cx.notify();
    }

    pub fn apply_colors(
        &mut self,
        colors: TerminalColors,
        selection_foreground: u32,
        selection_background: u32,
        cx: &mut Context<Self>,
    ) -> std::result::Result<(), String> {
        if self.colors == colors
            && self.selection_foreground == selection_foreground
            && self.selection_background == selection_background
        {
            return Ok(());
        }
        if let Some(engine) = self.engine.as_mut() {
            if let Err(error) = engine.apply_colors(colors) {
                self.status = format!("Terminal colors: {error:#}");
                return Err(self.status.clone());
            }
        }
        self.colors = colors;
        self.selection_foreground = selection_foreground;
        self.selection_background = selection_background;
        self.row_cache.clear();
        self.content_revision += 1;
        cx.notify();
        Ok(())
    }

    pub fn feed(&mut self, bytes: &[u8], replay: bool, cx: &mut Context<Self>) {
        self.bytes += bytes.len();
        let started = Instant::now();
        let result = self
            .engine
            .as_mut()
            .ok_or_else(|| anyhow!("Terminal engine is unavailable"))
            .and_then(|engine| engine.feed(bytes, replay));
        self.parse_micros = self
            .parse_micros
            .saturating_add(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX));
        if let Some(reply) = self.output(result) {
            self.send_bytes(reply);
        }
        if self.engine.is_none() {
            return;
        }
        self.links.output_changed();
        self.content_revision += 1;
        self.smoke_tail.push_str(&String::from_utf8_lossy(bytes));
        self.saw_smoke |= self.smoke_tail.contains("SPLITTY_GPUI_SMOKE_OK");
        if self.smoke_tail.len() > 4096 {
            let mut start = self.smoke_tail.len() - 2048;
            while !self.smoke_tail.is_char_boundary(start) {
                start += 1;
            }
            self.smoke_tail.drain(..start);
        }
        self.search_wake(cx);
        cx.notify();
    }

    pub fn reset(&mut self, generation: u64, cx: &mut Context<Self>) {
        self.cancel_pointer();
        self.geometry_ready = false;
        self.resized_pixel_cell = None;
        self.generation = generation;
        self.frames = 0;
        self.content_revision += 1;
        self.painted_revision = 0;
        self.row_cache.clear();
        self.composition.clear();
        self.saw_smoke = false;
        self.smoke_tail.clear();
        self.scroll_remainder = 0.;
        self.scroll_remainder_x = 0.;
        self.scroll_route = None;
        let result = if let Some(engine) = self.engine.as_mut() {
            engine.reset()
        } else {
            Engine::new(self.size.cols, self.size.rows).and_then(|mut engine| {
                engine.apply_colors(self.colors)?;
                self.engine = Some(engine);
                Ok(())
            })
        };
        if let Err(error) = result {
            self.engine = None;
            self.status = format!("Terminal engine reset: {error:#}");
        }
        self.search_reset(cx);
        cx.notify();
    }

    /// Ownership changed while the terminal entity survived. Its old bounds
    /// cannot claim a size for the new location, even if both grids are equal.
    pub fn invalidate_size_report(&mut self, cx: &mut Context<Self>) {
        self.cancel_pointer();
        self.geometry_ready = false;
        self.size_report_pending = true;
        self.content_revision = self.content_revision.wrapping_add(1);
        cx.notify();
    }

    pub fn report_size(&mut self) {
        if self.geometry_ready && self.send(Command::Resize(self.size.cols, self.size.rows)) {
            self.size_report_pending = false;
        }
    }

    fn send(&mut self, command: Command) -> bool {
        let is_input = matches!(&command, Command::Input { .. });
        if let Command::Input { bytes, .. } = &command {
            if bytes.len() > MAX_INPUT {
                self.reject_command("Input exceeds the 64 KiB limit".into());
                return false;
            }
        }
        match (self.commands)(command) {
            Ok(()) if is_input => {
                self.command_error = None;
                true
            }
            Ok(()) => true,
            Err(error) => {
                let action = if is_input { "Input" } else { "Resize" };
                self.reject_command(format!("{action} not sent: {error}"));
                false
            }
        }
    }

    fn reject_command(&mut self, error: String) {
        self.status.clone_from(&error);
        self.command_error = Some(error);
    }

    fn send_bytes(&mut self, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() {
            return false;
        }
        self.send(Command::Input {
            generation: self.generation,
            bytes,
        })
    }

    fn output(&mut self, result: Result<Vec<u8>>) -> Option<Vec<u8>> {
        match result {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                self.status = format!("Terminal engine: {error:#}");
                None
            }
        }
    }

    fn operation(&mut self, result: Option<Result<()>>) {
        if let Some(Err(error)) = result {
            self.status = format!("Terminal engine: {error:#}");
        }
    }

    fn paste_text(&mut self, text: &str) {
        let result = self.engine.as_mut().map(|engine| engine.paste(text));
        if let Some(reply) = result.and_then(|result| self.output(result)) {
            // One bounded message keeps bracketed-paste framing intact.
            self.send_bytes(reply);
        }
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let find_modifier = if cx.platform_info().is_mac() {
            key.modifiers.platform && !key.modifiers.control
        } else {
            key.modifiers.control && !key.modifiers.platform
        };
        if find_modifier
            && !key.modifiers.shift
            && !key.modifiers.alt
            && key.key.eq_ignore_ascii_case("f")
        {
            self.open_search(window, cx);
            cx.stop_propagation();
            return;
        }
        if self.search_input_focused(window, cx) {
            if key.key == "escape" {
                self.close_search(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if (key.modifiers.control && key.modifiers.shift) || key.modifiers.platform {
            match key.key.as_str() {
                "c" => {
                    let result = self.engine.as_mut().map(Engine::selection_text);
                    if let Some(Ok(Some(text))) = result.as_ref() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    }
                    if let Some(Err(error)) = result {
                        self.status = format!("Terminal selection: {error:#}");
                    }
                    cx.stop_propagation();
                    return;
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.paste_text(&text);
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        let result = self.engine.as_mut().map(|engine| {
            engine.key(
                &key.key,
                key.modifiers.control,
                key.modifiers.alt,
                key.modifiers.shift,
            )
        });
        match result {
            Some(Ok(Some(bytes))) => {
                let result = self.engine.as_mut().map(Engine::scroll_bottom);
                self.operation(result);
                self.links.cancel();
                self.search_wake(cx);
                self.send_bytes(bytes);
                window.notify_text_input_changed(&self.focus);
                cx.stop_propagation();
                cx.notify();
            }
            Some(Err(error)) => self.status = format!("Terminal key: {error:#}"),
            _ => {}
        }
        // Printable text is delivered by the platform input handler, including IME/dead keys.
    }

    fn cell_at(&self, position: Point<Pixels>) -> (u16, u16) {
        let col = ((position.x - self.bounds.left()) / self.metrics.cell_width)
            .floor()
            .clamp(0., f32::from(self.size.cols - 1)) as u16;
        let row = ((position.y - self.bounds.top()) / self.metrics.line_height)
            .floor()
            .clamp(0., f32::from(self.size.rows - 1)) as u16;
        (col, row)
    }

    fn link_cell_at(&self, position: Point<Pixels>) -> Option<links::Cell> {
        if !self.geometry_ready {
            return None;
        }
        let x = f32::from(position.x - self.bounds.left());
        let y = f32::from(position.y - self.bounds.top());
        if x >= f32::from(self.bounds.size.width) || y >= f32::from(self.bounds.size.height) {
            return None;
        }
        self.metrics.hit_cell(x, y, self.size)
    }

    fn link_at(&mut self, cell: links::Cell) -> Option<links::Target> {
        match self.engine.as_mut()?.link_at(cell.col, cell.row) {
            Ok(Some(hit)) => links::Target::allowed(hit),
            Ok(None) => None,
            Err(error) => {
                self.status = format!("Terminal link: {error:#}");
                None
            }
        }
    }

    fn hover_link(&mut self, position: Point<Pixels>, modifiers: Modifiers, mac: bool) -> bool {
        if !link_modifier(modifiers, mac) {
            return self.links.cancel_hover();
        }
        let Some(cell) = self.link_cell_at(position) else {
            return self.links.cancel_hover();
        };
        if self.links.cached_hover(cell).is_some() {
            return false;
        }
        let target = self.link_at(cell);
        self.links.set_hover(cell, target)
    }

    fn pointer_position(&self, position: Point<Pixels>, window: &Window) -> (f32, f32) {
        let scale = window.scale_factor();
        (
            f32::from(position.x - self.bounds.left()) * scale,
            f32::from(position.y - self.bounds.top()) * scale,
        )
    }

    fn pointer_policy(&mut self) -> Option<MousePolicy> {
        match self.engine.as_mut()?.mouse_policy() {
            Ok(policy) => Some(policy),
            Err(error) => {
                self.status = format!("Terminal mouse policy: {error:#}");
                None
            }
        }
    }

    fn mouse_report(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        position: (f32, f32),
        modifiers: Modifiers,
    ) -> bool {
        let input = MouseInput {
            action,
            button: button.map(engine_mouse_button),
            x: position.0,
            y: position.1,
            modifiers: mouse_modifiers(modifiers),
            any_button_pressed: self.pointer.any_application_pressed(),
        };
        let result = self.engine.as_mut().map(|engine| engine.mouse(input));
        result
            .and_then(|result| self.output(result))
            .is_some_and(|bytes| self.send_bytes(bytes))
    }

    fn pointer_down(&mut self, event: &MouseDownEvent, window: &Window, mac: bool) {
        if !self.geometry_ready {
            return;
        }
        let position = self.pointer_position(event.position, window);
        self.pointer.last_position = Some(position);
        if event.button == MouseButton::Left
            && link_modifier(event.modifiers, mac)
            && !self.pointer.any_application_pressed()
            && !self.pointer.any_local_pressed()
        {
            if let Some(cell) = self.link_cell_at(event.position) {
                if let Some(target) = self.link_at(cell) {
                    self.pointer.capture_link();
                    self.links.press(cell, target, position, self.generation);
                    return;
                }
            }
        }
        let Some(policy) = self.pointer_policy() else {
            return;
        };
        match self.pointer.begin(
            event.button,
            event.modifiers.shift,
            policy.reports_buttons(),
        ) {
            PointerRoute::Selection => {
                let (col, row) = self.cell_at(event.position);
                let result = self
                    .engine
                    .as_mut()
                    .map(|engine| engine.select_start(col, row, event.click_count >= 2));
                self.operation(result);
                self.row_cache.clear();
            }
            PointerRoute::Application => {
                if !self.mouse_report(
                    MouseAction::Press,
                    Some(event.button),
                    position,
                    event.modifiers,
                ) {
                    self.pointer.reject_application(event.button);
                    let result = self.engine.as_mut().map(Engine::reset_mouse);
                    self.operation(result);
                }
            }
            PointerRoute::Link => {}
            PointerRoute::Local => {}
        }
    }

    fn pointer_move(&mut self, event: &MouseMoveEvent, window: &Window, mac: bool) -> bool {
        if !self.geometry_ready {
            return self.links.cancel_hover();
        }
        let position = self.pointer_position(event.position, window);
        self.pointer.last_position = Some(position);
        self.links.moving(position);
        if self.pointer.route(MouseButton::Left) == Some(PointerRoute::Link) {
            return false;
        }
        // MouseExit/focus loss may revoke a captured link while the physical
        // button remains held. Reentry cannot turn that same gesture into an
        // application hover or motion report.
        if self.pointer.unadmitted_button_pressed(event.pressed_button) {
            return self.links.cancel_hover();
        }
        let hover_changed = self.hover_link(event.position, event.modifiers, mac);
        if self.pointer.route(MouseButton::Left) == Some(PointerRoute::Selection) {
            if event.dragging() {
                let (col, row) = self.cell_at(event.position);
                let result = self
                    .engine
                    .as_mut()
                    .map(|engine| engine.select_update(col, row));
                self.operation(result);
                self.row_cache.clear();
                return true;
            }
            return hover_changed;
        }
        if self.pointer.any_local_pressed() {
            return hover_changed;
        }
        let button = event
            .pressed_button
            .filter(|button| self.pointer.route(*button) == Some(PointerRoute::Application))
            .or_else(|| self.pointer.first_application_button());
        if button.is_none()
            && self
                .pointer_policy()
                .is_none_or(|policy| policy.tracking != MouseTracking::Any)
        {
            return hover_changed;
        }
        let before = self.status.clone();
        self.mouse_report(MouseAction::Motion, button, position, event.modifiers);
        hover_changed || self.status != before
    }

    fn pointer_up(
        &mut self,
        event: &MouseUpEvent,
        window: &Window,
        outside: bool,
    ) -> Option<splitty_client_core::external_links::WebUrl> {
        if !self.geometry_ready {
            return None;
        }
        let route = self.pointer.take(event.button)?;
        match route {
            PointerRoute::Selection => {
                let result = self.engine.as_mut().map(Engine::select_end);
                self.operation(result);
            }
            PointerRoute::Application => {
                let position = if outside {
                    self.pointer.last_position.unwrap_or((0., 0.))
                } else {
                    self.pointer_position(event.position, window)
                };
                self.mouse_report(
                    MouseAction::Release,
                    Some(event.button),
                    position,
                    event.modifiers,
                );
            }
            PointerRoute::Link => {
                let cell = (!outside)
                    .then(|| self.link_cell_at(event.position))
                    .flatten();
                let target = cell.and_then(|cell| self.link_at(cell));
                return self.links.finish(
                    cell,
                    self.pointer_position(event.position, window),
                    self.generation,
                    target,
                );
            }
            PointerRoute::Local => {}
        }
        None
    }

    /// End gestures before hiding or replacing this terminal's captured session.
    /// A rejected release is visible but never replayed into another session.
    pub fn cancel_pointer(&mut self) {
        self.links.cancel();
        let position = self.pointer.last_position.unwrap_or((0., 0.));
        for button in pointer_buttons() {
            match self.pointer.take(button) {
                Some(PointerRoute::Selection) => {
                    let result = self.engine.as_mut().map(Engine::select_end);
                    self.operation(result);
                }
                Some(PointerRoute::Application) => {
                    self.mouse_report(
                        MouseAction::Release,
                        Some(button),
                        position,
                        Modifiers::default(),
                    );
                }
                _ => {}
            }
        }
        self.pointer.last_position = None;
        let result = self.engine.as_mut().map(Engine::reset_mouse);
        self.operation(result);
    }

    fn on_pointer_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pty(window, cx);
        self.pointer_down(event, window, cx.platform_info().is_mac());
        cx.notify();
    }

    fn on_pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pointer_move(event, window, cx.platform_info().is_mac()) {
            cx.notify();
        }
    }

    fn on_pointer_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(url) = self.pointer_up(event, window, false) {
            if self.link_origin == LinkOrigin::Remote && url.is_loopback_or_unspecified() {
                cx.emit(LinkEvent::RemoteLoopback(url));
            } else if let Err(reason) = self.link_origin.activate(&url, |url| cx.open_url(url)) {
                // The workspace paints command_error; status alone is only shown by the probe.
                self.reject_command(reason.into());
            }
        }
        cx.notify();
    }

    fn on_pointer_up_out(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pointer_up(event, window, true);
        cx.notify();
    }

    fn on_pointer_exit(&mut self, _: &MouseExitEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel_pointer();
        cx.notify();
    }

    fn on_link_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !link_modifier(event.modifiers, cx.platform_info().is_mac()) && self.links.cancel_hover()
        {
            cx.notify();
        }
    }

    fn on_scroll_event(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.geometry_ready {
            cx.stop_propagation();
            return;
        }
        let alternate = match self.engine.as_ref().map(Engine::is_alternate_screen) {
            Some(Ok(alternate)) => alternate,
            Some(Err(error)) => {
                self.operation(Some(Err(error)));
                cx.notify();
                cx.stop_propagation();
                return;
            }
            None => {
                cx.stop_propagation();
                return;
            }
        };
        let Some(policy) = self.pointer_policy() else {
            cx.notify();
            cx.stop_propagation();
            return;
        };
        let route = wheel_route(policy, alternate, event.modifiers.shift);
        if self.scroll_route != Some(route) {
            self.scroll_remainder = 0.;
            self.scroll_remainder_x = 0.;
            self.scroll_route = Some(route);
        }
        let (delta_x, delta_y) = match event.delta {
            ScrollDelta::Pixels(delta) => (
                f32::from(delta.x) / f32::from(self.metrics.line_height),
                f32::from(delta.y) / f32::from(self.metrics.line_height),
            ),
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
        };
        let lines = wheel_lines(
            &mut self.scroll_remainder,
            delta_y,
            route != WheelRoute::Scrollback,
        );
        let horizontal = if route == WheelRoute::Application {
            wheel_lines(&mut self.scroll_remainder_x, delta_x, true)
        } else {
            self.scroll_remainder_x = 0.;
            0
        };
        if lines != 0 || horizontal != 0 {
            self.links.cancel();
            match route {
                WheelRoute::Scrollback => {
                    let result = self.engine.as_mut().map(|engine| engine.scroll(lines));
                    self.operation(result);
                    self.search_wake(cx);
                }
                WheelRoute::AlternateKeys => {
                    let result = self
                        .engine
                        .as_mut()
                        .map(|engine| alternate_wheel_input(engine, lines));
                    if let Some(bytes) = result.and_then(|result| self.output(result)) {
                        self.send_bytes(bytes);
                    }
                }
                WheelRoute::Application => {
                    let position = self.pointer_position(event.position, window);
                    let input = MouseInput {
                        action: MouseAction::Press,
                        button: None,
                        x: position.0,
                        y: position.1,
                        modifiers: mouse_modifiers(event.modifiers),
                        any_button_pressed: self.pointer.any_application_pressed(),
                    };
                    let result = self.engine.as_mut().map(|engine| {
                        let mut bytes = Vec::new();
                        append_wheel_reports(engine, input, lines, false, &mut bytes)?;
                        append_wheel_reports(engine, input, horizontal, true, &mut bytes)?;
                        Ok(bytes)
                    });
                    if let Some(bytes) = result.and_then(|result| self.output(result)) {
                        self.send_bytes(bytes);
                    }
                }
            }
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.bounds = bounds;
        let font_id = window.text_system().resolve_font(&self.face);
        if let Ok(advance) = window
            .text_system()
            .advance(font_id, self.metrics.font_size, 'M')
        {
            let width = advance.width.max(px(1.));
            if width != self.metrics.cell_width {
                self.metrics.cell_width = width;
                self.row_cache.clear();
                self.content_revision = self.content_revision.wrapping_add(1);
            }
        }
        // Pixel positions are floats; clamp before converting to protocol cell dimensions.
        let size = self
            .metrics
            .grid_size(bounds.size.width, bounds.size.height);
        let scale = window.scale_factor();
        self.links.geometry(links::Geometry {
            origin: (f32::from(bounds.origin.x), f32::from(bounds.origin.y)),
            extent: (f32::from(bounds.size.width), f32::from(bounds.size.height)),
            cell_width: f32::from(self.metrics.cell_width),
            scale,
            size: (size.cols, size.rows),
        });
        let mouse_width = f32::from(bounds.size.width)
            .min(f32::from(self.metrics.cell_width) * f32::from(size.cols))
            .max(1.)
            * scale;
        let mouse_height = f32::from(bounds.size.height)
            .min(f32::from(self.metrics.line_height) * f32::from(size.rows))
            .max(1.)
            * scale;
        let mouse_cell_width = f32::from(self.metrics.cell_width) * scale;
        let mouse_cell_height = f32::from(self.metrics.line_height) * scale;
        let result = self.engine.as_mut().map(|engine| {
            engine.set_mouse_geometry(
                mouse_width,
                mouse_height,
                mouse_cell_width,
                mouse_cell_height,
            )
        });
        self.operation(result);
        let size_changed = self.size != size;
        let pixel_cell = self.metrics.pixel_cell(scale);
        if size_changed || self.resized_pixel_cell != Some(pixel_cell) {
            self.row_cache.clear();
            let result = self
                .engine
                .as_mut()
                .map(|engine| engine.resize(size.cols, size.rows, pixel_cell.0, pixel_cell.1));
            if result.as_ref().is_some_and(|reply| reply.is_ok()) {
                self.resized_pixel_cell = Some(pixel_cell);
                self.size = size;
                self.geometry_ready = true;
            }
            if let Some(reply) = result.and_then(|result| self.output(result)) {
                self.send_bytes(reply);
            }
            self.content_revision += 1;
            self.search_wake(cx);
            if size_changed {
                self.report_size();
            }
            cx.notify();
        } else {
            self.geometry_ready = true;
        }
        if self.size_report_pending {
            self.report_size();
        }
    }

    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.geometry_ready {
            return;
        }
        // Element move/exit handlers can miss a held pointer while it crosses
        // another view. Observe window capture moves for the admitted link
        // gesture; a later return to the original cell must not reopen it.
        let terminal = cx.entity().downgrade();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let _ = terminal.update(cx, |this, cx| {
                if this.pointer.route(MouseButton::Left) != Some(PointerRoute::Link) {
                    return;
                }
                if this.link_cell_at(event.position).is_none() {
                    if this.links.cancel() {
                        cx.notify();
                    }
                } else {
                    this.links
                        .moving(this.pointer_position(event.position, window));
                }
            });
        });
        let started = Instant::now();
        self.frames += 1;
        self.last_paint = Some(started);
        if let Err(error) = self.paint_engine(window, cx) {
            self.status = format!("Terminal paint: {error:#}");
        } else {
            self.painted_revision = self.content_revision;
        }
        self.paint_micros = self
            .paint_micros
            .saturating_add(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX));
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(self.bounds, cx.entity()),
            cx,
        );
    }

    fn paint_engine(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<()> {
        let Some(engine) = self.engine.as_mut() else {
            return Err(anyhow!("Terminal engine is unavailable"));
        };
        let mut search_rows: HashMap<u16, Vec<&SearchHighlight>> = HashMap::new();
        if let Some(snapshot) = self
            .search
            .as_ref()
            .filter(|search| self.search_active && search.open)
            .and_then(|search| search.snapshot.as_ref())
            .filter(|snapshot| snapshot.revision == engine.search_revision())
        {
            for highlight in &snapshot.highlights {
                search_rows
                    .entry(highlight.row)
                    .or_default()
                    .push(highlight);
            }
        }
        let mut frame = engine.frame()?;
        if frame.full_dirty {
            self.row_cache.clear();
        }
        let full_dirty = frame.full_dirty;
        let base = frame.background;
        let cursor = frame.cursor;
        let current_frame = self.frames;
        window.paint_quad(fill(self.bounds, rgb(base)));
        while let Some(mut row) = frame.next_row()? {
            let index = row.index;
            let id = row.id;
            if needs_shape(self.row_cache.get(&id), row.dirty, full_dirty) {
                let cached = shape_row(
                    &mut row,
                    &self.face,
                    self.metrics,
                    base,
                    SelectionColors {
                        foreground: self.selection_foreground,
                        background: self.selection_background,
                    },
                    window,
                    current_frame,
                )?;
                self.shaped_rows = self.shaped_rows.saturating_add(1);
                self.shaped_runs = self
                    .shaped_runs
                    .saturating_add(u64::try_from(cached.runs.len()).unwrap_or(u64::MAX));
                self.row_cache.insert(id, cached);
            }
            if let Some(cached) = self.row_cache.get_mut(&id) {
                cached.last_frame = current_frame;
                paint_cached_row(
                    cached,
                    index,
                    self.bounds,
                    self.metrics,
                    SearchPaint {
                        highlights: search_rows
                            .get(&index)
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                        passive: self.colors.ansi[3],
                        active: self.colors.ansi[4],
                    },
                    window,
                    cx,
                )?;
            }
        }
        frame.finish()?;
        self.row_cache
            .retain(|_, row| row.last_frame == current_frame);
        paint_cursor(
            cursor,
            &self.composition,
            SelectionColors {
                foreground: self.selection_foreground,
                background: self.selection_background,
            },
            &self.face,
            CursorGeometry {
                bounds: self.bounds,
                metrics: self.metrics,
            },
            window,
            cx,
        )?;
        Ok(())
    }
}

fn needs_shape(cached: Option<&CachedRow>, row_dirty: bool, full_dirty: bool) -> bool {
    full_dirty || row_dirty || cached.is_none()
}

fn shape_row(
    row: &mut Row<'_>,
    face: &Font,
    metrics: TerminalMetrics,
    base_background: u32,
    selection: SelectionColors,
    window: &mut Window,
    frame: usize,
) -> Result<CachedRow> {
    let mut cached = CachedRow {
        last_frame: frame,
        backgrounds: Vec::new(),
        underlines: Vec::new(),
        runs: Vec::new(),
        selected: Vec::new(),
    };
    let mut pending: Option<PendingAscii> = None;
    while let Some(cell) = row.next_cell()? {
        if cell.skip {
            continue;
        }
        let width = u16::from(cell.width.max(1));
        if cell.selected {
            let end = cell.col.saturating_add(width);
            if let Some(last) = cached
                .selected
                .last_mut()
                .filter(|last| last.end == cell.col)
            {
                last.end = end;
            } else {
                cached.selected.push(cell.col..end);
            }
        }
        let background = if cell.selected {
            selection.background
        } else {
            cell.background
        };
        if background != base_background {
            push_background(&mut cached.backgrounds, cell.col, width, background);
        }
        if cell.underline {
            cached.underlines.push(UnderlineSpan {
                col: cell.col,
                width,
                color: if cell.selected {
                    selection.foreground
                } else {
                    cell.foreground
                },
            });
        }
        let visible = !cell.hidden && cell.text != " " && !cell.text.is_empty();
        let ascii = visible && width == 1 && cell.text.len() == 1 && cell.text.is_ascii();
        let style = RunStyle {
            foreground: if cell.selected {
                selection.foreground
            } else {
                cell.foreground
            },
            bold: cell.bold,
            italic: cell.italic,
        };
        if ascii {
            if let Some(run) = pending.as_mut() {
                if run.style == style
                    && usize::from(run.col) + run.text.len() == usize::from(cell.col)
                {
                    run.text.push_str(cell.text);
                    continue;
                }
            }
            flush_ascii(&mut pending, &mut cached.runs, face, metrics, window);
            pending = Some(PendingAscii {
                col: cell.col,
                text: cell.text.to_owned(),
                style,
            });
        } else {
            flush_ascii(&mut pending, &mut cached.runs, face, metrics, window);
            if visible {
                cached.runs.push(ShapedRun {
                    col: cell.col,
                    line: shape_text(cell.text, style, face, metrics.font_size, None, window),
                });
            }
        }
    }
    flush_ascii(&mut pending, &mut cached.runs, face, metrics, window);
    Ok(cached)
}

fn push_background(spans: &mut Vec<BackgroundSpan>, col: u16, width: u16, color: u32) {
    if let Some(last) = spans.last_mut() {
        if last.color == color && last.col.saturating_add(last.width) == col {
            last.width = last.width.saturating_add(width);
            return;
        }
    }
    spans.push(BackgroundSpan { col, width, color });
}

fn flush_ascii(
    pending: &mut Option<PendingAscii>,
    runs: &mut Vec<ShapedRun>,
    face: &Font,
    metrics: TerminalMetrics,
    window: &mut Window,
) {
    if let Some(run) = pending.take() {
        runs.push(ShapedRun {
            col: run.col,
            // GPUI fixes each glyph advance, not the total shaped line width.
            line: shape_text(
                &run.text,
                run.style,
                face,
                metrics.font_size,
                Some(metrics.cell_width),
                window,
            ),
        });
    }
}

fn shape_text(
    text: &str,
    style: RunStyle,
    base_face: &Font,
    font_size: Pixels,
    glyph_width: Option<Pixels>,
    window: &mut Window,
) -> ShapedLine {
    let mut face = base_face.clone();
    if style.bold {
        face.weight = FontWeight::BOLD;
    }
    if style.italic {
        face.style = FontStyle::Italic;
    }
    let run = TextRun {
        len: text.len(),
        font: face,
        color: rgb(style.foreground).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.to_owned().into(), font_size, &[run], glyph_width)
}

fn paint_cached_row(
    row: &CachedRow,
    index: u16,
    bounds: Bounds<Pixels>,
    metrics: TerminalMetrics,
    search: SearchPaint<'_>,
    window: &mut Window,
    cx: &mut Context<Terminal>,
) -> Result<()> {
    let y = bounds.top() + metrics.line_height * f32::from(index);
    for span in &row.backgrounds {
        let x = bounds.left() + metrics.cell_width * f32::from(span.col);
        window.paint_quad(fill(
            Bounds::new(
                point(x, y),
                size(
                    metrics.cell_width * f32::from(span.width),
                    metrics.line_height,
                ),
            ),
            rgb(span.color),
        ));
    }
    for highlight in search.highlights {
        let color = if highlight.selected {
            search.active
        } else {
            search.passive
        };
        each_unselected_segment(
            highlight.start_col,
            highlight.end_col,
            &row.selected,
            |segment| {
                let x = bounds.left() + metrics.cell_width * f32::from(segment.start);
                window.paint_quad(fill(
                    Bounds::new(
                        point(x, y),
                        size(
                            metrics.cell_width * f32::from(segment.end - segment.start),
                            metrics.line_height,
                        ),
                    ),
                    rgb(color).opacity(if highlight.selected { 0.55 } else { 0.35 }),
                ));
            },
        );
    }
    for run in &row.runs {
        let x = bounds.left() + metrics.cell_width * f32::from(run.col);
        run.line
            .paint(
                point(x, y),
                metrics.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .map_err(|error| anyhow!("Text paint: {error}"))?;
    }
    for span in &row.underlines {
        let x = bounds.left() + metrics.cell_width * f32::from(span.col);
        window.paint_quad(fill(
            Bounds::new(
                point(x, y + metrics.line_height - px(2. * metrics.scale)),
                size(
                    metrics.cell_width * f32::from(span.width),
                    px(metrics.scale),
                ),
            ),
            rgb(span.color),
        ));
    }
    Ok(())
}

fn each_unselected_segment(
    start: u16,
    end: u16,
    selected: &[Range<u16>],
    mut paint: impl FnMut(Range<u16>),
) {
    let mut col = start;
    for selection in selected {
        if selection.end <= col {
            continue;
        }
        if selection.start >= end {
            break;
        }
        if col < selection.start {
            paint(col..selection.start.min(end));
        }
        col = col.max(selection.end);
        if col >= end {
            return;
        }
    }
    if col < end {
        paint(col..end);
    }
}

fn paint_cursor(
    cursor: Cursor,
    composition: &str,
    selection: SelectionColors,
    face: &Font,
    geometry: CursorGeometry,
    window: &mut Window,
    cx: &mut Context<Terminal>,
) -> Result<()> {
    if !cursor.visible {
        return Ok(());
    }
    let metrics = geometry.metrics;
    let hairline = px(metrics.scale.max(1.));
    let bar = px((2. * metrics.scale).max(1.));
    let origin = point(
        geometry.bounds.left() + metrics.cell_width * f32::from(cursor.col),
        geometry.bounds.top() + metrics.line_height * f32::from(cursor.row),
    );
    let color = rgb(cursor.color);
    match cursor.shape {
        CursorShape::Block => {
            window.paint_quad(fill(
                Bounds::new(origin, size(metrics.cell_width, metrics.line_height)),
                color.opacity(0.35),
            ));
        }
        CursorShape::Bar => {
            window.paint_quad(fill(
                Bounds::new(origin, size(bar, metrics.line_height)),
                color,
            ));
        }
        CursorShape::Underline => {
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(px(0.), metrics.line_height - bar),
                    size(metrics.cell_width, bar),
                ),
                color,
            ));
        }
        CursorShape::Hollow => {
            window.paint_quad(fill(
                Bounds::new(origin, size(metrics.cell_width, hairline)),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(px(0.), metrics.line_height - hairline),
                    size(metrics.cell_width, hairline),
                ),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(origin, size(hairline, metrics.line_height)),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(metrics.cell_width - hairline, px(0.)),
                    size(hairline, metrics.line_height),
                ),
                color,
            ));
        }
    }
    if !composition.is_empty() {
        let text: SharedString = composition.to_owned().into();
        let run = TextRun {
            len: text.len(),
            font: face.clone(),
            color: rgb(selection.foreground).into(),
            background_color: Some(rgb(selection.background).into()),
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(text, metrics.font_size, &[run], None)
            .paint(
                origin,
                metrics.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .map_err(|error| anyhow!("Composition paint: {error}"))?;
    }
    Ok(())
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_out_subscription.is_none() {
            self.focus_out_subscription =
                Some(cx.on_focus_out(&self.focus, window, |this, _, _, _| this.cancel_pointer()));
        }
        if self.activation_subscription.is_none() {
            self.activation_subscription =
                Some(cx.observe_window_activation(window, |this, window, _| {
                    if !window.is_window_active() {
                        this.cancel_pointer();
                    }
                }));
        }
        let search_bar = self.search_bar(cx);
        let link_preview = self.links.preview().map(|target| {
            let modifier = if cx.platform_info().is_mac() {
                "Cmd"
            } else {
                "Ctrl"
            };
            format!("{}  ·  {modifier}-click to open", target.url.as_str())
        });
        let layout = cx.entity();
        let paint = layout.clone();
        div()
            .id("terminal")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(self.colors.background))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_modifiers_changed(cx.listener(Self::on_link_modifiers_changed))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_pointer_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_pointer_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_pointer_down))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(Self::on_pointer_down),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(Self::on_pointer_down),
            )
            .on_mouse_move(cx.listener(Self::on_pointer_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_pointer_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_pointer_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_pointer_up))
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(Self::on_pointer_up),
            )
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(Self::on_pointer_up),
            )
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_pointer_up_out))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_pointer_up_out))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_pointer_up_out))
            .on_mouse_up_out(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(Self::on_pointer_up_out),
            )
            .on_mouse_up_out(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(Self::on_pointer_up_out),
            )
            .on_mouse_exit(cx.listener(Self::on_pointer_exit))
            .on_scroll(cx.listener(Self::on_scroll_event))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        layout.update(cx, |this, cx| this.layout(bounds, window, cx))
                    },
                    move |_, (), window, cx| paint.update(cx, |this, cx| this.paint(window, cx)),
                )
                .size_full(),
            )
            .when_some(link_preview, |view, preview| {
                view.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .max_w_full()
                        .px_2()
                        .py_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(rems(12. / 16.))
                        .bg(rgb(self.colors.background))
                        .text_color(rgb(self.colors.foreground))
                        .child(preview),
                )
            })
            .when_some(search_bar, |view, bar| view.child(bar))
            .when_some(self.command_error.clone(), |view, error| {
                view.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .px_2()
                        .py_1()
                        .text_size(rems(12. / 16.))
                        .bg(rgb(0x612b32))
                        .text_color(rgb(0xffe4e6))
                        .child(error),
                )
            })
    }
}

impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let chars: Vec<u16> = self.composition.encode_utf16().collect();
        let range = range.start.min(chars.len())..range.end.min(chars.len());
        *adjusted = Some(range.clone());
        String::from_utf16(&chars[range]).ok()
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.composition.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composition.is_empty()).then(|| 0..self.composition.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.composition.clear();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.len() > MAX_INPUT {
            self.reject_command("Input exceeds the 64 KiB limit".into());
            cx.notify();
            return;
        }
        self.composition.clear();
        let result = self.engine.as_mut().map(Engine::scroll_bottom);
        self.operation(result);
        self.links.cancel();
        self.search_wake(cx);
        self.send(Command::Input {
            generation: self.generation,
            bytes: text.as_bytes().to_vec(),
        });
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.len() <= MAX_INPUT {
            self.composition = text.to_owned();
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if !self.geometry_ready {
            return None;
        }
        let cursor = self.engine.as_mut()?.cursor().ok()?;
        if !cursor.visible {
            return None;
        }
        Some(Bounds::new(
            self.bounds.origin
                + point(
                    self.metrics.cell_width * f32::from(cursor.col),
                    self.metrics.line_height * f32::from(cursor.row),
                ),
            size(self.metrics.cell_width, self.metrics.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }

    fn paste(&mut self, item: ClipboardItem, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.paste_text(&text);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        alternate_wheel_input, each_unselected_segment, engine_mouse_button, link_modifier,
        mouse_modifiers, needs_shape, wheel_button, wheel_lines, wheel_route, CachedRow,
        PointerGestures, PointerRoute, TerminalMetrics, WheelRoute,
    };
    use crate::terminal_engine::{
        Engine, MouseButton as EngineMouseButton, MouseModifiers, MousePolicy, MouseTracking,
    };
    use gpui_kit::{px, Modifiers, MouseButton, NavigationDirection};

    #[test]
    fn scaled_text_grid_and_hit_cells_share_the_same_metrics() {
        for (scale, font, row) in [(0.5, 7.5, 10.5), (1.25, 18.75, 26.25), (3., 45., 63.)] {
            let mut metrics = TerminalMetrics::new(scale);
            metrics.cell_width = px(9. * scale);
            assert_eq!(f32::from(metrics.font_size), font);
            assert_eq!(f32::from(metrics.line_height), row);
            let grid = metrics.grid_size(px(90. * scale), px(84. * scale));
            assert_eq!((grid.cols, grid.rows), (10, 4));
            let cell = metrics
                .hit_cell(3.5 * 9. * scale, 2.5 * row, grid)
                .expect("point in scaled grid");
            assert_eq!((cell.col, cell.row), (3, 2));
            assert!(metrics
                .hit_cell(10. * 9. * scale, 2.5 * row, grid)
                .is_none());
        }
    }

    #[test]
    fn pixel_cell_change_refreshes_vt_even_when_grid_dimensions_match() {
        let baseline = TerminalMetrics::new(1.);
        let enlarged = TerminalMetrics::new(1.25);
        let small_bounds = (px(20.), px(20.));
        assert_eq!(
            baseline.grid_size(small_bounds.0, small_bounds.1),
            enlarged.grid_size(small_bounds.0, small_bounds.1)
        );
        assert_ne!(baseline.pixel_cell(1.), enlarged.pixel_cell(1.));
        assert_ne!(baseline.pixel_cell(1.), baseline.pixel_cell(1.5));
        assert_eq!(TerminalMetrics::new(f32::NAN).scale, 1.);
    }

    #[test]
    fn link_modifier_matches_host_shortcut_without_claiming_shift_selection() {
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        let command = Modifiers {
            platform: true,
            ..Modifiers::default()
        };
        assert!(link_modifier(ctrl, false));
        assert!(!link_modifier(command, false));
        assert!(link_modifier(command, true));
        assert!(!link_modifier(ctrl, true));
        assert!(!link_modifier(
            Modifiers {
                shift: true,
                ..ctrl
            },
            false
        ));
    }

    #[test]
    fn search_highlights_skip_clipboard_selection_without_changing_glyph_runs() {
        let mut visible = Vec::new();
        each_unselected_segment(2, 12, &[4..6, 8..10], |segment| visible.push(segment));
        assert_eq!(visible, vec![2..4, 6..8, 10..12]);
        visible.clear();
        each_unselected_segment(3, 7, &[0..5, 6..9], |segment| visible.push(segment));
        assert_eq!(visible, vec![5..6]);
    }

    #[test]
    fn pointer_gesture_keeps_its_press_route_and_suppresses_rejected_presses() {
        let mut gestures = PointerGestures::default();
        assert_eq!(
            gestures.begin(MouseButton::Left, true, true),
            PointerRoute::Selection
        );
        assert!(!gestures.any_application_pressed());
        assert_eq!(
            gestures.route(MouseButton::Left),
            Some(PointerRoute::Selection)
        );
        // Releasing Shift does not change the captured selection route.
        assert_eq!(
            gestures.take(MouseButton::Left),
            Some(PointerRoute::Selection)
        );
        assert_eq!(gestures.take(MouseButton::Left), None);

        // Link admission consumes the whole left gesture locally, including a
        // release after the modifier changes; application motion stays silent.
        gestures.capture_link();
        assert_eq!(gestures.route(MouseButton::Left), Some(PointerRoute::Link));
        assert!(gestures.any_local_pressed());
        assert!(!gestures.any_application_pressed());
        assert_eq!(gestures.take(MouseButton::Left), Some(PointerRoute::Link));
        assert_eq!(gestures.take(MouseButton::Left), None);
        assert!(gestures.unadmitted_button_pressed(Some(MouseButton::Left)));
        assert!(!gestures.unadmitted_button_pressed(None));

        assert_eq!(
            gestures.begin(MouseButton::Right, false, true),
            PointerRoute::Application
        );
        assert!(gestures.any_application_pressed());
        // An empty encoding or rejected command keeps a local-only latch so
        // any-motion tracking cannot leak hover reports during the held press.
        gestures.reject_application(MouseButton::Right);
        assert!(!gestures.any_application_pressed());
        assert!(gestures.any_local_pressed());
        assert_eq!(gestures.take(MouseButton::Right), Some(PointerRoute::Local));
        assert_eq!(gestures.take(MouseButton::Right), None);

        assert_eq!(
            gestures.begin(MouseButton::Middle, true, true),
            PointerRoute::Local
        );
        assert!(gestures.any_local_pressed());
        assert!(!gestures.any_application_pressed());

        // A captured application route remains distinct from local gestures,
        // then clearing the terminal drains each button only once.
        assert_eq!(
            gestures.begin(
                MouseButton::Navigate(NavigationDirection::Back),
                false,
                true
            ),
            PointerRoute::Application
        );
        assert_eq!(
            gestures.route(MouseButton::Navigate(NavigationDirection::Back)),
            Some(PointerRoute::Application)
        );
        assert_eq!(
            gestures.take(MouseButton::Middle),
            Some(PointerRoute::Local)
        );
        assert_eq!(
            gestures.take(MouseButton::Navigate(NavigationDirection::Back)),
            Some(PointerRoute::Application)
        );
        assert!(!gestures.any_application_pressed());
    }

    #[test]
    fn mouse_buttons_and_modifiers_keep_native_protocol_meaning() {
        assert_eq!(
            engine_mouse_button(MouseButton::Middle),
            EngineMouseButton::Middle
        );
        assert_eq!(
            engine_mouse_button(MouseButton::Right),
            EngineMouseButton::Right
        );
        assert_eq!(
            engine_mouse_button(MouseButton::Navigate(NavigationDirection::Back)),
            EngineMouseButton::Back
        );
        assert_eq!(
            engine_mouse_button(MouseButton::Navigate(NavigationDirection::Forward)),
            EngineMouseButton::Forward
        );
        assert_eq!(wheel_button(1, false), EngineMouseButton::WheelUp);
        assert_eq!(wheel_button(-1, false), EngineMouseButton::WheelDown);
        assert_eq!(wheel_button(1, true), EngineMouseButton::WheelLeft);
        assert_eq!(wheel_button(-1, true), EngineMouseButton::WheelRight);
        let mut horizontal_remainder = 0.;
        let mut vertical_remainder = 0.;
        assert_eq!(wheel_lines(&mut horizontal_remainder, 0.6, true), 0);
        assert_eq!(wheel_lines(&mut vertical_remainder, -1.2, true), -1);
        assert_eq!(wheel_lines(&mut horizontal_remainder, 0.6, true), 1);
        assert_eq!(wheel_lines(&mut vertical_remainder, 0., true), 0);
        let modifiers = Modifiers {
            shift: true,
            control: true,
            alt: true,
            platform: true,
            ..Modifiers::default()
        };
        assert_eq!(
            mouse_modifiers(modifiers),
            MouseModifiers {
                shift: true,
                control: true,
                alt: true,
                super_key: true,
            }
        );
        assert_eq!(
            mouse_modifiers(Modifiers::default()),
            MouseModifiers::default()
        );
    }

    #[test]
    fn wheel_policy_leaves_x10_and_shift_on_local_paths() {
        for tracking in [MouseTracking::None, MouseTracking::X10] {
            let policy = MousePolicy { tracking };
            assert_eq!(wheel_route(policy, false, false), WheelRoute::Scrollback);
            assert_eq!(wheel_route(policy, true, false), WheelRoute::AlternateKeys);
        }
        for tracking in [
            MouseTracking::Normal,
            MouseTracking::Button,
            MouseTracking::Any,
        ] {
            let policy = MousePolicy { tracking };
            assert_eq!(wheel_route(policy, true, false), WheelRoute::Application);
            assert_eq!(wheel_route(policy, false, false), WheelRoute::Application);
            assert_eq!(wheel_route(policy, true, true), WheelRoute::Scrollback);
        }
    }

    #[test]
    fn alternate_wheel_accumulates_fraction_without_replaying_large_gestures() {
        let mut remainder = 0.;
        assert_eq!(wheel_lines(&mut remainder, 0.4, true), 0);
        assert_eq!(wheel_lines(&mut remainder, 0.7, true), 1);
        assert!((remainder - 0.1).abs() < 0.0001);
        assert_eq!(wheel_lines(&mut remainder, -0.7, true), 0);
        assert_eq!(wheel_lines(&mut remainder, -0.6, true), -1);
        assert_eq!(wheel_lines(&mut remainder, 100., true), 5);
        assert_eq!(
            wheel_lines(&mut remainder, 0., true),
            0,
            "excess lines are dropped"
        );
        remainder = 0.; // The view resets it when the active screen changes.
        assert_eq!(
            wheel_lines(&mut remainder, -9., false),
            -9,
            "normal scrollback retains its line count"
        );
        assert_eq!(wheel_lines(&mut remainder, f32::INFINITY, true), 0);
        assert_eq!(remainder, 0.);
    }

    #[test]
    fn alternate_wheel_uses_ghostty_key_mode_for_bounded_pty_input() {
        let mut engine = Engine::new(40, 8).expect("Ghostty engine");
        engine
            .feed(b"\x1b[?1049h", false)
            .expect("alternate screen");
        assert_eq!(
            alternate_wheel_input(&mut engine, 3).unwrap(),
            b"\x1b[A\x1b[A\x1b[A"
        );
        engine
            .feed(b"\x1b[?1h", false)
            .expect("application cursor keys");
        assert_eq!(
            alternate_wheel_input(&mut engine, -2).unwrap(),
            b"\x1bOB\x1bOB"
        );
    }

    #[test]
    fn row_cache_reuses_clean_snapshot_and_rebuilds_mutated_row() {
        let mut engine = Engine::new(8, 2).expect("native terminal");
        let mut first = engine.frame().expect("first frame");
        let first_full_dirty = first.full_dirty;
        let id = {
            let row = first.next_row().expect("first row").expect("visible row");
            assert!(needs_shape(None, row.dirty, first_full_dirty));
            row.id
        };
        while first.next_row().expect("remaining row").is_some() {}
        first.finish().expect("first frame clean");

        let mut second = engine.frame().expect("second frame");
        let second_full_dirty = second.full_dirty;
        let cached = CachedRow {
            last_frame: 1,
            backgrounds: Vec::new(),
            underlines: Vec::new(),
            runs: Vec::new(),
            selected: Vec::new(),
        };
        {
            let row = second.next_row().expect("first row").expect("visible row");
            assert_eq!(row.id, id);
            assert!(!needs_shape(Some(&cached), row.dirty, second_full_dirty));
        }
        while second.next_row().expect("remaining row").is_some() {}
        second.finish().expect("second frame clean");

        engine.feed(b"X", false).expect("write");
        let mut third = engine.frame().expect("changed frame");
        let row = third.next_row().expect("first row").expect("visible row");
        assert!(needs_shape(Some(&cached), row.dirty, third.full_dirty));
    }
}
