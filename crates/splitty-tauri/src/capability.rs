//! Packaged-document ACL is a coarse window boundary. The native broker also
//! checks the actual window and private current-document proof on every app
//! command; sharing a packaged origin does not grant another window's role.
use tauri::ipc::CapabilityBuilder;

const TRANSPORT: &[&str] = &[
    "allow-native-bootstrap-begin",
    "allow-native-bootstrap-confirm",
    "allow-native-open",
    "allow-native-cancel-open",
    "allow-native-start",
    "allow-native-write",
    "allow-native-consumed",
    "allow-native-close",
    "allow-native-connection-state",
];
const MAIN: &[&str] = &[
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
    "core:webview:allow-set-webview-zoom",
    "allow-set-title-bar-color",
    "allow-close-popout",
    "allow-wsl-setup-state",
    "allow-wsl-list-distributions",
    "allow-wsl-start-backend",
    "allow-wsl-open-backend",
    "allow-wsl-switch-distribution",
    "allow-remote-hosts",
    "allow-remote-open",
    "allow-remote-cancel",
    "allow-remote-progress",
    "allow-open-remote-editor",
    "allow-open-local-editor",
    "allow-reveal-download",
];

pub(crate) fn main_window() -> CapabilityBuilder {
    TRANSPORT
        .iter()
        .chain(MAIN)
        .fold(
            CapabilityBuilder::new("packaged-main")
                .window("main")
                .local(true),
            |capability, permission| capability.permission(*permission),
        )
        .permission_scoped(
            "opener:allow-open-url",
            vec![
                serde_json::json!({"url":"http://*"}),
                serde_json::json!({"url":"https://*"}),
            ],
            Vec::<serde_json::Value>::new(),
        )
}
pub(crate) fn remote_windows() -> CapabilityBuilder {
    TRANSPORT.iter().fold(
        CapabilityBuilder::new("packaged-remotes")
            .window("remote-*")
            .local(true),
        |capability, permission| capability.permission(*permission),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::ipc::RuntimeCapability;
    use tauri::utils::acl::capability::CapabilityFile;

    #[test]
    fn packaged_csp_allows_wasm_without_loopback_or_script_evaluation_grants() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let policy = config["app"]["security"]["csp"].as_str().unwrap();
        assert!(policy.contains("script-src 'self' 'wasm-unsafe-eval';"));
        assert!(!policy.contains("'unsafe-eval'"));
        assert!(!policy.contains("127.0.0.1:*"));
        assert!(policy.contains("frame-src 'none'"));
    }

    #[test]
    fn remote_windows_only_have_native_transport_commands_and_no_remote_origins() {
        let CapabilityFile::Capability(capability) = remote_windows().build() else {
            panic!("one capability");
        };
        let value = serde_json::to_value(capability).unwrap();
        assert_eq!(value["windows"], serde_json::json!(["remote-*"]));
        assert_eq!(value["local"], true);
        assert!(value["remote"].is_null());
        let grants = value["permissions"].as_array().unwrap();
        assert_eq!(grants.len(), TRANSPORT.len());
        for command in [
            "allow-open-local-editor",
            "allow-close-popout",
            "allow-reveal-download",
            "allow-remote-open",
            "allow-wsl-start-backend",
        ] {
            assert!(!grants.iter().any(|grant| grant == command));
        }
    }
}
