//! Notification engine: tracks terminal session state changes and generates
//! server events (process finished, session exited, process idle).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::debug;

use super::ServerEvent;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BUFFER_MAX_PER_SESSION: usize = 50;
const BUFFER_TTL: Duration = Duration::from_secs(3600); // 1 hour
/// Minimum time between process.idle events for the same session.
/// Suppresses rapid-fire during active conversation (e.g. quick back-and-forth
/// with Claude Code where each turn triggers idle→busy→idle).
const IDLE_NOTIF_COOLDOWN: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// EventBuffer
// ---------------------------------------------------------------------------

struct BufferedEvent {
    event: ServerEvent,
    expires_at: Instant,
}

/// Holds recent events per session for replay on reconnect.
pub struct EventBuffer {
    entries: Mutex<HashMap<String, Vec<BufferedEvent>>>,
}

impl EventBuffer {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Store an event, keyed by session ID.
    pub fn add(&self, session_id: &str, event: ServerEvent) {
        if session_id.is_empty() {
            return;
        }

        let mut entries = self.entries.lock().unwrap();
        let now = Instant::now();

        let list = entries.entry(session_id.to_string()).or_default();

        // Evict expired entries
        list.retain(|e| e.expires_at > now);

        // Append new entry
        list.push(BufferedEvent {
            event,
            expires_at: now + BUFFER_TTL,
        });

        // Cap at max
        if list.len() > BUFFER_MAX_PER_SESSION {
            let excess = list.len() - BUFFER_MAX_PER_SESSION;
            list.drain(..excess);
        }
    }

    /// Return all non-expired events for a set of session IDs.
    pub fn flush_sessions(&self, session_ids: &[String]) -> Vec<ServerEvent> {
        let entries = self.entries.lock().unwrap();
        let now = Instant::now();
        let mut result = Vec::new();
        for sid in session_ids {
            if let Some(list) = entries.get(sid) {
                for e in list {
                    if e.expires_at > now {
                        result.push(e.event.clone());
                    }
                }
            }
        }
        result
    }
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SessionTracker (private)
// ---------------------------------------------------------------------------

struct SessionTracker {
    /// Original command from session creation.
    command: String,

    // baseShell tracking
    last_base_shell: bool,
    last_proc: String,
    last_cmdline: String,

    // OSC 133 idle tracking
    idle_notified: bool,
    /// When the last process.idle event was emitted (cooldown).
    last_idle_notif_at: Option<Instant>,

    /// Set when the client sends a "close" message.
    user_initiated: bool,
}

// ---------------------------------------------------------------------------
// NotificationEngine
// ---------------------------------------------------------------------------

/// Callback invoked when a server event is generated.
pub type NotifyCallback = Box<dyn Fn(ServerEvent) + Send + Sync>;

/// Tracks session state and generates server events.
pub struct NotificationEngine {
    inner: Mutex<EngineInner>,
    callback: NotifyCallback,
    pub buffer: EventBuffer,
}

struct EngineInner {
    sessions: HashMap<String, SessionTracker>,
}

impl NotificationEngine {
    /// Create a new notification engine with the given callback.
    pub fn new(callback: NotifyCallback) -> Self {
        Self {
            inner: Mutex::new(EngineInner {
                sessions: HashMap::new(),
            }),
            callback,
            buffer: EventBuffer::new(),
        }
    }

    /// Return all buffered events for the given session IDs (for replay on attach).
    pub fn replay(&self, session_ids: &[String]) -> Vec<ServerEvent> {
        self.buffer.flush_sessions(session_ids)
    }

    /// Begin tracking a session for notifications.
    /// `command` is the original command used to create the session (may be empty).
    pub fn track_session(&self, session_id: &str, command: &str) {
        debug!(session_id, command, "notify: tracking session");
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.insert(
            session_id.to_string(),
            SessionTracker {
                command: command.to_string(),
                last_base_shell: true, // starts as base shell
                last_proc: String::new(),
                last_cmdline: String::new(),
                idle_notified: false,
                last_idle_notif_at: None,
                user_initiated: false,
            },
        );
    }

    /// Stop tracking a session.
    pub fn untrack_session(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.remove(session_id);
    }

    /// Mark a session close as user-initiated.
    pub fn mark_user_initiated(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(t) = inner.sessions.get_mut(session_id) {
            t.user_initiated = true;
        }
    }

