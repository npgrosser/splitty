//! The capabilities a server lists in `mux.welcome`, each defined once for
//! the server that announces it and the clients that look for it, and the
//! list itself, in the order servers send it ([`welcome`]).
//!
//! A client that does not find a capability takes the conservative path the
//! spec names for an older server.

use serde_json::Value;

/// Managed jobs (`managed-jobs`); also listed by `GET /api/jobs`.
pub const MANAGED_JOBS: &str = "managed-jobs-v1";
/// Per-session scratchpads (`session-scratchpad`).
pub const SCRATCHPAD: &str = "scratchpad-v1";
/// Window layout changes reach the server (`mux-protocol`, Window Layout
/// Changes Reach the Server).
pub const LAYOUT_ARRANGE: &str = "layout-arrange-v1";
/// Selected tabs reach the server (`mux-protocol`, Selected Tabs Reach the
/// Server).
pub const TAB_SELECT: &str = "tab-select-v1";
/// Exact tab placement (`mux-protocol`, Exact Tab Placement Capability).
pub const TAB_PLACEMENT: &str = "tab-placement-v1";
/// Viewer panes (`viewer-panes`, Viewer Capability).
pub const VIEWER: &str = "viewer-v1";
/// Viewer annotations (`viewer-annotations`, Annotations Capability).
pub const VIEWER_ANNOTATIONS: &str = "viewer-annotations-v1";
/// Templates kept by the server (`templates`).
pub const SERVER_TEMPLATES: &str = "server-templates-v1";
/// The ordered workspace catalog (`mux.workspace_snapshot`).
pub const WORKSPACE_CATALOG: &str = "workspace-catalog-v1";
/// Screen placement in the catalog (`move_screen`).
pub const SCREEN_PLACEMENT: &str = "screen-placement-v1";

/// Local main connections only: the hub decides about hosts for the windows
/// (`remote-screens`).
pub const HUB_HOST_DECISIONS: &str = "hub-host-decisions-v1";
/// Local main connections only: the service's SSH questions are announced
/// with `ssh.prompts.changed` and read with `ssh.prompts.get`.
pub const SSH_PROMPTS: &str = "ssh-prompts-v1";
/// Local main connections only: the hub forwards the ports that appear on
/// hosts with `remote.autoForwardPorts` itself, so a client keeps no
/// automatic forwarding of its own.
pub const HUB_AUTO_FORWARD: &str = "hub-auto-forward-v1";
/// Local main connections only: the hub writes remembered remote Screens per
/// host (`remoteScreens.updateHost`) and takes directory observations
/// without pane counts.
pub const HUB_HOST_STATE: &str = "hub-host-state-v1";

/// Pane titles and busy state come from the server's snapshots, the server
/// closes exited panes and leaves launch overrides out unless given.
pub const SERVER_DERIVED_PANE_STATE: &str = "serverDerivedPaneState";
/// Confirmed external-editor requests (`editor.request`).
pub const EDITOR_OPEN: &str = "editor-open-v1";
/// Files offered to the windows of a session (`file.offer`).
pub const FILE_OFFER: &str = "file-offer-v1";
/// The server arbitrates PTY sizes (`protocol::sizing`).
pub const PTY_SIZE: &str = "pty-size-v1";
/// Terminal input as bytes that need not be UTF-8 (`input_bytes`).
pub const TERMINAL_INPUT_BYTES: &str = "terminal-input-bytes-v1";
/// Upload destination selection and cancellation (`protocol::upload`).
pub const UPLOAD_CONTROL: &str = "upload-control-v1";
/// The server reads program status and sends `program_status.changed`.
pub const PROGRAM_STATUS: &str = "program-status-v1";
/// Read-only close assessment (`mux.assess_close`, `mux-protocol`, Close
/// Assessment).
pub const CLOSE_ASSESSMENT: &str = "close-assessment-v1";
/// Notification events carry a `Notice` (`protocol::notice`).
pub const NOTIFICATION_NOTICES: &str = "notification-notices-v1";
/// The server names Screens created without a name and takes `onlyIfEmpty`
/// on `create_screen` (`mux-protocol`, Server-Chosen Screen Names).
pub const SCREEN_NAMES: &str = "screen-names-v1";
/// The server derives saved templates' file names (`templates`, Saved
/// Template Names) and takes a save by name with an overwrite flag.
pub const TEMPLATE_NAMES: &str = "template-names-v1";
/// The server keeps a version per scratchpad and takes `ifVersion` on
/// `set_scratchpad` (`session-scratchpad`, Concurrent Edits).
pub const SCRATCHPAD_VERSIONS: &str = "scratchpad-versions-v1";
/// The server anchors annotations and builds reviews on request.
pub const VIEWER_REVIEWS: &str = "viewer-reviews-v1";
/// A whole tab group moves to another Screen (`move_tab_group_to_screen`,
/// `workspace-model`, Move A Tab Group To Another Screen).
pub const TAB_GROUP_TRANSFER: &str = "tab-group-transfer-v1";

