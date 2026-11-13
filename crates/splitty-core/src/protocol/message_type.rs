//! The `type` of each JSON message a server or a hub sends a window, so a
//! client tells messages apart with one shared name instead of a string of
//! its own. What a message carries is read by the module that handles it.
//!
//! A type this build does not know, and a message without a string `type`,
//! is [`MessageType::Unknown`], which every client ignores: newer servers may
//! send messages older clients do not know.

use serde::Deserialize;
use serde_json::Value;

/// The `type` of a message to a window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Deserialize)]
pub enum MessageType {
    // -- Events (`super::ServerEvent`) --------------------------------------
    #[serde(rename = "settings.changed")]
    SettingsChanged,
    #[serde(rename = "created")]
    Created,
    #[serde(rename = "attached")]
    Attached,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "title")]
    Title,
    #[serde(rename = "bell")]
    Bell,
    #[serde(rename = "size")]
    Size,
    #[serde(rename = "process.finished")]
    ProcessFinished,
    #[serde(rename = "process.idle")]
    ProcessIdle,
    #[serde(rename = "session.exited")]
    SessionExited,
    #[serde(rename = "ports.changed")]
    PortsChanged,
    #[serde(rename = "program_status.changed")]
    ProgramStatusChanged,
    #[serde(rename = "mux.welcome")]
    MuxWelcome,
    #[serde(rename = "mux.screen_snapshot")]
    MuxScreenSnapshot,
    #[serde(rename = "mux.workspace_snapshot")]
    MuxWorkspaceSnapshot,
    #[serde(rename = "mux.nudge")]
    MuxNudge,
    #[serde(rename = "mux.pane_spawned")]
    MuxPaneSpawned,
    #[serde(rename = "mux.op_failed")]
    MuxOpFailed,
    #[serde(rename = "image_saved")]
    ImageSaved,
    #[serde(rename = "image_failed")]
    ImageFailed,
    #[serde(rename = "upload_progress")]
    UploadProgress,
    #[serde(rename = "upload_saved")]
    UploadSaved,
    #[serde(rename = "upload_failed")]
    UploadFailed,
    #[serde(rename = "command")]
    Command,

    // -- One connection's messages (`super::session::SessionMessage`) -------
    #[serde(rename = "hub.changed")]
    HubChanged,
    #[serde(rename = "ssh.prompts.changed")]
    SshPromptsChanged,
    #[serde(rename = "mux.template_extracted")]
    MuxTemplateExtracted,
    #[serde(rename = "mux.close_assessed")]
    MuxCloseAssessed,
    #[serde(rename = "editor.request")]
    EditorRequest,
    #[serde(rename = "editor.cancelled")]
    EditorCancelled,
    #[serde(rename = "file.offer")]
    FileOffer,
    /// A chunk of an accepted offer's file.
    #[serde(rename = "file.chunk")]
    FileChunk,
    #[serde(rename = "file.closed")]
    FileClosed,

    // -- Viewer documents and reviews ---------------------------------------
    #[serde(rename = "viewer.chunk")]
    ViewerChunk,
    #[serde(rename = "viewer.unchanged")]
    ViewerUnchanged,
    #[serde(rename = "viewer.anchored")]
    ViewerAnchored,
    #[serde(rename = "viewer.reviewed")]
    ViewerReviewed,
    #[serde(rename = "viewer.failed")]
    ViewerFailed,
    #[serde(rename = "viewer.review_answered")]
    ViewerReviewAnswered,

    // -- A hub's messages before a host's relay -----------------------------
    #[serde(rename = "remote.progress")]
    RemoteProgress,
    #[serde(rename = "remote.ready")]
    RemoteReady,
    #[serde(rename = "remote.error")]
    RemoteError,

    /// A type this build does not know.
    #[serde(other)]
    Unknown,
}

impl MessageType {
    /// The type of `message`, a JSON message to a window.
    pub fn of(message: &Value) -> Self {
        message
            .get("type")
            .and_then(|kind| Self::deserialize(kind).ok())
            .unwrap_or(Self::Unknown)
    }

    /// Whether `message` is of this type.
    pub fn is(self, message: &Value) -> bool {
        self != Self::Unknown && Self::of(message) == self
    }
}

#[cfg(test)]
mod tests {
    use super::super::session::{FileClosedReason, SessionMessage};
    use super::super::ServerEvent;
    use super::*;
    use serde_json::json;

    fn event(event: ServerEvent) -> Value {
        serde_json::to_value(event).unwrap()
    }

    fn session(message: SessionMessage) -> Value {
        serde_json::to_value(message).unwrap()
    }

