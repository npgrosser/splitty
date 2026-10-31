//! The hosts a user knows: the ones this client connected to before, and the
//! `Host` names of the user's SSH configuration, such as DevPod's `*.devpod`
//! workspaces. A host picker offers them.
//!
//! The recent hosts are this client's own file, `recent-hosts.json` in its
//! configuration directory. The SSH configuration is only read, never
//! written: `Host` lines and the files `Include` names, without evaluating
//! `Match` or asking `ssh -G`, which cannot list hosts anyway.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ssh::SshTarget;

/// The recent hosts' file in the client's configuration directory.
pub const RECENT_FILE: &str = "recent-hosts.json";

/// How many recent hosts the file keeps.
pub const MAX_RECENT: usize = 20;

/// The format version of [`RECENT_FILE`].
const FORMAT: u32 = 1;

/// How deep `Include` nests, as in OpenSSH (`READCONF_MAX_DEPTH`).
const MAX_INCLUDE_DEPTH: usize = 16;

/// A host this client connected to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentHost {
    pub target: String,
    /// Seconds since the Unix epoch.
    pub last_connected: u64,
}

#[derive(Serialize, Deserialize)]
struct RecentFile {
    version: u32,
    hosts: Vec<RecentHost>,
}

/// Where a known host comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Recent,
    SshConfig,
}

/// A host the user knows, as a host picker lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHost {
    pub target: String,
    pub source: Source,
    /// A display name only; never used as an SSH destination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// For a recent host, when it was last connected to (seconds since the
    /// Unix epoch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected: Option<u64>,
}

/// The user's SSH directory: `~/.ssh`, on Windows `%USERPROFILE%\.ssh`.
pub fn default_ssh_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".ssh"))
}

/// The known hosts: the recent hosts of `config_dir` first, then the
/// configured hosts of `ssh_dir` not already listed, each target once.
/// Missing or unreadable files contribute nothing.
pub fn known(config_dir: &Path, ssh_dir: Option<&Path>) -> Vec<KnownHost> {
    let mut known: Vec<KnownHost> = recent(config_dir)
        .into_iter()
        .map(|host| KnownHost {
            target: host.target,
            source: Source::Recent,
            alias: None,
            last_connected: Some(host.last_connected),
        })
        .collect();
    for target in ssh_dir.map(configured).unwrap_or_default() {
        if !known.iter().any(|host| host.target == target) {
            known.push(KnownHost {
                target,
                source: Source::SshConfig,
                alias: None,
                last_connected: None,
            });
        }
    }
    let preferences = preferences(config_dir);
    for row in &preferences.hosts {
        if row.alias.is_some() && !row.hidden && !known.iter().any(|host| host.target == row.target)
        {
            known.push(KnownHost {
                target: row.target.clone(),
                source: Source::Recent,
                alias: None,
                last_connected: None,
            });
        }
    }
    known.retain(|host| {
        !preferences
            .hosts
            .iter()
            .any(|row| row.target == host.target && row.hidden)
    });
    for host in &mut known {
        host.alias = preferences
            .hosts
            .iter()
            .find(|row| row.target == host.target)
            .and_then(|row| row.alias.clone());
    }
    known
}

/// The recent hosts in `config_dir`, most recent first. A file that is
/// missing, unreadable or not in the format counts as empty; entries that are
/// not valid targets are left out.
pub fn recent(config_dir: &Path) -> Vec<RecentHost> {
    let Some(file) = fs::read(config_dir.join(RECENT_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RecentFile>(&bytes).ok())
        .filter(|file| file.version == FORMAT)
    else {
        return Vec::new();
    };
    let mut hosts: Vec<RecentHost> = Vec::new();
    for host in file.hosts {
        if SshTarget::parse(&host.target).is_ok_and(|target| target.as_str() == host.target)
            && !hosts.iter().any(|seen| seen.target == host.target)
        {
            hosts.push(host);
        }
    }
    hosts.truncate(MAX_RECENT);
    hosts
}

/// Record `target` as the most recent host in `config_dir`: moved to the
/// front, or added there, keeping at most [`MAX_RECENT`].
pub fn record_recent(config_dir: &Path, target: &SshTarget) -> io::Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    record_recent_at(config_dir, target, now)
}

