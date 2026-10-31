//! Last observed screen directories, separate from the client's open screens.
//! Reading or updating this local cache never contacts a host. Client writes
//! are validated as a whole; invalid host rows in a saved file are dropped.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use splitty_remote::hosts;
use splitty_remote::ssh::SshTarget;

const FILE: &str = "remote-catalog.json";
const FORMAT: u32 = 1;
const MAX_HOSTS: usize = 20;
const MAX_SCREENS: usize = 200;
const MAX_ID: usize = 256;
const MAX_NAME: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub version: u32,
    /// Most recently updated first, including updates within the same second.
    pub hosts: Vec<HostCatalog>,
}

impl Catalog {
    fn empty() -> Self {
        Self {
            version: FORMAT,
            hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostCatalog {
    pub target: String,
    pub last_seen: u64,
    pub screens: Vec<CatalogScreen>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogScreen {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub panes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogUpdate {
    target: String,
    screens: Vec<CatalogScreen>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("{0}")]
    Invalid(String),
    #[error("the host was removed; add it again before updating its directory")]
    Hidden,
    #[error("cannot write {FILE}: {0}")]
    Write(#[from] io::Error),
}

/// Missing, unreadable and malformed files yield an empty catalog. Invalid
/// or repeated hosts are skipped without discarding the other host rows.
pub fn read(config_dir: &Path) -> Catalog {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Unchecked {
        version: u32,
        hosts: Vec<serde_json::Value>,
    }

    let Some(file) = fs::read(config_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Unchecked>(&bytes).ok())
        .filter(|file| file.version == FORMAT)
    else {
        return Catalog::empty();
    };
    let mut catalog = Catalog::empty();
    for row in file.hosts {
        let Ok(host) = serde_json::from_value::<HostCatalog>(row) else {
            continue;
        };
        if check_host(&host.target, &host.screens).is_ok()
            && !catalog.hosts.iter().any(|seen| seen.target == host.target)
        {
            catalog.hosts.push(host);
            if catalog.hosts.len() == MAX_HOSTS {
                break;
            }
        }
    }
    let prefs = hosts::preferences(config_dir);
    catalog.hosts.retain(|host| {
        !prefs
            .hosts
            .iter()
            .any(|row| row.target == host.target && row.hidden)
    });
    catalog
}

/// Caller holds the shared remote metadata file lock.
pub fn forget(config_dir: &Path, target: &str) -> io::Result<()> {
    let mut catalog = read(config_dir);
    catalog.hosts.retain(|row| row.target != target);
    write(config_dir, &catalog)
}

fn write(config_dir: &Path, catalog: &Catalog) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(catalog)?;
    bytes.push(b'\n');
    hosts::write_private(config_dir, FILE, &bytes)
}

pub fn check(document: serde_json::Value) -> Result<CatalogUpdate, CatalogError> {
    let update: CatalogUpdate = serde_json::from_value(document)
        .map_err(|error| CatalogError::Invalid(format!("not a host screen directory: {error}")))?;
    check_host(&update.target, &update.screens).map_err(CatalogError::Invalid)?;
    Ok(update)
}

/// Merge under the shared remote metadata file lock, replacing only this host. The file
/// order records recency even if multiple writes share a Unix timestamp.
pub fn update(config_dir: &Path, update: CatalogUpdate) -> Result<HostCatalog, CatalogError> {
    if hosts::preferences(config_dir)
        .hosts
        .iter()
        .any(|row| row.target == update.target && row.hidden)
    {
        return Err(CatalogError::Hidden);
    }
    let host = HostCatalog {
        target: update.target,
        last_seen: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs(),
        screens: update.screens,
    };
    let mut catalog = read(config_dir);
    catalog.hosts.retain(|old| old.target != host.target);
    catalog.hosts.insert(0, host.clone());
    catalog.hosts.truncate(MAX_HOSTS);
    write(config_dir, &catalog)?;
    Ok(host)
}

fn check_host(target: &str, screens: &[CatalogScreen]) -> Result<(), String> {
    let parsed = SshTarget::parse(target).map_err(|error| error.to_string())?;
    if parsed.as_str() != target {
        return Err(format!("the target {target:?} has blanks around it"));
    }
    if screens.len() > MAX_SCREENS {
        return Err(format!("at most {MAX_SCREENS} screens per host"));
    }
    for (index, screen) in screens.iter().enumerate() {
        let length = screen.id.chars().count();
        if length == 0 || length > MAX_ID {
            return Err(format!(
                "screens[{index}]: the screen id must have 1 to {MAX_ID} characters"
            ));
        }
        if screen
            .name
            .as_ref()
            .is_some_and(|name| name.chars().count() > MAX_NAME)
        {
            return Err(format!(
                "screens[{index}]: the name has more than {MAX_NAME} characters"
            ));
        }
        if screens[..index].iter().any(|seen| seen.id == screen.id) {
            return Err(format!(
                "screens[{index}]: screen {:?} is listed twice",
                screen.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invalid_saved_hosts_are_dropped_individually() {
        let dir = tempfile::tempdir().expect("dir");
        let host = |target: &str| {
            json!({
                "target": target, "lastSeen": 100,
                "screens": [{"id": "s1", "name": "api", "panes": 1}],
            })
        };
        let mut unknown = host("extra");
        unknown["unknown"] = json!(true);
        let mut invalid_screen = host("bad-screen");
        invalid_screen["screens"][0]["panes"] = json!(-1);
        let mut duplicate_screen = host("duplicate-screen");
        duplicate_screen["screens"] = json!([
            {"id": "s1", "panes": 0}, {"id": "s1", "panes": 1},
        ]);
        let mut invalid_time = host("bad-time");
        invalid_time["lastSeen"] = json!(-1);
        let document = json!({"version": 1, "hosts": [
            host("vm1"), host(" vm2"), unknown, invalid_screen,
            duplicate_screen, invalid_time, host("vm1"), host("vm3"),
        ]});
        let text = serde_json::to_vec(&document).expect("JSON");
        fs::write(dir.path().join(FILE), &text).expect("write");
        assert_eq!(
            serde_json::to_value(read(dir.path())).expect("JSON"),
            json!({"version": 1, "hosts": [host("vm1"), host("vm3")]}),
        );
        assert_eq!(fs::read(dir.path().join(FILE)).expect("file"), text);
    }

    #[test]
    fn saved_catalogs_are_bounded_and_corrupt_files_are_empty() {
        let dir = tempfile::tempdir().expect("dir");
        assert_eq!(read(dir.path()), Catalog::empty());
        let rows: Vec<_> = (0..25)
            .map(|n| json!({"target": format!("vm{n}"), "lastSeen": 100, "screens": []}))
            .collect();
        fs::write(
            dir.path().join(FILE),
            json!({"version": 1, "hosts": rows}).to_string(),
        )
        .expect("write");
        let catalog = read(dir.path());
        assert_eq!(catalog.hosts.len(), MAX_HOSTS);
        assert_eq!(catalog.hosts[0].target, "vm0");
        assert_eq!(catalog.hosts[MAX_HOSTS - 1].target, "vm19");
        for text in [
            "not JSON",
            r#"{"version":2,"hosts":[]}"#,
            r#"{"hosts":[]}"#,
            r#"{"version":1,"hosts":null}"#,
            r#"{"version":1,"hosts":[],"extra":true}"#,
        ] {
            fs::write(dir.path().join(FILE), text).expect("write");
            assert_eq!(read(dir.path()), Catalog::empty(), "{text}");
        }
    }
}
