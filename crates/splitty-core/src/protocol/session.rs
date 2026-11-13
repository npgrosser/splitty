//! The messages a server sends one connection besides [`super::ServerEvent`]s:
//! answers to its requests and requests to it. They carry no event envelope
//! (`id`, `timestamp`): each is transient and bound to its connection.
//!
//! A client reads them ignoring fields it does not know; a message of a type
//! it does not know is not one of these.

use serde::{Deserialize, Serialize};

use crate::mux::CloseAssessment;
use crate::template::Template;

/// A message of [`super::capability`]-gated exchanges a server sends one
/// connection, tagged by `type`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionMessage {
    /// The local hub's host state changed; a window reads it again. Only on
    /// local main connections (`hub-host-decisions-v1`).
    #[serde(rename = "hub.changed")]
    HubChanged,

    /// The answer to `mux.extract_template`: the template, or why there is
    /// none.
    #[serde(rename = "mux.template_extracted", rename_all = "camelCase")]
    TemplateExtracted {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<Template>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// The answer to `mux.assess_close` (`close-assessment-v1`): what the
    /// close would end, or why the scope was refused.
    #[serde(rename = "mux.close_assessed", rename_all = "camelCase")]
    CloseAssessed {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assessment: Option<CloseAssessment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// A process in a session asks the window to open a path in an editor
    /// (`editor-open-v1`); the window confirms it with the user and answers
    /// with `editor.result` before `expires_at`.
    #[serde(rename = "editor.request", rename_all = "camelCase")]
    EditorRequest {
        request_id: String,
        session_id: String,
        path: String,
        kind: PathKind,
        /// The editor the process named; `null` lets the window choose.
        editor: Option<Editor>,
        /// UTC Unix milliseconds.
        expires_at: u64,
    },

    /// An `editor.request` ended before the window answered it.
    #[serde(rename = "editor.cancelled", rename_all = "camelCase")]
    EditorCancelled { request_id: String },

    /// A process in a session offers a file to the windows that show it
    /// (`file-offer-v1`). `name` is the server's untrusted file name.
    #[serde(rename = "file.offer", rename_all = "camelCase")]
    FileOffer {
        offer_id: String,
        session_id: String,
        name: String,
        size: u64,
        /// UTC Unix milliseconds.
        expires_at: u64,
    },

    /// A `file.offer` ended for this connection.
    #[serde(rename = "file.closed", rename_all = "camelCase")]
    FileClosed {
        offer_id: String,
        reason: FileClosedReason,
    },
}

/// What an `editor.request` names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    File,
    Folder,
}

/// The editors a process may name in an editor request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Editor {
    Vscode,
    Zed,
}

/// Why a `file.offer` ended for a connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileClosedReason {
    /// Another window accepted it.
    Taken,
    Declined,
    Expired,
    Disconnected,
    Failed,
    /// A reason a newer server names; the offer ended all the same.
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn wire(message: SessionMessage) -> Value {
        serde_json::to_value(message).unwrap()
    }

    /// Each message has the JSON the server built with `json!` before.
    #[test]
    fn messages_keep_their_json() {
        assert_eq!(
            wire(SessionMessage::HubChanged),
            json!({"type":"hub.changed"})
        );
        assert_eq!(
            wire(SessionMessage::TemplateExtracted {
                request_id: "r1".into(),
                template: None,
                error: Some("screen not found".into()),
            }),
            json!({"type":"mux.template_extracted", "requestId":"r1", "error":"screen not found"})
        );
        let template: Template = serde_json::from_value(json!({
            "name":"Saved","panes":{"main":{"dir":"/srv"}},"layout":{"slot":"main"}
        }))
        .unwrap();
        let expected = serde_json::to_value(&template).unwrap();
        assert_eq!(
            wire(SessionMessage::TemplateExtracted {
                request_id: "r2".into(),
                template: Some(template),
                error: None,
            }),
            json!({"type":"mux.template_extracted", "requestId":"r2", "template":expected})
        );
        let assessment = CloseAssessment::default();
        assert_eq!(
            wire(SessionMessage::CloseAssessed {
                request_id: "a1".into(),
                assessment: Some(assessment.clone()),
                error: None,
            }),
            json!({"type":"mux.close_assessed", "requestId":"a1", "assessment":assessment})
        );
        assert_eq!(
            wire(SessionMessage::CloseAssessed {
                request_id: "a2".into(),
                assessment: None,
                error: Some("missing scope".into()),
            }),
            json!({"type":"mux.close_assessed", "requestId":"a2", "error":"missing scope"})
        );
        for (editor, named) in [(None, Value::Null), (Some(Editor::Zed), json!("zed"))] {
            assert_eq!(
                wire(SessionMessage::EditorRequest {
                    request_id: "e1".into(),
                    session_id: "s1".into(),
                    path: "/tmp/a".into(),
                    kind: PathKind::Folder,
                    editor,
                    expires_at: 1_700_000_000_000,
                }),
                json!({"type":"editor.request","requestId":"e1","sessionId":"s1","path":"/tmp/a",
                       "kind":"folder","editor":named,"expiresAt":1_700_000_000_000_u64})
            );
        }
        assert_eq!(
            wire(SessionMessage::EditorCancelled {
                request_id: "e1".into()
            }),
            json!({"type":"editor.cancelled","requestId":"e1"})
        );
        assert_eq!(
            wire(SessionMessage::FileOffer {
                offer_id: "0123456789abcdef0123456789abcdef".into(),
                session_id: "s1".into(),
                name: "report.pdf".into(),
                size: 12,
                expires_at: 5,
            }),
            json!({"type":"file.offer","offerId":"0123456789abcdef0123456789abcdef",
                   "sessionId":"s1","name":"report.pdf","size":12,"expiresAt":5})
        );
        for (reason, named) in [
            (FileClosedReason::Taken, "taken"),
            (FileClosedReason::Declined, "declined"),
            (FileClosedReason::Expired, "expired"),
            (FileClosedReason::Disconnected, "disconnected"),
            (FileClosedReason::Failed, "failed"),
        ] {
            assert_eq!(
                wire(SessionMessage::FileClosed {
                    offer_id: "o1".into(),
                    reason,
                }),
                json!({"type":"file.closed","offerId":"o1","reason":named})
            );
        }
    }

    #[test]
    fn messages_decode_with_fields_and_reasons_this_build_does_not_know() {
        let closed: SessionMessage = serde_json::from_value(
            json!({"type":"file.closed","offerId":"o1","reason":"moved","by":"x"}),
        )
        .unwrap();
        assert!(matches!(
            closed,
            SessionMessage::FileClosed {
                reason: FileClosedReason::Other,
                ..
            }
        ));
        let changed: SessionMessage =
            serde_json::from_value(json!({"type":"hub.changed","hosts":["vm1"]})).unwrap();
        assert!(matches!(changed, SessionMessage::HubChanged));
        assert!(serde_json::from_value::<SessionMessage>(json!({"type":"mux.welcome"})).is_err());
    }
}
