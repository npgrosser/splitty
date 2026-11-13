//! The local hub's forwards of host ports to this machine (`forward.*`) and
//! its remembered forward rules (`forwardPreferences.*`), as its replies
//! carry them. The hub keeps its file with its own record and checks; these
//! types describe the replies, which a client reads ignoring fields it does
//! not know.

use serde::{Deserialize, Serialize};

/// An active forward: `localhost:local` here reaches port `remote` of the
/// host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForwardInfo {
    pub target: String,
    pub remote: u16,
    pub local: u16,
    /// Where a browser opens the forward.
    #[serde(default)]
    pub url: String,
    pub state: ForwardState,
}

/// Whether an active forward or share carries connections now. A reply
/// with another state is refused as malformed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardState {
    Up,
    Down,
}

/// A remembered forward rule, with the failure of its last start or restore
/// in a reply (`lastError`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedForward {
    pub target: String,
    pub remote: u16,
    pub local: u16,
    pub label: String,
    /// Restored when the host connects.
    pub enabled: bool,
    #[serde(rename = "lastError", default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<SavedError>,
}

/// Why a remembered forward or share did not start: the `remote.error` code,
/// a message and, where the user can act, a hint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// The remembered forward rules (`forwardPreferences.get`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForwardPreferences {
    pub version: u32,
    pub forwards: Vec<SavedForward>,
}

impl Default for ForwardPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            forwards: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replies_decode_with_fields_this_build_does_not_know() {
        let active: Vec<ForwardInfo> = serde_json::from_value(json!([
            {"target":"vm1","remote":3000,"local":4173,"url":"http://localhost:4173/",
             "state":"up","since":1},
            {"target":"vm1","remote":3001,"local":3001,"state":"down"}
        ]))
        .unwrap();
        assert_eq!(active[0].state, ForwardState::Up);
        assert_eq!(active[1].state, ForwardState::Down);
        assert_eq!(active[1].url, "");
        assert!(serde_json::from_value::<ForwardInfo>(
            json!({"target":"vm1","remote":1,"local":1,"state":"rebuilding"})
        )
        .is_err());
        let saved: ForwardPreferences = serde_json::from_value(json!({
            "version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,
                "label":"App\nPreview","enabled":true,"future":true,
                "lastError":{"code":"local_port_conflict","message":"occupied","future":1}}],
            "future":[]
        }))
        .unwrap();
        let error = saved.forwards[0].last_error.as_ref().unwrap();
        assert_eq!(
            (error.code.as_str(), error.hint.as_deref()),
            ("local_port_conflict", None)
        );
        assert!(serde_json::from_value::<ForwardInfo>(
            json!({"target":"vm1","remote":70000,"local":1,"state":"up"})
        )
        .is_err());
    }
}
