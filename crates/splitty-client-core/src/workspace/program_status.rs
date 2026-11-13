//! Program status (OSC 7501 root records) as each connection reports it.
//! Records are scoped to the connection that sent them and resolve to panes
//! and Screens of that source only. What the user saw of a `done` or `error`
//! record is the window's own knowledge (`ProgramStatusSeen`).
use super::{SourceId, SourceToken, SourceWorkspace, WindowWorkspace};
use serde_json::Value;
use splitty_transport::budget::{Charged, ConnectionBudget};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::Arc,
};

/// Listed in `mux.welcome.capabilities` by a server that reports program status.
pub const PROGRAM_STATUS_CAPABILITY: &str = "program-status-v1";
/// Sessions per source whose root record a window keeps.
pub const MAX_STATUS_SESSIONS: usize = 4096;
const MAX_ID_BYTES: usize = 256;
const MAX_APP_BYTES: usize = 32;
const MAX_TITLE_BYTES: usize = 192;
const MAX_MESSAGE_BYTES: usize = 2048;
/// A record's text as a tab or a notification shows it.
pub const STATUS_TEXT_CHARS: usize = 120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}
impl ProgramState {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "done" => Self::Done,
            "blocked" => Self::Blocked,
            "error" => Self::Error,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }
}

/// What a blocked program waits for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}
impl BlockedKind {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "permission" => Self::Permission,
            "question" => Self::Question,
            "auth" => Self::Auth,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// One session's root record, decoded and sanitized by its server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramStatus {
    state: ProgramState,
    kind: Option<BlockedKind>,
    progress: Option<u8>,
    app: Option<String>,
    title: Option<String>,
    message: Option<String>,
    updated_at: u64,
}
impl ProgramStatus {
    /// A record as `program_status.changed` carries it. A record outside the
    /// protocol's limits is refused whole; an unknown kind or a progress out
    /// of range counts as absent, as the server reads them. Texts keep no
    /// hidden characters, whatever the server sent.
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        let state = ProgramState::parse(value.get("state")?.as_str()?)?;
        let kind = (state == ProgramState::Blocked)
            .then(|| value.get("kind").and_then(Value::as_str))
            .flatten()
            .and_then(BlockedKind::parse);
        let progress = matches!(state, ProgramState::Working | ProgramState::Blocked)
            .then(|| value.get("progress").and_then(Value::as_u64))
            .flatten()
            .filter(|progress| *progress <= 100)
            .and_then(|progress| u8::try_from(progress).ok());
        let text = |field: &str, limit: usize| -> Option<Option<String>> {
            match value.get(field) {
                None | Some(Value::Null) => Some(None),
                Some(Value::String(text)) if text.len() <= limit => {
                    let text: String = text.chars().filter(|c| !hidden(*c)).collect();
                    Some((!text.is_empty()).then_some(text))
                }
                Some(_) => None,
            }
        };
        Some(Self {
            state,
            kind,
            progress,
            app: text("app", MAX_APP_BYTES)?,
            title: text("title", MAX_TITLE_BYTES)?,
            message: text("msg", MAX_MESSAGE_BYTES)?,
            updated_at: value.get("updatedAt")?.as_u64()?,
        })
    }
    pub fn state(&self) -> ProgramState {
        self.state
    }
    pub fn kind(&self) -> Option<BlockedKind> {
        self.kind
    }
    pub fn progress(&self) -> Option<u8> {
        self.progress
    }
    pub fn app(&self) -> Option<&str> {
        self.app.as_deref()
    }
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
    /// Unix milliseconds of the report, by the server's clock.
    pub fn updated_at(&self) -> u64 {
        self.updated_at
    }
    /// The state in the window's words.
    pub fn label(&self) -> String {
        match (self.state, self.kind) {
            (ProgramState::Idle, _) => "Idle".into(),
            (ProgramState::Working, _) => match self.progress {
                Some(progress) => format!("Working, {progress}%"),
                None => "Working".into(),
            },
            (ProgramState::Blocked, Some(BlockedKind::Permission)) => {
                "Waiting for permission".into()
            }
            (ProgramState::Blocked, Some(BlockedKind::Question)) => "Waiting for an answer".into(),
            (ProgramState::Blocked, Some(BlockedKind::Auth)) => "Waiting for sign-in".into(),
            (ProgramState::Blocked, None) => "Waiting".into(),
            (ProgramState::Done, _) => "Done".into(),
            (ProgramState::Error, _) => "Failed".into(),
        }
    }
    /// The label and the record's message, else its title, as plain text
    /// shortened to `STATUS_TEXT_CHARS`.
    pub fn summary(&self) -> String {
        let text = self
            .message
            .as_deref()
            .or(self.title.as_deref())
            .map(|text| plain_text(text, STATUS_TEXT_CHARS))
            .filter(|text| !text.is_empty());
        match text {
            Some(text) => format!("{}: {text}", self.label()),
            None => self.label(),
        }
    }
    fn heap_bytes(&self) -> usize {
        [&self.app, &self.title, &self.message]
            .into_iter()
            .flatten()
            .map(String::len)
            .sum()
    }
}

