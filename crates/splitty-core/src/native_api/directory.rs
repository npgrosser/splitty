//! The passive local host-directory response shared by native and HTTP clients.
//!
//! These records describe an API response. Host preferences, SSH discovery and
//! the persisted catalog retain their own types and validation in their owners.

use serde::{Deserialize, Serialize};

/// The merged directory returned by `remote.directory.get` and HTTP.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Directory {
    pub version: u32,
    pub order: Vec<String>,
    pub hosts: Vec<DirectoryHost>,
}

impl Directory {
    pub fn new(order: Vec<String>, hosts: Vec<DirectoryHost>) -> Self {
        Self {
            version: 1,
            order,
            hosts,
        }
    }
}

/// One known, cached or forwarding-only host in the merged response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DirectoryHost {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DirectorySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
    pub screens: Vec<DirectoryScreen>,
}

impl DirectoryHost {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            alias: None,
            source: None,
            last_connected: None,
            last_seen: None,
            screens: Vec::new(),
        }
    }
}

/// A cached observation, not an opened or currently reachable Screen.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DirectoryScreen {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub panes: u64,
}

impl DirectoryScreen {
    pub fn new(id: impl Into<String>, name: Option<String>, panes: u64) -> Self {
        Self {
            id: id.into(),
            name,
            panes,
        }
    }
}

/// Storage and SSH discovery keep their separate source type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DirectorySource {
    Recent,
    SshConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn exact_directory_json_keeps_names_source_and_unsigned_values() {
        let mut recent = DirectoryHost::new("vm1");
        recent.alias = Some("Build".into());
        recent.source = Some(DirectorySource::Recent);
        recent.last_connected = Some(u64::MAX);
        recent.last_seen = Some(0);
        recent.screens = vec![DirectoryScreen::new("s1", Some("Work".into()), u64::MAX)];
        let mut configured = DirectoryHost::new("api.devpod");
        configured.source = Some(DirectorySource::SshConfig);
        let response = Directory::new(
            vec!["vm1".into(), "api.devpod".into()],
            vec![recent, configured],
        );
        let expected = json!({
            "version": 1,
            "order": ["vm1", "api.devpod"],
            "hosts": [
                {"target":"vm1", "alias":"Build", "source":"recent",
                 "lastConnected":u64::MAX, "lastSeen":0,
                 "screens":[{"id":"s1","name":"Work","panes":u64::MAX}]},
                {"target":"api.devpod","source":"ssh-config","screens":[]}
            ]
        });
        assert_eq!(serde_json::to_value(&response).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<Directory>(expected).unwrap(),
            response
        );
    }

    #[test]
    fn absent_observations_omit_optionals_without_claiming_freshness() {
        let directory = Directory::new(vec![], vec![DirectoryHost::new("vm1")]);
        assert_eq!(
            serde_json::to_value(directory).unwrap(),
            json!({"version":1,"order":[],"hosts":[{"target":"vm1","screens":[]}]})
        );
        assert_eq!(
            serde_json::to_value(DirectoryScreen::new("s1", None, 0)).unwrap(),
            json!({"id":"s1","panes":0})
        );
    }

    #[test]
    fn decoding_ignores_additions_but_requires_core_fields_and_known_sources() {
        let mut value = json!({"version":1,"order":[],"hosts":[
            {"target":"vm1","screens":[],"futureHostField":true}
        ],"futureRootField":true});
        assert!(serde_json::from_value::<Directory>(value.clone()).is_ok());
        value["hosts"][0]["source"] = Value::String("other".into());
        assert!(serde_json::from_value::<Directory>(value.clone()).is_err());
        value["hosts"][0].as_object_mut().unwrap().remove("source");
        value["hosts"][0].as_object_mut().unwrap().remove("screens");
        assert!(serde_json::from_value::<Directory>(value).is_err());
        assert!(serde_json::from_value::<Directory>(json!({
            "version":1,"order":[],"hosts":[{"target":"vm1","screens":[
                {"id":"s1","panes":-1}
            ]}]
        }))
        .is_err());
    }
}