    /// A message of `kind` as its sender builds it; every type has one.
    fn sample(kind: MessageType) -> Value {
        use MessageType::*;
        match kind {
            SettingsChanged => event(ServerEvent::settings_changed(json!({}))),
            Created => event(ServerEvent::session_created("s1", None, None)),
            Attached => event(ServerEvent::session_attached("s1", None, None, None, None)),
            Closed => event(ServerEvent::session_closed("s1", Some(0))),
            Error => event(ServerEvent::error(Some("s1".into()), "gone")),
            Title => json!({"type":"title","id":"e","timestamp":1,"sessionId":"s1"}),
            Bell => event(ServerEvent::bell("s1")),
            Size => event(ServerEvent::size("s1", 80, 24, "largest")),
            ProcessFinished => event(ServerEvent::process_finished("s1", "make".into(), None)),
            ProcessIdle => event(ServerEvent::process_idle("s1", "make".into(), None, false)),
            SessionExited => event(ServerEvent::session_exited("s1", 0, None, None, false)),
            PortsChanged => event(ServerEvent::ports_changed(vec![])),
            ProgramStatusChanged => {
                event(ServerEvent::program_status_changed("s1", None, None, None))
            }
            MuxWelcome => event(ServerEvent::mux_welcome("c1".into(), vec![], None, false)),
            MuxScreenSnapshot => json!({"type":"mux.screen_snapshot","id":"e","timestamp":1}),
            MuxWorkspaceSnapshot => event(ServerEvent::mux_workspace_snapshot(vec![], None)),
            MuxNudge => event(ServerEvent::mux_nudge("mux_op".into(), None)),
            MuxPaneSpawned => event(ServerEvent::mux_pane_spawned("p1", "s1")),
            MuxOpFailed => event(ServerEvent::mux_op_failed("close_pane", None, None, "x")),
            ImageSaved => event(ServerEvent::image_saved("r1", "/tmp/a.png")),
            ImageFailed => event(ServerEvent::image_failed("r1", "x")),
            UploadProgress => event(ServerEvent::upload_progress("r1", 1)),
            UploadSaved => event(ServerEvent::upload_saved("r1", "/tmp/a")),
            UploadFailed => event(ServerEvent::upload_failed("r1", "x")),
            Command => event(ServerEvent::command(
                super::super::ClientCommand::PaneActivate { pane_id: None },
            )),
            HubChanged => session(SessionMessage::HubChanged),
            SshPromptsChanged => session(SessionMessage::SshPromptsChanged),
            MuxTemplateExtracted => session(SessionMessage::TemplateExtracted {
                request_id: "r1".into(),
                template: None,
                error: Some("x".into()),
            }),
            MuxCloseAssessed => session(SessionMessage::CloseAssessed {
                request_id: "r1".into(),
                assessment: None,
                error: Some("x".into()),
            }),
            EditorRequest => session(SessionMessage::EditorRequest {
                request_id: "r1".into(),
                session_id: "s1".into(),
                path: "/a".into(),
                kind: super::super::session::PathKind::File,
                editor: None,
                expires_at: 1,
            }),
            EditorCancelled => session(SessionMessage::EditorCancelled {
                request_id: "r1".into(),
            }),
            FileOffer => session(SessionMessage::FileOffer {
                offer_id: "o1".into(),
                session_id: "s1".into(),
                name: "a".into(),
                size: 1,
                expires_at: 1,
            }),
            FileChunk => {
                json!({"type":"file.chunk","offerId":"o1","offset":0,"data":"","eof":true})
            }
            FileClosed => session(SessionMessage::FileClosed {
                offer_id: "o1".into(),
                reason: FileClosedReason::Taken,
            }),
            ViewerChunk => json!({"type":"viewer.chunk","requestId":"r1","offset":0,"size":0,
                "version":"v","data":"","eof":true}),
            ViewerUnchanged => json!({"type":"viewer.unchanged","requestId":"r1","version":"v"}),
            ViewerAnchored => json!({"type":"viewer.anchored","requestId":"r1","paneId":"v1",
                "version":"v","anchors":[]}),
            ViewerReviewed => json!({"type":"viewer.reviewed","requestId":"r1","paneId":"v1",
                "version":"v","text":"","count":0,"entries":[]}),
            ViewerFailed => json!({"type":"viewer.failed","requestId":"r1","code":"unknown_pane",
                "error":"x"}),
            ViewerReviewAnswered => json!({"type":"viewer.review_answered","paneId":"v1",
                "waitId":"w","accepted":true}),
            RemoteProgress => json!({"type":"remote.progress","step":"connect","message":"x"}),
            RemoteReady => json!({"type":"remote.ready","target":"vm1"}),
            RemoteError => json!({"type":"remote.error","code":"auth","message":"x",
                "retryable":true}),
            Unknown => json!({"type":"future.message"}),
        }
    }

    #[test]
    fn every_type_reads_from_a_message_of_it() {
        use MessageType::*;
        let all = [
            SettingsChanged,
            Created,
            Attached,
            Closed,
            Error,
            Title,
            Bell,
            Size,
            ProcessFinished,
            ProcessIdle,
            SessionExited,
            PortsChanged,
            ProgramStatusChanged,
            MuxWelcome,
            MuxScreenSnapshot,
            MuxWorkspaceSnapshot,
            MuxNudge,
            MuxPaneSpawned,
            MuxOpFailed,
            ImageSaved,
            ImageFailed,
            UploadProgress,
            UploadSaved,
            UploadFailed,
            Command,
            HubChanged,
            SshPromptsChanged,
            MuxTemplateExtracted,
            MuxCloseAssessed,
            EditorRequest,
            EditorCancelled,
            FileOffer,
            FileChunk,
            FileClosed,
            ViewerChunk,
            ViewerUnchanged,
            ViewerAnchored,
            ViewerReviewed,
            ViewerFailed,
            ViewerReviewAnswered,
            RemoteProgress,
            RemoteReady,
            RemoteError,
            Unknown,
        ];
        for kind in all {
            let message = sample(kind);
            assert_eq!(MessageType::of(&message), kind, "{message}");
            assert_eq!(kind.is(&message), kind != Unknown);
        }
    }

    #[test]
    fn messages_without_a_known_string_type_are_unknown() {
        for message in [
            json!({}),
            json!({"type": null}),
            json!({"type": 7}),
            json!({"type": "Mux.Welcome"}),
            json!("mux.welcome"),
            json!(["mux.welcome"]),
        ] {
            assert_eq!(MessageType::of(&message), MessageType::Unknown, "{message}");
        }
        assert!(!MessageType::Unknown.is(&json!({"type":"future.message"})));
    }
}
