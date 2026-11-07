//! Thread-confined Ghostty ownership and protocol operations, without UI or I/O.
//!
//! The session owner supplies bytes and consumes bounded replies. Native handles
//! and untracked grid references never escape this module. A frame exclusively
//! borrows the engine; its lending row/cell API prevents snapshot invalidation.
mod ffi;

use anyhow::{anyhow, bail, ensure, Context, Result};
use std::{cell::RefCell, ffi::c_void, marker::PhantomData, mem, ptr, rc::Rc};

pub const MAX_INPUT: usize = 64 * 1024;
const MAX_COPY: usize = 16 * 1024 * 1024;
const SCROLLBACK_BYTES: usize = 32 * 1024 * 1024;
const SCROLLBACK_LINES: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
    pub shape: CursorShape,
    pub color: u32,
}

#[derive(Default)]
struct Replies {
    bytes: Vec<u8>,
    suppress: bool,
    overflow: bool,
}

/// Owns every native handle; the callback box's address survives moves of Engine.
/// Rc's marker makes thread confinement explicit even if handle types change.
pub struct Engine {
    terminal: ffi::Handle,
    render: ffi::Handle,
    rows: ffi::Handle,
    cells: ffi::Handle,
    encoder: ffi::Handle,
    gesture: ffi::Handle,
    replies: Box<RefCell<Replies>>,
    cols: u16,
    height: u16,
    cell_width: u32,
    cell_height: u32,
    text: Vec<u8>,
    _thread: PhantomData<Rc<()>>,
}

