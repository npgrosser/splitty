//! Build script that invokes `tauri_build` to generate Tauri platform bindings.
//!
//! The app manifest lists every custom command, which makes Tauri check them
//! against the capabilities like plugin commands. Without it, Tauri skips the
//! ACL for app commands entirely, so any page that ended up in the main window
//! could call them, `wsl_start_backend` included (security review SR-03, see
//! docs/security/findings.md). Each command now has to be granted explicitly
//! as `allow-<command>`: in `capabilities/` for the bundled pages, and in the
//! main window's capability, which `capability.rs` adds at runtime.

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "set_title_bar_color",
            "wsl_setup_state",
            "wsl_list_distributions",
            "wsl_start_backend",
            "wsl_switch_distribution",
            "remote_hosts",
            "remote_open",
            "remote_cancel",
            "open_remote_editor",
            "open_local_editor",
            "reveal_download",
        ]),
    ))
    .expect("failed to run tauri-build");
}
