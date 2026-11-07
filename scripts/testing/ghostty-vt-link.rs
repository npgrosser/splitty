//! Native static-link check for the pinned libghostty-vt bootstrap.

use std::ffi::{c_char, c_void, CStr};
use std::ptr;

#[cfg_attr(
    target_os = "windows",
    link(name = "ghostty-vt-static", kind = "static")
)]
#[cfg_attr(not(target_os = "windows"), link(name = "ghostty-vt", kind = "static"))]
unsafe extern "C" {
    fn ghostty_version() -> *const c_char;
    fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut *mut c_void,
        cols: u16,
        rows: u16,
    ) -> i32;
    fn ghostty_terminal_free(terminal: *mut c_void);
}

fn main() {
    // SAFETY: These signatures match ghostty/vt/version.h and terminal.h in
    // libghostty-vt 1.0.0. The terminal is released before the process exits.
    unsafe {
        let version = CStr::from_ptr(ghostty_version());
        assert_eq!(version.to_bytes(), b"1.0.0");
        let mut terminal = ptr::null_mut();
        assert_eq!(ghostty_terminal_new(ptr::null(), &mut terminal, 2, 2), 0);
        assert!(!terminal.is_null());
        ghostty_terminal_free(terminal);
    }
}