/// Record a hub connection/reconnection only while its target remains visible.
/// Automatic reconnects must not undo an explicit removal in another window.
pub fn record_recent_if_visible(config_dir: &Path, target: &SshTarget) -> io::Result<()> {
    let _writing = lock(config_dir)?;
    if preferences(config_dir)
        .hosts
        .iter()
        .any(|row| row.target == target.as_str() && row.hidden)
    {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    record_recent_locked(config_dir, target, now)
}

fn record_recent_at(config_dir: &Path, target: &SshTarget, now: u64) -> io::Result<()> {
    let _writing = lock(config_dir)?;
    record_recent_locked(config_dir, target, now)
}

fn record_recent_locked(config_dir: &Path, target: &SshTarget, now: u64) -> io::Result<()> {
    let mut prefs = preferences(config_dir);
    if let Some(row) = prefs
        .hosts
        .iter_mut()
        .find(|row| row.target == target.as_str() && row.hidden)
    {
        row.hidden = false;
        write_preferences(config_dir, &prefs)?;
    }
    let mut hosts = recent(config_dir);
    hosts.retain(|host| host.target != target.as_str());
    hosts.insert(
        0,
        RecentHost {
            target: target.as_str().to_owned(),
            last_connected: now,
        },
    );
    hosts.truncate(MAX_RECENT);
    write_recent(config_dir, hosts)
}

fn write_recent(config_dir: &Path, hosts: Vec<RecentHost>) -> io::Result<()> {
    let mut text = serde_json::to_vec_pretty(&RecentFile {
        version: FORMAT,
        hosts,
    })?;
    text.push(b'\n');
    write_private(config_dir, RECENT_FILE, &text)
}

/// Local display names and tombstones, shared by desktop shells and server.
pub const PREFERENCES_FILE: &str = "remote-host-preferences.json";
const LOCK_FILE: &str = ".remote-hosts.lock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostPreference {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostPreferences {
    pub version: u32,
    pub hosts: Vec<HostPreference>,
}

