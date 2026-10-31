//! Durable, opt-in forward mappings. Reading never repairs a file or dials a
//! host. Mutations run under the forward manager's short storage lock; publish
//! the candidate only after private atomic replacement succeeds.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use splitty_remote::{hosts, ssh::SshTarget};

use super::forwards::{ForwardError, Key, MAX_PER_HOST};

pub const FILE: &str = "remote-forwards.json";
pub const MAX_TOTAL: usize = 256;
pub const MAX_LABEL: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedForward {
    pub target: String,
    pub remote: u16,
    pub local: u16,
    pub label: String,
    pub enabled: bool,
    /// Only the API attaches errors. They are never accepted from disk.
    #[serde(
        rename = "lastError",
        skip_deserializing,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_error: Option<SavedError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SavedError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub forwards: Vec<SavedForward>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            forwards: Vec::new(),
        }
    }
}

pub struct Store {
    path: PathBuf,
    document: Preferences,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        let document = match fs::read(path.join(FILE)) {
            Ok(bytes) => match serde_json::from_slice::<Preferences>(&bytes) {
                Ok(mut document) if valid(&document) => {
                    document
                        .forwards
                        .sort_by(|a, b| (&a.target, a.remote).cmp(&(&b.target, b.remote)));
                    document
                }
                _ => {
                    tracing::warn!(
                        "invalid {FILE}; using empty forward preferences without rewriting"
                    );
                    Preferences::default()
                }
            },
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("cannot read {FILE}: {error}");
                }
                Preferences::default()
            }
        };
        Self { path, document }
    }

    pub fn list(&self) -> Preferences {
        self.document.clone()
    }

    pub fn get(&self, key: &Key) -> Option<SavedForward> {
        self.document
            .forwards
            .iter()
            .find(|row| key.matches(row))
            .cloned()
    }

    pub fn put(&mut self, row: SavedForward) -> Result<(), ForwardError> {
        let mut next = self.document.clone();
        if let Some(old) = next
            .forwards
            .iter_mut()
            .find(|old| old.target == row.target && old.remote == row.remote)
        {
            *old = row;
        } else {
            if next.forwards.len() >= MAX_TOTAL
                || next
                    .forwards
                    .iter()
                    .filter(|old| old.target == row.target)
                    .count()
                    >= MAX_PER_HOST
            {
                return Err(ForwardError::PreferenceLimit);
            }
            next.forwards.push(row);
        }
        next.forwards
            .sort_by(|a, b| (&a.target, a.remote).cmp(&(&b.target, b.remote)));
        self.commit(next)
    }

    pub fn remove(&mut self, key: &Key) -> Result<(), ForwardError> {
        let mut next = self.document.clone();
        next.forwards.retain(|row| !key.matches(row));
        if next != self.document {
            self.commit(next)?;
        }
        Ok(())
    }

    fn commit(&mut self, next: Preferences) -> Result<(), ForwardError> {
        let mut bytes = serde_json::to_vec_pretty(&next).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        hosts::write_private(&self.path, FILE, &bytes)?;
        self.document = next;
        Ok(())
    }
}

