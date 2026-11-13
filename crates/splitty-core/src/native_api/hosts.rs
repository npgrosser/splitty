//! The local hub's answers about host metadata (`hostPreferences.*` and
//! `hostOrder.*`). The hub keeps its files with their own records and
//! checks; these types describe the replies, which a client reads ignoring
//! fields it does not know.

use serde::{Deserialize, Serialize};

/// One host's metadata: its alias, and whether it was forgotten.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostPreference {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// Every host's metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostPreferences {
    pub version: u32,
    pub hosts: Vec<HostPreference>,
}

/// The order in which the user arranged the windows' host groups.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostOrder {
    pub version: u32,
    pub order: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replies_keep_their_shape_and_decode_with_unknown_fields() {
        let row = HostPreference {
            target: "vm1".into(),
            alias: None,
            hidden: true,
        };
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            json!({"target":"vm1","hidden":true})
        );
        let read: HostPreference =
            serde_json::from_value(json!({"target":"vm1","alias":"Build","color":"red"})).unwrap();
        assert_eq!((read.alias.as_deref(), read.hidden), (Some("Build"), false));
        let order: HostOrder =
            serde_json::from_value(json!({"version":1,"order":["vm1"],"updated":3})).unwrap();
        assert_eq!(order.order, ["vm1"]);
        let all: HostPreferences =
            serde_json::from_value(json!({"version":1,"hosts":[{"target":"vm1"}],"future":true}))
                .unwrap();
        assert_eq!(all.hosts.len(), 1);
    }
}
