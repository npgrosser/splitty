//! Token generation, persistence, resolution, and validation.
//!
//! Tokens are 128-bit random values stored as 32-char hex strings in
//! `~/.config/splitty/auth-token` with restrictive file permissions (0600).

use std::fs;
use std::path::{Path, PathBuf};

use rand::Rng;
use subtle::ConstantTimeEq;

use super::config_dir;

const TOKEN_BYTES: usize = 16; // 128-bit

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to generate random token: {0}")]
    Rand(String),
}

/// Returns the path to the auth token file (`~/.config/splitty/auth-token`).
pub fn token_path() -> Result<PathBuf, AuthError> {
    Ok(config_dir()?.join("auth-token"))
}

/// Reads the connection token from disk, or generates and persists a new one.
///
/// Uses the default config directory (`~/.config/splitty`).
pub fn load_or_create() -> Result<String, AuthError> {
    let dir = config_dir()?;
    load_or_create_in(&dir)
}

/// Reads the connection token from the given directory, or generates and
/// persists a new one if the file does not exist.
pub fn load_or_create_in(dir: &Path) -> Result<String, AuthError> {
    let path = dir.join("auth-token");

    // Try to read an existing token.
    //
    // Permissions are repaired on this path too (security review
    // SR-15). The original code only set 0600 when it *created* the file, so a
    // token file carried over from an older version, restored from a backup, or
    // written by hand under a permissive umask kept whatever mode it had — and
    // the branch that would have fixed it was never reached again.
    if let Ok(data) = fs::read_to_string(&path) {
        let token = data.trim().to_string();
        if !token.is_empty() {
            crate::config::ensure_private_dir(dir)?;
            harden_token_file(&path)?;
            return Ok(token);
        }
    }

    let token = generate_hex_token()?;
    crate::config::ensure_private_dir(dir)?;
    crate::config::write_private_file(&path, format!("{token}\n").as_bytes())?;

    Ok(token)
}

/// Force a token file to 0600 if it is not already.
#[cfg(unix)]
fn harden_token_file(path: &Path) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode != 0o600 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn harden_token_file(_path: &Path) -> Result<(), AuthError> {
    Ok(())
}

/// Determines the auth token from flag, env, or file (in that order).
///
/// Returns `(token, source)` where source is a human-readable description.
pub fn resolve(flag_val: &str) -> Result<(String, String), AuthError> {
    if !flag_val.is_empty() {
        return Ok((flag_val.to_string(), "--token flag".to_string()));
    }

    // `SPLITTY_TOKEN` is no longer consulted (security review
    // SR-05). Panes no longer carry it, but the reason to drop it here is
    // separate: a `splitty serve` started from inside a pane would have adopted
    // the parent server's credential, quietly giving two processes the same
    // identity. A server should always mint or load its own.
    let token = load_or_create()?;
    let path = token_path()?;
    Ok((token, path.display().to_string()))
}

/// Creates a random token that is **not** persisted to disk.
pub fn generate_oneshot() -> Result<String, AuthError> {
    generate_hex_token()
}

/// Compares the provided token against the expected token using
/// constant-time comparison.
///
/// Empty values on either side are rejected before the comparison
/// (security review SR-17/SR-22). Without those guards two empty
/// strings compare equal — same length, and `ct_eq` over zero bytes is true — so
/// a server that somehow came up without a token would have authenticated every
/// request that also sent nothing. This is the only comparison in the codebase;
/// a second, subtly different copy in the HTTP middleware was removed rather
/// than fixed twice.
pub fn validate(provided: &str, expected: &str) -> bool {
    if provided.is_empty() || expected.is_empty() {
        return false;
    }

    let provided_bytes = provided.as_bytes();
    let expected_bytes = expected.as_bytes();

    if provided_bytes.len() != expected_bytes.len() {
        return false;
    }

    provided_bytes.ct_eq(expected_bytes).into()
}

/// Generates 16 random bytes and returns a 32-char hex-encoded string.
fn generate_hex_token() -> Result<String, AuthError> {
    let mut buf = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill(&mut buf);
    Ok(hex::encode(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_generate_oneshot_length() {
        let token = generate_oneshot().unwrap();
        assert_eq!(token.len(), 32);
        // Verify it is valid hex.
        hex::decode(&token).expect("should be valid hex");
    }

    #[test]
    fn test_generate_oneshot_uniqueness() {
        let a = generate_oneshot().unwrap();
        let b = generate_oneshot().unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_validate_matching() {
        assert!(validate("abc", "abc"));
    }

    #[test]
    fn test_validate_non_matching() {
        assert!(!validate("abc", "def"));
    }

    #[test]
    fn test_validate_different_lengths() {
        assert!(!validate("short", "longerstring"));
    }

    /// SR-17/SR-22: every empty-value combination must fail closed.
    ///
    /// The dangerous case is `("", "")`: equal lengths, and a constant-time
    /// comparison over zero bytes reports equality, so a server missing its
    /// token would have accepted a request that also carried none.
    #[test]
    fn empty_values_never_validate() {
        assert!(!validate("", ""), "empty/empty must not authenticate");
        assert!(
            !validate("abc", ""),
            "an empty expected token must not authenticate"
        );
        assert!(
            !validate("", "abc"),
            "an empty provided token must not authenticate"
        );
    }

    /// SR-15: a pre-existing token file with loose permissions must be repaired,
    /// not merely read. This is the migration case — an older Splitty wrote the
    /// file before the 0600 handling existed.
    #[test]
    #[cfg(unix)]
    fn reusing_a_token_file_repairs_its_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-token");

        fs::write(&path, "deadbeefdeadbeefdeadbeefdeadbeef\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

        let token = load_or_create_in(dir.path()).unwrap();
        assert_eq!(
            token, "deadbeefdeadbeefdeadbeefdeadbeef",
            "token must be preserved"
        );

        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "token file must be repaired to 0600");
        assert_eq!(dir_mode, 0o700, "config directory must be repaired to 0700");
    }

    #[test]
    fn test_load_or_create_and_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_in(dir.path()).unwrap();
        let second = load_or_create_in(dir.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);

        // Verify file permissions are 0o600 (Unix only).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("auth-token");
            let meta = fs::metadata(&path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
    }
}