impl Engine {
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        ensure!(cols > 0 && rows > 0, "Terminal dimensions must be nonzero");
        let mut engine = Self {
            terminal: ptr::null_mut(),
            render: ptr::null_mut(),
            rows: ptr::null_mut(),
            cells: ptr::null_mut(),
            encoder: ptr::null_mut(),
            gesture: ptr::null_mut(),
            replies: Box::default(),
            cols,
            height: rows,
            cell_width: 0,
            cell_height: 0,
            text: vec![0; 64],
            _thread: PhantomData,
        };
        // SAFETY: each constructor receives a fresh output slot and the default
        // allocator. Partial construction is covered by Engine::drop, which
        // frees only initialized handles in dependency order.
        unsafe {
            check(ffi::ghostty_terminal_new(
                ptr::null(),
                &mut engine.terminal,
                cols,
                rows,
            ))?;
            check(ffi::ghostty_render_state_new(
                ptr::null(),
                &mut engine.render,
            ))?;
            check(ffi::ghostty_render_state_row_iterator_new(
                ptr::null(),
                &mut engine.rows,
            ))?;
            check(ffi::ghostty_render_state_row_cells_new(
                ptr::null(),
                &mut engine.cells,
            ))?;
            check(ffi::ghostty_key_encoder_new(
                ptr::null(),
                &mut engine.encoder,
            ))?;
            check(ffi::ghostty_selection_gesture_new(
                ptr::null(),
                &mut engine.gesture,
            ))?;
            // Pointer-valued options take the pointer itself, unlike scalar options.
            check(ffi::ghostty_terminal_set(
                engine.terminal,
                ffi::OPT_USERDATA,
                ptr::from_ref(engine.replies.as_ref()).cast(),
            ))?;
            check(ffi::ghostty_terminal_set(
                engine.terminal,
                ffi::OPT_WRITE_PTY,
                on_write as *const c_void,
            ))?;
        }
        engine.option(ffi::OPT_SCROLLBACK_BYTES, &SCROLLBACK_BYTES)?;
        engine.option(ffi::OPT_SCROLLBACK_LINES, &SCROLLBACK_LINES)?;
        engine.option(ffi::OPT_FOREGROUND, &rgb(0xd9e2ef))?;
        engine.option(ffi::OPT_BACKGROUND, &rgb(0x10151c))?;
        engine.option(ffi::OPT_CURSOR, &rgb(0x80cbc4))?;
        engine.option(ffi::OPT_PALETTE, &palette())?;
        // Image presentation is outside this probe. No file/shared-memory or
        // clipboard callbacks are installed, and image storage stays disabled.
        let disabled: u64 = 0;
        // SAFETY: the option takes a uint64_t; unsupported means compiled out.
        let result = unsafe {
            ffi::ghostty_terminal_set(
                engine.terminal,
                ffi::OPT_IMAGE_LIMIT,
                ptr::from_ref(&disabled).cast(),
            )
        };
        if result != ffi::UNSUPPORTED {
            check(result)?;
        }
        Ok(engine)
    }

    // Only private, statically paired option/type call sites use this helper.
    fn option<T>(&mut self, option: i32, value: &T) -> Result<()> {
        // SAFETY: call sites pair each discriminant with its documented C type;
        // these options copy their input synchronously and never retain it.
        check(unsafe {
            ffi::ghostty_terminal_set(self.terminal, option, ptr::from_ref(value).cast())
        })
    }

    fn begin_effects(&mut self, suppress: bool) {
        let replies = self.replies.get_mut();
        replies.bytes.clear();
        replies.overflow = false;
        replies.suppress = suppress;
    }

    fn end_effects(&mut self, result: Result<()>) -> Result<Vec<u8>> {
        let replies = self.replies.get_mut();
        replies.suppress = false;
        if replies.overflow || result.is_err() {
            replies.bytes.clear();
            result?;
            bail!("Terminal output exceeds the 64 KiB input limit");
        }
        Ok(mem::take(&mut replies.bytes))
    }

    pub fn feed(&mut self, bytes: &[u8], replay: bool) -> Result<Vec<u8>> {
        self.begin_effects(replay);
        // SAFETY: exclusive terminal access, live input slice for this synchronous
        // call, and no RefCell borrow held while the callback accesses its box.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len());
        }
        self.end_effects(Ok(()))
    }

    pub fn reset(&mut self) -> Result<()> {
        let mut replacement = Self::new(self.cols, self.height)?;
        replacement.resize(self.cols, self.height, self.cell_width, self.cell_height)?;
        *self = replacement;
        Ok(())
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<Vec<u8>> {
        ensure!(cols > 0 && rows > 0, "Terminal dimensions must be nonzero");
        self.begin_effects(false);
        // SAFETY: exclusive terminal access and checked nonzero dimensions.
        let result = check(unsafe {
            ffi::ghostty_terminal_resize(self.terminal, cols, rows, cell_width, cell_height)
        });
        if result.is_ok() {
            self.cols = cols;
            self.height = rows;
            self.cell_width = cell_width;
            self.cell_height = cell_height;
        }
        self.end_effects(result)
    }

    /// Positive lines move upward, matching the view's wheel convention.
    pub fn scroll(&mut self, lines: i32) -> Result<()> {
        let delta = isize::try_from(-i64::from(lines))?;
        self.scroll_to(ffi::Scroll {
            tag: 2,
            value: ffi::ScrollValue { delta },
        })
    }

    pub fn scroll_bottom(&mut self) -> Result<()> {
        self.scroll_to(ffi::Scroll {
            tag: 1,
            value: ffi::ScrollValue { padding: [0; 2] },
        })
    }

    fn scroll_to(&mut self, scroll: ffi::Scroll) -> Result<()> {
        // SAFETY: the tagged union is constructed only by the two methods above.
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.terminal, scroll) })
    }

    pub fn key(
        &mut self,
        key: &str,
        control: bool,
        alt: bool,
        shift: bool,
    ) -> Result<Option<Vec<u8>>> {
        let Some((code, text)) = key_code(key, control, alt) else {
            return Ok(None);
        };
        let mut event = Owned::new(ffi::ghostty_key_event_free);
        // SAFETY: the event is freed before this method returns, including every
        // error path, so its borrowed `text` never outlives key. No event escapes.
        unsafe {
            check(ffi::ghostty_key_event_new(ptr::null(), &mut event.raw))?;
            check(ffi::ghostty_key_event_set_action(event.raw, 1))?;
            check(ffi::ghostty_key_event_set_key(event.raw, code))?;
            let mods = u16::from(shift) | (u16::from(control) << 1) | (u16::from(alt) << 2);
            check(ffi::ghostty_key_event_set_mods(event.raw, mods))?;
            check(ffi::ghostty_key_event_set_utf8(
                event.raw,
                ffi::StringView {
                    ptr: text.as_ptr(),
                    len: text.len(),
                },
            ))?;
            check(ffi::ghostty_key_event_set_unshifted_codepoint(
                event.raw,
                text.chars().next().map(u32::from).unwrap_or(0),
            ))?;
            check(ffi::ghostty_key_encoder_set_from_terminal(
                self.encoder,
                self.terminal,
            ))?;
        }
        let mut bytes = output_buffer(MAX_INPUT, |buffer, len, written| {
            // SAFETY: all handles and the output slice live through this call.
            unsafe {
                ffi::ghostty_key_encoder_encode(self.encoder, event.raw, buffer, len, written)
            }
        })?;
        if control && !alt && !shift {
            let mut kitty_flags = 0u8;
            // SAFETY: the terminal getter documents uint8_t for this data key.
            check(unsafe {
                ffi::ghostty_terminal_get(
                    self.terminal,
                    ffi::DATA_KITTY_FLAGS,
                    ptr::from_mut(&mut kitty_flags).cast(),
                )
            })?;
            // Ghostty 1.0 emits fixterms CSI-u for these ambiguous legacy
            // controls even without negotiation. Preserve the probe's familiar
            // Tab/Enter/Escape aliases for ordinary applications. Kitty requests
            // retain the native encoding; modifyOtherKeys-2 emits a different
            // sequence and cannot match this fallback.
            let legacy = match bytes.as_slice() {
                b"\x1b[105;5u" => Some(b'\t'),
                b"\x1b[109;5u" => Some(b'\r'),
                b"\x1b[91;5u" => Some(0x1b),
                _ => None,
            };
            if let Some(byte) = legacy.filter(|_| kitty_flags == 0) {
                bytes.clear();
                bytes.push(byte);
            }
        }
        Ok(Some(bytes))
    }

    pub fn paste(&mut self, text: &str) -> Result<Vec<u8>> {
        ensure!(
            text.len() <= MAX_INPUT - 12,
            "Paste exceeds the 64 KiB input limit"
        );
        // Preserve the existing explicit-paste policy: strip ESC so pasted text
        // cannot close its bracket framing, and normalize line endings first.
        let text = text
            .replace('\u{1b}', "")
            .replace("\r\n", "\n")
            .replace('\n', "\r");
        let source = ffi::StringView {
            ptr: text.as_ptr(),
            len: text.len(),
        };
        let mime = ffi::StringView {
            ptr: b"text/plain".as_ptr(),
            len: 10,
        };
        let paste = ffi::Paste {
            size: mem::size_of::<ffi::Paste>(),
            location: 0,
            source: 0,
            mimes: &mime,
            mimes_len: 1,
            reader: ffi::MimeReader {
                read: Some(read_paste),
                userdata: ptr::from_ref(&source).cast_mut().cast(),
            },
            allow_unsafe: true,
        };
        let mut pasted = false;
        self.begin_effects(false);
        // SAFETY: paste, MIME, reader context and text live throughout this
        // synchronous call. The reader does not retain the supplied writer.
        let result =
            check(unsafe { ffi::ghostty_terminal_paste(self.terminal, &paste, &mut pasted) });
        self.end_effects(result)
    }

    pub fn select_start(&mut self, col: u16, row: u16, word: bool) -> Result<()> {
        // SAFETY: no external reference to gesture/terminal; reset releases its
        // tracked anchor before we start another selection.
        unsafe {
            ffi::ghostty_selection_gesture_reset(self.gesture, self.terminal);
            check(ffi::ghostty_terminal_set(
                self.terminal,
                ffi::OPT_SELECTION,
                ptr::null(),
            ))?;
        }
        self.selection_event(0, Some((col, row)), word)
    }

    pub fn select_update(&mut self, col: u16, row: u16) -> Result<()> {
        self.selection_event(2, Some((col, row)), false)
    }

    pub fn select_end(&mut self) -> Result<()> {
        self.selection_event(1, None, false)
    }

    fn selection_event(&mut self, kind: i32, cell: Option<(u16, u16)>, word: bool) -> Result<()> {
        let mut event = Owned::new(ffi::ghostty_selection_gesture_event_free);
        // SAFETY: the event and untracked references remain local to this call.
        // No terminal mutation occurs between resolving a point and applying it.
        unsafe {
            check(ffi::ghostty_selection_gesture_event_new(
                ptr::null(),
                &mut event.raw,
                kind,
            ))?;
            if let Some((col, row)) = cell {
                let point = ffi::Point {
                    tag: 1,
                    value: ffi::PointValue {
                        coordinate: ffi::Coordinate {
                            x: col.min(self.cols - 1),
                            y: u32::from(row.min(self.height - 1)),
                        },
                    },
                };
                let mut reference: ffi::GridRef = mem::zeroed();
                check(ffi::ghostty_terminal_grid_ref(
                    self.terminal,
                    point,
                    &mut reference,
                ))?;
                check(ffi::ghostty_selection_gesture_event_set(
                    event.raw,
                    ffi::EVENT_REF,
                    ptr::from_ref(&reference).cast(),
                ))?;
                // Virtual cells are ten pixels wide to preserve a right-side drag
                // endpoint without retaining any GPUI geometry inside the engine.
                let position = ffi::Position {
                    x: f64::from(col) * 10. + if kind == 2 { 9. } else { 1. },
                    y: f64::from(row) * 10. + 5.,
                };
                check(ffi::ghostty_selection_gesture_event_set(
                    event.raw,
                    ffi::EVENT_POSITION,
                    ptr::from_ref(&position).cast(),
                ))?;
            }
            if kind == 0 {
                let behaviors = ffi::GestureBehaviors {
                    single_click: i32::from(word),
                    double_click: 1,
                    triple_click: 2,
                };
                check(ffi::ghostty_selection_gesture_event_set(
                    event.raw,
                    ffi::EVENT_BEHAVIORS,
                    ptr::from_ref(&behaviors).cast(),
                ))?;
            }
            if kind == 2 {
                let geometry = ffi::GestureGeometry {
                    columns: u32::from(self.cols),
                    cell_width: 10,
                    padding_left: 0,
                    screen_height: u32::from(self.height) * 10,
                };
                check(ffi::ghostty_selection_gesture_event_set(
                    event.raw,
                    ffi::EVENT_GEOMETRY,
                    ptr::from_ref(&geometry).cast(),
                ))?;
            }
            let mut selection: ffi::Selection = mem::zeroed();
            let result = ffi::ghostty_selection_gesture_event(
                self.gesture,
                self.terminal,
                event.raw,
                &mut selection,
            );
            if result == ffi::NO_VALUE {
                return Ok(());
            }
            check(result)?;
            // Installation immediately converts the snapshot into tracked state.
            check(ffi::ghostty_terminal_set(
                self.terminal,
                ffi::OPT_SELECTION,
                ptr::from_ref(&selection).cast(),
            ))
        }
    }

    pub fn selection_text(&mut self) -> Result<Option<String>> {
        let mut formatter = Owned::new(ffi::ghostty_formatter_free);
        // SAFETY: resolve and format the untracked selection with no intervening
        // terminal mutation. The formatter is destroyed before returning.
        unsafe {
            let mut selection: ffi::Selection = mem::zeroed();
            let result = ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_SELECTION,
                ptr::from_mut(&mut selection).cast(),
            );
            if result == ffi::NO_VALUE {
                return Ok(None);
            }
            check(result)?;
            check(ffi::ghostty_formatter_terminal_new(
                ptr::null(),
                &mut formatter.raw,
                self.terminal,
            ))?;
            check(ffi::ghostty_formatter_set(
                formatter.raw,
                ffi::FORMAT_SELECTION,
                ptr::from_ref(&selection).cast(),
            ))?;
            check(ffi::ghostty_formatter_set(
                formatter.raw,
                ffi::FORMAT_UNWRAP,
                ptr::from_ref(&true).cast(),
            ))?;
        }
        let bytes = output_buffer(MAX_COPY, |buffer, len, written| {
            // SAFETY: formatter borrows the unchanged live terminal above.
            unsafe { ffi::ghostty_formatter_format_buf(formatter.raw, buffer, len, written) }
        })?;
        Ok(Some(
            String::from_utf8(bytes).context("Terminal selection is not UTF-8")?,
        ))
    }

    pub fn cursor(&mut self) -> Result<Cursor> {
        let mut col = 0u16;
        let mut row = 0u16;
        let mut visible = false;
        // SAFETY: initialized outputs exactly match each terminal data key.
        let mut scrollbar: ffi::Scrollbar = unsafe { mem::zeroed() };
        unsafe {
            check(ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_CURSOR_X,
                ptr::from_mut(&mut col).cast(),
            ))?;
            check(ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_CURSOR_Y,
                ptr::from_mut(&mut row).cast(),
            ))?;
            check(ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_CURSOR_VISIBLE,
                ptr::from_mut(&mut visible).cast(),
            ))?;
            check(ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_SCROLLBAR,
                ptr::from_mut(&mut scrollbar).cast(),
            ))?;
        }
        let viewport_row = scrollbar
            .total
            .saturating_sub(scrollbar.len)
            .saturating_sub(scrollbar.offset)
            + u64::from(row);
        visible &= viewport_row < u64::from(self.height);
        Ok(Cursor {
            col,
            row: u16::try_from(viewport_row.min(u64::from(self.height - 1)))?,
            visible,
            shape: CursorShape::Block,
            color: 0x80cbc4,
        })
    }

    pub fn frame(&mut self) -> Result<Frame<'_>> {
        // SAFETY: Engine is exclusively borrowed for the entire Frame lifetime.
        // All outputs below have the precise documented ABI layout. Zeroing is
        // valid for these C structs, whose bools and pointers allow zero.
        let mut colors: ffi::RenderColors = unsafe { mem::zeroed() };
        colors.size = mem::size_of::<ffi::RenderColors>();
        let mut cursor: ffi::RenderCursor = unsafe { mem::zeroed() };
        cursor.size = mem::size_of::<ffi::RenderCursor>();
        let mut dirty = 0i32;
        unsafe {
            check(ffi::ghostty_render_state_update(self.render, self.terminal))?;
            check(ffi::ghostty_render_state_get(
                self.render,
                ffi::RENDER_COLORS,
                ptr::from_mut(&mut colors).cast(),
            ))?;
            check(ffi::ghostty_render_state_get(
                self.render,
                ffi::RENDER_CURSOR,
                ptr::from_mut(&mut cursor).cast(),
            ))?;
            check(ffi::ghostty_render_state_get(
                self.render,
                ffi::RENDER_DIRTY,
                ptr::from_mut(&mut dirty).cast(),
            ))?;
            check(ffi::ghostty_render_state_get(
                self.render,
                ffi::RENDER_ROWS,
                ptr::from_mut(&mut self.rows).cast(),
            ))?;
        }
        let visible = cursor.visible && cursor.viewport_has_value;
        let cursor = Cursor {
            col: if cursor.viewport_has_value {
                cursor.viewport_x
            } else {
                0
            },
            row: if cursor.viewport_has_value {
                cursor.viewport_y
            } else {
                0
            },
            visible,
            shape: match cursor.visual_style {
                0 => CursorShape::Bar,
                2 => CursorShape::Underline,
                3 => CursorShape::Hollow,
                _ => CursorShape::Block,
            },
            color: if colors.cursor_has_value {
                packed(colors.cursor)
            } else {
                packed(colors.foreground)
            },
        };
        Ok(Frame {
            cursor,
            background: packed(colors.background),
            full_dirty: dirty == 2,
            colors,
            engine: self,
        })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: all handles are uniquely owned, all nullable destructors accept
        // partial initialization. Gesture cleanup requires the still-live terminal.
        // Callback storage is dropped only after this function returns.
        unsafe {
            ffi::ghostty_selection_gesture_free(self.gesture, self.terminal);
            ffi::ghostty_key_encoder_free(self.encoder);
            ffi::ghostty_render_state_row_cells_free(self.cells);
            ffi::ghostty_render_state_row_iterator_free(self.rows);
            ffi::ghostty_render_state_free(self.render);
            ffi::ghostty_terminal_free(self.terminal);
        }
    }
}

