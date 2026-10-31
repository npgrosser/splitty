//! PTY (pseudo-terminal) session management.
//!
//! Provides the `Backend` trait for managing terminal session lifecycles and the
//! `SessionHandle` trait for interacting with individual sessions (output
//! subscriptions, write, resize, close, title info, scrollback). Key types:
//!
//! - `Manager` -- the production `Backend` implementation backed by real PTYs.
//! - `Session` -- a single PTY process with scrollback ring buffer and OSC title scanning.
//! - `OscScanner` -- finite state machine that extracts OSC 0/2 title sequences from output.
//! - `RingBuffer` -- circular byte buffer for caching scrollback data.
//! - `procinfo` -- platform-specific process info lookups (/proc on Linux, ps/lsof on macOS).
//! - `ports` -- the TCP ports the processes of sessions listen on (/proc on Linux).
//! - `replay` -- historical output without report requests and clipboard writes, for terminal clients.
//! - `startup` -- shell startup argument construction for different restore modes.
//! - `terminal` -- what a session's program runs on: a PTY in this process, or one in a session keeper.

pub mod cli_path;
pub(crate) mod lifecycle;
mod log_text;
pub mod manager;
pub mod osc;
pub mod output;
pub mod ports;
pub mod procinfo;
pub mod replay;
pub mod ringbuffer;
pub mod session;
pub mod startup;
pub mod terminal;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

use async_trait::async_trait;
use std::io;
use std::sync::Arc;
use std::time::Instant;

use osc::SemanticAction;

/// Actual process termination, distinct from output EOF and from a pending process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessResult {
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
    pub error: Option<String>,
}

/// Information about the foreground process in a terminal session.
#[derive(Clone, Debug, Default)]
pub struct TitleInfo {
    pub proc: String,
    pub cwd: String,
    pub cmdline: String,
    pub base_shell: bool,
}

/// Event pushed through the OSC title watch channel.
#[derive(Clone, Debug)]
pub struct OscTitleEvent {
    pub title: String,
    pub changed_at: Instant,
}

impl Default for OscTitleEvent {
    fn default() -> Self {
        Self {
            title: String::new(),
            changed_at: Instant::now(),
        }
    }
}

/// Options for creating a new terminal session.
#[derive(Clone, Debug)]
pub struct CreateSessionOpts {
    pub id: String,
    pub shell_profile_id: String,
    pub shell: String,
    /// Profile argv. Each entry is passed directly to `CommandBuilder::arg`.
    pub shell_args: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub dir: String,
    pub command: String,
    pub restore_mode: String,
    pub extra_env: Vec<String>,
}

/// Lightweight metrics for the activity monitor.
#[derive(Clone, Debug)]
pub struct SessionStats {
    pub id: String,
    pub pid: u32,
    pub proc_name: String,
    pub cwd: String,
    pub cmdline: String,
    pub base_shell: bool,
    pub scrollback_used: usize,
    pub scrollback_max: usize,
    pub child_rss_bytes: i64,
}

// ---------------------------------------------------------------------------
// OSC 133 Semantic State
// ---------------------------------------------------------------------------

/// The semantic phase of a terminal session, as reported by OSC 133.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionPhase {
    /// No OSC 133 sequences seen yet (shell integration not active).
    #[default]
    Unknown,
    /// After `133;A` — shell is rendering the prompt.
    Prompt,
    /// After `133;B` — user is typing a command.
    Input,
    /// After `133;C` — command is executing.
    Executing,
    /// After `133;D` — command finished, shell is idle.
    Idle,
}

impl SessionPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionPhase::Unknown => "unknown",
            SessionPhase::Prompt => "prompt",
            SessionPhase::Input => "input",
            SessionPhase::Executing => "executing",
            SessionPhase::Idle => "idle",
        }
    }
}

/// Aggregated semantic state for a terminal session, maintained by the backend.
#[derive(Debug, Clone)]
pub struct SemanticState {
    pub phase: SessionPhase,
    pub phase_since: Instant,
    pub last_exit_code: Option<i32>,
    pub command_count: u32,
}

impl Default for SemanticState {
    fn default() -> Self {
        Self {
            phase: SessionPhase::Unknown,
            phase_since: Instant::now(),
            last_exit_code: None,
            command_count: 0,
        }
    }
}

impl SemanticState {
    /// Apply a semantic action and return the new phase.
    pub fn apply(&mut self, action: &SemanticAction) {
        let now = Instant::now();
        match action {
            SemanticAction::PromptStart => {
                self.phase = SessionPhase::Prompt;
                self.phase_since = now;
            }
            SemanticAction::InputStart => {
                self.phase = SessionPhase::Input;
                self.phase_since = now;
            }
            SemanticAction::OutputStart => {
                self.phase = SessionPhase::Executing;
                self.phase_since = now;
            }
            SemanticAction::CommandFinished { exit_code } => {
                self.phase = SessionPhase::Idle;
                self.phase_since = now;
                self.last_exit_code = *exit_code;
                self.command_count += 1;
            }
        }
    }
}