#[derive(Debug, thiserror::Error)]
pub enum PreferenceError {
    #[error("{0}")]
    Invalid(String),
    #[error("cannot write remote host metadata: {0}")]
    Write(#[from] io::Error),
}

/// Missing or malformed metadata is empty. Invalid rows are discarded.
pub fn preferences(config_dir: &Path) -> HostPreferences {
    let empty = || HostPreferences {
        version: FORMAT,
        hosts: Vec::new(),
    };
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Unchecked {
        version: u32,
        hosts: Vec<serde_json::Value>,
    }
    let Some(file) = fs::read(config_dir.join(PREFERENCES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Unchecked>(&bytes).ok())
        .filter(|prefs| prefs.version == FORMAT)
    else {
        return empty();
    };
    let mut prefs = empty();
    for value in file.hosts {
        let Ok(row) = serde_json::from_value::<HostPreference>(value) else {
            continue;
        };
        if SshTarget::parse(&row.target).is_ok_and(|target| target.as_str() == row.target)
            && row.alias.as_ref().is_none_or(|alias| valid_alias(alias))
            && !prefs.hosts.iter().any(|seen| seen.target == row.target)
        {
            prefs.hosts.push(row);
        }
    }
    prefs
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.trim() == alias
        && alias.chars().count() <= 200
        && !alias.chars().any(char::is_control)
}

fn write_preferences(config_dir: &Path, prefs: &HostPreferences) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(prefs)?;
    bytes.push(b'\n');
    write_private(config_dir, PREFERENCES_FILE, &bytes)
}

/// Serialize metadata changes across clients and desktop/server processes.
/// The file remains in place; closing it releases the OS lock, even on panic.
pub fn lock(config_dir: &Path) -> io::Result<fs::File> {
    fs::create_dir_all(config_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(config_dir, fs::Permissions::from_mode(0o700))?;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(config_dir.join(LOCK_FILE))?;
    file.lock()?;
    Ok(file)
}

/// Set or clear an alias, or explicitly reveal a forgotten target. Omitted
/// fields preserve their previous value. Only `hidden:false` is accepted;
/// forgetting also needs to remove recent/catalog metadata.
pub fn update_preference(
    config_dir: &Path,
    document: serde_json::Value,
) -> Result<HostPreference, PreferenceError> {
    use PreferenceError::Invalid;
    let Some(object) = document.as_object() else {
        return Err(Invalid("expected a host preference object".into()));
    };
    if object
        .keys()
        .any(|key| !["target", "alias", "hidden"].contains(&key.as_str()))
    {
        return Err(Invalid("unknown host preference field".into()));
    }
    let target = checked_target(object.get("target"))?;
    if !object.contains_key("alias") && !object.contains_key("hidden") {
        return Err(Invalid("supply alias or hidden:false".into()));
    }
    let alias = match object.get("alias") {
        None => None,
        Some(serde_json::Value::Null) => Some(None),
        Some(serde_json::Value::String(alias)) => {
            if alias.chars().any(char::is_control) {
                return Err(Invalid("alias must not contain controls".into()));
            }
            let alias = alias.trim();
            if !alias.is_empty() && !valid_alias(alias) {
                return Err(Invalid(
                    "alias must have at most 200 characters and no controls".into(),
                ));
            }
            Some((!alias.is_empty()).then(|| alias.to_owned()))
        }
        Some(_) => return Err(Invalid("alias must be a string or null".into())),
    };
    if object
        .get("hidden")
        .is_some_and(|value| value != &serde_json::Value::Bool(false))
    {
        return Err(Invalid(
            "hidden must be false; use DELETE to forget a host".into(),
        ));
    }
    let _writing = lock(config_dir)?;
    let mut prefs = preferences(config_dir);
    let position = prefs.hosts.iter().position(|row| row.target == target);
    let index = position.unwrap_or_else(|| {
        prefs.hosts.push(HostPreference {
            target,
            alias: None,
            hidden: false,
        });
        prefs.hosts.len() - 1
    });
    let row = &mut prefs.hosts[index];
    if let Some(alias) = alias {
        row.alias = alias;
    }
    if object.contains_key("hidden") {
        row.hidden = false;
    }
    let result = row.clone();
    write_preferences(config_dir, &prefs)?;
    Ok(result)
}

pub fn checked_target(value: Option<&serde_json::Value>) -> Result<String, PreferenceError> {
    let raw = value
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| PreferenceError::Invalid("target must be a string".into()))?;
    let target =
        SshTarget::parse(raw).map_err(|error| PreferenceError::Invalid(error.to_string()))?;
    if target.as_str() != raw {
        return Err(PreferenceError::Invalid(
            "target must not have surrounding whitespace".into(),
        ));
    }
    Ok(raw.to_owned())
}

/// Forget local recent, alias and order metadata. Caller holds `lock` while
/// also dropping the host's catalog, so stale active connections cannot
/// restore it.
pub fn forget_locked(config_dir: &Path, target: &str) -> io::Result<()> {
    let mut prefs = preferences(config_dir);
    prefs.hosts.retain(|row| row.target != target);
    prefs.hosts.push(HostPreference {
        target: target.into(),
        alias: None,
        hidden: true,
    });
    write_preferences(config_dir, &prefs)?;
    let mut order = host_order(config_dir);
    if order.order.iter().any(|row| row == target) {
        order.order.retain(|row| row != target);
        write_order(config_dir, &order)?;
    }
    let mut rows = recent(config_dir);
    rows.retain(|row| row.target != target);
    write_recent(config_dir, rows)
}

/// The order in which the user arranged the client's host groups. A file of
/// its own: the preference file's reader rejects unknown fields, so a field
/// there would make an older Splitty read every alias as gone.
pub const ORDER_FILE: &str = "remote-host-order.json";
/// The most targets the order holds.
pub const MAX_ORDER: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostOrder {
    pub version: u32,
    pub order: Vec<String>,
}

/// Missing or malformed order is empty; invalid and repeated targets are
/// dropped, each on its own.
pub fn host_order(config_dir: &Path) -> HostOrder {
    let mut order = HostOrder {
        version: FORMAT,
        order: Vec::new(),
    };
    let Some(file) = fs::read(config_dir.join(ORDER_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<HostOrder>(&bytes).ok())
        .filter(|file| file.version == FORMAT)
    else {
        return order;
    };
    for target in file.order {
        if order.order.len() == MAX_ORDER {
            break;
        }
        if SshTarget::parse(&target).is_ok_and(|parsed| parsed.as_str() == target)
            && !order.order.contains(&target)
        {
            order.order.push(target);
        }
    }
    order
}

fn write_order(config_dir: &Path, order: &HostOrder) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(order)?;
    bytes.push(b'\n');
    write_private(config_dir, ORDER_FILE, &bytes)
}

/// Replace the order with the `{order:[target,…]}` a client sent and return
/// what is stored. A repeated target is kept once.
pub fn set_host_order(
    config_dir: &Path,
    document: serde_json::Value,
) -> Result<HostOrder, PreferenceError> {
    use PreferenceError::Invalid;
    let Some(object) = document.as_object() else {
        return Err(Invalid("expected a host order object".into()));
    };
    if object.keys().any(|key| key != "order") {
        return Err(Invalid("only order is accepted".into()));
    }
    let Some(rows) = object.get("order").and_then(serde_json::Value::as_array) else {
        return Err(Invalid("order must be a list of targets".into()));
    };
    if rows.len() > MAX_ORDER {
        return Err(Invalid(format!("order holds at most {MAX_ORDER} targets")));
    }
    let mut order = HostOrder {
        version: FORMAT,
        order: Vec::with_capacity(rows.len()),
    };
    for row in rows {
        let target = checked_target(Some(row))?;
        if !order.order.contains(&target) {
            order.order.push(target);
        }
    }
    let _writing = lock(config_dir)?;
    write_order(config_dir, &order)?;
    Ok(order)
}

/// Replace `dir/name` atomically with `contents`: written to a fresh file in
/// the same directory, then renamed over it. On Unix the file is 0600 from its
/// creation and the directory 0700, repaired when it is laxer. The client's
/// other files in its configuration directory are written the same way.
pub fn write_private(dir: &Path, name: &str, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;

    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(dir)?.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }

    let (temporary, mut file) = create_temporary(dir, name)?;
    let written = file
        .write_all(contents)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temporary, dir.join(name)));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// A new file next to `dir/name` that no one else has opened, owner-only on
/// Unix. Two clients recording at once each get their own.
fn create_temporary(dir: &Path, name: &str) -> io::Result<(PathBuf, fs::File)> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    for attempt in 0..100u32 {
        let path = dir.join(format!(".{name}.{}.{attempt}.tmp", std::process::id()));
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("no free temporary file name"))
}

/// The concrete `Host` names of the SSH configuration in `ssh_dir` (`config`
/// and the files it includes), in the order of the files, each once. Names
/// with `*` or `?`, negated names and names that are not valid targets are
/// left out. `~` in an `Include` is the directory that holds `ssh_dir`.
pub fn configured(ssh_dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    read_config(&ssh_dir.join("config"), ssh_dir, 0, &mut names);
    names
}

fn read_config(path: &Path, ssh_dir: &Path, depth: usize, names: &mut Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let Some((keyword, arguments)) = split_line(line) else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            for name in arguments {
                let concrete = !name.contains(['*', '?']) && !name.starts_with('!');
                if concrete
                    && SshTarget::parse(&name).is_ok_and(|target| target.as_str() == name)
                    && !names.contains(&name)
                {
                    names.push(name);
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") && depth < MAX_INCLUDE_DEPTH {
            for pattern in arguments {
                for file in include_files(&pattern, ssh_dir) {
                    read_config(&file, ssh_dir, depth + 1, names);
                }
            }
        }
    }
}

/// A configuration line's keyword and arguments: `Keyword arguments` or
/// `Keyword=arguments`; arguments split at whitespace, with double quotes
/// grouping, up to a comment. `None` for blank and comment lines.
fn split_line(line: &str) -> Option<(&str, Vec<String>)> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);

    let mut arguments = Vec::new();
    let mut chars = rest.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        match chars.peek() {
            None | Some('#') => break,
            Some(_) => {}
        }
        let mut argument = String::new();
        let mut quoted = false;
        while let Some(c) = chars.next_if(|c| quoted || !c.is_whitespace()) {
            if c == '"' {
                quoted = !quoted;
            } else {
                argument.push(c);
            }
        }
        arguments.push(argument);
    }
    Some((keyword, arguments))
}

/// The files an `Include` argument names: `~` expanded, relative to `ssh_dir`
/// otherwise, with `*` and `?` matched against the names in each directory,
/// sorted as `glob(3)` sorts them.
fn include_files(pattern: &str, ssh_dir: &Path) -> Vec<PathBuf> {
    let home = ssh_dir.parent();
    let path = if pattern == "~" {
        home.map(Path::to_path_buf)
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        home.map(|home| home.join(rest))
    } else if Path::new(pattern).is_absolute() {
        Some(PathBuf::from(pattern))
    } else {
        Some(ssh_dir.join(pattern))
    };
    let Some(path) = path else {
        return Vec::new();
    };

    let mut found = vec![PathBuf::new()];
    for component in path.components() {
        let part = match component {
            Component::Normal(part) => part,
            other => {
                for base in &mut found {
                    base.push(other.as_os_str());
                }
                continue;
            }
        };
        match part.to_str().filter(|part| part.contains(['*', '?'])) {
            None => {
                for base in &mut found {
                    base.push(part);
                }
            }
            Some(pattern) => {
                found = found
                    .iter()
                    .flat_map(|base| matching_entries(base, pattern))
                    .collect();
            }
        }
    }
    found.retain(|path| path.is_file());
    found
}

