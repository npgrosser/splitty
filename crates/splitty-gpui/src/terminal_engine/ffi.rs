//! Private declarations for the pinned libghostty-vt 1.0.0 C ABI.
//!
//! Keep layouts in sync with include/ghostty/vt at the pinned release. No
//! Rust enum crosses this boundary: C enums are ints and may gain variants.
use std::ffi::{c_int, c_void};

pub type Handle = *mut c_void;
pub type Status = c_int;
pub const SUCCESS: Status = 0;
pub const INVALID_VALUE: Status = -2;
pub const OUT_OF_SPACE: Status = -3;
pub const NO_VALUE: Status = -4;
pub const UNSUPPORTED: Status = -8;

#[repr(C)]
pub struct Allocator {
    pub context: *mut c_void,
    pub vtable: *const AllocatorVtable,
}
#[repr(C)]
pub struct AllocatorVtable {
    pub alloc: Option<unsafe extern "C" fn(*mut c_void, usize, u8, usize) -> *mut c_void>,
    pub resize:
        Option<unsafe extern "C" fn(*mut c_void, *mut c_void, usize, u8, usize, usize) -> bool>,
    pub remap: Option<
        unsafe extern "C" fn(*mut c_void, *mut c_void, usize, u8, usize, usize) -> *mut c_void,
    >,
    pub free: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, usize, u8, usize)>,
    pub reserved: [usize; 4],
}
#[repr(C)]
pub struct SelectionBuffer {
    pub ptr: *mut Selection,
    pub cap: usize,
    pub len: usize,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StringView {
    pub ptr: *const u8,
    pub len: usize,
}
#[repr(C)]
pub struct Buffer {
    pub ptr: *mut u8,
    pub cap: usize,
    pub len: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union StyleColorValue {
    pub palette: u8,
    pub rgb: Rgb,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StyleColor {
    pub tag: u8,
    pub value: StyleColorValue,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Style {
    pub fg_color: StyleColor,
    pub bg_color: StyleColor,
    pub underline_color: StyleColor,
    pub flags: u16,
    pub reserved: u16,
}
#[repr(C)]
pub struct RenderColors {
    pub size: usize,
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Rgb,
    pub cursor_has_value: bool,
    pub palette: [Rgb; 256],
}
#[repr(C)]
pub struct RenderCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Coordinate {
    pub x: u16,
    pub y: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union PointValue {
    pub coordinate: Coordinate,
    pub padding: [u64; 2],
}
#[repr(C)]
pub struct Point {
    pub tag: c_int,
    pub value: PointValue,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GridRef {
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}
#[repr(C)]
pub struct Selection {
    pub start: GridRef,
    pub end: GridRef,
    pub rectangle: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MousePosition {
    pub x: f32,
    pub y: f32,
}
#[repr(C)]
pub struct MouseSize {
    pub size: usize,
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}
#[repr(C)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
pub struct GestureGeometry {
    pub columns: u32,
    pub cell_width: u32,
    pub padding_left: u32,
    pub screen_height: u32,
}
#[repr(C)]
pub struct GestureBehaviors {
    pub single_click: c_int,
    pub double_click: c_int,
    pub triple_click: c_int,
}
#[repr(C)]
pub union ScrollValue {
    pub delta: isize,
    pub padding: [u64; 2],
}
#[repr(C)]
pub struct Scroll {
    pub tag: c_int,
    pub value: ScrollValue,
}
#[repr(C)]
pub struct Scrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Writer {
    pub write: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool>,
    pub userdata: *mut c_void,
}
#[repr(C)]
pub struct MimeReader {
    pub read: Option<unsafe extern "C" fn(*mut c_void, StringView, Writer) -> bool>,
    pub userdata: *mut c_void,
}
#[repr(C)]
pub struct Paste {
    pub size: usize,
    pub location: c_int,
    pub source: c_int,
    pub mimes: *const StringView,
    pub mimes_len: usize,
    pub reader: MimeReader,
    pub allow_unsafe: bool,
}

// Option/data discriminants from terminal.h, render.h, selection.h, and
// formatter.h. Keeping their names separate prevents confusing equal numbers.
pub const OPT_USERDATA: c_int = 1;
pub const OPT_WRITE_PTY: c_int = 2;
pub const OPT_FOREGROUND: c_int = 12;
pub const OPT_BACKGROUND: c_int = 13;
pub const OPT_CURSOR: c_int = 14;
pub const OPT_PALETTE: c_int = 15;
pub const OPT_IMAGE_LIMIT: c_int = 16;
pub const OPT_SELECTION: c_int = 22;
pub const OPT_SCROLLBACK_BYTES: c_int = 28;
pub const OPT_SCROLLBACK_LINES: c_int = 29;
pub const DATA_CURSOR_X: c_int = 3;
pub const DATA_CURSOR_Y: c_int = 4;
pub const DATA_CURSOR_VISIBLE: c_int = 7;
pub const DATA_KITTY_FLAGS: c_int = 8;
pub const DATA_SCROLLBAR: c_int = 9;
pub const DATA_SELECTION: c_int = 31;
pub const RENDER_DIRTY: c_int = 3;
pub const RENDER_ROWS: c_int = 4;
pub const RENDER_CURSOR: c_int = 18;
pub const RENDER_COLORS: c_int = 19;
pub const ROW_DIRTY: c_int = 1;
pub const ROW_CELLS: c_int = 3;
pub const ROW_Y: c_int = 6;
pub const ROW_ID: c_int = 7;
pub const CELL_RAW: c_int = 1;
pub const CELL_STYLE: c_int = 2;
pub const CELL_BACKGROUND: c_int = 5;
pub const CELL_SELECTED: c_int = 7;
pub const CELL_UTF8: c_int = 9;
pub const CELL_WIDE: c_int = 3;
pub const EVENT_REF: c_int = 1;
pub const EVENT_POSITION: c_int = 2;
pub const EVENT_BEHAVIORS: c_int = 7;
pub const EVENT_GEOMETRY: c_int = 9;
pub const FORMAT_UNWRAP: c_int = 2;
pub const FORMAT_SELECTION: c_int = 4;

unsafe extern "C" {
    pub fn ghostty_search_new(
        allocator: *const Allocator,
        out: *mut Handle,
        terminal: Handle,
    ) -> Status;
    pub fn ghostty_search_free(search: Handle);
    pub fn ghostty_search_set(search: Handle, option: c_int, value: *const c_void) -> Status;
    pub fn ghostty_search_get(search: Handle, data: c_int, out: *mut c_void) -> Status;
    pub fn ghostty_search_feed(search: Handle) -> Status;
    pub fn ghostty_search_tick(search: Handle, status: *mut c_int) -> Status;
    pub fn ghostty_search_select_next(search: Handle) -> Status;
    pub fn ghostty_search_select_prev(search: Handle) -> Status;
    pub fn ghostty_terminal_point_from_grid_ref(
        terminal: Handle,
        reference: *const GridRef,
        tag: c_int,
        out: *mut Coordinate,
    ) -> Status;
    pub fn ghostty_grid_ref_cell(reference: *const GridRef, out: *mut u64) -> Status;
    pub fn ghostty_grid_ref_row(reference: *const GridRef, out: *mut u64) -> Status;
    pub fn ghostty_row_get(row: u64, data: c_int, out: *mut c_void) -> Status;
    pub fn ghostty_grid_ref_graphemes(
        reference: *const GridRef,
        buf: *mut u32,
        cap: usize,
        written: *mut usize,
    ) -> Status;
    pub fn ghostty_grid_ref_hyperlink_uri(
        reference: *const GridRef,
        buf: *mut u8,
        cap: usize,
        written: *mut usize,
    ) -> Status;

    pub fn ghostty_terminal_new(
        allocator: *const c_void,
        out: *mut Handle,
        cols: u16,
        rows: u16,
    ) -> Status;
    pub fn ghostty_terminal_free(terminal: Handle);
    pub fn ghostty_terminal_set(terminal: Handle, option: c_int, value: *const c_void) -> Status;
    pub fn ghostty_terminal_get(terminal: Handle, data: c_int, out: *mut c_void) -> Status;
    pub fn ghostty_terminal_vt_write(terminal: Handle, data: *const u8, len: usize);
    pub fn ghostty_terminal_resize(
        terminal: Handle,
        cols: u16,
        rows: u16,
        cell_width: u32,
        cell_height: u32,
    ) -> Status;
    pub fn ghostty_terminal_scroll_viewport(terminal: Handle, scroll: Scroll) -> Status;
    pub fn ghostty_terminal_grid_ref(terminal: Handle, point: Point, out: *mut GridRef) -> Status;
    pub fn ghostty_terminal_paste(
        terminal: Handle,
        paste: *const Paste,
        pasted: *mut bool,
    ) -> Status;
    pub fn ghostty_render_state_new(allocator: *const c_void, out: *mut Handle) -> Status;
    pub fn ghostty_render_state_free(state: Handle);
    pub fn ghostty_render_state_update(state: Handle, terminal: Handle) -> Status;
    pub fn ghostty_render_state_clean(state: Handle) -> Status;
    pub fn ghostty_render_state_get(state: Handle, data: c_int, out: *mut c_void) -> Status;
    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const c_void,
        out: *mut Handle,
    ) -> Status;
    pub fn ghostty_render_state_row_iterator_free(rows: Handle);
    pub fn ghostty_render_state_row_iterator_next(rows: Handle) -> bool;
    pub fn ghostty_render_state_row_get(rows: Handle, data: c_int, out: *mut c_void) -> Status;
    pub fn ghostty_render_state_row_cells_new(allocator: *const c_void, out: *mut Handle)
        -> Status;
    pub fn ghostty_render_state_row_cells_free(cells: Handle);
    pub fn ghostty_render_state_row_cells_next(cells: Handle) -> bool;
    pub fn ghostty_render_state_row_cells_get(
        cells: Handle,
        data: c_int,
        out: *mut c_void,
    ) -> Status;
    pub fn ghostty_cell_get(cell: u64, data: c_int, out: *mut c_void) -> Status;
    pub fn ghostty_mouse_encoder_new(allocator: *const c_void, out: *mut Handle) -> Status;
    pub fn ghostty_mouse_encoder_free(encoder: Handle);
    pub fn ghostty_mouse_encoder_set(
        encoder: Handle,
        option: c_int,
        value: *const c_void,
    ) -> Status;
    pub fn ghostty_mouse_encoder_set_from_terminal(encoder: Handle, terminal: Handle) -> Status;
    pub fn ghostty_mouse_encoder_reset(encoder: Handle) -> Status;
    pub fn ghostty_mouse_encoder_encode(
        encoder: Handle,
        event: Handle,
        buf: *mut u8,
        cap: usize,
        written: *mut usize,
    ) -> Status;
    pub fn ghostty_mouse_event_new(allocator: *const c_void, out: *mut Handle) -> Status;
    pub fn ghostty_mouse_event_free(event: Handle);
    pub fn ghostty_mouse_event_set_action(event: Handle, action: c_int) -> Status;
    pub fn ghostty_mouse_event_set_button(event: Handle, button: c_int) -> Status;
    pub fn ghostty_mouse_event_clear_button(event: Handle) -> Status;
    pub fn ghostty_mouse_event_set_mods(event: Handle, mods: u16) -> Status;
    pub fn ghostty_mouse_event_set_position(event: Handle, position: MousePosition) -> Status;
    pub fn ghostty_key_encoder_new(allocator: *const c_void, out: *mut Handle) -> Status;
    pub fn ghostty_key_encoder_free(encoder: Handle);
    pub fn ghostty_key_encoder_set_from_terminal(encoder: Handle, terminal: Handle) -> Status;
    pub fn ghostty_key_encoder_encode(
        encoder: Handle,
        event: Handle,
        buf: *mut u8,
        cap: usize,
        written: *mut usize,
    ) -> Status;
    pub fn ghostty_key_event_new(allocator: *const c_void, out: *mut Handle) -> Status;
    pub fn ghostty_key_event_free(event: Handle);
    pub fn ghostty_key_event_set_action(event: Handle, action: c_int) -> Status;
    pub fn ghostty_key_event_set_key(event: Handle, key: c_int) -> Status;
    pub fn ghostty_key_event_set_mods(event: Handle, mods: u16) -> Status;
    pub fn ghostty_key_event_set_utf8(event: Handle, text: StringView) -> Status;
    pub fn ghostty_key_event_set_unshifted_codepoint(event: Handle, codepoint: u32) -> Status;
    pub fn ghostty_selection_gesture_new(allocator: *const c_void, out: *mut Handle) -> Status;
    pub fn ghostty_selection_gesture_free(gesture: Handle, terminal: Handle);
    pub fn ghostty_selection_gesture_reset(gesture: Handle, terminal: Handle);
    pub fn ghostty_selection_gesture_event_new(
        allocator: *const c_void,
        out: *mut Handle,
        kind: c_int,
    ) -> Status;
    pub fn ghostty_selection_gesture_event_free(event: Handle);
    pub fn ghostty_selection_gesture_event_set(
        event: Handle,
        option: c_int,
        value: *const c_void,
    ) -> Status;
    pub fn ghostty_selection_gesture_event(
        gesture: Handle,
        terminal: Handle,
        event: Handle,
        selection: *mut Selection,
    ) -> Status;
    pub fn ghostty_formatter_terminal_new(
        allocator: *const c_void,
        out: *mut Handle,
        terminal: Handle,
    ) -> Status;
    pub fn ghostty_formatter_free(formatter: Handle);
    pub fn ghostty_formatter_set(formatter: Handle, option: c_int, value: *const c_void) -> Status;
    pub fn ghostty_formatter_format_buf(
        formatter: Handle,
        buf: *mut u8,
        cap: usize,
        written: *mut usize,
    ) -> Status;
}
