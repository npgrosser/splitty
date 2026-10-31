//! Authentication subsystem for Splitty connections.
//!
//! Manages a 128-bit hex token stored in the platform config directory.
//! Tokens are resolved from a CLI flag or the
//! persisted file (in that priority order). Validation uses constant-time
//! comparison to prevent timing attacks.

mod token;

use std::path::PathBuf;

pub use token::{
    generate_oneshot, load_or_create, load_or_create_in, resolve, token_path, validate, AuthError,
};

/// Returns the default Splitty config directory.
///
/// Uses the directory `SPLITTY_CONFIG_DIR` names, otherwise the platform
/// config directory: `~/.config/splitty` on Linux,
/// `~/Library/Application Support/splitty` on macOS,
/// `AppData\Roaming\splitty` on Windows.
pub fn config_dir() -> Result<PathBuf, AuthError> {
    if let Some(dir) = crate::config::config_dir_override() {
        return Ok(dir);
    }
    let base = dirs::config_dir().ok_or_else(|| {
        AuthError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not determine config directory",
        ))
    })?;
    Ok(base.join("splitty"))
}
