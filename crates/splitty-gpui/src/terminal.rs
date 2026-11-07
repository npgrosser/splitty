//! A deliberately small native terminal renderer; the service owns the PTY.
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as CellPoint, Side},
    selection::{Selection, SelectionType},
    term::{cell::Flags, Config, Osc52, TermMode},
    vte::ansi::{Color, CursorShape, NamedColor, Processor},
    Term,
};
use gpui_kit::{prelude::*, *};
use std::{
    ops::Range,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::mpsc;

const FONT_SIZE: f32 = 15.;
const LINE_HEIGHT: f32 = 21.;
const MAX_INPUT: usize = 64 * 1024;

#[derive(Debug)]
pub enum Command {
    Input { generation: u64, text: String },
    Resize(u16, u16),
}

#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<String>>>);
impl Listener {
    fn replies(&self, replay: bool) -> Vec<String> {
        let mut replies = self
            .0
            .lock()
            .map(|mut replies| std::mem::take(&mut *replies))
            .unwrap_or_default();
        if replay {
            replies.clear();
        }
        replies
    }
}
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        // OSC52 is disabled. Only terminal protocol replies can cross this sink.
        if let Event::PtyWrite(reply) = event {
            if let Ok(mut replies) = self.0.lock() {
                if replies.len() < 64 {
                    replies.push(reply);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Size {
    cols: u16,
    rows: u16,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        usize::from(self.rows)
    }
    fn screen_lines(&self) -> usize {
        usize::from(self.rows)
    }
    fn columns(&self) -> usize {
        usize::from(self.cols)
    }
}

pub struct Terminal {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
    commands: mpsc::Sender<Command>,
    focus: FocusHandle,
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    face: Font,
    size: Size,
    generation: u64,
    composition: String,
    selecting: bool,
    scroll_remainder: f32,
    pub status: String,
    pub bytes: usize,
    pub frames: usize,
    content_revision: u64,
    painted_revision: u64,
    pub saw_smoke: bool,
    smoke_tail: String,
    pub last_paint: Option<Instant>,
}

impl Terminal {
    pub fn new(commands: mpsc::Sender<Command>, cx: &mut Context<Self>) -> Self {
        let listener = Listener::default();
        let size = Size { cols: 80, rows: 24 };
        Self {
            term: Term::new(
                Config {
                    scrolling_history: 5000,
                    osc52: Osc52::Disabled,
                    ..Config::default()
                },
                &size,
                listener.clone(),
            ),
            parser: Processor::new(),
            listener,
            commands,
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
            status: "Connecting…".into(),
            bytes: 0,
            frames: 0,
            content_revision: 1,
            painted_revision: 0,
            saw_smoke: false,
            smoke_tail: String::new(),
            last_paint: None,
        }
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }
    pub fn painted_current(&self) -> bool {
        self.frames > 0 && self.painted_revision == self.content_revision
    }
    pub fn feed(&mut self, bytes: &[u8], replay: bool, cx: &mut Context<Self>) {
        self.bytes += bytes.len();
        self.content_revision += 1;
        self.parser.advance(&mut self.term, bytes);
        for reply in self.listener.replies(replay) {
            self.send(Command::Input {
                generation: self.generation,
                text: reply,
            });
        }
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
        self.term = Term::new(
            Config {
                scrolling_history: 5000,
                osc52: Osc52::Disabled,
                ..Config::default()
            },
            &self.size,
            self.listener.clone(),
        );
        self.parser = Processor::new();
        self.composition.clear();
        self.saw_smoke = false;
        self.smoke_tail.clear();
        self.selecting = false;
        if let Ok(mut replies) = self.listener.0.lock() {
            replies.clear();
        }
        cx.notify();
    }
    pub fn report_size(&mut self) {
        self.send(Command::Resize(self.size.cols, self.size.rows));
    }
    fn send(&mut self, command: Command) {
        if let Command::Input { text, .. } = &command {
            if text.len() > MAX_INPUT {
                self.status = "Input exceeds the 64 KiB probe limit".into();
                return;
            }
        }
        if let Err(error) = self.commands.try_send(command) {
            self.status = format!("Input not sent: {error}");
        }
    }
    fn paste_text(&mut self, text: &str) {
        if text.len() > MAX_INPUT - 12 {
            self.status = "Paste exceeds the 64 KiB probe limit".into();
            return;
        }
        // ESC inside pasted text must not terminate bracketed-paste framing.
        let text = text
            .replace('\u{1b}', "")
            .replace("\r\n", "\n")
            .replace('\n', "\r");
        let text = if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\u{1b}[200~{text}\u{1b}[201~")
        } else {
            text
        };
        self.send(Command::Input {
            generation: self.generation,
            text,
        });
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if (key.modifiers.control && key.modifiers.shift) || key.modifiers.platform {
            match key.key.as_str() {
                "c" => {
                    if let Some(text) = self.term.selection_to_string() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    cx.stop_propagation();
                    return;
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.paste_text(&text);
                    }
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        if let Some(text) = encode_key(
            &key.key,
            key.modifiers.control,
            key.modifiers.alt,
            key.modifiers.shift,
            self.term.mode().contains(TermMode::APP_CURSOR),
        ) {
            self.term.scroll_display(Scroll::Bottom);
            self.send(Command::Input {
                generation: self.generation,
                text,
            });
            window.notify_text_input_changed(&self.focus);
            cx.stop_propagation();
            cx.notify();
        }
        // Printable text is delivered by the platform input handler, including IME/dead keys.
    }
    fn cell_at(&self, position: Point<Pixels>) -> CellPoint {
        let col = ((position.x - self.bounds.left()) / self.cell_width)
            .floor()
            .max(0.) as usize;
        let row = ((position.y - self.bounds.top()) / px(LINE_HEIGHT))
            .floor()
            .max(0.) as i32;
        let offset = i32::try_from(self.term.grid().display_offset()).unwrap_or(i32::MAX);
        CellPoint::new(
            Line(row.min(i32::from(self.size.rows) - 1) - offset),
            Column(col.min(usize::from(self.size.cols) - 1)),
        )
    }
    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.bounds = bounds;
        let font_id = window.text_system().resolve_font(&self.face);
        if let Ok(advance) = window.text_system().advance(font_id, px(FONT_SIZE), 'M') {
            self.cell_width = advance.width.max(px(1.));
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
            self.term.resize(size);
            self.content_revision += 1;
            self.report_size();
            cx.notify();
        }
    }
    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.frames += 1;
        self.last_paint = Some(Instant::now());
        let content = self.term.renderable_content();
        let offset = i32::try_from(content.display_offset).unwrap_or(i32::MAX);
        let cursor = content.cursor;
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row >= i32::from(self.size.rows) {
                continue;
            }
            let cell = indexed.cell;
            let x = self.bounds.left() + self.cell_width * indexed.point.column.0 as f32;
            let y = self.bounds.top() + px(LINE_HEIGHT) * row as f32;
            let width = self.cell_width
                * if cell.flags.contains(Flags::WIDE_CHAR) {
                    2.
                } else {
                    1.
                };
            let mut foreground = resolve_color(cell.fg, content.colors);
            let mut background = resolve_color(cell.bg, content.colors);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            if content
                .selection
                .is_some_and(|range| range.contains(indexed.point))
            {
                background = rgb(0x345778).into();
            }
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(width, px(LINE_HEIGHT))),
                background,
            ));
            if cell.flags.intersects(
                Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN,
            ) {
                continue;
            }
            let mut text = cell.c.to_string();
            if let Some(extra) = cell.zerowidth() {
                text.extend(extra);
            }
            if text == " " {
                continue;
            }
            let mut face = self.face.clone();
            if cell.flags.contains(Flags::BOLD) {
                face.weight = FontWeight::BOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                face.style = FontStyle::Italic;
            }
            let run = TextRun {
                len: text.len(),
                font: face,
                color: foreground,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                text.into(),
                px(FONT_SIZE),
                &[run],
                Some(self.cell_width),
            );
            if let Err(error) = line.paint(
                point(x, y),
                px(LINE_HEIGHT),
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                self.status = format!("Text paint: {error}");
            }
            if cell.flags.contains(Flags::UNDERLINE) {
                window.paint_quad(fill(
                    Bounds::new(point(x, y + px(LINE_HEIGHT - 2.)), size(width, px(1.))),
                    foreground,
                ));
            }
        }
        let row = cursor.point.line.0 + offset;
        if cursor.shape != CursorShape::Hidden && row >= 0 && row < i32::from(self.size.rows) {
            let origin = point(
                self.bounds.left() + self.cell_width * cursor.point.column.0 as f32,
                self.bounds.top() + px(LINE_HEIGHT) * row as f32,
            );
            window.paint_quad(fill(
                Bounds::new(
                    origin + point(px(0.), px(LINE_HEIGHT - 2.)),
                    size(self.cell_width, px(2.)),
                ),
                rgb(0x80cbc4),
            ));
            if !self.composition.is_empty() {
                let text: SharedString = self.composition.clone().into();
                let run = TextRun {
                    len: text.len(),
                    font: self.face.clone(),
                    color: rgb(0xffffff).into(),
                    background_color: Some(rgb(0x345778).into()),
                    underline: None,
                    strikethrough: None,
                };
                let line = window
                    .text_system()
                    .shape_line(text, px(FONT_SIZE), &[run], None);
                let _ = line.paint(origin, px(LINE_HEIGHT), TextAlign::Left, None, window, cx);
            }
        }
        self.painted_revision = self.content_revision;
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(self.bounds, cx.entity()),
            cx,
        );
    }
}

impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let layout = cx.entity();
        let paint = layout.clone();
        div()
            .id("terminal")
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
                    this.term.selection = Some(Selection::new(
                        if event.click_count >= 2 {
                            SelectionType::Semantic
                        } else {
                            SelectionType::Simple
                        },
                        this.cell_at(event.position),
                        Side::Left,
                    ));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.selecting && event.dragging() {
                    let point = this.cell_at(event.position);
                    if let Some(selection) = &mut this.term.selection {
                        selection.update(point, Side::Right);
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                this.scroll_remainder += match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y) / LINE_HEIGHT,
                    ScrollDelta::Lines(delta) => delta.y,
                };
                let lines = this.scroll_remainder.trunc() as i32;
                this.scroll_remainder -= lines as f32;
                if lines != 0 {
                    this.term.scroll_display(Scroll::Delta(lines));
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
            self.status = "Input exceeds the 64 KiB probe limit".into();
            return;
        }
        self.composition.clear();
        self.term.scroll_display(Scroll::Bottom);
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
        let cursor = self.term.renderable_content().cursor.point;
        Some(Bounds::new(
            self.bounds.origin
                + point(
                    self.cell_width * cursor.column.0 as f32,
                    px(LINE_HEIGHT) * cursor.line.0 as f32,
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

fn encode_key(
    key: &str,
    control: bool,
    alt: bool,
    shift: bool,
    app_cursor: bool,
) -> Option<String> {
    let text = match key {
        "enter" => "\r",
        "backspace" => "\u{7f}",
        "tab" if shift => "\u{1b}[Z",
        "tab" => "\t",
        "escape" => "\u{1b}",
        "up" => {
            if app_cursor {
                "\u{1b}OA"
            } else {
                "\u{1b}[A"
            }
        }
        "down" => {
            if app_cursor {
                "\u{1b}OB"
            } else {
                "\u{1b}[B"
            }
        }
        "right" => {
            if app_cursor {
                "\u{1b}OC"
            } else {
                "\u{1b}[C"
            }
        }
        "left" => {
            if app_cursor {
                "\u{1b}OD"
            } else {
                "\u{1b}[D"
            }
        }
        "home" => "\u{1b}[H",
        "end" => "\u{1b}[F",
        "delete" => "\u{1b}[3~",
        "pageup" => "\u{1b}[5~",
        "pagedown" => "\u{1b}[6~",
        _ if control && key.len() == 1 => {
            let byte = key.as_bytes()[0];
            return (byte.is_ascii_alphabetic() || matches!(byte, b'@'..=b'_'))
                .then(|| char::from(byte.to_ascii_uppercase() & 0x1f).to_string());
        }
        _ if alt && key.len() == 1 => return Some(format!("\u{1b}{key}")),
        _ => return None,
    };
    Some(text.to_owned())
}
fn resolve_color(color: Color, overrides: &alacritty_terminal::term::color::Colors) -> Hsla {
    match color {
        Color::Spec(value) => {
            rgb((u32::from(value.r) << 16) | (u32::from(value.g) << 8) | u32::from(value.b)).into()
        }
        Color::Named(name) => overrides[name]
            .map(|value| resolve_color(Color::Spec(value), overrides))
            .unwrap_or_else(|| palette(name as usize)),
        Color::Indexed(index) => overrides[usize::from(index)]
            .map(|value| resolve_color(Color::Spec(value), overrides))
            .unwrap_or_else(|| palette(usize::from(index))),
    }
}
fn palette(index: usize) -> Hsla {
    const ANSI: [u32; 16] = [
        0x18212d, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370,
        0xff7a85, 0xb5e890, 0xffd891, 0x82c7ff, 0xe8a0ff, 0x7adbea, 0xffffff,
    ];
    let value = match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let n = u32::try_from(index - 16).unwrap_or_default();
            let component = |n: u32| if n == 0 { 0 } else { 55 + 40 * n };
            (component(n / 36) << 16) | (component((n / 6) % 6) << 8) | component(n % 6)
        }
        232..=255 => {
            let level = 8 + u32::try_from(index - 232).unwrap_or_default() * 10;
            level * 0x010101
        }
        n if n == NamedColor::Background as usize => 0x10151c,
        _ => 0xd9e2ef,
    };
    rgb(value).into()
}

#[cfg(test)]
mod tests {
    use super::{encode_key, Listener, Size};
    use alacritty_terminal::{term::Config, vte::ansi::Processor, Term};
    #[test]
    fn keys_preserve_terminal_control_and_application_cursor_modes() {
        assert_eq!(
            encode_key("c", true, false, false, false).as_deref(),
            Some("\u{3}")
        );
        assert_eq!(
            encode_key("up", false, false, false, true).as_deref(),
            Some("\u{1b}OA")
        );
        assert_eq!(encode_key("é", false, false, false, false), None);
    }
    #[test]
    fn parser_preserves_split_utf8_and_queues_protocol_replies() {
        let listener = Listener::default();
        let mut term = Term::new(
            Config::default(),
            &Size { cols: 20, rows: 3 },
            listener.clone(),
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, &[0xc3]);
        parser.advance(&mut term, &[0xa9]);
        parser.advance(&mut term, b"\x1b[6n");
        assert_eq!(
            term.grid()[alacritty_terminal::index::Point::new(
                alacritty_terminal::index::Line(0),
                alacritty_terminal::index::Column(0)
            )]
            .c,
            'é'
        );
        assert!(listener.replies(true).is_empty());
        assert!(
            listener.replies(false).is_empty(),
            "replay replies must not leak into the next live chunk"
        );
        parser.advance(&mut term, b"\x1b[6n");
        assert_eq!(listener.replies(false), ["\x1b[1;2R"]);
    }
}