/// Borrowing this frame prevents feed, resize, reset, and another snapshot update.
pub struct Frame<'a> {
    pub cursor: Cursor,
    pub background: u32,
    pub full_dirty: bool,
    colors: ffi::RenderColors,
    engine: &'a mut Engine,
}

impl Frame<'_> {
    /// Dropping a row without reading its cells is safe and supports row caches.
    pub fn next_row(&mut self) -> Result<Option<Row<'_>>> {
        // SAFETY: the snapshot cannot change during this exclusive frame borrow.
        if !unsafe { ffi::ghostty_render_state_row_iterator_next(self.engine.rows) } {
            return Ok(None);
        }
        let mut index = 0i32;
        let mut id = [0u64; 2];
        let mut dirty = false;
        unsafe {
            check(ffi::ghostty_render_state_row_get(
                self.engine.rows,
                ffi::ROW_Y,
                ptr::from_mut(&mut index).cast(),
            ))?;
            check(ffi::ghostty_render_state_row_get(
                self.engine.rows,
                ffi::ROW_ID,
                ptr::from_mut(&mut id).cast(),
            ))?;
            check(ffi::ghostty_render_state_row_get(
                self.engine.rows,
                ffi::ROW_DIRTY,
                ptr::from_mut(&mut dirty).cast(),
            ))?;
            check(ffi::ghostty_render_state_row_get(
                self.engine.rows,
                ffi::ROW_CELLS,
                ptr::from_mut(&mut self.engine.cells).cast(),
            ))?;
        }
        Ok(Some(Row {
            index: u16::try_from(index)?,
            id,
            dirty,
            cells: self.engine.cells,
            next_col: 0,
            text: &mut self.engine.text,
            colors: &self.colors,
        }))
    }

    /// Call only after the complete frame was successfully drawn or cached.
    pub fn finish(self) -> Result<()> {
        // SAFETY: consuming Frame releases all row/cell borrows before cleaning.
        check(unsafe { ffi::ghostty_render_state_clean(self.engine.render) })
    }
}

