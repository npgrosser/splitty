//! The main window's native commands, granted at runtime for the exact origin
//! of the app's own backend.
//!
//! A capability file cannot name the backend's port, which is only known once
//! the backend listens. The former `capabilities/app.json` therefore matched
//! every `http://127.0.0.1:*` page, and only the navigation guard kept other
//! loopback pages out of the main window. This capability is added when the
//! port is known (at setup on macOS and Linux, by `wsl_start_backend` on
//! Windows), so a page on any other port gets no command even in the main
//! window (`access-control`, Desktop Shell Command Scope). Remote windows
//! (`remote-<n>`) are in no capability at all.

use tauri::ipc::CapabilityBuilder;

/// The window the capability is bound to.
const WINDOW: &str = "main";

/// What the UI served by the backend invokes, and nothing else.
const PERMISSIONS: &[&str] = &[
    "core:event:default",
    "core:window:allow-minimize",
    "core:window:allow-maximize",
    "core:window:allow-unmaximize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-close",
    "core:window:allow-destroy",
    "core:window:allow-is-maximized",
    "core:window:allow-show",
    "core:window:allow-unminimize",
    "core:window:allow-set-focus",
    "core:window:allow-start-dragging",
    "core:window:allow-start-resize-dragging",
    "core:window:allow-set-title-bar-style",
    "core:window:allow-internal-toggle-maximize",
    // The zoom shortcuts zoom the whole page (display-zoom).
    "core:webview:allow-set-webview-zoom",
    "allow-set-title-bar-color",
    "allow-wsl-setup-state",
    "allow-wsl-list-distributions",
    "allow-wsl-switch-distribution",
    "allow-remote-hosts",
    "allow-remote-open",
    "allow-remote-cancel",
    "allow-open-remote-editor",
    "allow-open-local-editor",
    "allow-reveal-download",
];

/// The remote URL pattern of the backend on `port`. Tauri reads a pattern
/// without a path as "every path, query and fragment", and the port as
/// exactly that port.
pub fn backend_pattern(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The capability of the main window while it shows the backend on `port`.
///
/// One per port: on Windows, switching the distribution starts a backend on
/// another port, and the old capability stays registered until the app quits,
/// behind the navigation guard (D9 of remote-window, provisional).
pub fn main_window(port: u16) -> CapabilityBuilder {
    PERMISSIONS
        .iter()
        .fold(
            CapabilityBuilder::new(format!("app-{port}"))
                .window(WINDOW)
                // Not the bundled pages: the Windows setup page has its own
                // capability, `capabilities/setup.json`.
                .local(false)
                .remote(backend_pattern(port)),
            |capability, permission| capability.permission(*permission),
        )
        // Links leave through the system browser; only web URLs.
        .permission_scoped(
            "opener:allow-open-url",
            vec![
                serde_json::json!({ "url": "http://*" }),
                serde_json::json!({ "url": "https://*" }),
            ],
            Vec::<serde_json::Value>::new(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tauri::ipc::RuntimeCapability;
    use tauri::utils::acl::capability::CapabilityFile;
    use tauri::utils::acl::manifest::Manifest;
    use tauri::utils::acl::resolved::Resolved;
    use tauri::utils::acl::{ExecutionContext, RemoteUrlPattern};
    use tauri::utils::platform::Target;
    use tauri::Url;

    const PORT: u16 = 45299;

    fn url(raw: &str) -> Url {
        raw.parse().expect("valid url")
    }

    /// The capability resolved against this app's ACL, as `add_capability`
    /// resolves it. `tauri-build` writes the ACL into `OUT_DIR`.
    fn resolved() -> Resolved {
        let acl: BTreeMap<String, Manifest> = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/acl-manifests.json"
        )))
        .expect("the ACL tauri-build wrote");
        let CapabilityFile::Capability(capability) = main_window(PORT).build() else {
            panic!("a single capability");
        };
        Resolved::resolve(
            &acl,
            BTreeMap::from([(capability.identifier.clone(), capability)]),
            Target::current(),
        )
        .expect("every permission exists")
    }

    #[test]
    fn the_pattern_matches_every_path_of_the_backend() {
        let pattern: RemoteUrlPattern = backend_pattern(PORT).parse().expect("a valid pattern");
        for raw in [
            "http://127.0.0.1:45299/",
            "http://127.0.0.1:45299/?bootstrap=abc&desktop=1",
            "http://127.0.0.1:45299/popout.html#pane",
        ] {
            assert!(pattern.test(&url(raw)), "{raw}");
        }
    }

    /// Every other loopback port is another server. Its pages get no command,
    /// even in the main window.
    #[test]
    fn the_pattern_matches_no_other_origin() {
        let pattern: RemoteUrlPattern = backend_pattern(PORT).parse().expect("a valid pattern");
        for raw in [
            "http://127.0.0.1:45298/",
            "http://127.0.0.1/",
            "http://localhost:45299/",
            "https://127.0.0.1:45299/",
            "http://127.0.0.1.evil.example:45299/",
        ] {
            assert!(!pattern.test(&url(raw)), "{raw}");
        }
    }

    #[test]
    fn the_commands_are_granted_to_the_main_window_on_the_backend_only() {
        let resolved = resolved();
        for command in [
            "remote_hosts",
            "remote_open",
            "remote_cancel",
            "open_remote_editor",
            "open_local_editor",
            "reveal_download",
            "set_title_bar_color",
            "plugin:window|start_dragging",
            "plugin:webview|set_webview_zoom",
            "plugin:opener|open_url",
        ] {
            let grants = &resolved.allowed_commands[command];
            assert!(!grants.is_empty(), "{command}");
            for grant in grants {
                let ExecutionContext::Remote { url: pattern } = &grant.context else {
                    panic!("{command} is granted to the bundled pages");
                };
                assert!(pattern.test(&url("http://127.0.0.1:45299/")), "{command}");
                assert!(!pattern.test(&url("http://127.0.0.1:45300/")), "{command}");
                let windows: Vec<&str> = grant.windows.iter().map(|w| w.as_str()).collect();
                assert_eq!(windows, [WINDOW], "{command}");
            }
        }
    }

    #[test]
    fn editor_is_not_granted_to_setup_and_generic_opener_stays_web_only() {
        let setup: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/setup.json"))
                .expect("setup capability");
        assert!(!setup["permissions"]
            .as_array()
            .expect("permissions")
            .iter()
            .any(|p| p == "allow-open-remote-editor" || p == "allow-open-local-editor"));
        let CapabilityFile::Capability(capability) = main_window(PORT).build() else {
            panic!("a single capability");
        };
        let value = serde_json::to_value(capability).expect("serializable capability");
        let opener = value["permissions"]
            .as_array()
            .expect("permissions")
            .iter()
            .find(|p| p["identifier"] == "opener:allow-open-url")
            .expect("generic opener");
        assert_eq!(
            opener["allow"],
            serde_json::json!([{"url":"http://*"}, {"url":"https://*"}])
        );
    }

    /// The page that hands out a bootstrap URL stays with the setup page.
    #[test]
    fn starting_the_wsl_backend_is_not_granted() {
        assert!(!resolved()
            .allowed_commands
            .contains_key("wsl_start_backend"));
    }
}
