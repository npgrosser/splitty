//! Thread-confined Ghostty ownership and protocol operations, without UI or I/O.
//!
//! The session owner supplies bytes and consumes bounded replies. Native handles
//! and untracked grid references never escape this module. A frame exclusively
//! borrows the engine; its lending row/cell API prevents snapshot invalidation.
mod ffi;
mod links;
mod search;
pub use search::{SearchHighlight, SearchProgress, SearchSnapshot};

use anyhow::{anyhow, bail, ensure, Context, Result};
use std::{cell::RefCell, ffi::c_void, marker::PhantomData, mem, ptr, rc::Rc};

pub const MAX_INPUT: usize = 64 * 1024;
const MAX_COPY: usize = 16 * 1024 * 1024;
/// The largest text one clipboard write of a program may carry, as in the
/// web client.
pub const MAX_CLIPBOARD_WRITE: usize = 64 * 1024;
const SCROLLBACK_BYTES: usize = 32 * 1024 * 1024;
const SCROLLBACK_LINES: usize = 5000;
/// The cursor shape until a program selects one with DECSCUSR, and the shape
/// its reset (`CSI 0 q`) restores: a bar, as in the web client.
const DEFAULT_CURSOR_STYLE: std::ffi::c_int = ffi::CURSOR_STYLE_BAR;
const DEFAULT_ANSI: [u32; 16] = [
    0x18212d, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370,
    0xff7a85, 0xb5e890, 0xffd891, 0x82c7ff, 0xe8a0ff, 0x7adbea, 0xffffff,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Osc8,
    Detected,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkHit {
    pub uri: String,
    pub kind: LinkKind,
}

/// Concrete RGB colors only. The service and settings resolver stay outside
/// Ghostty; indices 16–255 retain the standard xterm palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorConfig {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub ansi: [u32; 16],
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            foreground: 0xd9e2ef,
            background: 0x10151c,
            cursor: 0x80cbc4,
            ansi: DEFAULT_ANSI,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

/// The viewport within the active screen: `rows` visible rows that start
/// `offset` rows below the oldest of `total` rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Scrollback {
    pub total: u64,
    pub offset: u64,
    pub rows: u64,
}