/// What every connection is told.
const EVERY_CONNECTION: [&str; 10] = [
    MANAGED_JOBS,
    SCRATCHPAD,
    LAYOUT_ARRANGE,
    TAB_SELECT,
    TAB_PLACEMENT,
    VIEWER,
    VIEWER_ANNOTATIONS,
    SERVER_TEMPLATES,
    WORKSPACE_CATALOG,
    SCREEN_PLACEMENT,
];

/// What a local main connection to a service with a hub is told besides.
const LOCAL_HUB: [&str; 4] = [
    HUB_HOST_DECISIONS,
    SSH_PROMPTS,
    HUB_AUTO_FORWARD,
    HUB_HOST_STATE,
];

/// What every connection is told after the hub's capabilities.
const SESSION: [&str; 14] = [
    SERVER_DERIVED_PANE_STATE,
    EDITOR_OPEN,
    FILE_OFFER,
    PTY_SIZE,
    TERMINAL_INPUT_BYTES,
    UPLOAD_CONTROL,
    PROGRAM_STATUS,
    CLOSE_ASSESSMENT,
    NOTIFICATION_NOTICES,
    SCREEN_NAMES,
    TEMPLATE_NAMES,
    SCRATCHPAD_VERSIONS,
    VIEWER_REVIEWS,
    TAB_GROUP_TRANSFER,
];

/// The capabilities of `mux.welcome`, in the order servers send them;
/// `local_hub` for a local main connection to a service with a hub.
pub fn welcome(local_hub: bool) -> Vec<String> {
    let hub: &[&str] = if local_hub { &LOCAL_HUB } else { &[] };
    EVERY_CONNECTION
        .iter()
        .chain(hub)
        .chain(&SESSION)
        .map(|capability| (*capability).to_owned())
        .collect()
}

/// Whether `welcome` (a `mux.welcome` message) lists `capability`. A welcome
/// without a list of capabilities, as from an old server, lists none.
pub fn offered(welcome: &Value, capability: &str) -> bool {
    welcome
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|all| all.iter().any(|offered| offered == capability))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_capability_is_listed_once() {
        let all = welcome(true);
        for (index, capability) in all.iter().enumerate() {
            assert!(!all[..index].contains(capability), "{capability}");
        }
        assert_eq!(all.len(), welcome(false).len() + LOCAL_HUB.len());
    }

    #[test]
    fn only_a_listed_capability_is_offered() {
        let welcome = json!({"type":"mux.welcome","capabilities":["x", VIEWER]});
        assert!(offered(&welcome, VIEWER));
        assert!(!offered(&welcome, VIEWER_REVIEWS));
        assert!(!offered(&json!({"type":"mux.welcome"}), VIEWER));
        assert!(!offered(&json!({"capabilities": VIEWER}), VIEWER));
        assert!(!offered(&json!({"capabilities": [1, null]}), VIEWER));
    }
}