/// Characters that never reach the window's text: controls, bidirectional
/// controls, invisible characters and line and paragraph separators.
pub(crate) fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}'
                | '\u{200b}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
        )
}

/// `value` without hidden characters, at most `limit` characters, ended by
/// an ellipsis when it was longer.
pub(crate) fn plain_text(value: &str, limit: usize) -> String {
    let mut visible = value.chars().filter(|c| !hidden(*c));
    let mut text: String = visible.by_ref().take(limit).collect();
    if visible.next().is_some() {
        text.pop();
        text.push('…');
    }
    text
}

#[derive(Debug)]
struct Entry {
    status: ProgramStatus,
    /// The pane and Screen the event named, for a Screen the window has no
    /// snapshot of.
    pane: Option<String>,
    screen: Option<String>,
}

/// One source connection's root records by session. Cleared with the
/// connection: `SourceWorkspace::reset_connection` starts a new table.
#[derive(Clone, Debug, Default)]
pub(super) struct ProgramStatusTable {
    supported: bool,
    entries: Arc<HashMap<String, Arc<Charged<Entry>>>>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_BYTES && !id.chars().any(hidden)
}

impl SourceWorkspace {
    /// Whether this source's current connection announced program status.
    pub fn program_status_supported(&self) -> bool {
        self.program_status.supported
    }
    /// A source that is not connected shows no status.
    pub(crate) fn clear_program_status(&mut self) {
        self.program_status = ProgramStatusTable::default();
    }
    pub(crate) fn set_program_status_supported(&mut self, supported: bool) {
        self.program_status = ProgramStatusTable {
            supported,
            ..ProgramStatusTable::default()
        };
    }
    /// Apply one `program_status.changed` of this source's current
    /// connection. Nothing changes for another connection, before readiness
    /// or without the capability; an invalid event changes nothing either.
    pub(crate) fn observe_program_status(
        &mut self,
        source: SourceToken,
        json: &Value,
        budget: &ConnectionBudget,
    ) -> io::Result<bool> {
        if source != self.source_token() || !self.is_ready() || !self.program_status.supported {
            return Ok(false);
        }
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid program status");
        let session = json
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|id| valid_id(id))
            .ok_or_else(invalid)?;
        let optional = |field: &str| -> io::Result<Option<&str>> {
            match json.get(field) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(id)) if valid_id(id) => Ok(Some(id)),
                Some(_) => Err(invalid()),
            }
        };
        let pane = optional("paneId")?;
        let screen = optional("screenId")?;
        let status = match json.get("status") {
            Some(Value::Null) => None,
            Some(value) => Some(ProgramStatus::parse(value).ok_or_else(invalid)?),
            None => return Err(invalid()),
        };
        let Some(status) = status else {
            if !self.program_status.entries.contains_key(session) {
                return Ok(false);
            }
            Arc::make_mut(&mut self.program_status.entries).remove(session);
            return Ok(true);
        };
        let capacity = || {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "Program status exceeds capacity",
            )
        };
        if !self.program_status.entries.contains_key(session)
            && self.program_status.entries.len() >= MAX_STATUS_SESSIONS
        {
            return Err(capacity());
        }
        let bytes = std::mem::size_of::<Entry>()
            + session.len()
            + pane.map_or(0, str::len)
            + screen.map_or(0, str::len)
            + status.heap_bytes();
        let lease = budget.reserve(bytes, false).map_err(|_| capacity())?;
        let entry = Arc::new(lease.charge(Entry {
            status,
            pane: pane.map(str::to_owned),
            screen: screen.map(str::to_owned),
        }));
        Arc::make_mut(&mut self.program_status.entries).insert(session.to_owned(), entry);
        Ok(true)
    }
    /// The root record of one of this source's sessions.
    pub fn session_program_status(&self, session: &str) -> Option<&ProgramStatus> {
        self.program_status
            .entries
            .get(session)
            .map(|entry| &entry.status)
    }
    /// The root record of the session bound to `pane`.
    pub fn pane_program_status(&self, pane: &str) -> Option<&ProgramStatus> {
        self.session_for(pane)
            .and_then(|session| self.session_program_status(session))
    }
    /// The Screen of a session's pane: the Screen of the pane the session is
    /// bound to, else the Screen an event named when this source's catalog
    /// lists it. A Screen the catalog does not know is none.
    pub(crate) fn event_screen<'a>(
        &'a self,
        session: &str,
        named: Option<&'a str>,
    ) -> Option<&'a str> {
        if let Some(screen) = self
            .pane_for(session)
            .and_then(|pane| self.screen_for(pane))
        {
            return Some(screen);
        }
        let screen = named?;
        self.live_catalog()?
            .iter()
            .any(|info| info.id.as_str() == screen)
            .then_some(screen)
    }
    fn status_screen<'a>(&'a self, session: &str, entry: &'a Entry) -> Option<&'a str> {
        self.event_screen(session, entry.screen.as_deref())
    }
    /// The Screens of this source that hold a pane whose program is blocked,
    /// whether or not the window opened them.
    pub fn blocked_screens(&self) -> HashSet<&str> {
        self.program_status
            .entries
            .iter()
            .filter(|(_, entry)| entry.status.state == ProgramState::Blocked)
            .filter_map(|(session, entry)| self.status_screen(session, entry))
            .collect()
    }
    /// The pane the event named for a session this source has not bound.
    pub fn program_status_pane(&self, session: &str) -> Option<&str> {
        self.pane_for(session).or_else(|| {
            self.program_status
                .entries
                .get(session)
                .and_then(|entry| entry.pane.as_deref())
        })
    }
    /// The Screen a session's record marks, see `blocked_screens`.
    pub fn program_status_screen(&self, session: &str) -> Option<&str> {
        let entry = self.program_status.entries.get(session)?;
        self.status_screen(session, entry)
    }
}

