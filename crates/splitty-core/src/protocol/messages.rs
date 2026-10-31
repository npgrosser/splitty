//! Protocol message types and constants for client-server communication.
//!
//! Defines `Request` (client-to-server), `ServerEvent` (server-to-client tagged enum),
//! message type constants for client→server dispatching, and `ClientCommand`.

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::mux::{ScreenInfo, ScreenState};
use crate::pty::ports::ListeningPort;

// ---------------------------------------------------------------------------
// Client → Server message types (used for request dispatching)
// ---------------------------------------------------------------------------
pub const MSG_CREATE: &str = "create";
pub const MSG_ATTACH: &str = "attach";
pub const MSG_DETACH: &str = "detach";
pub const MSG_INPUT: &str = "input";
pub const MSG_RESIZE: &str = "resize";
pub const MSG_CLOSE: &str = "close";
/// Store a pasted image on this server and answer with its path.
pub const MSG_SAVE_IMAGE: &str = "save_image";
/// One chunk of a file uploaded to this server (`protocol::upload`).
pub const MSG_UPLOAD: &str = "upload";
/// The connection's user acted (`takeover`: use its sizes now).
pub const MSG_ACTIVITY: &str = "activity";
/// The connection is not in use until its next `activity`.
pub const MSG_IDLE: &str = "idle";
/// The connection keeps a session but does not show it.
pub const MSG_HIDE: &str = "hide";

// Mux client → server
pub const MSG_MUX_OP: &str = "mux.op";
pub const MSG_MUX_RESYNC: &str = "mux.resync";

// ---------------------------------------------------------------------------
// Request — transport-agnostic client-to-server message
// ---------------------------------------------------------------------------
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub msg_type: String,
    pub id: String,
    pub data: Vec<u8>,
    pub shell: String,
    pub shell_profile_id: String,
    pub cols: u16,
    pub rows: u16,
    pub command: String,
    pub dir: String,
    pub root: String,
    pub restore_mode: String,
    pub pane_id: String,
    pub screen_id: String,
    /// The client's id of a request that is answered on its own
    /// (`save_image`, `upload`), echoed in the answer.
    pub request_id: String,
    /// `upload`, first chunk: the client's name of the file.
    pub name: String,
    /// `upload`, first chunk: the file's length in bytes.
    pub size: Option<u64>,
    /// `upload`: where in the file the chunk starts.
    pub offset: u64,
    /// `activity`: every other connection is idle until its next activity.
    pub takeover: bool,
    /// `resize`: redraw the session even if its PTY size does not change.
    pub redraw: bool,
}

// ---------------------------------------------------------------------------
// ClientCommand — server-initiated command sent over the wire
// ---------------------------------------------------------------------------

/// A command the server asks the client to execute (e.g. from CLI or context menu).
/// Serializes as a flat JSON object with `type` as the discriminator, matching
/// the TypeScript `ClientCommand` union and the canonical protocol spec.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientCommand {
    #[serde(rename = "pane.activate", rename_all = "camelCase")]
    PaneActivate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    #[serde(rename = "pane.copy", rename_all = "camelCase")]
    PaneCopy {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    #[serde(rename = "pane.paste", rename_all = "camelCase")]
    PanePaste {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    #[serde(rename = "pane.clear", rename_all = "camelCase")]
    PaneClear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    #[serde(rename = "pane.close", rename_all = "camelCase")]
    PaneClose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
    },
    #[serde(rename = "pane.split", rename_all = "camelCase")]
    PaneSplit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
        direction: String,
    },
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// ServerEvent — unified server-to-client event enum
// ---------------------------------------------------------------------------

/// Generate a unique event ID (e-<16 hex chars>).
pub fn generate_event_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill(&mut bytes);
    format!("e-{}", hex::encode(bytes))
}

