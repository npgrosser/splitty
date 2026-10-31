//! Local control channel: a Unix domain socket for processes running in a pane.
//!
//! # Why this exists
//!
//! Every PTY used to inherit `SPLITTY_SERVER` and `SPLITTY_TOKEN`
//! (security review SR-05), and `daemon.json` held the same token
//! in a world-readable file (SR-21). So every build script, package postinstall
//! and test runner started from a Splitty shell received the server's master
//! credential — one that also authenticated the HTTP API in full, including
//! configuration and shell-profile writes.
//!
//! # What this does and does not achieve
//!
//! It does **not** stop a process that already runs as the user from driving the
//! multiplexer. That is not achievable without OS-level sandboxing, and `tmux`
//! and `screen` have the same property: their control socket is reachable by any
//! process of the same user. Claiming otherwise would be dishonest.
//!
//! What changes is narrower and worth stating precisely:
//!
//! 1. **The credential stops being portable.** A token can be copied out over
//!    any outbound connection and used later from a browser page the user
//!    visits; that indirect path is what made the inherited token more than a
//!    local-privilege equivalence. A socket path is worthless off the machine.
//! 2. **The socket's operation set is small.** This channel exposes multiplexer
//!    operations only; configuration, shell profiles, themes and templates are
//!    not routed here. That limits the socket, not the user: a process of the
//!    same user can still edit those files directly or read the 0600 token file.
//! 3. **Credentials leave the places that leak.** Process environments show up in
//!    `env` output pasted into bug reports, in `/proc/<pid>/environ`, in crash
//!    dumps and in CI logs. A socket path in those places discloses nothing.
//!
//! # Authorisation
//!
//! Two independent gates, because either alone has a failure mode:
//!
//! - The socket lives in a directory with mode 0700, so no other user can even
//!   traverse to it. This is the primary control.
//! - Every accepted connection's peer credentials are checked against the
//!   server's own uid. This catches the case where the directory mode is wrong —
//!   a bad umask, a bad migration, a hand-created directory.
//!
//! # WSL constraint
//!
//! The socket must live on a Linux filesystem. Unix sockets do not work on
//! DrvFs mounts (`/mnt/c`, `/mnt/wsl/...`), which is exactly where a WSL user's
//! `$HOME` may point if they have remapped it. Binding is refused there rather
//! than failing later with an obscure error.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::{UnixListener, UnixStream};

/// Where the control socket for this process lives.
///
/// Keyed by pid so several servers can run side by side without collision, and
/// so a stale socket from a crashed process is distinguishable from a live one.
pub fn socket_path() -> PathBuf {
    runtime_dir().join(format!("{}.sock", std::process::id()))
}

/// Where a daemon's control socket lives: one name per configuration
/// directory, so the `SPLITTY_SOCK` of a shell the daemon adopts after a
/// restart still reaches it, in the run directory a logout does not remove.
pub fn daemon_socket_path() -> PathBuf {
    splitty_core::config::run_dir().join("daemon.sock")
}

fn runtime_dir() -> PathBuf {
    runtime_dir_for(splitty_core::config::config_dir_override())
}

fn runtime_dir_for(config_override: Option<PathBuf>) -> PathBuf {
    // A server given a configuration directory of its own keeps its sockets
    // there too, apart from other servers of the user (a hub's server on a
    // macOS host next to the desktop app).
    if let Some(dir) = config_override {
        return dir.join("run");
    }

    // `$XDG_RUNTIME_DIR` is the right home for sockets: it is already per-user,
    // mode 0700, and cleared on logout.
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return dir.join("splitty");
        }
    }

    // Fallback for environments without one — some WSL distributions do not set
    // it for non-login shells.
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("splitty")
        .join("run")
}

/// Reject paths on filesystems where Unix sockets do not work.
///
/// Under WSL these are the Windows drive mounts. Binding there appears to
/// succeed and then fails on connect, which is a miserable thing to debug.
fn is_supported_filesystem(dir: &Path) -> bool {
    splitty_core::config::socket_dir_supported(dir)
}