/// Event pushed through the semantic watch channel.
///
/// The `Option` wrapper exists because `tokio::watch` requires an initial value.
/// The initial `None` is synthetic and will never be seen by watchers that use
/// `rx.changed()` before `rx.borrow()`.
pub type SemanticEvent = Option<SemanticEventInner>;

/// Inner data for a semantic event.
#[derive(Clone, Debug)]
pub struct SemanticEventInner {
    pub action: SemanticAction,
    pub timestamp: Instant,
}

/// Event pushed through the bell watch channel.
///
/// `None` is the initial synthetic value (no bell yet).
/// `Some(Instant)` is the timestamp of the most recent BEL.
pub type BellEvent = Option<Instant>;

/// One consumer's view of a session's output: everything buffered when it
/// subscribed, then every chunk produced after that. Together the two hold
/// each byte from the oldest buffered one onwards exactly once.
pub struct Subscription {
    /// The scrollback buffer at the moment of subscribing.
    pub snapshot: Vec<u8>,
    /// Output produced after the snapshot. Yields `None` once the session's
    /// output has ended, because its terminal closed.
    pub receiver: tokio::sync::mpsc::UnboundedReceiver<Arc<[u8]>>,
}

/// Provides access to a single terminal session.
#[async_trait]
pub trait SessionHandle: Send + Sync {
    fn session_id(&self) -> &str;
    fn shell_profile_id(&self) -> Option<&str> {
        None
    }
    /// The process id of the session's shell while it runs.
    fn shell_pid(&self) -> Option<u32> {
        None
    }
    /// Start receiving the session's output. Subscribing never affects the
    /// session or other subscribers; dropping the receiver ends only this one.
    fn subscribe(&self) -> Subscription;
    async fn write(&self, data: &[u8]) -> io::Result<()>;
    fn resize(&self, cols: u16, rows: u16) -> io::Result<()>;
    fn close(&self) -> io::Result<()>;
    /// Let go of the session without ending its program, where the program
    /// outlives this process (a daemon's session keeper).
    fn release(&self) {}
    fn done(&self) -> tokio::sync::watch::Receiver<bool>;
    fn exit_code(&self) -> i32;
    fn process_result(&self) -> Option<ProcessResult> {
        (*self.done().borrow()).then(|| ProcessResult {
            exit_code: u32::try_from(self.exit_code()).ok(),
            signal: None,
            error: None,
        })
    }
    fn is_managed(&self) -> bool {
        false
    }
    async fn stop(&self) -> io::Result<ProcessResult> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "managed stop is unsupported by this session",
        ))
    }
    fn title(&self) -> TitleInfo;
    fn osc_title_watch(&self) -> tokio::sync::watch::Receiver<OscTitleEvent>;
    fn semantic_state(&self) -> SemanticState;
    fn semantic_watch(&self) -> tokio::sync::watch::Receiver<SemanticEvent>;
    fn bell_watch(&self) -> tokio::sync::watch::Receiver<BellEvent>;
    fn scrollback(&self) -> Vec<u8>;
    fn read_output(
        &self,
        _job: &str,
        _query: &output::OutputQuery,
    ) -> Result<output::OutputPage, output::OutputError> {
        Err(output::OutputError::Unavailable)
    }
    fn output_watch(&self) -> tokio::sync::watch::Receiver<u64> {
        tokio::sync::watch::channel(0).1
    }
    fn output_complete(&self) -> bool {
        *self.done().borrow()
    }
    fn stats(&self) -> Option<SessionStats>;
    /// Claim the single report of this session's exit to the notification
    /// engine. True for the first caller only, however many streams see the
    /// exit.
    fn claim_exit_report(&self) -> bool;
}

/// Manages the lifecycle of terminal sessions.
#[async_trait]
pub trait Backend: Send + Sync {
    async fn create(&self, opts: CreateSessionOpts) -> Result<Arc<dyn SessionHandle>, PtyError>;
    /// Direct execution is optional for alternate backends; never emulate it by typing into a shell.
    async fn create_program(
        &self,
        _opts: CreateSessionOpts,
        _argv: Vec<String>,
    ) -> Result<Arc<dyn SessionHandle>, PtyError> {
        Err(PtyError::Other(
            "this backend does not support direct program launch".into(),
        ))
    }
    fn get(&self, id: &str) -> Option<Arc<dyn SessionHandle>>;
    fn close(&self, id: &str) -> Result<(), PtyError>;
    fn close_all(&self);
    /// Let go of every session without ending the programs that outlive this
    /// process; the others are closed.
    fn release_all(&self) {
        self.close_all();
    }
    fn sessions(&self) -> Vec<Arc<dyn SessionHandle>>;
}

/// Errors that can occur during PTY operations.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("failed to spawn PTY: {0}")]
    SpawnFailed(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("write failed: {0}")]
    WriteFailed(#[from] io::Error),
    #[error("PTY error: {0}")]
    Other(String),
}
