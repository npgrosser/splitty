//! Declarative side-effect types produced by the mux reducer.
//!
//! Effects are pure data describing PTY operations (spawn or kill a session).
//! They are executed by the `EffectRunner` after the reducer returns.

use crate::ids::*;

/// Side effects produced by the reducer. Pure data — describes what should happen, not how.
#[derive(Clone, Debug)]
pub enum Effect {
    /// A direct program launch, never a command typed into a shell.
    SpawnJob { job: Box<super::jobs::JobRecord> },
    /// Request creation of a new PTY session.
    SpawnSession {
        session_id: SessionId,
        pane_id: PaneId,
        screen_id: ScreenId,
        command: Option<String>,
        cwd: Option<String>,
        shell_profile_id: Option<String>,
        shell: Option<String>,
    },

    /// Request termination of a PTY session.
    KillSession { session_id: SessionId },
}
