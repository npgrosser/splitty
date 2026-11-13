//! The remembered remote Screens of the local hub: the list it returns
//! (`remoteScreens.get`, and in answer to each write) and the per-host write
//! (`remoteScreens.updateHost`), shared by the hub and its clients.
//!
//! A body names one host and the Screens to remember for it; the hub
//! replaces only that host's entries in the list it keeps. The hub keeps its
//! file and checks a list it is sent with its own record; a client reads the
//! list ignoring fields it does not know.

use serde::{Deserialize, Serialize};

/// The format version of the list.
pub const FORMAT: u32 = 1;

/// How many Screens the list holds at most.
pub const MAX_SCREENS: usize = 200;

/// The longest Screen id, in characters.
pub const MAX_ID: usize = 256;

/// The longest name, in characters: the windows' cap on names.
pub const MAX_NAME: usize = 200;

/// The remembered list, as the hub returns it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Remembered {
    pub version: u32,
    pub screens: Vec<RememberedScreen>,
}

impl Default for Remembered {
    fn default() -> Self {
        Self {
            version: FORMAT,
            screens: Vec::new(),
        }
    }
}

/// A remote Screen a window shows: its host, its id on the host, and what
/// the window needs to show it before the host is connected.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RememberedScreen {
    pub target: String,
    pub screen_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its place among the window's Screens, local ones included, written
    /// by clients that keep one list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// Its host was closed in the window: the Screen is offered again, but
    /// not reopened at start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
}

impl RememberedScreen {
    /// An open Screen; a name is cut to [`MAX_NAME`] characters.
    pub fn new(target: &str, screen_id: &str, name: Option<&str>) -> Self {
        Self {
            target: target.into(),
            screen_id: screen_id.into(),
            name: name.map(|name| name.chars().take(MAX_NAME).collect()),
            position: None,
            closed: false,
        }
    }

    /// The same Screen, its host closed.
    pub fn closed(mut self) -> Self {
        self.closed = true;
        self
    }
}

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
    fn the_list_decodes_with_fields_this_build_does_not_know() {
        let list: Remembered = serde_json::from_value(json!({"version":1,"screens":[
            {"target":"vm1","screenId":"s1","name":"api","position":2,"closed":true,"color":"red"}
        ],"updated":5}))
        .unwrap();
        let mut expected = RememberedScreen::new("vm1", "s1", Some("api")).closed();
        expected.position = Some(2);
        assert_eq!(list.screens, [expected]);
        assert_eq!(
            serde_json::to_value(RememberedScreen::new("vm1", "a", None)).unwrap(),
            json!({"target":"vm1","screenId":"a"})
        );
        assert_eq!(
            RememberedScreen::new("vm1", "a", Some(&"x".repeat(MAX_NAME + 1)))
                .name
                .map(|name| name.chars().count()),
            Some(MAX_NAME)
        );
    }

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