    /// Called by the proc-info poller on each poll.
    /// Handles baseShell transition detection and process-change tracking.
    pub fn on_proc_update(
        &self,
        session_id: &str,
        proc_name: &str,
        cmdline: &str,
        base_shell: bool,
    ) {
        let mut pending: Vec<ServerEvent> = Vec::new();

        {
            let mut inner = self.inner.lock().unwrap();
            let t = match inner.sessions.get_mut(session_id) {
                Some(t) => t,
                None => return,
            };

            let prev_base_shell = t.last_base_shell;
            let prev_proc = t.last_proc.clone();

            t.last_base_shell = base_shell;
            t.last_proc = proc_name.to_string();
            t.last_cmdline = cmdline.to_string();

            // Reset idle state when the foreground process changes
            if proc_name != prev_proc {
                t.idle_notified = false;
            }

            // baseShell transition: false→true = process.finished
            if !prev_base_shell && base_shell && !prev_proc.is_empty() {
                debug!(
                    session_id,
                    proc = prev_proc,
                    "notify: baseShell transition false→true (process.finished)"
                );
                let cmdline_opt = if cmdline.is_empty() {
                    None
                } else {
                    Some(cmdline.to_string())
                };
                pending.push(ServerEvent::process_finished(
                    session_id,
                    prev_proc,
                    cmdline_opt,
                ));
            }
        }

        for event in pending {
            self.emit(session_id, event);
        }
    }

    /// Called when an OSC 133 semantic event is received.
    /// Emits `process.idle` on `CommandFinished` transitions (respecting cooldown).
    pub fn on_semantic_event(&self, session_id: &str, action: &crate::pty::osc::SemanticAction) {
        use crate::pty::osc::SemanticAction;

        match action {
            SemanticAction::CommandFinished { exit_code } => {
                let mut pending: Vec<ServerEvent> = Vec::new();

                {
                    let mut inner = self.inner.lock().unwrap();
                    let t = match inner.sessions.get_mut(session_id) {
                        Some(t) => t,
                        None => return,
                    };

                    let in_cooldown = t
                        .last_idle_notif_at
                        .is_some_and(|at| at.elapsed() < IDLE_NOTIF_COOLDOWN);

                    if !in_cooldown {
                        t.last_idle_notif_at = Some(Instant::now());

                        let proc_name = t.last_proc.clone();
                        pending.push(ServerEvent::process_idle(session_id, proc_name, *exit_code));
                    }
                }

                for event in pending {
                    self.emit(session_id, event);
                }
            }
            SemanticAction::OutputStart => {
                // Reset idle state when a new command starts executing
                let mut inner = self.inner.lock().unwrap();
                if let Some(t) = inner.sessions.get_mut(session_id) {
                    t.idle_notified = false;
                }
            }
            _ => {}
        }
    }

    /// Called when a PTY session exits.
    pub fn on_session_exit(&self, session_id: &str, exit_code: i32) {
        debug!(session_id, exit_code, "notify: session exit");
        let (user_initiated, proc_name, command) = {
            let mut inner = self.inner.lock().unwrap();
            let t = match inner.sessions.remove(session_id) {
                Some(t) => t,
                None => return,
            };
            (t.user_initiated, t.last_proc, t.command)
        };

        let process = if proc_name.is_empty() {
            None
        } else {
            Some(proc_name)
        };
        let cmd = if command.is_empty() {
            None
        } else {
            Some(command)
        };

        let event =
            ServerEvent::session_exited(session_id, exit_code, process, cmd, user_initiated);
        self.emit(session_id, event);
    }

    fn emit(&self, session_id: &str, event: ServerEvent) {
        debug!(session_id, "notify: emitting event");
        self.buffer.add(session_id, event.clone());
        (self.callback)(event);
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    fn collecting_engine() -> (NotificationEngine, Arc<StdMutex<Vec<ServerEvent>>>) {
        let collected = Arc::new(StdMutex::new(Vec::new()));
        let c = collected.clone();
        let engine = NotificationEngine::new(Box::new(move |event| {
            c.lock().unwrap().push(event);
        }));
        (engine, collected)
    }

    // -----------------------------------------------------------------------
    // process.finished detection
    // -----------------------------------------------------------------------

    #[test]
    fn process_finished_on_base_shell_transition() {
        let (engine, collected) = collecting_engine();
        engine.track_session("s1", "");

        engine.on_proc_update("s1", "cargo", "cargo build", false);
        engine.on_proc_update("s1", "zsh", "", true);

        let events = collected.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            ServerEvent::ProcessFinished {
                session_id,
                process,
                ..
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(process, "cargo");
            }
            other => panic!("expected ProcessFinished, got {:?}", other),
        }
    }

