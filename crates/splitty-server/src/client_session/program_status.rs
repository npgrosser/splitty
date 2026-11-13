//! `program_status.changed` (mux-protocol, Program Status Events): each
//! session's root program status record, sent to a client right after its
//! `mux.welcome` and to every client whenever a record changes, goes, or its
//! pane moves to another screen.
//!
//! The records live in the sessions (`splitty_core::pty::status`), which read
//! them from the output whether or not a connection streams the session. While
//! at least one client is connected, a task checks every session's status
//! revision every [`INTERVAL`], one atomic read each, so a session's events
//! are at least that far apart and carry its latest record. A client that
//! connects gets the records the others were sent; only when no check runs
//! yet are they read for it. The notification engine is not involved: it
//! tracks only streamed sessions.
//!
//! An event is marked `notify` when the record turned `blocked`, `done` or
//! `error`, or changed its kind or message while in that state, from live
//! output, and the session's last marked event with that state is at least
//! [`NOTIFY_INTERVAL`] old. The greeting and the event for a moved pane are
//! never marked. A marked event carries the notice a window shows for it,
//! naming the screen by the server's catalog; a window decides whether it
//! notifies.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use splitty_core::ids::SessionId;
use splitty_core::protocol::notice::{screen_label, Notice};
use splitty_core::protocol::ServerEvent;
use splitty_core::pty::status::{ProgramStatus, StatusState};
use tokio::sync::Mutex;

use super::sender::ClientSender;
use super::ClientState;

/// How often the records are checked while clients are connected; the least
/// time between two events for one session.
pub const INTERVAL: Duration = Duration::from_millis(100);

/// The least time between two events of one session marked `notify` with the
/// same state.
pub const NOTIFY_INTERVAL: Duration = Duration::from_secs(10);

/// The program status records of one server's sessions, as its clients know
/// them.
#[derive(Default)]
pub struct StatusWatch {
    state: Mutex<Watching>,
}

#[derive(Default)]
struct Watching {
    /// Whether the task that checks every [`INTERVAL`] runs.
    running: bool,
    /// The store's change count the locations were read at.
    workspace: Option<u64>,
    locations: HashMap<SessionId, (String, String)>,
    /// Each screen's label in a notice, by its id.
    screen_labels: HashMap<String, String>,
    /// What every client was sent, per session.
    sessions: HashMap<String, Tracker>,
}

/// What a session's clients were sent.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    revision: u64,
    sent: Option<ProgramStatus>,
    location: Option<(String, String)>,
    /// When an event with each state was last marked `notify`.
    notified: HashMap<StatusState, Instant>,
}

impl Tracker {
    /// The root record changed to `record` at `now`: whether its event is
    /// marked `notify`. `read_again` marks a record rebuilt from output read
    /// again.
    pub(crate) fn change(
        &mut self,
        record: Option<&ProgramStatus>,
        read_again: bool,
        now: Instant,
    ) -> bool {
        let previous = std::mem::replace(&mut self.sent, record.cloned());
        let Some(record) = record else {
            return false;
        };
        if read_again
            || !matches!(
                record.state,
                StatusState::Blocked | StatusState::Done | StatusState::Error
            )
        {
            return false;
        }
        let differs = previous.as_ref().is_none_or(|previous| {
            previous.state != record.state
                || previous.kind != record.kind
                || previous.msg != record.msg
        });
        if !differs {
            return false;
        }
        let recent = self
            .notified
            .get(&record.state)
            .is_some_and(|at| now.saturating_duration_since(*at) < NOTIFY_INTERVAL);
        if recent {
            return false;
        }
        self.notified.insert(record.state, now);
        true
    }
}

impl Watching {
    /// Read each session's pane and screen again when the workspace changed.
    fn locate(&mut self, client_state: &ClientState) {
        let changes = client_state.mux_store.change_count();
        if self.workspace == Some(changes) {
            return;
        }
        self.workspace = Some(changes);
        self.locations = client_state
            .mux_store
            .session_locations()
            .into_iter()
            .map(|(session, (pane, screen))| (session, (pane.to_string(), screen.to_string())))
            .collect();
        let (catalog, _) = client_state.mux_store.workspace_catalog();
        self.screen_labels = catalog
            .into_iter()
            .map(|info| {
                let label = screen_label(info.name.as_deref());
                (info.id.to_string(), label)
            })
            .collect();
    }

