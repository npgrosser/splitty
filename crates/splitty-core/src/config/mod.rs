//! Configuration loading, saving, and validation.
//!
//! User settings live in `~/.config/splitty/config.toml` (snake_case keys)
//! and are served to the frontend as camelCase JSON. This module handles the
//! bidirectional key transformation, deep-merge partial updates, and value
//! validation with safe fallbacks to defaults.

mod settings;

pub use settings::*;

use std::ffi::OsString;
use std::path::PathBuf;

/// The environment variable that names the configuration directory in place
/// of the platform's, when it holds an absolute path. A client sets it for the
/// server it installed on a macOS host, so that this server keeps apart from a
/// desktop app of the same user (`remote-hosts`, Host Server Apart From The
/// Desktop App).
pub const CONFIG_DIR_VARIABLE: &str = "SPLITTY_CONFIG_DIR";

/// The directory [`CONFIG_DIR_VARIABLE`] names, if it is absolute.
pub fn config_dir_override() -> Option<PathBuf> {
    absolute_dir(std::env::var_os(CONFIG_DIR_VARIABLE))
}

/// A relative or empty value names no directory.
fn absolute_dir(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|dir| dir.is_absolute())
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = config_dir_override() {
        return dir;
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("splitty")
}

/// Create a directory if needed and make sure it is private to the owner.
///
/// Every writer of `~/.config/splitty` must go through this
/// (security review SR-15/SR-21). Six different call sites used
/// to create the directory with a bare `create_dir_all`, which applies the
/// process umask — typically mode 0755. Which mode the directory ended up with
/// therefore depended on which subsystem happened to run first, and the auth
/// token module's careful 0700 was easily pre-empted by, say, the template
/// store. The directory holds credentials, so its mode must not be a race.
///
/// Repairs an existing directory rather than only setting the mode on creation:
/// a config directory migrated from an older version, or created by hand, is
/// exactly the case that needs fixing.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir)?.permissions().mode() & 0o777;
        if mode != 0o700 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }

    Ok(())
}

/// Write a file that only its owner may read.
///
/// Used for anything holding a credential. `fs::write` alone leaves the mode to
/// the umask — typically 0644 — which is how `daemon.json` came to hold the
/// master token in a world-readable file (SR-21).
#[cfg(unix)]
pub fn write_private_file(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }

    // Write to a temporary file in the same directory and rename, so a reader
    // never observes a partially written file. `mode(0o600)` applies at
    // creation, so there is no window in which the file exists world-readable.
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);

    // An existing target may predate this function and carry a laxer mode;
    // the rename keeps the source's 0600, but be explicit for clarity.
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn write_private_file(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    std::fs::write(path, contents)
}

/// Where a daemon keeps its control socket and the sockets of its session
/// keepers: below the configuration directory a server was given, otherwise in
/// the user's data directory. Not `$XDG_RUNTIME_DIR`, which systemd-logind
/// removes at the last logout while a daemon and its keepers keep running.
pub fn run_dir() -> PathBuf {
    run_dir_for(config_dir_override())
}

fn run_dir_for(config_override: Option<PathBuf>) -> PathBuf {
    match config_override {
        Some(dir) => dir.join("run"),
        None => dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("splitty")
            .join("run"),
    }
}

/// Whether Unix sockets work in `dir`. Under WSL the Windows drive mounts
/// (`/mnt/c`, `/mnt/wsl`) and removable media are not such places: binding
/// there appears to succeed and then fails on connect.
pub fn socket_dir_supported(dir: &std::path::Path) -> bool {
    let text = dir.to_string_lossy();
    !(text.starts_with("/mnt/") || text.starts_with("/mnt\\") || text.starts_with("/media/"))
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn server_config_path() -> PathBuf {
    config_dir().join("server.toml")
}

pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_directory_follows_a_named_configuration_directory() {
        assert_eq!(
            run_dir_for(Some(PathBuf::from("/srv/splitty"))),
            PathBuf::from("/srv/splitty/run")
        );
        let default = run_dir_for(None);
        assert!(default.ends_with("splitty/run"), "{}", default.display());
        assert!(
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .is_none_or(|runtime| !default.starts_with(runtime)),
            "never below $XDG_RUNTIME_DIR"
        );
    }

    #[test]
    fn only_an_absolute_directory_overrides_the_platforms() {
        assert_eq!(
            absolute_dir(Some("/Users/me/.config/splitty".into())),
            Some(PathBuf::from("/Users/me/.config/splitty"))
        );
        assert_eq!(absolute_dir(Some("relative/dir".into())), None);
        assert_eq!(absolute_dir(Some("".into())), None);
        assert_eq!(absolute_dir(None), None);
    }

    /// SR-15/SR-21: the config directory must end up 0700 regardless of umask
    /// and regardless of which subsystem created it first.
    #[test]
    #[cfg(unix)]
    fn ensure_private_dir_creates_and_repairs_0700() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("splitty");

        ensure_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "a freshly created directory must be 0700");

        // Simulate a directory left behind by an older version.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "an existing directory must be repaired to 0700"
        );
    }

    /// A credential file must never be readable by anyone but its owner.
    #[test]
    #[cfg(unix)]
    fn write_private_file_is_0600_in_a_0700_directory() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("splitty").join("daemon.json");

        write_private_file(&path, b"{\"secret\":true}").unwrap();

        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"secret\":true}");
    }

    /// Overwriting must not widen an existing file's permissions, and must not
    /// leave a temporary file behind.
    #[test]
    #[cfg(unix)]
    fn write_private_file_overwrites_without_widening() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.json");

        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private_file(&path, b"new").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "an existing loose file must be replaced by a 0600 one"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(
            !path.with_extension("tmp").exists(),
            "no temporary file may survive"
        );
    }
}
