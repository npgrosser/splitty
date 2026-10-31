//! Binary entry point for the Splitty Tauri desktop application.
//! Delegates immediately to [`splitty_tauri_lib::run`].

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Work around WebKitGTK DMABUF protocol errors on Wayland compositors.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    splitty_tauri_lib::run()
}
