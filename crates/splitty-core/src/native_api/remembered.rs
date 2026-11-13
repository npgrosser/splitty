//! The per-host write of the remembered remote Screens
//! (`remoteScreens.updateHost`), shared by the local hub and its clients.
//!
//! A body names one host and the Screens to remember for it; the hub
//! replaces only that host's entries in the list it keeps.

use serde::{Deserialize, Serialize};

/// The Screens to remember for one host. An empty list forgets the host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct HostScreens {
    pub target: String,
    pub screens: Vec<HostScreen>,
    /// The host was closed in the window: its Screens are offered again but
    /// not reopened at start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
}

impl HostScreens {
    pub fn new(target: impl Into<String>, screens: Vec<HostScreen>, closed: bool) -> Self {
        Self {
            target: target.into(),
            screens,
            closed,
        }
    }
}

/// One remembered Screen of the host the body names.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct HostScreen {
    pub screen_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its place among the window's Screens, local ones included, for
    /// clients that keep one list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

impl HostScreen {
    pub fn new(screen_id: impl Into<String>, name: Option<String>) -> Self {
        Self {
            screen_id: screen_id.into(),
            name,
            position: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_host_write_has_one_shape() {
        let body = HostScreens::new("vm1", vec![HostScreen::new("s1", Some("api".into()))], true);
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({"target":"vm1","screens":[{"screenId":"s1","name":"api"}],"closed":true})
        );
        let open: HostScreens = serde_json::from_value(
            json!({"target":"vm1","screens":[{"screenId":"s1","position":3}]}),
        )
        .unwrap();
        assert!(!open.closed);
        assert_eq!(open.screens[0].position, Some(3));
        for invalid in [
            json!({"target":"vm1"}),
            json!({"target":"vm1","screens":[],"extra":1}),
            json!({"target":"vm1","screens":[{"screenId":"s1","target":"vm2"}]}),
        ] {
            assert!(serde_json::from_value::<HostScreens>(invalid).is_err());
        }
    }
}