pub struct Row<'a> {
    pub index: u16,
    pub id: [u64; 2],
    pub dirty: bool,
    cells: ffi::Handle,
    next_col: u16,
    text: &'a mut Vec<u8>,
    colors: &'a ffi::RenderColors,
}

/// The text borrows a reused UTF-8 buffer until the next cell is requested.
pub struct Cell<'a> {
    pub col: u16,
    pub text: &'a str,
    pub width: u8,
    pub skip: bool,
    pub foreground: u32,
    pub background: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub hidden: bool,
    pub selected: bool,
}

impl Row<'_> {
    pub fn next_cell(&mut self) -> Result<Option<Cell<'_>>> {
        // SAFETY: Row borrows the frame and serializes access to its cell cursor.
        if !unsafe { ffi::ghostty_render_state_row_cells_next(self.cells) } {
            return Ok(None);
        }
        let col = self.next_col;
        self.next_col = self.next_col.saturating_add(1);
        let mut raw = 0u64;
        let mut wide = 0i32;
        let mut selected = false;
        // SAFETY: zero is a valid initialization for this plain C struct.
        let mut style: ffi::Style = unsafe { mem::zeroed() };
        unsafe {
            check(ffi::ghostty_render_state_row_cells_get(
                self.cells,
                ffi::CELL_RAW,
                ptr::from_mut(&mut raw).cast(),
            ))?;
            check(ffi::ghostty_cell_get(
                raw,
                ffi::CELL_WIDE,
                ptr::from_mut(&mut wide).cast(),
            ))?;
            check(ffi::ghostty_render_state_row_cells_get(
                self.cells,
                ffi::CELL_STYLE,
                ptr::from_mut(&mut style).cast(),
            ))?;
            check(ffi::ghostty_render_state_row_cells_get(
                self.cells,
                ffi::CELL_SELECTED,
                ptr::from_mut(&mut selected).cast(),
            ))?;
        }
        let len = loop {
            let mut buf = ffi::Buffer {
                ptr: self.text.as_mut_ptr(),
                cap: self.text.len(),
                len: 0,
            };
            // SAFETY: initialized writable scratch storage, stable current cell.
            let result = unsafe {
                ffi::ghostty_render_state_row_cells_get(
                    self.cells,
                    ffi::CELL_UTF8,
                    ptr::from_mut(&mut buf).cast(),
                )
            };
            if result == ffi::OUT_OF_SPACE {
                ensure!(
                    buf.len <= MAX_COPY && buf.len > self.text.len(),
                    "Invalid terminal grapheme length"
                );
                self.text.try_reserve(buf.len - self.text.len())?;
                self.text.resize(buf.len, 0);
                continue;
            }
            check(result)?;
            ensure!(
                buf.len <= self.text.len(),
                "Invalid terminal grapheme length"
            );
            break buf.len;
        };
        let mut foreground = style_color(style.fg_color, self.colors, self.colors.foreground);
        // Blank/erased cells can carry a background directly in their packed
        // content, without any style entry. The dedicated getter handles both.
        let mut cell_background = self.colors.background;
        // SAFETY: this key writes a GhosttyColorRgb to the initialized output.
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells,
                ffi::CELL_BACKGROUND,
                ptr::from_mut(&mut cell_background).cast(),
            )
        };
        if result != ffi::NO_VALUE {
            check(result)?;
        }
        let mut background = packed(cell_background);
        if style.flags & (1 << 4) != 0 {
            mem::swap(&mut foreground, &mut background);
        }
        Ok(Some(Cell {
            col,
            text: std::str::from_utf8(&self.text[..len])?,
            width: if wide == 1 { 2 } else { 1 },
            skip: wide == 2 || wide == 3,
            foreground,
            background,
            bold: style.flags & 1 != 0,
            italic: style.flags & 2 != 0,
            underline: style.flags & 0x700 != 0,
            hidden: style.flags & (1 << 5) != 0,
            selected,
        }))
    }
}