/// Bind the control socket at `path`, creating and hardening its directory.
fn bind_at(path: PathBuf) -> io::Result<(UnixListener, PathBuf)> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent"))?;

    if !is_supported_filesystem(dir) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "refusing to place the control socket in {} — Unix sockets do not \
                 work on Windows drive mounts under WSL",
                dir.display()
            ),
        ));
    }

    splitty_core::config::ensure_private_dir(dir)?;

    // A socket left behind by a crashed process with the same pid would make
    // bind() fail with EADDRINUSE.
    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path)?;

    // Belt and braces alongside the 0700 directory: if the directory mode is
    // ever wrong, the socket itself still refuses other users.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok((listener, path))
}

/// Is this connection's peer the same user as this process?
#[cfg(target_os = "linux")]
fn peer_is_owner(stream: &UnixStream) -> bool {
    use std::os::fd::AsFd;

    match nix::sys::socket::getsockopt(&stream.as_fd(), nix::sys::socket::sockopt::PeerCredentials)
    {
        Ok(creds) => creds.uid() == nix::unistd::geteuid().as_raw(),
        // Fail closed: an unreadable peer credential is not a reason to trust.
        Err(_) => false,
    }
}

/// Is this connection's peer the same user as this process? macOS has no
/// SO_PEERCRED; `getpeereid` names the peer's effective user.
#[cfg(target_os = "macos")]
fn peer_is_owner(stream: &UnixStream) -> bool {
    match nix::unistd::getpeereid(stream) {
        Ok((uid, _)) => uid == nix::unistd::geteuid(),
        // Fail closed: an unreadable peer credential is not a reason to trust.
        Err(_) => false,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn peer_is_owner(_stream: &UnixStream) -> bool {
    // Only the directory and socket modes gate access on platforms without a
    // peer credential call.
    true
}

/// A `UnixListener` that drops connections from other users before they reach
/// the HTTP stack.
pub struct PeerCheckedListener {
    inner: UnixListener,
}

impl PeerCheckedListener {
    pub fn new(inner: UnixListener) -> Self {
        Self { inner }
    }
}

impl axum::serve::Listener for PeerCheckedListener {
    type Io = UnixStream;
    type Addr = tokio::net::unix::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => {
                    if peer_is_owner(&stream) {
                        return (stream, addr);
                    }
                    tracing::warn!("control socket: rejected connection from another user");
                    // Dropping the stream closes it; keep serving.
                }
                Err(error) => {
                    tracing::warn!("control socket accept failed: {error}");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// Handle for a running control channel, so it can be torn down with the server.
pub struct ControlChannel {
    path: Option<PathBuf>,
    task: Option<tokio::task::AbortHandle>,
}

impl ControlChannel {
    /// The bound socket, or `None` when binding failed and the server runs
    /// without one.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Remove the socket file. A stale socket would make a later server with the
    /// same pid fail to bind, and leaves a misleading entry in the runtime dir.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for ControlChannel {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the local control channel for processes running inside a pane.
///
/// A failure to bind is logged, not fatal: the terminal itself must keep working
/// even where a socket cannot be placed (an exotic `$HOME`, a read-only runtime
/// directory). The consequence is only that the in-shell `splitty` CLI cannot
/// find this server — never that the server refuses to start.
///
/// Lives here rather than in the binary so the Tauri app's embedded server uses
/// the identical implementation; two copies of an authorisation boundary is one
/// too many.
pub fn spawn(state: crate::http::AppState) -> ControlChannel {
    spawn_at(state, socket_path())
}

/// Serve the control socket at `path` (a daemon's [`daemon_socket_path`]).
pub fn spawn_at(state: crate::http::AppState, path: PathBuf) -> ControlChannel {
    match bind_at(path) {
        Ok((listener, path)) => {
            tracing::debug!("control socket listening on {}", path.display());
            let router = crate::http::build_control_router(state);
            let task = tokio::spawn(async move {
                let listener = PeerCheckedListener::new(listener);
                if let Err(error) = axum::serve(listener, router).await {
                    tracing::warn!("control socket server stopped: {error}");
                }
            });
            ControlChannel {
                path: Some(path),
                task: Some(task.abort_handle()),
            }
        }
        Err(error) => {
            tracing::warn!(
                "control socket unavailable ({error}); the in-shell `splitty` CLI \
                 will not find this server"
            );
            ControlChannel {
                path: None,
                task: None,
            }
        }
    }
}

/// Provision only a local listener; its route set has no owner/admin endpoints.
pub fn spawn_grant(
    jobs: &Arc<crate::jobs::Jobs>,
    name: String,
    anchor: Option<crate::jobs::Anchor>,
) -> Result<serde_json::Value, crate::jobs::JobError> {
    use crate::jobs::{Actor, Grant, JobError};
    let dir = if jobs.owner_socket().is_empty() {
        runtime_dir()
    } else {
        Path::new(jobs.owner_socket())
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    };
    if !is_supported_filesystem(&dir) {
        return Err(JobError::new(
            503,
            "socket_unavailable",
            "unsupported socket location",
        ));
    }
    splitty_core::config::ensure_private_dir(&dir)
        .map_err(|e| JobError::new(503, "socket_unavailable", e.to_string()))?;
    let id = splitty_core::ids::AgentId::new();
    let path = dir.join(format!("agent-{id}.sock"));
    let listener = UnixListener::bind(&path)
        .map_err(|e| JobError::new(503, "socket_unavailable", e.to_string()))?;
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
        let _ = std::fs::remove_file(&path);
        return Err(JobError::new(503, "socket_unavailable", error.to_string()));
    }
    let grant = Grant::new(
        name,
        path.to_string_lossy().into_owned(),
        anchor,
        id.clone(),
    );
    let router: axum::Router = crate::http::jobs::routes(Arc::downgrade(jobs), Actor::Agent(id));
    let task = tokio::spawn(async move {
        let _ = axum::serve(PeerCheckedListener::new(listener), router).await;
    });
    grant.set_listener(task.abort_handle());
    jobs.register_grant(grant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_socket_has_a_fixed_name_in_the_run_directory() {
        let daemon = daemon_socket_path();
        assert!(daemon.ends_with("run/daemon.sock"), "{}", daemon.display());
        assert_eq!(
            daemon.parent(),
            Some(splitty_core::config::run_dir().as_path())
        );
        let embedded = socket_path();
        assert_eq!(
            embedded.file_name().and_then(|name| name.to_str()),
            Some(format!("{}.sock", std::process::id()).as_str()),
            "any other server's by pid"
        );
    }

    #[test]
    fn windows_drive_mounts_are_refused() {
        // Unix sockets silently misbehave on DrvFs, so a $HOME remapped under
        // /mnt must be caught at bind time rather than at first connect.
        assert!(!is_supported_filesystem(Path::new(
            "/mnt/c/Users/me/.local/share/splitty"
        )));
        assert!(!is_supported_filesystem(Path::new("/mnt/wsl/splitty")));
        assert!(!is_supported_filesystem(Path::new("/media/usb/splitty")));
    }

    #[test]
    fn linux_paths_are_accepted() {
        assert!(is_supported_filesystem(Path::new("/run/user/1000/splitty")));
        assert!(is_supported_filesystem(Path::new(
            "/home/me/.local/share/splitty/run"
        )));
        assert!(is_supported_filesystem(Path::new("/tmp/splitty")));
    }

    #[test]
    fn a_configuration_directory_of_its_own_holds_the_sockets() {
        assert_eq!(
            runtime_dir_for(Some(PathBuf::from("/Users/me/.config/splitty"))),
            PathBuf::from("/Users/me/.config/splitty/run")
        );
        assert_ne!(
            runtime_dir_for(None),
            PathBuf::from("/Users/me/.config/splitty/run")
        );
    }

    #[test]
    fn socket_path_is_per_process() {
        let path = socket_path();
        assert!(
            path.to_string_lossy()
                .contains(&std::process::id().to_string()),
            "socket path must be keyed by pid so concurrent servers do not collide",
        );
    }
}