    #[test]
    fn no_process_finished_when_stays_base_shell() {
        let (engine, collected) = collecting_engine();
        engine.track_session("s1", "");

        engine.on_proc_update("s1", "zsh", "", true);
        engine.on_proc_update("s1", "zsh", "", true);

        let events = collected.lock().unwrap();
        assert!(events.is_empty());
    }

    // -----------------------------------------------------------------------
    // session exit
    // -----------------------------------------------------------------------

    #[test]
    fn session_exited_on_clean_exit() {
        let (engine, collected) = collecting_engine();
        engine.track_session("s1", "vim");

        engine.on_session_exit("s1", 0);

        let events = collected.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            ServerEvent::SessionExited {
                session_id,
                exit_code,
                ..
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(*exit_code, 0);
            }
            other => panic!("expected SessionExited, got {:?}", other),
        }
    }

    #[test]
    fn session_crashed_on_nonzero_exit() {
        let (engine, collected) = collecting_engine();
        engine.track_session("s1", "node server.js");

        engine.on_session_exit("s1", 1);

        let events = collected.lock().unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            ServerEvent::SessionExited {
                session_id,
                exit_code,
                command,
                ..
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(*exit_code, 1);
                assert_eq!(command.as_deref(), Some("node server.js"));
            }
            other => panic!("expected SessionExited, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // buffer limits
    // -----------------------------------------------------------------------

    #[test]
    fn buffer_caps_at_max_per_session() {
        let buf = EventBuffer::new();
        for i in 0..60 {
            buf.add("s1", ServerEvent::bell(format!("s1-{}", i)));
        }
        let result = buf.flush_sessions(&["s1".to_string()]);
        assert_eq!(result.len(), BUFFER_MAX_PER_SESSION);
    }

    #[test]
    fn buffer_ignores_empty_session_id() {
        let buf = EventBuffer::new();
        buf.add("", ServerEvent::bell(""));
        let result = buf.flush_sessions(&[String::new()]);
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // semantic idle detection (OSC 133)
    // -----------------------------------------------------------------------

    #[test]
    fn semantic_command_finished_emits_idle() {
        use crate::pty::osc::SemanticAction;

        let (engine, collected) = collecting_engine();
        engine.track_session("s1", "");
        engine.on_proc_update("s1", "claude", "claude", false);

        engine.on_semantic_event(
            "s1",
            &SemanticAction::CommandFinished { exit_code: Some(0) },
        );

        let events = collected.lock().unwrap();
        let idle_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ServerEvent::ProcessIdle { .. }))
            .collect();
        assert_eq!(idle_events.len(), 1);
        match idle_events[0] {
            ServerEvent::ProcessIdle { process, .. } => {
                assert_eq!(process, "claude");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn semantic_idle_respects_cooldown() {
        use crate::pty::osc::SemanticAction;

        let (engine, collected) = collecting_engine();
        engine.track_session("s1", "");
        engine.on_proc_update("s1", "claude", "claude", false);

        // First should fire
        engine.on_semantic_event(
            "s1",
            &SemanticAction::CommandFinished { exit_code: Some(0) },
        );
        // Second within cooldown should not
        engine.on_semantic_event(
            "s1",
            &SemanticAction::CommandFinished { exit_code: Some(0) },
        );

        let events = collected.lock().unwrap();
        let idle_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ServerEvent::ProcessIdle { .. }))
            .collect();
        assert_eq!(idle_events.len(), 1);
    }

    // -----------------------------------------------------------------------
    // replay returns buffered events
    // -----------------------------------------------------------------------

    #[test]
    fn replay_returns_buffered_events() {
        let (engine, _collected) = collecting_engine();
        engine.track_session("s1", "vim");
        engine.on_session_exit("s1", 0);

        let replayed = engine.replay(&["s1".to_string()]);
        assert_eq!(replayed.len(), 1);
        assert!(matches!(
            &replayed[0],
            ServerEvent::SessionExited { exit_code: 0, .. }
        ));
    }
}