impl Scrollback {
    /// Rows below the viewport; zero while it shows the newest rows.
    pub fn behind(&self) -> u64 {
        self.total
            .saturating_sub(self.rows)
            .saturating_sub(self.offset)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
    pub shape: CursorShape,
    pub color: u32,
}

/// Tracking policy derived from the native encoder, not independently parsed modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseTracking {
    None,
    X10,
    Normal,
    Button,
    Any,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MousePolicy {
    pub tracking: MouseTracking,
}
impl MousePolicy {
    pub fn reports_buttons(self) -> bool {
        self.tracking != MouseTracking::None
    }
    pub fn reports_wheel(self) -> bool {
        matches!(
            self.tracking,
            MouseTracking::Normal | MouseTracking::Button | MouseTracking::Any
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    Back,
    Forward,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}
impl MouseModifiers {
    fn native_bits(self) -> u16 {
        u16::from(self.shift)
            | (u16::from(self.control) << 1)
            | (u16::from(self.alt) << 2)
            | (u16::from(self.super_key) << 3)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct MouseInput {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    /// Physical pixels relative to the terminal content. Outside positions are valid.
    pub x: f32,
    pub y: f32,
    pub modifiers: MouseModifiers,
    /// Only buttons whose press was admitted by the UI's command sink count.
    pub any_button_pressed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct MouseGeometry {
    width: f32,
    height: f32,
    cell_width: f32,
    cell_height: f32,
}
impl Default for MouseGeometry {
    fn default() -> Self {
        Self {
            width: 1.0,
            height: 1.0,
            cell_width: 1.0,
            cell_height: 1.0,
        }
    }
}

/// What the terminal's callbacks collect during one operation. `suppress`
/// drops every effect of replayed output: replies and clipboard writes.
#[derive(Default)]
struct Replies {
    bytes: Vec<u8>,
    suppress: bool,
    overflow: bool,
    /// Whether a program may write the clipboard at all.
    clipboard_allowed: bool,
    /// The last accepted clipboard write of the current operation.
    clipboard: Option<String>,
}

/// Owns every native handle; the callback box's address survives moves of Engine.
/// Rc's marker makes thread confinement explicit even if handle types change.
pub struct Engine {
    terminal: ffi::Handle,
    search: search::SearchState,
    render: ffi::Handle,
    rows: ffi::Handle,
    cells: ffi::Handle,
    images: ffi::Handle,
    encoder: ffi::Handle,
    gesture: ffi::Handle,
    mouse_encoder: ffi::Handle,
    mouse_event: ffi::Handle,
    mouse_probe: ffi::Handle,
    mouse_dirty: bool,
    mouse_policy: MousePolicy,
    mouse_pixels: bool,
    mouse_geometry: MouseGeometry,
    replies: Box<RefCell<Replies>>,
    cols: u16,
    height: u16,
    cell_width: u32,
    cell_height: u32,
    text: Vec<u8>,
    colors: ColorConfig,
    _thread: PhantomData<Rc<()>>,
}

impl Engine {
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        ensure!(cols > 0 && rows > 0, "Terminal dimensions must be nonzero");
        let mut engine = Self {
            terminal: ptr::null_mut(),
            search: search::SearchState::default(),
            render: ptr::null_mut(),
            rows: ptr::null_mut(),
            cells: ptr::null_mut(),
            images: ptr::null_mut(),
            encoder: ptr::null_mut(),
            gesture: ptr::null_mut(),
            mouse_encoder: ptr::null_mut(),
            mouse_event: ptr::null_mut(),
            mouse_probe: ptr::null_mut(),
            mouse_dirty: true,
            mouse_policy: MousePolicy {
                tracking: MouseTracking::None,
            },
            mouse_pixels: false,
            mouse_geometry: MouseGeometry::default(),
            replies: Box::default(),
            cols,
            height: rows,
            cell_width: 0,
            cell_height: 0,
            text: vec![0; 64],
            colors: ColorConfig::default(),
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
            check(ffi::ghostty_render_state_image_iterator_new(
                ptr::null(),
                &mut engine.images,
            ))?;
            check(ffi::ghostty_key_encoder_new(
                ptr::null(),
                &mut engine.encoder,
            ))?;
            check(ffi::ghostty_mouse_encoder_new(
                ptr::null(),
                &mut engine.mouse_encoder,
            ))?;
            check(ffi::ghostty_mouse_encoder_new(
                ptr::null(),
                &mut engine.mouse_probe,
            ))?;
            check(ffi::ghostty_mouse_event_new(
                ptr::null(),
                &mut engine.mouse_event,
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
            let clipboard_write: ffi::ClipboardWriteFn = on_clipboard_write;
            check(ffi::ghostty_terminal_set(
                engine.terminal,
                ffi::OPT_CLIPBOARD_WRITE,
                clipboard_write as *const c_void,
            ))?;
        }
        engine.option(ffi::OPT_SCROLLBACK_BYTES, &SCROLLBACK_BYTES)?;
        engine.option(ffi::OPT_SCROLLBACK_LINES, &SCROLLBACK_LINES)?;
        engine.option(ffi::OPT_DEFAULT_CURSOR_STYLE, &DEFAULT_CURSOR_STYLE)?;
        engine.install_colors(ColorConfig::default())?;
        engine.install_mouse_geometry(engine.mouse_geometry)?;
        install_png_decoder();
        // Images arrive in the terminal's own stream only: the file and
        // shared-memory media stay off, so a program cannot make the window
        // read a file. Of the clipboard callbacks only the write callback is
        // installed, and it accepts nothing until `set_clipboard_writes`
        // allows it; without a read callback the terminal answers no
        // clipboard query, so a program never reads the clipboard.
        let disabled: u64 = IMAGE_STORAGE_BYTES;
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

    fn install_colors(&mut self, colors: ColorConfig) -> Result<()> {
        self.option(ffi::OPT_FOREGROUND, &rgb(colors.foreground))?;
        self.option(ffi::OPT_BACKGROUND, &rgb(colors.background))?;
        self.option(ffi::OPT_CURSOR, &rgb(colors.cursor))?;
        self.option(ffi::OPT_PALETTE, &palette(colors.ansi))?;
        self.colors = colors;
        Ok(())
    }

    pub fn apply_colors(&mut self, colors: ColorConfig) -> Result<()> {
        if self.colors != colors {
            self.install_colors(colors)?;
        }
        Ok(())
    }

    fn begin_effects(&mut self, suppress: bool) {
        let replies = self.replies.get_mut();
        replies.bytes.clear();
        replies.overflow = false;
        replies.suppress = suppress;
        replies.clipboard = None;
    }

    fn end_effects(&mut self, result: Result<()>) -> Result<Vec<u8>> {
        let replies = self.replies.get_mut();
        replies.suppress = false;
        if replies.overflow || result.is_err() {
            replies.bytes.clear();
            replies.clipboard = None;
            result?;
            bail!("Terminal output exceeds the 64 KiB input limit");
        }
        Ok(mem::take(&mut replies.bytes))
    }

    pub fn feed(&mut self, bytes: &[u8], replay: bool) -> Result<Vec<u8>> {
        self.mouse_dirty |= !bytes.is_empty();
        if !bytes.is_empty() {
            self.search.invalidate();
        }
        self.begin_effects(replay);
        // SAFETY: exclusive terminal access, live input slice for this synchronous
        // call, and no RefCell borrow held while the callback accesses its box.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len());
        }
        self.end_effects(Ok(()))
    }

    /// Whether a program's clipboard write (OSC 52 and the other write
    /// protocols of the terminal) is accepted. Off until set; kept by `reset`.
    pub fn set_clipboard_writes(&mut self, allowed: bool) {
        let replies = self.replies.get_mut();
        replies.clipboard_allowed = allowed;
        if !allowed {
            replies.clipboard = None;
        }
    }

    /// The text the last `feed` of live output asked to put on the standard
    /// clipboard, once. Replayed output, other clipboards, empty or non-UTF-8
    /// text and text over `MAX_CLIPBOARD_WRITE` never yield one.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.replies.get_mut().clipboard.take()
    }

    pub fn reset(&mut self) -> Result<()> {
        let mut replacement = Self::new(self.cols, self.height)?;
        replacement.set_clipboard_writes(self.replies.get_mut().clipboard_allowed);
        replacement.apply_colors(self.colors)?;
        replacement.resize(self.cols, self.height, self.cell_width, self.cell_height)?;
        let geometry = self.mouse_geometry;
        replacement.set_mouse_geometry(
            geometry.width,
            geometry.height,
            geometry.cell_width,
            geometry.cell_height,
        )?;
        replacement.search.revision = self.search.revision.wrapping_add(1);
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
            self.search.invalidate();
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

    /// Where the viewport is in the active screen's rows.
    pub fn scrollback(&self) -> Result<Scrollback> {
        // SAFETY: the zeroed output matches the scrollbar data key's layout.
        let mut scrollbar: ffi::Scrollbar = unsafe { mem::zeroed() };
        check(unsafe {
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_SCROLLBAR,
                ptr::from_mut(&mut scrollbar).cast(),
            )
        })?;
        Ok(Scrollback {
            total: scrollbar.total,
            offset: scrollbar.offset,
            rows: scrollbar.len,
        })
    }

    /// Put the viewport's first row at `offset` rows from the oldest row;
    /// an offset past the newest rows stops at the bottom.
    pub fn scroll_to_row(&mut self, offset: u64) -> Result<()> {
        let now = self.scrollback()?;
        let offset = offset.min(now.total.saturating_sub(now.rows));
        if offset == now.offset {
            return Ok(());
        }
        let delta = isize::try_from(i128::from(offset) - i128::from(now.offset))?;
        self.scroll_to(ffi::Scroll {
            tag: 2,
            value: ffi::ScrollValue { delta },
        })
    }

    fn scroll_to(&mut self, scroll: ffi::Scroll) -> Result<()> {
        // SAFETY: the tagged union is constructed only by the two methods above.
        check(unsafe { ffi::ghostty_terminal_scroll_viewport(self.terminal, scroll) })?;
        self.search.invalidate();
        Ok(())
    }

    /// The pinned terminal.h exposes GhosttyTerminalScreen as a C enum.
    /// Keep the mode query beside the native handle so the view never reads
    /// emulator state through presentation cells or output bytes.
    pub fn is_alternate_screen(&self) -> Result<bool> {
        const DATA_ACTIVE_SCREEN: i32 = 6;
        let mut screen = 0i32;
        // SAFETY: terminal.h documents a GhosttyTerminalScreen* output for
        // data key 6. C enums have the same representation as c_int here.
        check(unsafe {
            ffi::ghostty_terminal_get(
                self.terminal,
                DATA_ACTIVE_SCREEN,
                ptr::from_mut(&mut screen).cast(),
            )
        })?;
        match screen {
            0 => Ok(false),
            1 => Ok(true),
            other => bail!("Unknown Ghostty active screen: {other}"),
        }
    }

    /// No native pointer escapes. GPUI retains gesture routing and admitted presses.
    pub fn mouse_policy(&mut self) -> Result<MousePolicy> {
        self.sync_mouse()?;
        Ok(self.mouse_policy)
    }

    pub fn set_mouse_geometry(
        &mut self,
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Result<()> {
        let geometry = MouseGeometry {
            width,
            height,
            cell_width,
            cell_height,
        };
        for value in [width, height, cell_width, cell_height] {
            ensure!(
                value.is_finite() && value > 0.0 && value <= 1_000_000.0,
                "Invalid mouse geometry"
            );
        }
        // The native renderer uses u16 grid coordinates. Refuse geometry that
        // could overflow those coordinates before it crosses the C boundary.
        ensure!(
            width / cell_width < 65_535.0 && height / cell_height < 65_535.0,
            "Mouse grid exceeds native coordinate range"
        );
        if geometry != self.mouse_geometry {
            self.install_mouse_geometry(geometry)?;
            self.mouse_geometry = geometry;
        }
        Ok(())
    }

    /// Clear motion suppression after capture/admission changes. This never
    /// invents a release: only the UI knows whether a press reached its sink.
    pub fn reset_mouse(&mut self) -> Result<()> {
        // SAFETY: uniquely owned live encoder; resetting has no terminal effect.
        check(unsafe { ffi::ghostty_mouse_encoder_reset(self.mouse_encoder) })
    }

    pub fn mouse(&mut self, mut event: MouseInput) -> Result<Vec<u8>> {
        ensure!(
            event.x.is_finite() && event.y.is_finite(),
            "Invalid mouse position"
        );
        self.sync_mouse()?;
        if !self.mouse_pixels {
            // Native cell sizes are integers. Normalize against the actual
            // fractional font advance instead of rounding it and drifting
            // into the wrong column near the right edge of a wide terminal.
            event.x = event.x / self.mouse_geometry.cell_width * 256.0;
            event.y = event.y / self.mouse_geometry.cell_height * 256.0;
        }
        // SGR pixels converts floats to i32 internally. Leave ample headroom
        // for rounding; outside-view drags remain valid without undefined casts.
        ensure!(
            event.x.abs() <= 1_000_000_000.0 && event.y.abs() <= 1_000_000_000.0,
            "Mouse position exceeds native coordinate range"
        );
        // Native conversion casts to u16 before clamping to the viewport,
        // including for pixel reports. Refuse extreme outside positions before
        // that cast; ordinary outside drags/releases retain their coordinates.
        let (cell_width, cell_height) = if self.mouse_pixels {
            (
                self.mouse_geometry.cell_width.ceil().max(1.0),
                self.mouse_geometry.cell_height.ceil().max(1.0),
            )
        } else {
            (256.0, 256.0)
        };
        ensure!(
            event.x / cell_width < 65_535.0 && event.y / cell_height < 65_535.0,
            "Mouse position exceeds native cell range"
        );
        self.encode_mouse(event)
    }

    fn mouse_option<T>(&mut self, option: i32, value: &T) -> Result<()> {
        // SAFETY: private call sites pair options with their exact C input type;
        // native setters copy values synchronously, retaining no Rust borrow.
        Self::mouse_option_for(self.mouse_encoder, option, value)
    }

    fn mouse_option_for<T>(encoder: ffi::Handle, option: i32, value: &T) -> Result<()> {
        // SAFETY: all callers use a live owned encoder and the documented scalar
        // or sized-struct type. The native setter copies the value.
        check(unsafe {
            ffi::ghostty_mouse_encoder_set(encoder, option, ptr::from_ref(value).cast())
        })
    }

    fn install_mouse_geometry(&mut self, geometry: MouseGeometry) -> Result<()> {
        let (width, height, cell_width, cell_height) = if self.mouse_pixels {
            (
                geometry.width,
                geometry.height,
                geometry.cell_width,
                geometry.cell_height,
            )
        } else {
            (
                geometry.width / geometry.cell_width * 256.0,
                geometry.height / geometry.cell_height * 256.0,
                256.0,
                256.0,
            )
        };
        // Inputs were checked finite/positive/bounded by set_mouse_geometry;
        // conversion here deliberately rounds physical surface dimensions.
        let size = ffi::MouseSize {
            size: mem::size_of::<ffi::MouseSize>(),
            screen_width: width.round().max(1.0) as u32,
            screen_height: height.round().max(1.0) as u32,
            cell_width: cell_width.ceil().max(1.0) as u32,
            cell_height: cell_height.ceil().max(1.0) as u32,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        self.mouse_option(3, &size)
    }

    fn sync_mouse(&mut self) -> Result<()> {
        if !self.mouse_dirty {
            return Ok(());
        }
        // DATA_MODE/MOUSE_TRACKING expose mode bits, while the encoder uses
        // terminal flags. Overlapping DEC sets/resets can make them disagree.
        // Probe canonical native behavior in constant work after feed. A
        // separate encoder is necessary: set_from_terminal clears dedup even
        // when only ordinary text arrived. The real encoder's setters preserve
        // dedup when tracking/format did not change, preventing repaint loops.
        // SAFETY: both handles are live and exclusively borrowed.
        check(unsafe {
            ffi::ghostty_mouse_encoder_set_from_terminal(self.mouse_probe, self.terminal)
        })?;
        let size = ffi::MouseSize {
            size: mem::size_of::<ffi::MouseSize>(),
            screen_width: 2000,
            screen_height: 100,
            cell_width: 10,
            cell_height: 10,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        Self::mouse_option_for(self.mouse_probe, 3, &size)?;
        let press = MouseInput {
            action: MouseAction::Press,
            button: Some(MouseButton::Left),
            x: 1003.0,
            y: 3.0,
            modifiers: MouseModifiers::default(),
            any_button_pressed: true,
        };
        let bytes = self.encode_mouse_with(self.mouse_probe, press)?;
        // A high cell distinguishes legacy raw bytes from UTF-8; the subcell
        // position distinguishes SGR cells from SGR pixels. These are native
        // outputs, not a second implementation of terminal mode precedence.
        let format: i32 = match bytes.as_slice() {
            b"\x1b[M \x85!" => 0,
            b"\x1b[M \xc2\x85!" => 1,
            b"\x1b[<0;101;1M" => 2,
            b"\x1b[32;101;1M" => 3,
            b"\x1b[<0;1003;3M" => 4,
            b"" => 0, // Disabled tracking: format is immaterial until enabled.
            _ => bail!("Unknown native mouse format"),
        };
        let tracking = if bytes.is_empty() {
            MouseTracking::None
        } else if self
            .encode_mouse_with(
                self.mouse_probe,
                MouseInput {
                    action: MouseAction::Release,
                    ..press
                },
            )?
            .is_empty()
        {
            MouseTracking::X10
        } else if !self
            .encode_mouse_with(
                self.mouse_probe,
                MouseInput {
                    action: MouseAction::Motion,
                    button: None,
                    any_button_pressed: false,
                    ..press
                },
            )?
            .is_empty()
        {
            MouseTracking::Any
        } else if !self
            .encode_mouse_with(
                self.mouse_probe,
                MouseInput {
                    action: MouseAction::Motion,
                    ..press
                },
            )?
            .is_empty()
        {
            MouseTracking::Button
        } else {
            MouseTracking::Normal
        };
        let tracking_code: i32 = match tracking {
            MouseTracking::None => 0,
            MouseTracking::X10 => 1,
            MouseTracking::Normal => 2,
            MouseTracking::Button => 3,
            MouseTracking::Any => 4,
        };
        self.mouse_option(1, &tracking_code)?;
        self.mouse_option(2, &format)?;
        if self.mouse_pixels != (format == 4) {
            self.mouse_pixels = format == 4;
            self.install_mouse_geometry(self.mouse_geometry)?;
        }
        self.mouse_option(5, &true)?;
        self.mouse_policy = MousePolicy { tracking };
        self.mouse_dirty = false;
        Ok(())
    }

    fn encode_mouse(&mut self, event: MouseInput) -> Result<Vec<u8>> {
        self.encode_mouse_with(self.mouse_encoder, event)
    }

    fn encode_mouse_with(&mut self, encoder: ffi::Handle, event: MouseInput) -> Result<Vec<u8>> {
        let action = match event.action {
            MouseAction::Press => 0,
            MouseAction::Release => 1,
            MouseAction::Motion => 2,
        };
        Self::mouse_option_for(encoder, 4, &event.any_button_pressed)?;
        // SAFETY: handles are uniquely owned. All setters copy scalars, and the
        // fixed output buffer lives through encoding. No borrowed C data escapes.
        unsafe {
            check(ffi::ghostty_mouse_event_set_action(
                self.mouse_event,
                action,
            ))?;
            if let Some(button) = event.button {
                let button = match button {
                    MouseButton::Left => 1,
                    MouseButton::Right => 2,
                    MouseButton::Middle => 3,
                    MouseButton::WheelUp => 4,
                    MouseButton::WheelDown => 5,
                    MouseButton::WheelLeft => 6,
                    MouseButton::WheelRight => 7,
                    MouseButton::Back => 8,
                    MouseButton::Forward => 9,
                };
                check(ffi::ghostty_mouse_event_set_button(
                    self.mouse_event,
                    button,
                ))?;
            } else {
                check(ffi::ghostty_mouse_event_clear_button(self.mouse_event))?;
            }
            check(ffi::ghostty_mouse_event_set_mods(
                self.mouse_event,
                event.modifiers.native_bits(),
            ))?;
            check(ffi::ghostty_mouse_event_set_position(
                self.mouse_event,
                ffi::MousePosition {
                    x: event.x,
                    y: event.y,
                },
            ))?;
            let mut bytes = [0_u8; 128];
            let mut written = 0;
            check(ffi::ghostty_mouse_encoder_encode(
                encoder,
                self.mouse_event,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut written,
            ))?;
            ensure!(written <= bytes.len(), "Invalid native mouse output length");
            Ok(bytes[..written].to_vec())
        }
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
            color: self.colors.cursor,
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
        self.clear_search();
        // SAFETY: all handles are uniquely owned, all nullable destructors accept
        // partial initialization. Gesture cleanup requires the still-live terminal.
        // Callback storage is dropped only after this function returns.
        unsafe {
            ffi::ghostty_selection_gesture_free(self.gesture, self.terminal);
            ffi::ghostty_mouse_event_free(self.mouse_event);
            ffi::ghostty_mouse_encoder_free(self.mouse_encoder);
            ffi::ghostty_mouse_encoder_free(self.mouse_probe);
            ffi::ghostty_key_encoder_free(self.encoder);
            ffi::ghostty_render_state_image_iterator_free(self.images);
            ffi::ghostty_render_state_row_cells_free(self.cells);
            ffi::ghostty_render_state_row_iterator_free(self.rows);
            ffi::ghostty_render_state_free(self.render);
            ffi::ghostty_terminal_free(self.terminal);
        }
    }
}

/// One image as the frame places it: where its top-left cell is, how large it
/// is drawn and which part of the image shows. Offsets and sizes are in the
/// device pixels the terminal was last resized with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImagePlacement {
    pub image_id: u32,
    /// Changes whenever the image's pixels do.
    pub generation: u64,
    pub above_text: bool,
    pub z: i32,
    pub col: i32,
    pub row: i32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub width: u32,
    pub height: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
}

/// An image's pixels as four bytes per pixel in blue, green, red, alpha order.
pub struct ImagePixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What one terminal keeps of the images programs sent it, per screen.
const IMAGE_STORAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Give the library a PNG decoder, once for the process: without one it
/// rejects the PNG images most programs send.
fn install_png_decoder() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: the option takes the callback itself; it lives as long as
        // the process. An unsupported option leaves PNG images rejected.
        let _ =
            unsafe { ffi::ghostty_sys_set(ffi::SYS_OPT_DECODE_PNG, decode_png as *const c_void) };
    });
}

/// Decode a PNG of the terminal's stream to RGBA in a buffer of the
/// library's allocator, which bounds it; a refused image is rejected.
unsafe extern "C" fn decode_png(
    _userdata: *mut c_void,
    allocator: *const ffi::Allocator,
    data: *const u8,
    len: usize,
    out: *mut ffi::SysImage,
) -> bool {
    if data.is_null() || out.is_null() || len == 0 {
        return false;
    }
    // SAFETY: the library lends `len` bytes at `data` for this call.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    // A decoder must not unwind into the library.
    let decoded = std::panic::catch_unwind(|| {
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(IMAGE_STORAGE_BYTES);
        reader.limits(limits);
        reader.decode().ok().map(image::DynamicImage::into_rgba8)
    });
    let Ok(Some(pixels)) = decoded else {
        return false;
    };
    let (width, height) = pixels.dimensions();
    let raw = pixels.into_raw();
    if raw.is_empty() {
        return false;
    }
    // SAFETY: the allocator is the one lent for this call; the buffer holds
    // `raw.len()` bytes, and `out` is the library's zeroed struct.
    unsafe {
        let buffer = ffi::ghostty_alloc(allocator, raw.len());
        if buffer.is_null() {
            return false;
        }
        ptr::copy_nonoverlapping(raw.as_ptr(), buffer, raw.len());
        (*out).width = width;
        (*out).height = height;
        (*out).data = buffer;
        (*out).data_len = raw.len();
    }
    true
}

/// An image larger than this on one side is not drawn.
const MAX_IMAGE_SIDE: u32 = 8192;

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
    /// The images of this frame in drawing order: by layer, then z-index.
    pub fn images(&mut self) -> Result<Vec<ImagePlacement>> {
        let images = self.engine.images;
        // SAFETY: the iterator is owned by the exclusively borrowed engine and
        // filled from the render state this frame updated. Each output has the
        // documented type of its key.
        unsafe {
            check(ffi::ghostty_render_state_get(
                self.engine.render,
                ffi::RENDER_IMAGES,
                ptr::from_mut(&mut self.engine.images).cast(),
            ))?;
        }
        let mut placements = Vec::new();
        while unsafe { ffi::ghostty_render_state_image_iterator_next(images) } {
            let read_u32 = |key| -> Result<u32> {
                let mut value = 0u32;
                check(unsafe {
                    ffi::ghostty_render_state_image_get(
                        images,
                        key,
                        ptr::from_mut(&mut value).cast(),
                    )
                })?;
                Ok(value)
            };
            let read_i32 = |key| -> Result<i32> {
                let mut value = 0i32;
                check(unsafe {
                    ffi::ghostty_render_state_image_get(
                        images,
                        key,
                        ptr::from_mut(&mut value).cast(),
                    )
                })?;
                Ok(value)
            };
            let mut generation = 0u64;
            check(unsafe {
                ffi::ghostty_render_state_image_get(
                    images,
                    ffi::IMAGE_GENERATION,
                    ptr::from_mut(&mut generation).cast(),
                )
            })?;
            placements.push(ImagePlacement {
                image_id: read_u32(ffi::IMAGE_ID)?,
                generation,
                above_text: read_i32(ffi::IMAGE_LAYER)? >= ffi::IMAGE_LAYER_ABOVE_TEXT,
                z: read_i32(ffi::IMAGE_Z)?,
                col: read_i32(ffi::IMAGE_COL)?,
                row: read_i32(ffi::IMAGE_ROW)?,
                offset_x: read_u32(ffi::IMAGE_OFFSET_X)?,
                offset_y: read_u32(ffi::IMAGE_OFFSET_Y)?,
                width: read_u32(ffi::IMAGE_PIXEL_WIDTH)?,
                height: read_u32(ffi::IMAGE_PIXEL_HEIGHT)?,
                source_x: read_u32(ffi::IMAGE_SOURCE_X)?,
                source_y: read_u32(ffi::IMAGE_SOURCE_Y)?,
                source_width: read_u32(ffi::IMAGE_SOURCE_WIDTH)?,
                source_height: read_u32(ffi::IMAGE_SOURCE_HEIGHT)?,
            });
            if placements.len() >= 256 {
                break;
            }
        }
        placements.sort_by_key(|placement| (placement.above_text, placement.z));
        Ok(placements)
    }

    /// A copy of an image's current pixels, or None while they are not all
    /// there or the image is too large to draw.
    pub fn image_pixels(&mut self, image_id: u32) -> Result<Option<ImagePixels>> {
        let mut graphics: ffi::Handle = ptr::null_mut();
        // SAFETY: the storage and image handles are borrowed from the terminal,
        // which this frame keeps from changing; the pixel view is copied before
        // the borrow ends.
        unsafe {
            let status = ffi::ghostty_terminal_get(
                self.engine.terminal,
                ffi::DATA_KITTY_GRAPHICS,
                ptr::from_mut(&mut graphics).cast(),
            );
            if status != ffi::SUCCESS || graphics.is_null() {
                return Ok(None);
            }
            let image = ffi::ghostty_kitty_graphics_image(graphics, image_id);
            if image.is_null() {
                return Ok(None);
            }
            let (mut width, mut height, mut format) = (0u32, 0u32, 0i32);
            check(ffi::ghostty_kitty_graphics_image_get(
                image,
                ffi::KITTY_IMAGE_WIDTH,
                ptr::from_mut(&mut width).cast(),
            ))?;
            check(ffi::ghostty_kitty_graphics_image_get(
                image,
                ffi::KITTY_IMAGE_HEIGHT,
                ptr::from_mut(&mut height).cast(),
            ))?;
            check(ffi::ghostty_kitty_graphics_image_get(
                image,
                ffi::KITTY_IMAGE_FORMAT,
                ptr::from_mut(&mut format).cast(),
            ))?;
            let mut view = ffi::StringView {
                ptr: ptr::null(),
                len: 0,
            };
            let status = ffi::ghostty_kitty_graphics_image_get(
                image,
                ffi::KITTY_IMAGE_PIXELS,
                ptr::from_mut(&mut view).cast(),
            );
            if status == ffi::NO_VALUE {
                return Ok(None);
            }
            check(status)?;
            let channels = match format {
                ffi::KITTY_FORMAT_RGB => 3,
                ffi::KITTY_FORMAT_RGBA => 4,
                ffi::KITTY_FORMAT_GRAY_ALPHA => 2,
                ffi::KITTY_FORMAT_GRAY => 1,
                _ => return Ok(None),
            };
            let pixels = width as usize * height as usize;
            if width == 0
                || height == 0
                || width > MAX_IMAGE_SIDE
                || height > MAX_IMAGE_SIDE
                || view.ptr.is_null()
                || view.len != pixels * channels
            {
                return Ok(None);
            }
            let source = std::slice::from_raw_parts(view.ptr, view.len);
            let mut bgra = Vec::with_capacity(pixels * 4);
            for pixel in source.chunks_exact(channels) {
                bgra.extend_from_slice(&match channels {
                    3 => [pixel[2], pixel[1], pixel[0], 255],
                    4 => [pixel[2], pixel[1], pixel[0], pixel[3]],
                    2 => [pixel[0], pixel[0], pixel[0], pixel[1]],
                    _ => [pixel[0], pixel[0], pixel[0], 255],
                });
            }
            Ok(Some(ImagePixels {
                width,
                height,
                bgra,
            }))
        }
    }

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
        // Faint text (SGR 2) is drawn at half strength over its background,
        // as the web client's xterm does.
        if style.flags & STYLE_FAINT != 0 {
            foreground = half_way(foreground, background);
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

/// `GHOSTTY_STYLE_FLAG_FAINT` (style.h).
const STYLE_FAINT: u16 = 1 << 2;

/// The color halfway between two packed RGB colors.
fn half_way(color: u32, background: u32) -> u32 {
    [16, 8, 0].into_iter().fold(0, |packed, shift| {
        let channel = |value: u32| (value >> shift) & 255;
        packed | (channel(color) + channel(background)).div_ceil(2) << shift
    })
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

unsafe extern "C" fn on_clipboard_write(
    _: ffi::Handle,
    userdata: *mut c_void,
    write: *const ffi::ClipboardWrite,
) {
    // SAFETY: userdata is Engine's stable RefCell box, as in on_write. The
    // request and everything it points to are valid until this returns. No
    // panicking borrow, indexing or reentrant VT call occurs here, and the
    // only allocation is fallible, so nothing unwinds across the C boundary.
    let state = unsafe { &*userdata.cast::<RefCell<Replies>>() };
    let Some(write) = (unsafe { write.as_ref() }) else {
        return;
    };
    // SAFETY: the request's own slices, borrowed for this call only.
    let text = state
        .try_borrow_mut()
        .ok()
        .filter(|state| state.clipboard_allowed && !state.suppress)
        .and_then(|state| Some((state, unsafe { clipboard_text(write) }?)));
    let result = if let Some((mut state, text)) = text {
        state.clipboard = Some(text);
        ffi::CLIPBOARD_WRITE_SUCCESS
    } else {
        ffi::CLIPBOARD_WRITE_DENIED
    };
    // OSC 52 has no acknowledgement and discards the reply; a protocol with
    // one learns whether the text was accepted.
    if let Some(reply) = write.reply {
        let answer = ffi::ClipboardWriteReply {
            size: mem::size_of::<ffi::ClipboardWriteReply>(),
            result,
            remember: false,
        };
        // SAFETY: called once, within the callback, with the request itself.
        unsafe { reply(write, &answer) };
    }
}

/// The plain text of a write to the standard clipboard, within the limit.
///
/// # Safety
/// `write` and the arrays and strings it points to must be live.
unsafe fn clipboard_text(write: &ffi::ClipboardWrite) -> Option<String> {
    if write.location != ffi::CLIPBOARD_LOCATION_STANDARD
        || write.contents.is_null()
        || write.contents_len == 0
    {
        return None;
    }
    // SAFETY: the caller guarantees contents_len live entries.
    let contents = unsafe { std::slice::from_raw_parts(write.contents, write.contents_len) };
    let bytes = |view: &ffi::StringView| {
        if view.ptr.is_null() || view.len == 0 {
            return &[][..];
        }
        // SAFETY: a nonnull string of the request holds len live bytes.
        unsafe { std::slice::from_raw_parts(view.ptr, view.len) }
    };
    let data = contents
        .iter()
        .find(|content| bytes(&content.mime).starts_with(b"text/plain"))
        .map(|content| bytes(&content.data))?;
    if data.is_empty() || data.len() > MAX_CLIPBOARD_WRITE {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let mut owned = String::new();
    owned.try_reserve_exact(text.len()).ok()?;
    owned.push_str(text);
    Some(owned)
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
fn palette(ansi: [u32; 16]) -> [ffi::Rgb; 256] {
    let mut colors = [ffi::Rgb::default(); 256];
    for index in 0u16..256 {
        let value = match index {
            0..=15 => ansi[usize::from(index)],
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