/// What the user saw of `done` and `error` records, by the record's time,
/// per source and session. A window keeps it in memory; a record seen once
/// is not marked again when a replaced connection reports it again, only a
/// newer one is.
#[derive(Debug, Default)]
pub struct ProgramStatusSeen {
    seen: HashMap<SourceId, HashMap<String, u64>>,
}
impl ProgramStatusSeen {
    /// Note the `done` or `error` record of a pane the user sees now.
    pub fn see(&mut self, source: &SourceWorkspace, pane: &str) -> bool {
        let Some(session) = source.session_for(pane) else {
            return false;
        };
        let Some(status) = source.session_program_status(session) else {
            return false;
        };
        if !matches!(status.state, ProgramState::Done | ProgramState::Error) {
            return false;
        }
        let sessions = self.seen.entry(source.source_id()).or_default();
        if sessions
            .get(session)
            .is_some_and(|seen| *seen >= status.updated_at)
        {
            return false;
        }
        if sessions.len() >= MAX_STATUS_SESSIONS && !sessions.contains_key(session) {
            // Keep what a current record still needs, then the newest.
            sessions.retain(|session, _| {
                source
                    .session_program_status(session)
                    .is_some_and(|status| {
                        matches!(status.state, ProgramState::Done | ProgramState::Error)
                    })
            });
            if sessions.len() >= MAX_STATUS_SESSIONS {
                if let Some(oldest) = sessions
                    .iter()
                    .min_by_key(|(_, seen)| **seen)
                    .map(|(session, _)| session.clone())
                {
                    sessions.remove(&oldest);
                }
            }
        }
        sessions.insert(session.to_owned(), status.updated_at);
        true
    }
    /// Forget the sources the window no longer has.
    pub fn reconcile(&mut self, window: &WindowWorkspace) {
        self.seen
            .retain(|source, _| window.source(*source).is_some());
    }
    /// What a pane's tab shows: the root record of its session, except a
    /// `done` or `error` record the user saw.
    pub fn shown<'a>(&self, source: &'a SourceWorkspace, pane: &str) -> Option<&'a ProgramStatus> {
        let session = source.session_for(pane)?;
        let status = source.session_program_status(session)?;
        let seen = matches!(status.state, ProgramState::Done | ProgramState::Error)
            && self
                .seen
                .get(&source.source_id())
                .and_then(|sessions| sessions.get(session))
                .is_some_and(|seen| *seen >= status.updated_at);
        (!seen).then_some(status)
    }
}

#[cfg(test)]
mod tests;
