//! Native terminal presentation. The service owns the PTY; Ghostty owns VT state.
use crate::terminal_engine::{Cursor, CursorShape, Engine, Row, MAX_INPUT};
use anyhow::{anyhow, Result};
use gpui_kit::{prelude::*, *};
use std::{collections::HashMap, ops::Range, time::Instant};

const FONT_SIZE: f32 = 15.;
const LINE_HEIGHT: f32 = 21.;
const SELECTION_BACKGROUND: u32 = 0x345778;

#[derive(Debug)]
pub enum Command {
    Input { generation: u64, text: String },
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
    cell_width: Pixels,
    face: Font,
    size: Size,
    generation: u64,
    composition: String,
    selecting: bool,
    scroll_remainder: f32,
    row_cache: HashMap<[u64; 2], CachedRow>,
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
    pub fn new(
        commands: impl Fn(Command) -> std::result::Result<(), String> + 'static,
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
            cell_width: px(9.),
            // GPUI font lookup takes concrete family names, not CSS generic families.
            face: font(gpui_kit::base::Theme::global(cx).tokens.typography.mono),
            size,
            generation: 0,
            composition: String::new(),
            selecting: false,
            scroll_remainder: 0.,
            row_cache: HashMap::new(),
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

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    pub fn painted_current(&self) -> bool {
        self.engine.is_some() && self.frames > 0 && self.painted_revision == self.content_revision
    }

    pub fn engine_ready(&self) -> bool {
        self.engine.is_some()
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
        cx.notify();
    }

    pub fn reset(&mut self, generation: u64, cx: &mut Context<Self>) {
        self.generation = generation;
        self.frames = 0;
        self.content_revision += 1;
        self.painted_revision = 0;
        self.row_cache.clear();
        self.composition.clear();
        self.saw_smoke = false;
        self.smoke_tail.clear();
        self.selecting = false;
        let result = if let Some(engine) = self.engine.as_mut() {
            engine.reset()
        } else {
            Engine::new(self.size.cols, self.size.rows).map(|engine| {
                self.engine = Some(engine);
            })
        };
        if let Err(error) = result {
            self.engine = None;
            self.status = format!("Terminal engine reset: {error:#}");
        }
        cx.notify();
    }

    pub fn report_size(&mut self) {
        self.send(Command::Resize(self.size.cols, self.size.rows));
    }

    fn send(&mut self, command: Command) {
        let is_input = matches!(&command, Command::Input { .. });
        if let Command::Input { text, .. } = &command {
            if text.len() > MAX_INPUT {
                self.reject_command("Input exceeds the 64 KiB limit".into());
                return;
            }
        }
        match (self.commands)(command) {
            Ok(()) if is_input => self.command_error = None,
            Ok(()) => {}
            Err(error) => {
                let action = if is_input { "Input" } else { "Resize" };
                self.reject_command(format!("{action} not sent: {error}"));
            }
        }
    }

    fn reject_command(&mut self, error: String) {
        self.status.clone_from(&error);
        self.command_error = Some(error);
    }

    fn send_bytes(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        match String::from_utf8(bytes) {
            Ok(text) => self.send(Command::Input {
                generation: self.generation,
                text,
            }),
            Err(error) => self.status = format!("Terminal output is not UTF-8: {error}"),
        }
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
        let col = ((position.x - self.bounds.left()) / self.cell_width)
            .floor()
            .clamp(0., f32::from(self.size.cols - 1)) as u16;
        let row = ((position.y - self.bounds.top()) / px(LINE_HEIGHT))
            .floor()
            .clamp(0., f32::from(self.size.rows - 1)) as u16;
        (col, row)
    }

    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.bounds = bounds;
        let font_id = window.text_system().resolve_font(&self.face);
        if let Ok(advance) = window.text_system().advance(font_id, px(FONT_SIZE), 'M') {
            let width = advance.width.max(px(1.));
            if width != self.cell_width {
                self.cell_width = width;
                self.row_cache.clear();
            }
        }
        // Pixel positions are floats; clamp before converting to protocol cell dimensions.
        let size = Size {
            cols: (bounds.size.width / self.cell_width)
                .floor()
                .clamp(2., 400.) as u16,
            rows: (bounds.size.height / px(LINE_HEIGHT))
                .floor()
                .clamp(1., 200.) as u16,
        };
        if self.size != size {
            self.size = size;
            self.row_cache.clear();
            let cell_width = (f32::from(self.cell_width) * window.scale_factor()).round() as u32;
            let cell_height = (LINE_HEIGHT * window.scale_factor()).round() as u32;
            let result = self
                .engine
                .as_mut()
                .map(|engine| engine.resize(size.cols, size.rows, cell_width, cell_height));
            if let Some(reply) = result.and_then(|result| self.output(result)) {
                self.send_bytes(reply);
            }
            self.content_revision += 1;
            self.report_size();
            cx.notify();
        }
    }

    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    self.cell_width,
                    base,
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
                paint_cached_row(cached, index, self.bounds, self.cell_width, window, cx)?;
            }
        }
        frame.finish()?;
        self.row_cache
            .retain(|_, row| row.last_frame == current_frame);
        paint_cursor(
            cursor,
            &self.composition,
            &self.face,
            self.bounds,
            self.cell_width,
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
    cell_width: Pixels,
    base_background: u32,
    window: &mut Window,
    frame: usize,
) -> Result<CachedRow> {
    let mut cached = CachedRow {
        last_frame: frame,
        backgrounds: Vec::new(),
        underlines: Vec::new(),
        runs: Vec::new(),
    };
    let mut pending: Option<PendingAscii> = None;
    while let Some(cell) = row.next_cell()? {
        if cell.skip {
            continue;
        }
        let width = u16::from(cell.width.max(1));
        let background = if cell.selected {
            SELECTION_BACKGROUND
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
                color: cell.foreground,
            });
        }
        let visible = !cell.hidden && cell.text != " " && !cell.text.is_empty();
        let ascii = visible && width == 1 && cell.text.len() == 1 && cell.text.is_ascii();
        let style = RunStyle {
            foreground: cell.foreground,
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
            flush_ascii(&mut pending, &mut cached.runs, face, cell_width, window);
            pending = Some(PendingAscii {
                col: cell.col,
                text: cell.text.to_owned(),
                style,
            });
        } else {
            flush_ascii(&mut pending, &mut cached.runs, face, cell_width, window);
            if visible {
                cached.runs.push(ShapedRun {
                    col: cell.col,
                    line: shape_text(cell.text, style, face, None, window),
                });
            }
        }
    }
    flush_ascii(&mut pending, &mut cached.runs, face, cell_width, window);
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
    cell_width: Pixels,
    window: &mut Window,
) {
    if let Some(run) = pending.take() {
        runs.push(ShapedRun {
            col: run.col,
            // GPUI fixes each glyph advance, not the total shaped line width.
            line: shape_text(&run.text, run.style, face, Some(cell_width), window),
        });
    }
}