/// Unified server→client event.
///
/// All JSON events share `id` (event ID for dedup) and `timestamp` (Unix ms).
/// Session-scoped events additionally have `session_id`.
///
/// Serialized flat on the wire with `type` as the discriminator tag.
/// Terminal output bypasses this enum entirely (binary frames).
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    // -- Session lifecycle --------------------------------------------------
    #[serde(rename = "created", rename_all = "camelCase")]
    SessionCreated {
        id: String,
        timestamp: i64,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        shell_profile_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        home_dir: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        semantic_phase: Option<String>,
    },

    #[serde(rename = "attached", rename_all = "camelCase")]
    SessionAttached {
        id: String,
        timestamp: i64,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        home_dir: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        semantic_phase: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        semantic_exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        semantic_command_count: Option<u32>,
    },

    #[serde(rename = "closed", rename_all = "camelCase")]
    SessionClosed {
        id: String,
        timestamp: i64,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<String>,
    },

    #[serde(rename = "error", rename_all = "camelCase")]
    Error {
        id: String,
        timestamp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        error: String,
    },

    // -- Session stream events (derived from PTY) ---------------------------
    #[serde(rename = "title", rename_all = "camelCase")]
    TitleChanged {
        id: String,
        timestamp: i64,
        session_id: String,
        title: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cmdline: Option<String>,
        base_shell: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        osc_title: Option<String>,
    },

    #[serde(rename = "osc133", rename_all = "camelCase")]
    SemanticBoundary {
        id: String,
        timestamp: i64,
        session_id: String,
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    #[serde(rename = "bell", rename_all = "camelCase")]
    Bell {
        id: String,
        timestamp: i64,
        session_id: String,
    },

    /// The session's PTY size, as its size policy set it.
    #[serde(rename = "size", rename_all = "camelCase")]
    Size {
        id: String,
        timestamp: i64,
        session_id: String,
        cols: u16,
        rows: u16,
        policy: String,
    },

    // -- Process events (formerly notifications) ----------------------------
    #[serde(rename = "process.finished", rename_all = "camelCase")]
    ProcessFinished {
        id: String,
        timestamp: i64,
        session_id: String,
        process: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cmdline: Option<String>,
    },

    #[serde(rename = "process.idle", rename_all = "camelCase")]
    ProcessIdle {
        id: String,
        timestamp: i64,
        session_id: String,
        process: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    #[serde(rename = "session.exited", rename_all = "camelCase")]
    SessionExited {
        id: String,
        timestamp: i64,
        session_id: String,
        exit_code: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        process: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        user_initiated: bool,
    },

    // -- Generic notification (fallback) ------------------------------------
    #[serde(rename = "notification", rename_all = "camelCase")]
    Notification {
        id: String,
        timestamp: i64,
        severity: String,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },

    // -- Listening ports ----------------------------------------------------
    /// The ports processes of the server's sessions listen on, in full, after
    /// `mux.welcome` and whenever they change (Linux only).
    #[serde(rename = "ports.changed", rename_all = "camelCase")]
    PortsChanged {
        id: String,
        timestamp: i64,
        ports: Vec<ListeningPort>,
    },

    // -- Mux events ---------------------------------------------------------
    #[serde(rename = "mux.welcome", rename_all = "camelCase")]
    MuxWelcome {
        id: String,
        timestamp: i64,
        client_id: String,
        screens: Vec<ScreenInfo>,
        capabilities: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        server_mode: Option<String>,
    },

    #[serde(rename = "mux.screen_snapshot", rename_all = "camelCase")]
    MuxScreenSnapshot {
        id: String,
        timestamp: i64,
        screen: ScreenState,
    },

    #[serde(rename = "mux.nudge", rename_all = "camelCase")]
    MuxNudge {
        id: String,
        timestamp: i64,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        screen_id: Option<String>,
    },

    /// Sent after a mux effect spawns a PTY session for a pane.
    /// Clients use this to attach to the session instead of creating one.
    #[serde(rename = "mux.pane_spawned", rename_all = "camelCase")]
    MuxPaneSpawned {
        id: String,
        timestamp: i64,
        pane_id: String,
        session_id: String,
    },

    /// Sent to the originating client when a mux op was rejected. Carries the
    /// pane the op would have created so a client waiting for `mux.pane_spawned`
    /// can stop waiting and spawn the session itself instead of hanging.
    #[serde(rename = "mux.op_failed", rename_all = "camelCase")]
    MuxOpFailed {
        id: String,
        timestamp: i64,
        op: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        screen_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
        error: String,
    },

    // -- Pasted images ------------------------------------------------------
    /// Answer to `save_image`: the image is stored at `path`, an absolute path.
    #[serde(rename = "image_saved", rename_all = "camelCase")]
    ImageSaved {
        id: String,
        timestamp: i64,
        request_id: String,
        path: String,
    },

    /// Answer to `save_image`: the image was refused or could not be stored,
    /// and nothing was stored.
    #[serde(rename = "image_failed", rename_all = "camelCase")]
    ImageFailed {
        id: String,
        timestamp: i64,
        request_id: String,
        error: String,
    },

    // -- Uploaded files -----------------------------------------------------
    /// Answer to an `upload` chunk while bytes of the file are missing.
    #[serde(rename = "upload_progress", rename_all = "camelCase")]
    UploadProgress {
        id: String,
        timestamp: i64,
        request_id: String,
        received: u64,
    },

    /// Answer to the last `upload` chunk: the file is stored at `path`, an
    /// absolute path.
    #[serde(rename = "upload_saved", rename_all = "camelCase")]
    UploadSaved {
        id: String,
        timestamp: i64,
        request_id: String,
        path: String,
    },

    /// Answer to an `upload` chunk that ended the upload: nothing of it is
    /// kept.
    #[serde(rename = "upload_failed", rename_all = "camelCase")]
    UploadFailed {
        id: String,
        timestamp: i64,
        request_id: String,
        error: String,
    },

    // -- Commands -----------------------------------------------------------
    #[serde(rename = "command", rename_all = "camelCase")]
    Command {
        id: String,
        timestamp: i64,
        command: ClientCommand,
    },
}

/// Convenience constructors. Each generates a fresh `id` and `timestamp`.
impl ServerEvent {
    pub fn session_created(
        session_id: impl Into<String>,
        home_dir: Option<String>,
        semantic_phase: Option<String>,
    ) -> Self {
        Self::session_created_with_profile(session_id, home_dir, semantic_phase, None)
    }

    pub fn session_created_with_profile(
        session_id: impl Into<String>,
        home_dir: Option<String>,
        semantic_phase: Option<String>,
        shell_profile_id: Option<String>,
    ) -> Self {
        Self::SessionCreated {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            shell_profile_id,
            home_dir,
            semantic_phase,
        }
    }

    pub fn session_attached(
        session_id: impl Into<String>,
        home_dir: Option<String>,
        semantic_phase: Option<String>,
        semantic_exit_code: Option<i32>,
        semantic_command_count: Option<u32>,
    ) -> Self {
        Self::SessionAttached {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            home_dir,
            semantic_phase,
            semantic_exit_code,
            semantic_command_count,
        }
    }

    pub fn session_closed(session_id: impl Into<String>, exit_code: Option<i32>) -> Self {
        Self::session_closed_result(session_id, exit_code, None)
    }

    pub fn session_closed_result(
        session_id: impl Into<String>,
        exit_code: Option<i32>,
        signal: Option<String>,
    ) -> Self {
        Self::SessionClosed {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            exit_code,
            signal,
        }
    }

    pub fn error(session_id: Option<String>, error: impl Into<String>) -> Self {
        Self::Error {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id,
            error: error.into(),
        }
    }

    pub fn title_changed(
        session_id: impl Into<String>,
        title: String,
        cwd: String,
        cmdline: Option<String>,
        base_shell: bool,
        osc_title: Option<String>,
    ) -> Self {
        Self::TitleChanged {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            title,
            cwd,
            cmdline,
            base_shell,
            osc_title,
        }
    }

    pub fn semantic_boundary(
        session_id: impl Into<String>,
        action: String,
        exit_code: Option<i32>,
    ) -> Self {
        Self::SemanticBoundary {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            action,
            exit_code,
        }
    }

    pub fn bell(session_id: impl Into<String>) -> Self {
        Self::Bell {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
        }
    }

    pub fn size(session_id: impl Into<String>, cols: u16, rows: u16, policy: &str) -> Self {
        Self::Size {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            cols,
            rows,
            policy: policy.to_string(),
        }
    }

    pub fn process_finished(
        session_id: impl Into<String>,
        process: String,
        cmdline: Option<String>,
    ) -> Self {
        Self::ProcessFinished {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            process,
            cmdline,
        }
    }

    pub fn process_idle(
        session_id: impl Into<String>,
        process: String,
        exit_code: Option<i32>,
    ) -> Self {
        Self::ProcessIdle {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            process,
            exit_code,
        }
    }

    pub fn session_exited(
        session_id: impl Into<String>,
        exit_code: i32,
        process: Option<String>,
        command: Option<String>,
        user_initiated: bool,
    ) -> Self {
        Self::SessionExited {
            id: generate_event_id(),
            timestamp: now_millis(),
            session_id: session_id.into(),
            exit_code,
            process,
            command,
            user_initiated,
        }
    }

    pub fn mux_welcome(
        client_id: String,
        screens: Vec<ScreenInfo>,
        server_mode: Option<String>,
    ) -> Self {
        Self::MuxWelcome {
            id: generate_event_id(),
            timestamp: now_millis(),
            client_id,
            screens,
            capabilities: vec![
                "managed-jobs-v1".into(),
                "scratchpad-v1".into(),
                // mux-protocol, Window Layout Changes Reach the Server.
                "layout-arrange-v1".into(),
                // mux-protocol, Selected Tabs Reach the Server.
                "tab-select-v1".into(),
            ],
            server_mode,
        }
    }

    pub fn mux_screen_snapshot(screen: ScreenState) -> Self {
        Self::MuxScreenSnapshot {
            id: generate_event_id(),
            timestamp: now_millis(),
            screen,
        }
    }

    pub fn mux_nudge(reason: String, screen_id: Option<String>) -> Self {
        Self::MuxNudge {
            id: generate_event_id(),
            timestamp: now_millis(),
            reason,
            screen_id,
        }
    }

    pub fn mux_pane_spawned(pane_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self::MuxPaneSpawned {
            id: generate_event_id(),
            timestamp: now_millis(),
            pane_id: pane_id.into(),
            session_id: session_id.into(),
        }
    }

    pub fn mux_op_failed(
        op: impl Into<String>,
        screen_id: Option<String>,
        pane_id: Option<String>,
        error: impl Into<String>,
    ) -> Self {
        Self::MuxOpFailed {
            id: generate_event_id(),
            timestamp: now_millis(),
            op: op.into(),
            screen_id,
            pane_id,
            error: error.into(),
        }
    }

    pub fn image_saved(request_id: impl Into<String>, path: impl Into<String>) -> Self {
        Self::ImageSaved {
            id: generate_event_id(),
            timestamp: now_millis(),
            request_id: request_id.into(),
            path: path.into(),
        }
    }

    pub fn image_failed(request_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self::ImageFailed {
            id: generate_event_id(),
            timestamp: now_millis(),
            request_id: request_id.into(),
            error: error.into(),
        }
    }

    pub fn upload_progress(request_id: impl Into<String>, received: u64) -> Self {
        Self::UploadProgress {
            id: generate_event_id(),
            timestamp: now_millis(),
            request_id: request_id.into(),
            received,
        }
    }

    pub fn upload_saved(request_id: impl Into<String>, path: impl Into<String>) -> Self {
        Self::UploadSaved {
            id: generate_event_id(),
            timestamp: now_millis(),
            request_id: request_id.into(),
            path: path.into(),
        }
    }

    pub fn upload_failed(request_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self::UploadFailed {
            id: generate_event_id(),
            timestamp: now_millis(),
            request_id: request_id.into(),
            error: error.into(),
        }
    }

    pub fn command(command: ClientCommand) -> Self {
        Self::Command {
            id: generate_event_id(),
            timestamp: now_millis(),
            command,
        }
    }

    pub fn ports_changed(ports: Vec<ListeningPort>) -> Self {
        Self::PortsChanged {
            id: generate_event_id(),
            timestamp: now_millis(),
            ports,
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_job_capability_is_advertised_without_changing_the_welcome_envelope() {
        let wire = serde_json::to_value(ServerEvent::mux_welcome(
            "client".into(),
            vec![],
            Some("embedded".into()),
        ))
        .expect("welcome");
        assert_eq!(
            wire["capabilities"],
            serde_json::json!([
                "managed-jobs-v1",
                "scratchpad-v1",
                "layout-arrange-v1",
                "tab-select-v1"
            ])
        );
        assert_eq!(wire["type"], "mux.welcome");
        assert_eq!(wire["clientId"], "client");
        assert_eq!(wire["serverMode"], "embedded");
    }

    #[test]
    fn test_client_command_serialization() {
        let cmd = ClientCommand::PaneSplit {
            pane_id: Some("terminal-1".to_string()),
            direction: "right".to_string(),
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["type"], "pane.split");
        assert_eq!(json["paneId"], "terminal-1");
        assert_eq!(json["direction"], "right");

        // Roundtrip
        let json_str = serde_json::to_string(&cmd).unwrap();
        let roundtrip: ClientCommand = serde_json::from_str(&json_str).unwrap();
        match roundtrip {
            ClientCommand::PaneSplit { pane_id, direction } => {
                assert_eq!(pane_id.as_deref(), Some("terminal-1"));
                assert_eq!(direction, "right");
            }
            _ => panic!("expected PaneSplit"),
        }
    }

    #[test]
    fn test_client_command_no_pane_id() {
        let cmd = ClientCommand::PaneActivate { pane_id: None };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["type"], "pane.activate");
        assert!(json.get("paneId").is_none());
    }

    #[test]
    fn test_client_command_deserialize_from_api() {
        // Simulates JSON from POST /api/command
        let json = r#"{"type": "pane.split", "paneId": "t-3", "direction": "below"}"#;
        let cmd: ClientCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ClientCommand::PaneSplit { pane_id, direction } => {
                assert_eq!(pane_id.as_deref(), Some("t-3"));
                assert_eq!(direction, "below");
            }
            _ => panic!("expected PaneSplit"),
        }
    }

    #[test]
    fn test_request_default() {
        let r = Request::default();
        assert_eq!(r.msg_type, "");
        assert_eq!(r.id, "");
        assert!(r.data.is_empty());
        assert_eq!(r.shell, "");
        assert_eq!(r.cols, 0);
        assert_eq!(r.rows, 0);
        assert_eq!(r.command, "");
        assert_eq!(r.dir, "");
        assert_eq!(r.root, "");
        assert_eq!(r.restore_mode, "");
        assert_eq!(r.pane_id, "");
        assert_eq!(r.screen_id, "");
        assert_eq!(r.request_id, "");
    }

    // -----------------------------------------------------------------------
    // ServerEvent serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn server_event_ports_changed_serialization() {
        let event = ServerEvent::ports_changed(vec![ListeningPort {
            port: 5173,
            session_id: "sess-1".into(),
            process: "vite".into(),
        }]);
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "ports.changed");
        assert_eq!(
            json["ports"],
            serde_json::json!([{"port": 5173, "sessionId": "sess-1", "process": "vite"}])
        );
        assert!(json["id"].as_str().unwrap().starts_with("e-"));
        assert!(json["timestamp"].is_i64());

        let empty = serde_json::to_value(ServerEvent::ports_changed(Vec::new())).unwrap();
        assert_eq!(empty["ports"], serde_json::json!([]));
    }

    #[test]
    fn server_event_bell_serialization() {
        let event = ServerEvent::bell("sess-123");
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "bell");
        assert_eq!(json["sessionId"], "sess-123");
        assert!(json["id"].as_str().unwrap().starts_with("e-"));
        assert!(json["timestamp"].is_i64());
        // No ghost fields
        assert!(json.get("baseShell").is_none());
        assert!(json.get("title").is_none());
        assert!(json.get("error").is_none());
    }

    #[test]
    fn server_event_size_serialization() {
        let json = serde_json::to_value(ServerEvent::size("sess-123", 100, 30, "active")).unwrap();
        assert_eq!(json["type"], "size");
        assert_eq!(json["sessionId"], "sess-123");
        assert_eq!(json["cols"], 100);
        assert_eq!(json["rows"], 30);
        assert_eq!(json["policy"], "active");
    }

    #[test]
    fn server_event_title_changed_serialization() {
        let event = ServerEvent::title_changed(
            "sess-abc",
            "vim".into(),
            "/home".into(),
            Some("vim main.rs".into()),
            false,
            Some("✳ vim".into()),
        );
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "title");
        assert_eq!(json["sessionId"], "sess-abc");
        assert_eq!(json["title"], "vim");
        assert_eq!(json["cwd"], "/home");
        assert_eq!(json["cmdline"], "vim main.rs");
        assert_eq!(json["baseShell"], false);
        assert_eq!(json["oscTitle"], "✳ vim");
    }

    #[test]
    fn server_event_session_closed_serialization() {
        let event = ServerEvent::session_closed("sess-x", Some(137));
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "closed");
        assert_eq!(json["sessionId"], "sess-x");
        assert_eq!(json["exitCode"], 137);
    }

    #[test]
    fn server_event_session_closed_no_exit_code() {
        let event = ServerEvent::session_closed("sess-x", None);
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "closed");
        assert!(json.get("exitCode").is_none());
    }

    #[test]
    fn server_event_error_serialization() {
        let event = ServerEvent::error(Some("sess-y".into()), "not found");
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "error");
        assert_eq!(json["sessionId"], "sess-y");
        assert_eq!(json["error"], "not found");
    }

    #[test]
    fn server_event_error_no_session() {
        let event = ServerEvent::error(None, "bad request");
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "error");
        assert!(json.get("sessionId").is_none());
        assert_eq!(json["error"], "bad request");
    }

    #[test]
    fn server_event_session_exited_serialization() {
        let event = ServerEvent::session_exited(
            "sess-z",
            1,
            Some("node".into()),
            Some("node server.js".into()),
            false,
        );
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "session.exited");
        assert_eq!(json["sessionId"], "sess-z");
        assert_eq!(json["exitCode"], 1);
        assert_eq!(json["process"], "node");
        assert_eq!(json["command"], "node server.js");
        assert_eq!(json["userInitiated"], false);
    }

    #[test]
    fn server_event_process_finished_serialization() {
        let event =
            ServerEvent::process_finished("sess-a", "cargo".into(), Some("cargo build".into()));
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "process.finished");
        assert_eq!(json["sessionId"], "sess-a");
        assert_eq!(json["process"], "cargo");
        assert_eq!(json["cmdline"], "cargo build");
    }

    #[test]
    fn server_event_semantic_boundary_serialization() {
        let event = ServerEvent::semantic_boundary("sess-b", "D".into(), Some(0));
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "osc133");
        assert_eq!(json["sessionId"], "sess-b");
        assert_eq!(json["action"], "D");
        assert_eq!(json["exitCode"], 0);
    }

    #[test]
    fn server_event_image_saved_serialization() {
        let event = ServerEvent::image_saved("r1", "/home/u/.cache/splitty/paste/paste-x.png");
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "image_saved");
        assert_eq!(json["requestId"], "r1");
        assert_eq!(json["path"], "/home/u/.cache/splitty/paste/paste-x.png");
        assert!(json["id"].as_str().unwrap().starts_with("e-"));
        assert!(json["timestamp"].is_i64());
        assert!(json.get("error").is_none());
    }

    #[test]
    fn server_event_image_failed_serialization() {
        let event = ServerEvent::image_failed("r2", "not a PNG, JPEG, GIF or WebP image");
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "image_failed");
        assert_eq!(json["requestId"], "r2");
        assert_eq!(json["error"], "not a PNG, JPEG, GIF or WebP image");
        assert!(json["id"].as_str().unwrap().starts_with("e-"));
        assert!(json["timestamp"].is_i64());
        assert!(json.get("path").is_none());
    }

    #[test]
    fn server_event_id_format() {
        let id = generate_event_id();
        assert!(id.starts_with("e-"), "id should start with 'e-': {}", id);
        assert_eq!(id.len(), 18, "id should be 18 chars: {}", id);
    }
}