fn valid(document: &Preferences) -> bool {
    if document.version != 1 || document.forwards.len() > MAX_TOTAL {
        return false;
    }
    let mut seen = HashSet::new();
    for row in &document.forwards {
        if row.remote == 0
            || row.local == 0
            || row.label.chars().count() > MAX_LABEL
            || !matches!(SshTarget::parse(&row.target), Ok(target) if target.as_str() == row.target)
            || !seen.insert((&row.target, row.remote))
            || document
                .forwards
                .iter()
                .filter(|other| other.target == row.target)
                .count()
                > MAX_PER_HOST
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(host: &str, port: u16) -> SavedForward {
        SavedForward {
            target: host.into(),
            remote: port,
            local: port,
            label: "Frontend".into(),
            enabled: false,
            last_error: None,
        }
    }

    #[test]
    fn preferences_reconstruct_private_independent_mappings() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config");
        let mut store = Store::new(path.clone());
        store.put(row("vm1", 5173)).expect("save frontend");
        let mut api = row("vm1", 8000);
        api.label = "API".into();
        api.local = 18000;
        api.enabled = true;
        store.put(api).expect("save API");
        store.put(row("vm2", 5173)).expect("save second host");
        let bytes = fs::read(path.join(FILE)).expect("file");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(value["forwards"].as_array().expect("rows").len(), 3);
        assert!(value["forwards"]
            .as_array()
            .expect("rows")
            .iter()
            .all(|row| row.as_object().expect("row").len() == 5));
        assert_eq!(Store::new(path.clone()).list(), store.list());
        assert_eq!(
            fs::read_dir(&path).expect("dir").count(),
            1,
            "no temp files"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path.join(FILE))
                    .expect("stat")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&path).expect("stat").permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn invalid_preferences_are_empty_without_rewriting() {
        let dir = tempfile::tempdir().expect("dir");
        assert_eq!(Store::new(dir.path().into()).list(), Preferences::default());
        assert!(!dir.path().join(FILE).exists());
        let good = serde_json::to_value(row("vm1", 3000)).expect("json");
        let mut cases = vec![
            json!(null),
            json!({"version":2,"forwards":[]}),
            json!({"version":1,"forwards":[good.clone(),good.clone()]}),
            json!({"version":1,"forwards":[],"token":"forbidden"}),
        ];
        for (field, value) in [
            ("target", json!("-oProxyCommand=x")),
            ("target", json!(" vm1 ")),
            ("remote", json!(0)),
            ("local", json!(65536)),
            ("local", json!("3000")),
            ("local", json!(-1)),
            ("local", json!(30.5)),
            ("label", json!("é".repeat(201))),
            ("label", json!(null)),
            ("enabled", json!("yes")),
            (
                "lastError",
                json!({"code":"secret","message":"never import"}),
            ),
        ] {
            let mut invalid = good.clone();
            invalid[field] = value;
            cases.push(json!({"version":1,"forwards":[invalid]}));
        }
        for bytes in cases
            .into_iter()
            .map(|value| value.to_string())
            .chain(["not JSON".to_owned()])
        {
            fs::write(dir.path().join(FILE), &bytes).expect("write");
            assert_eq!(
                Store::new(dir.path().into()).list(),
                Preferences::default(),
                "{bytes}"
            );
            assert_eq!(
                fs::read_to_string(dir.path().join(FILE)).expect("read"),
                bytes
            );
        }
        fs::remove_file(dir.path().join(FILE)).expect("remove");
        fs::create_dir(dir.path().join(FILE)).expect("unreadable document");
        assert_eq!(Store::new(dir.path().into()).list(), Preferences::default());
    }

    #[test]
    fn preference_capacity_rejects_without_eviction_and_allows_edits() {
        let dir = tempfile::tempdir().expect("dir");
        let mut store = Store::new(dir.path().into());
        for host in 0..8 {
            for port in 1..=32 {
                store.put(row(&format!("vm{host}"), port)).expect("save");
            }
        }
        assert!(matches!(
            store.put(row("vm0", 33)),
            Err(ForwardError::PreferenceLimit)
        ));
        assert!(matches!(
            store.put(row("vm8", 1)),
            Err(ForwardError::PreferenceLimit)
        ));
        let mut edited = row("vm0", 1);
        edited.label = "API".into();
        store.put(edited).expect("edit full store");
        assert_eq!(store.list().forwards.len(), MAX_TOTAL);
        assert_eq!(Store::new(dir.path().into()).list(), store.list());
    }

    #[test]
    fn failed_preference_write_keeps_memory_and_old_file() {
        let dir = tempfile::tempdir().expect("dir");
        let config = dir.path().join("config");
        let backup = dir.path().join("backup");
        let mut store = Store::new(config.clone());
        store.put(row("vm1", 3000)).expect("save");
        let before = store.list();
        fs::rename(&config, &backup).expect("move");
        fs::write(&config, b"not a directory").expect("block");
        assert!(matches!(
            store.put(row("vm2", 4000)),
            Err(ForwardError::Storage(_))
        ));
        assert_eq!(store.list(), before);
        assert_eq!(Store::new(backup).list(), before);
    }
}
