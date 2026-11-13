//! Ports of this machine shared with hosts (`reverse.*`) and their
//! remembered rules (`reversePreferences.*`), as the local hub's replies
//! carry them; read like [`super::forwards`].

use serde::{Deserialize, Serialize};

pub use super::forwards::{ForwardState, SavedError};

/// An active share: port `remote` of the host reaches `localhost:local`
/// here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReverseInfo {
    pub target: String,
    /// The host's port.
    pub remote: u16,
    /// The port here.
    pub local: u16,
    pub state: ForwardState,
}

/// A remembered share rule, with the failure of its last start or restore in
/// a reply (`lastError`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedReverse {
    pub target: String,
    pub remote: u16,
    pub local: u16,
    /// Restored when the host connects.
    pub enabled: bool,
    #[serde(rename = "lastError", default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<SavedError>,
}

/// The remembered share rules (`reversePreferences.get`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReversePreferences {
    pub version: u32,
    pub forwards: Vec<SavedReverse>,
}

impl Default for ReversePreferences {
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
        let active: Vec<ReverseInfo> = serde_json::from_value(json!([
            {"target":"vm1","remote":8080,"local":9090,"state":"down","future":1}
        ]))
        .unwrap();
        assert_eq!(active[0].state, ForwardState::Down);
        let saved: ReversePreferences = serde_json::from_value(json!({"version":1,"forwards":[
            {"target":"vm1","remote":8080,"local":9090,"enabled":false,
             "lastError":{"code":"bridge","message":"gone"},"future":null}
        ]}))
        .unwrap();
        assert_eq!(
            saved.forwards[0].last_error.as_ref().unwrap().message,
            "gone"
        );
    }
}
