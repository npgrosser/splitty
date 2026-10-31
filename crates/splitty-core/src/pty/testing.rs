//! Helpers for tests that run real PTYs.
//!
//! These tests run in CI. A machine without PTYs skips them at runtime with a
//! message instead of failing; they are never `#[ignore]`d, because ignored
//! tests are tests nobody runs.

use std::time::{Duration, Instant};

use super::CreateSessionOpts;

/// Whether this machine can open a PTY. Prints why when it cannot, so a
/// skipped test is visible in the output.
pub fn pty_available() -> bool {
    match portable_pty::native_pty_system().openpty(portable_pty::PtySize::default()) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("skipping: no PTY available ({e})");
            false
        }
    }
}

/// Options for a session running exactly `/bin/sh -c <script>`.
pub fn sh_opts(id: &str, script: &str) -> CreateSessionOpts {
    CreateSessionOpts {
        id: id.to_string(),
        shell_profile_id: String::new(),
        shell: "/bin/sh".to_string(),
        shell_args: vec!["-c".to_string(), script.to_string()],
        cols: 80,
        rows: 24,
        dir: String::new(),
        command: String::new(),
        restore_mode: String::new(),
        extra_env: vec![],
    }
}

/// Poll `cond` every 20 ms until it holds or `timeout` passes.
pub async fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