fn shape_text(
    text: &str,
    style: RunStyle,
    base_face: &Font,
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
        .shape_line(text.to_owned().into(), px(FONT_SIZE), &[run], glyph_width)
}

fn paint_cached_row(
    row: &CachedRow,
    index: u16,
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    window: &mut Window,
    cx: &mut Context<Terminal>,
) -> Result<()> {
    let y = bounds.top() + px(LINE_HEIGHT) * f32::from(index);
    for span in &row.backgrounds {
        let x = bounds.left() + cell_width * f32::from(span.col);
        window.paint_quad(fill(
            Bounds::new(
                point(x, y),
                size(cell_width * f32::from(span.width), px(LINE_HEIGHT)),
            ),
            rgb(span.color),
        ));
    }
    for run in &row.runs {
        let x = bounds.left() + cell_width * f32::from(run.col);
        run.line
            .paint(
                point(x, y),
                px(LINE_HEIGHT),
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .map_err(|error| anyhow!("Text paint: {error}"))?;
    }
    for span in &row.underlines {
        let x = bounds.left() + cell_width * f32::from(span.col);
        window.paint_quad(fill(
            Bounds::new(
                point(x, y + px(LINE_HEIGHT - 2.)),
                size(cell_width * f32::from(span.width), px(1.)),
            ),
            rgb(span.color),
        ));
    }
    Ok(())
}

fn paint_cursor(
    cursor: Cursor,
    composition: &str,
    face: &Font,
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    window: &mut Window,
    cx: &mut Context<Terminal>,
) -> Result<()> {
    if !cursor.visible {
        return Ok(());
    }
    let origin = point(
        bounds.left() + cell_width * f32::from(cursor.col),
        bounds.top() + px(LINE_HEIGHT) * f32::from(cursor.row),
    );
    let color = rgb(cursor.color);
    match cursor.shape {
        CursorShape::Block => {
            window.paint_quad(fill(
                Bounds::new(origin, size(cell_width, px(LINE_HEIGHT))),
                color.opacity(0.35),
            ));
        }
        CursorShape::Bar => {
            window.paint_quad(fill(
                Bounds::new(origin, size(px(2.), px(LINE_HEIGHT))),
                color,
            ));
        }
        CursorShape::Underline => {
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(px(0.), px(LINE_HEIGHT - 2.)),
                    size(cell_width, px(2.)),
                ),
                color,
            ));
        }
        CursorShape::Hollow => {
            window.paint_quad(fill(Bounds::new(origin, size(cell_width, px(1.))), color));
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(px(0.), px(LINE_HEIGHT - 1.)),
                    size(cell_width, px(1.)),
                ),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(origin, size(px(1.), px(LINE_HEIGHT))),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(cell_width - px(1.), px(0.)),
                    size(px(1.), px(LINE_HEIGHT)),
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
            color: rgb(0xffffff).into(),
            background_color: Some(rgb(SELECTION_BACKGROUND).into()),
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(text, px(FONT_SIZE), &[run], None)
            .paint(origin, px(LINE_HEIGHT), TextAlign::Left, None, window, cx)
            .map_err(|error| anyhow!("Composition paint: {error}"))?;
    }
    Ok(())
}

impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = cx.entity();
        let paint = layout.clone();
        div()
            .id("terminal")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x10151c))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus(window, cx);
                    this.selecting = true;
                    let (col, row) = this.cell_at(event.position);
                    let result = this
                        .engine
                        .as_mut()
                        .map(|engine| engine.select_start(col, row, event.click_count >= 2));
                    this.operation(result);
                    this.row_cache.clear();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.selecting && event.dragging() {
                    let (col, row) = this.cell_at(event.position);
                    let result = this
                        .engine
                        .as_mut()
                        .map(|engine| engine.select_update(col, row));
                    this.operation(result);
                    this.row_cache.clear();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.selecting = false;
                    let result = this.engine.as_mut().map(Engine::select_end);
                    this.operation(result);
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                this.scroll_remainder += match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y) / LINE_HEIGHT,
                    ScrollDelta::Lines(delta) => delta.y,
                };
                let lines = this.scroll_remainder.trunc() as i32;
                this.scroll_remainder -= lines as f32;
                if lines != 0 {
                    let result = this.engine.as_mut().map(|engine| engine.scroll(lines));
                    this.operation(result);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        layout.update(cx, |this, cx| this.layout(bounds, window, cx))
                    },
                    move |_, (), window, cx| paint.update(cx, |this, cx| this.paint(window, cx)),
                )
                .size_full(),
            )
            .when_some(self.command_error.clone(), |view, error| {
                view.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .px_2()
                        .py_1()
                        .text_size(px(12.))
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
        self.send(Command::Input {
            generation: self.generation,
            text: text.to_owned(),
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
        let cursor = self.engine.as_mut()?.cursor().ok()?;
        if !cursor.visible {
            return None;
        }
        Some(Bounds::new(
            self.bounds.origin
                + point(
                    self.cell_width * f32::from(cursor.col),
                    px(LINE_HEIGHT) * f32::from(cursor.row),
                ),
            size(self.cell_width, px(LINE_HEIGHT)),
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
    use super::{needs_shape, CachedRow};
    use crate::terminal_engine::Engine;

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