    /// The events for what changed since the last check, recording them as
    /// sent. With `quiet`, the first check fills the table and sends nothing.
    fn check(&mut self, client_state: &ClientState, now: Instant, quiet: bool) -> Vec<ServerEvent> {
        self.locate(client_state);
        let mut events = Vec::new();
        let mut present = HashSet::new();
        for session in client_state.dispatcher.backend().sessions() {
            let id = session.session_id().to_string();
            let location = self.locations.get(&SessionId::from(id.as_str())).cloned();
            let tracker = self.sessions.entry(id.clone()).or_default();
            present.insert(id.clone());
            let moved = tracker.location != location;
            tracker.location = location.clone();
            if tracker.revision != session.program_status_revision() {
                let root = session.program_status();
                tracker.revision = root.revision;
                if quiet {
                    tracker.sent = root.record;
                    continue;
                }
                if root.record != tracker.sent {
                    let notify = tracker.change(root.record.as_ref(), root.read_again, now);
                    let notice = notify
                        .then(|| {
                            notice(&self.screen_labels, location.as_ref(), root.record.as_ref())
                        })
                        .flatten();
                    events.push(ServerEvent::program_status_changed(
                        id,
                        location,
                        root.record,
                        notice,
                    ));
                    continue;
                }
            }
            if moved && tracker.sent.is_some() {
                events.push(ServerEvent::program_status_changed(
                    id,
                    location,
                    tracker.sent.clone(),
                    None,
                ));
            }
        }
        // Closed and removed sessions take their records with them.
        self.sessions.retain(|id, tracker| {
            if present.contains(id) {
                return true;
            }
            if tracker.sent.is_some() {
                events.push(ServerEvent::program_status_changed(
                    id.clone(),
                    tracker.location.clone(),
                    None,
                    None,
                ));
            }
            false
        });
        if quiet {
            Vec::new()
        } else {
            events
        }
    }

    /// The greeting: every record the clients hold, unmarked.
    fn greeting(&self) -> Vec<ServerEvent> {
        self.sessions
            .iter()
            .filter_map(|(id, tracker)| {
                tracker.sent.as_ref().map(|record| {
                    ServerEvent::program_status_changed(
                        id.clone(),
                        tracker.location.clone(),
                        Some(record.clone()),
                        None,
                    )
                })
            })
            .collect()
    }
}

/// The notice of a marked event for `record` at `location`, its screen
/// named by `labels`.
fn notice(
    labels: &HashMap<String, String>,
    location: Option<&(String, String)>,
    record: Option<&ProgramStatus>,
) -> Option<Notice> {
    let screen = location.and_then(|(_, screen)| labels.get(screen));
    Notice::program_status(screen.map(String::as_str), record?)
}

/// Send `sender`, a client that just got its `mux.welcome` and the listening
/// ports, the records held, and keep checking while clients are connected.
/// The client must be registered already, so the check that stops without
/// clients sees it.
pub(super) async fn greet(client_state: &Arc<ClientState>, sender: &ClientSender) {
    let watch = &client_state.program_status;
    let mut state = watch.state.lock().await;
    if !state.running {
        state.check(client_state, Instant::now(), true);
        state.running = true;
        tokio::spawn(check(client_state.clone()));
    }
    for event in state.greeting() {
        let _ = sender.send_event(&event).await;
    }
}

/// Check every [`INTERVAL`] until no client is connected, and send every
/// client the records that changed.
async fn check(client_state: Arc<ClientState>) {
    let watch = &client_state.program_status;
    loop {
        tokio::time::sleep(INTERVAL).await;
        // Under the watch's lock: a client that connects meanwhile finds this
        // task running, or starts the next one.
        let mut state = watch.state.lock().await;
        if client_state
            .clients
            .lock()
            .expect("client registry")
            .is_empty()
        {
            *state = Watching::default();
            return;
        }
        let events = state.check(&client_state, Instant::now(), false);
        client_state.broadcast_events(&events, None);
    }
}

#[cfg(test)]
mod tests;