/// The entries of `dir` whose names match `pattern`, sorted by name. As in
/// `glob(3)`, a leading `.` is matched only by a pattern that starts with one.
fn matching_entries(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            (pattern.starts_with('.') || !name.starts_with('.')) && matches(pattern, name)
        })
        .collect();
    names.sort();
    names.into_iter().map(|name| dir.join(name)).collect()
}

/// Whether `name` matches `pattern`, where `*` stands for any run of
/// characters and `?` for exactly one.
fn matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    // Where the last `*` was, and the position in the name it was tried at.
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some(&c) if c == '?' || c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                // Let the last `*` take one more character.
                Some((star_p, star_n)) => {
                    p = star_p + 1;
                    n = star_n + 1;
                    star = Some((star_p, star_n + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(raw: &str) -> SshTarget {
        SshTarget::parse(raw).expect("target")
    }

    fn targets(hosts: &[RecentHost]) -> Vec<&str> {
        hosts.iter().map(|host| host.target.as_str()).collect()
    }

    /// A home directory with `.ssh/config` and further files below `.ssh`.
    fn ssh_home(config: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("home");
        let ssh = home.path().join(".ssh");
        fs::create_dir(&ssh).expect(".ssh");
        fs::write(ssh.join("config"), config).expect("config");
        for (name, text) in files {
            let path = ssh.join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, text).expect("file");
        }
        home
    }

    fn configured_in(home: &tempfile::TempDir) -> Vec<String> {
        configured(&home.path().join(".ssh"))
    }

    #[test]
    fn a_record_writes_the_format_owner_only() {
        let dir = tempfile::tempdir().expect("dir");
        let config = dir.path().join("splitty");
        record_recent_at(&config, &target("vm1"), 1_760_000_000).expect("record");

        let text = fs::read_to_string(config.join(RECENT_FILE)).expect("file");
        let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(
            value,
            serde_json::json!({"version": 1, "hosts": [{"target": "vm1", "lastConnected": 1_760_000_000}]})
        );
        let leftovers: Vec<_> = fs::read_dir(&config)
            .expect("dir")
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        assert_eq!(
            leftovers.len(),
            2,
            "only the recent file and persistent lock remain"
        );
        assert!(leftovers.contains(&RECENT_FILE.to_owned()));
        assert!(leftovers.contains(&LOCK_FILE.to_owned()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::metadata(path).expect("mode").permissions().mode() & 0o777;
            assert_eq!(mode(&config.join(RECENT_FILE)), 0o600);
            assert_eq!(mode(&config), 0o700);

            fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).expect("chmod");
            record_recent_at(&config, &target("vm2"), 1).expect("record");
            assert_eq!(mode(&config), 0o700, "a laxer directory is repaired");
        }
    }

    /// `remote-hosts`, Known Hosts: Reconnect to a recent host.
    #[test]
    fn a_reconnected_host_moves_to_the_front_once() {
        let dir = tempfile::tempdir().expect("dir");
        for (n, name) in ["vm1", "vm2", "vm3"].into_iter().enumerate() {
            record_recent_at(dir.path(), &target(name), u64::try_from(n).expect("n"))
                .expect("record");
        }
        assert_eq!(targets(&recent(dir.path())), ["vm3", "vm2", "vm1"]);

        record_recent_at(dir.path(), &target("vm1"), 99).expect("record");
        let hosts = recent(dir.path());
        assert_eq!(targets(&hosts), ["vm1", "vm3", "vm2"]);
        assert_eq!(hosts[0].last_connected, 99);
    }

    #[test]
    fn at_most_twenty_recent_hosts_are_kept() {
        let dir = tempfile::tempdir().expect("dir");
        for n in 0..25u64 {
            record_recent_at(dir.path(), &target(&format!("vm{n}")), n).expect("record");
        }
        let hosts = recent(dir.path());
        assert_eq!(hosts.len(), MAX_RECENT);
        assert_eq!(hosts[0].target, "vm24");
        assert_eq!(hosts[MAX_RECENT - 1].target, "vm5");
    }

    /// `remote-hosts`, Known Hosts: Corrupted file.
    #[test]
    fn a_corrupted_file_counts_as_empty_and_is_replaced() {
        for corrupt in [
            "not JSON",
            r#"{"version":2,"hosts":[{"target":"vm9","lastConnected":1}]}"#,
            r#"{"hosts":[]}"#,
            r#"[{"target":"vm9"}]"#,
        ] {
            let dir = tempfile::tempdir().expect("dir");
            fs::write(dir.path().join(RECENT_FILE), corrupt).expect("write");
            assert_eq!(recent(dir.path()), [], "{corrupt}");

            record_recent_at(dir.path(), &target("vm1"), 7).expect("record");
            assert_eq!(
                recent(dir.path()),
                [RecentHost {
                    target: "vm1".into(),
                    last_connected: 7
                }],
                "{corrupt}"
            );
        }
        let missing = tempfile::tempdir().expect("dir");
        assert_eq!(recent(&missing.path().join("nothing")), []);
    }

    #[test]
    fn invalid_or_repeated_recent_targets_are_left_out() {
        let dir = tempfile::tempdir().expect("dir");
        fs::write(
            dir.path().join(RECENT_FILE),
            r#"{"version":1,"hosts":[
                {"target":"-oProxyCommand=x","lastConnected":3},
                {"target":"vm1","lastConnected":2},
                {"target":" vm1","lastConnected":2},
                {"target":"vm1","lastConnected":1}]}"#,
        )
        .expect("write");
        assert_eq!(targets(&recent(dir.path())), ["vm1"]);
    }

    /// `remote-hosts`, Known Hosts: DevPod workspaces.
    #[test]
    fn devpod_workspaces_are_configured_hosts() {
        let home = ssh_home(
            "# DevPod Start api.devpod\nHost api.devpod\n  ForwardAgent yes\n  \
             ProxyCommand \"devpod\" ssh --stdio --context default --user vscode api.devpod\n  \
             User vscode\n# DevPod End api.devpod\n",
            &[],
        );
        assert_eq!(configured_in(&home), ["api.devpod"]);
    }

    /// `remote-hosts`, Known Hosts: Patterns skipped.
    #[test]
    fn patterns_and_negations_are_skipped() {
        let home = ssh_home(
            "Host *.internal !bastion jump\nHost web? db-1\n  HostName 10.0.0.1\n",
            &[],
        );
        assert_eq!(configured_in(&home), ["jump", "db-1"]);
    }

    /// `remote-hosts`, Known Hosts: Included files.
    #[test]
    fn included_files_are_followed() {
        let home = ssh_home(
            "Host first\nInclude config.d/*\nhost last\n",
            &[
                ("config.d/work", "Host vm2\n"),
                ("config.d/a-team", "Host vm1 vm2\n"),
                ("config.d/.hidden", "Host hidden\n"),
            ],
        );
        assert_eq!(configured_in(&home), ["first", "vm1", "vm2", "last"]);
    }

    #[test]
    fn includes_expand_the_home_directory_absolute_paths_and_question_marks() {
        let home = ssh_home("", &[]);
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        fs::write(elsewhere.path().join("hosts"), "Host absolute\n").expect("write");
        fs::create_dir(home.path().join("conf")).expect("mkdir");
        fs::write(home.path().join("conf/vm1.conf"), "Host tilde1\n").expect("write");
        fs::write(home.path().join("conf/vm22.conf"), "Host tilde22\n").expect("write");
        fs::write(
            home.path().join(".ssh/config"),
            format!(
                "Include ~/conf/vm?.conf\nInclude {}\nInclude missing/* ~/nothing\n",
                elsewhere.path().join("hosts").display()
            ),
        )
        .expect("config");
        assert_eq!(configured_in(&home), ["tilde1", "absolute"]);
    }

    #[test]
    fn keywords_are_case_insensitive_and_take_an_equals_sign() {
        let home = ssh_home(
            "HOST=one\nhost = two\n  Host\t\"three\" # a comment\nMatch host four\n  \
             HostName example.org\nhostname five\nHost under_score\n",
            &[],
        );
        assert_eq!(configured_in(&home), ["one", "two", "three"]);
    }

    #[test]
    fn an_include_that_includes_itself_ends() {
        let home = ssh_home("Host loop\nInclude config\n", &[]);
        assert_eq!(configured_in(&home), ["loop"]);
    }

    #[test]
    fn a_missing_configuration_has_no_hosts() {
        let home = tempfile::tempdir().expect("home");
        assert_eq!(configured(&home.path().join(".ssh")), Vec::<String>::new());
    }

    /// `cli`, Known Hosts Command: Recent and configured hosts.
    #[test]
    fn known_hosts_list_recent_hosts_first_and_each_target_once() {
        let config = tempfile::tempdir().expect("config");
        record_recent_at(config.path(), &target("api.devpod"), 5).expect("record");
        record_recent_at(config.path(), &target("vm1"), 6).expect("record");
        let home = ssh_home("Host api.devpod jump\n", &[]);

        let known = known(config.path(), Some(&home.path().join(".ssh")));
        assert_eq!(
            serde_json::to_value(&known).expect("JSON"),
            serde_json::json!([
                {"target": "vm1", "source": "recent", "lastConnected": 6},
                {"target": "api.devpod", "source": "recent", "lastConnected": 5},
                {"target": "jump", "source": "ssh-config"},
            ])
        );
        assert_eq!(super::known(&config.path().join("none"), None), []);
    }

    #[test]
    fn aliases_tombstones_and_explicit_reconnect_share_persistent_metadata() {
        use serde_json::json;
        let config = tempfile::tempdir().expect("config");
        let home = ssh_home("Host vm1\n", &[]);
        let ssh = home.path().join(".ssh");
        let alias = json!({"target":"vm1", "alias":"  Home Lab  "});
        assert_eq!(
            update_preference(config.path(), alias)
                .expect("alias")
                .alias
                .as_deref(),
            Some("Home Lab")
        );
        assert_eq!(
            known(config.path(), Some(&ssh))[0].alias.as_deref(),
            Some("Home Lab")
        );
        // Alias-only targets stay discoverable after recent-host eviction.
        update_preference(
            config.path(),
            json!({"target":"user@vm2:2222", "alias":"Work"}),
        )
        .expect("alias");
        assert!(known(config.path(), Some(&ssh))
            .iter()
            .any(|row| row.target == "user@vm2:2222"));
        record_recent(config.path(), &target("vm1")).expect("record");
        {
            let _lock = lock(config.path()).expect("lock");
            forget_locked(config.path(), "vm1").expect("forget");
        }
        record_recent_if_visible(config.path(), &target("vm1")).expect("automatic reconnect");
        assert!(
            recent(config.path()).is_empty(),
            "automatic reconnect keeps the target forgotten"
        );
        assert!(!known(config.path(), Some(&ssh))
            .iter()
            .any(|row| row.target == "vm1"));
        assert!(
            preferences(config.path())
                .hosts
                .iter()
                .find(|row| row.target == "vm1")
                .expect("row")
                .hidden
        );
        record_recent(config.path(), &target("vm1")).expect("explicit reconnect");
        let row = known(config.path(), Some(&ssh))
            .into_iter()
            .find(|row| row.target == "vm1")
            .expect("visible again");
        assert!(row.alias.is_none(), "forgotten alias is gone");
    }

    #[test]
    fn preference_validation_and_partial_updates_preserve_saved_state() {
        use serde_json::json;
        let config = tempfile::tempdir().expect("config");
        for value in [
            json!([]),
            json!({"target":"vm1"}),
            json!({"target":" vm1", "alias":"X"}),
            json!({"target":"vm1", "alias":3}),
            json!({"target":"vm1", "hidden":true}),
            json!({"target":"vm1", "alias":"line\nbreak"}),
            json!({"target":"vm1", "alias":"X".repeat(201)}),
            json!({"target":"vm1", "alias":"X", "extra":true}),
        ] {
            assert!(matches!(
                update_preference(config.path(), value),
                Err(PreferenceError::Invalid(_))
            ));
        }
        assert!(!config.path().join(PREFERENCES_FILE).exists());
        update_preference(config.path(), json!({"target":"vm1", "alias":"Home"})).expect("save");
        let row = update_preference(config.path(), json!({"target":"vm1", "hidden":false}))
            .expect("reveal");
        assert_eq!(row.alias.as_deref(), Some("Home"));
        let row =
            update_preference(config.path(), json!({"target":"vm1", "alias":null})).expect("clear");
        assert!(row.alias.is_none());
    }

    #[test]
    fn host_order_survives_and_keeps_repeated_targets_once() {
        use serde_json::json;
        let config = tempfile::tempdir().expect("config");
        assert!(host_order(config.path()).order.is_empty());
        let stored =
            set_host_order(config.path(), json!({"order":["vm2","vm1","vm2"]})).expect("order");
        assert_eq!(stored.order, ["vm2", "vm1"]);
        assert_eq!(host_order(config.path()), stored);
        let text = fs::read_to_string(config.path().join(ORDER_FILE)).expect("file");
        assert!(text.contains("\"version\": 1"), "{text}");
    }

    #[test]
    fn invalid_host_orders_are_rejected_without_a_write() {
        use serde_json::json;
        let config = tempfile::tempdir().expect("config");
        let too_many: Vec<String> = (0..=MAX_ORDER).map(|n| format!("vm{n}")).collect();
        for value in [
            json!(["vm1"]),
            json!({"order":"vm1"}),
            json!({"order":[" vm1"]}),
            json!({"order":["-bad"]}),
            json!({"order":[1]}),
            json!({"order":["vm1"], "extra":true}),
            json!({"order": too_many}),
        ] {
            assert!(matches!(
                set_host_order(config.path(), value),
                Err(PreferenceError::Invalid(_))
            ));
        }
        assert!(!config.path().join(ORDER_FILE).exists());
    }

    #[test]
    fn a_corrupt_host_order_is_empty_and_leaves_the_aliases() {
        use serde_json::json;
        let config = tempfile::tempdir().expect("config");
        update_preference(config.path(), json!({"target":"vm1", "alias":"Home"})).expect("alias");
        fs::write(config.path().join(ORDER_FILE), "{not json").expect("file");
        assert!(host_order(config.path()).order.is_empty());
        fs::write(
            config.path().join(ORDER_FILE),
            json!({"version":1,"order":["vm2","-bad","vm2","vm1"]}).to_string(),
        )
        .expect("file");
        assert_eq!(host_order(config.path()).order, ["vm2", "vm1"]);
        assert_eq!(
            preferences(config.path()).hosts[0].alias.as_deref(),
            Some("Home")
        );
    }

    #[test]
    fn a_forgotten_host_leaves_the_order() {
        use serde_json::json;
        let config = tempfile::tempdir().expect("config");
        set_host_order(config.path(), json!({"order":["vm2","vm1","vm3"]})).expect("order");
        let _writing = lock(config.path()).expect("lock");
        forget_locked(config.path(), "vm1").expect("forget");
        assert_eq!(host_order(config.path()).order, ["vm2", "vm3"]);
    }

    #[test]
    fn invalid_saved_preferences_are_skipped_individually() {
        let config = tempfile::tempdir().expect("config");
        fs::write(
            config.path().join(PREFERENCES_FILE),
            serde_json::json!({"version":1,"hosts":[
                {"target":"vm1","alias":"Home"}, {"target":"-bad","hidden":true},
                {"target":"vm1","alias":"Duplicate"}, {"target":"vm2","hidden":true},
                {"target":"vm3","alias":"bad\nname"}, {"target":"vm4","extra":true}
            ]})
            .to_string(),
        )
        .expect("file");
        let prefs = preferences(config.path());
        assert_eq!(prefs.hosts.len(), 2);
        assert_eq!(prefs.hosts[0].alias.as_deref(), Some("Home"));
        assert!(prefs.hosts[1].hidden);
    }

    #[test]
    fn concurrent_clients_keep_each_others_preferences_and_recents() {
        let config = tempfile::tempdir().expect("config");
        std::thread::scope(|scope| {
            for n in 0..16 {
                let path = config.path();
                scope.spawn(move || {
                    let name = format!("vm{n}");
                    update_preference(
                        path,
                        serde_json::json!({"target":name, "alias":format!("Host {n}")}),
                    )
                    .expect("alias");
                    record_recent(path, &target(&name)).expect("record");
                });
            }
        });
        assert_eq!(preferences(config.path()).hosts.len(), 16);
        assert_eq!(recent(config.path()).len(), 16);
    }

    #[test]
    fn wildcards_match_like_glob() {
        for (pattern, name) in [
            ("*", "work"),
            ("*.conf", "a.conf"),
            ("vm?", "vm1"),
            ("a*b*c", "abxbc"),
            ("*a", "aaa"),
            ("**", ""),
        ] {
            assert!(matches(pattern, name), "{pattern} {name}");
        }
        for (pattern, name) in [
            ("vm?", "vm"),
            ("vm?", "vm12"),
            ("*.conf", "a.confx"),
            ("a*b", "ac"),
        ] {
            assert!(!matches(pattern, name), "{pattern} {name}");
        }
    }
}