fn check(result: ffi::Status) -> Result<()> {
    if result == ffi::SUCCESS {
        Ok(())
    } else {
        Err(anyhow!("Ghostty operation failed ({result})"))
    }
}

/// A small guard for temporary handles; no allocator or handle escapes its scope.
struct Owned {
    raw: ffi::Handle,
    free: unsafe extern "C" fn(ffi::Handle),
}
impl Owned {
    fn new(free: unsafe extern "C" fn(ffi::Handle)) -> Self {
        Self {
            raw: ptr::null_mut(),
            free,
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: raw is uniquely owned; its matching destructor accepts NULL.
        unsafe {
            (self.free)(self.raw);
        }
    }
}

fn output_buffer(
    limit: usize,
    mut encode: impl FnMut(*mut u8, usize, *mut usize) -> ffi::Status,
) -> Result<Vec<u8>> {
    let mut bytes = vec![0; 128];
    loop {
        let mut written = 0;
        let result = encode(bytes.as_mut_ptr(), bytes.len(), &mut written);
        if result == ffi::OUT_OF_SPACE {
            ensure!(
                written <= limit && written > bytes.len(),
                "Terminal output exceeds its size limit"
            );
            bytes.try_reserve(written - bytes.len())?;
            bytes.resize(written, 0);
            continue;
        }
        check(result)?;
        ensure!(
            written <= bytes.len() && written <= limit,
            "Invalid terminal output length"
        );
        bytes.truncate(written);
        return Ok(bytes);
    }
}

unsafe extern "C" fn on_write(_: ffi::Handle, userdata: *mut c_void, data: *const u8, len: usize) {
    // SAFETY: userdata always points to Engine's stable RefCell box and remains
    // alive through native teardown. Payload is borrowed only during this call.
    // No panicking borrow, allocation, indexing, user callback, or reentrant VT
    // call occurs here, so Rust unwinding cannot cross the C boundary.
    let state = unsafe { &*userdata.cast::<RefCell<Replies>>() };
    let Ok(mut state) = state.try_borrow_mut() else {
        return;
    };
    if state.suppress || state.overflow || len == 0 {
        return;
    }
    if len > MAX_INPUT.saturating_sub(state.bytes.len())
        || data.is_null()
        || state.bytes.try_reserve(len).is_err()
    {
        state.overflow = true;
        state.bytes.clear();
        return;
    }
    // SAFETY: native callback provides len valid bytes; length was bounded above.
    state
        .bytes
        .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
}

unsafe extern "C" fn read_paste(
    userdata: *mut c_void,
    _: ffi::StringView,
    writer: ffi::Writer,
) -> bool {
    // SAFETY: source and its bytes live for the synchronous terminal_paste call.
    let source = unsafe { &*userdata.cast::<ffi::StringView>() };
    match writer.write {
        Some(write) if source.len > 0 => unsafe { write(writer.userdata, source.ptr, source.len) },
        Some(_) => true,
        None => false,
    }
}

fn rgb(value: u32) -> ffi::Rgb {
    let bytes = value.to_be_bytes();
    ffi::Rgb {
        r: bytes[1],
        g: bytes[2],
        b: bytes[3],
    }
}
fn packed(rgb: ffi::Rgb) -> u32 {
    (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
}
fn style_color(color: ffi::StyleColor, colors: &ffi::RenderColors, fallback: ffi::Rgb) -> u32 {
    // SAFETY: the documented tag selects the active union member. Unknown tags
    // use the default without reading union contents.
    unsafe {
        packed(match color.tag {
            1 => colors.palette[usize::from(color.value.palette)],
            2 => color.value.rgb,
            _ => fallback,
        })
    }
}
fn palette() -> [ffi::Rgb; 256] {
    const ANSI: [u32; 16] = [
        0x18212d, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370,
        0xff7a85, 0xb5e890, 0xffd891, 0x82c7ff, 0xe8a0ff, 0x7adbea, 0xffffff,
    ];
    let mut colors = [ffi::Rgb::default(); 256];
    for index in 0u16..256 {
        let value = match index {
            0..=15 => ANSI[usize::from(index)],
            16..=231 => {
                let n = u32::from(index - 16);
                let component = |n: u32| if n == 0 { 0 } else { 55 + 40 * n };
                (component(n / 36) << 16) | (component((n / 6) % 6) << 8) | component(n % 6)
            }
            _ => {
                let level = 8 + u32::from(index - 232) * 10;
                level * 0x010101
            }
        };
        colors[usize::from(index)] = rgb(value);
    }
    colors
}

fn key_code(key: &str, control: bool, alt: bool) -> Option<(i32, &str)> {
    let code = match key {
        "enter" => 58,
        "backspace" => 53,
        "tab" => 64,
        "escape" => 120,
        "up" => 78,
        "down" => 75,
        "left" => 76,
        "right" => 77,
        "home" => 71,
        "end" => 69,
        "delete" => 68,
        "pageup" => 74,
        "pagedown" => 73,
        _ if control || alt => {
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let code = match ch.to_ascii_lowercase() {
                'a'..='z' => 20 + i32::from(ch.to_ascii_lowercase() as u8 - b'a'),
                '0'..='9' => 6 + i32::from(ch as u8 - b'0'),
                '[' => 3,
                ']' => 4,
                '\\' => 2,
                ' ' => 63,
                _ => 0,
            };
            return Some((code, key));
        }
        _ => return None,
    };
    Some((code, ""))
}

#[cfg(test)]
mod tests;
