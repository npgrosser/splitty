//! What a window tells each server for its PTY size policy, and the sizes it
//! learns back (terminal-sessions: Client Activity Reports, Client Display of
//! the PTY Size).
//!
//! Only the user's own interaction counts: the window reports it, and the
//! connection of the shown Screen gets an `activity`, at most every
//! [`ACTIVITY_INTERVAL`]. A hidden window is idle on every connection. Each
//! `mux.welcome` of a server that decides sizes gets the window's state before
//! anything is attached, so a window nobody touched (opened, reconnected)
//! never takes the size from a client in use. Servers without
//! [`CAPABILITY`] get none of these messages.
use serde_json::Value;
pub use splitty_core::protocol::sizing::{SizePolicy, CAPABILITY};
use std::{
    io,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// At most one `activity` per connection in this time; servers wait minutes
/// (`terminal.size_idle_seconds`, at least 10 s) before a client is idle.
pub const ACTIVITY_INTERVAL: Duration = Duration::from_secs(5);
/// The largest PTY size taken from a server. A host must not make a terminal
/// allocate a huge buffer; a larger size is ignored.
pub const MAX_PTY_COLS: u16 = 1000;
pub const MAX_PTY_ROWS: u16 = 500;

/// Whether the platform presents the window's frames, and whether it is the
/// active window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowPresence {
    pub visible: bool,
    pub focused: bool,
}

impl WindowPresence {
    /// A user is looking at the window and can act in it.
    pub fn in_use(self) -> bool {
        self.visible && self.focused
    }

    fn bits(self) -> u8 {
        u8::from(self.visible) | (u8::from(self.focused) << 1)
    }

    fn from_bits(bits: u8) -> Self {
        Self {
            visible: bits & 1 != 0,
            focused: bits & 2 != 0,
        }
    }
}

/// The window's presence and its user's interaction, written by the UI and
/// read by its worker without a queue: the worker greets a welcome with the
/// state at that moment, and notices a hidden window or an interaction at its
/// next turn. Typing thus never adds to the worker's bounded command queue.
/// Until the UI writes it, the window counts as not in use.
#[derive(Clone, Debug, Default)]
pub struct WindowPresenceCell(Arc<PresenceState>);

#[derive(Debug, Default)]
struct PresenceState {
    presence: AtomicU8,
    acted: AtomicBool,
}

impl WindowPresenceCell {
    pub fn set(&self, presence: WindowPresence) {
        self.0.presence.store(presence.bits(), Ordering::Release);
    }

    pub fn get(&self) -> WindowPresence {
        WindowPresence::from_bits(self.0.presence.load(Ordering::Acquire))
    }

    /// The user acted in the window: a key, a pointer button, the window
    /// becoming active or changing its size. The connection of the Screen
    /// shown at the worker's next turn hears of it.
    pub fn acted(&self) {
        self.0.acted.store(true, Ordering::Release);
    }

    /// Whether the user acted since the last call; the worker's to ask.
    pub fn take_acted(&self) -> bool {
        self.0.acted.swap(false, Ordering::AcqRel)
    }
}

/// A session's PTY size, as its server's size policy set it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub policy: SizePolicy,
}

impl PtySize {
    /// The size a `size` event carries, or `None` for one out of bounds, with
    /// an unknown policy or malformed.
    pub fn from_event(json: &Value) -> Option<Self> {
        let dimension = |key: &str, max: u16| {
            json[key]
                .as_u64()
                .and_then(|value| u16::try_from(value).ok())
                .filter(|value| (1..=max).contains(value))
        };
        Some(Self {
            cols: dimension("cols", MAX_PTY_COLS)?,
            rows: dimension("rows", MAX_PTY_ROWS)?,
            policy: SizePolicy::parse(json["policy"].as_str()?)?,
        })
    }

    /// Under `smallest` every client counts; there is nothing to take over.
    pub fn offers_takeover(self) -> bool {
        self.policy != SizePolicy::Smallest
    }
}

/// Whether a `mux.welcome` announces a server that decides PTY sizes.
pub fn decides_sizes(welcome: &Value) -> bool {
    welcome["capabilities"]
        .as_array()
        .is_some_and(|capabilities| capabilities.iter().any(|cap| cap == CAPABILITY))
}

/// One message for a connection's activity state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Report {
    Activity { takeover: bool },
    Idle,
}

/// What one connection was told and still has to be told. The reports wait
/// here until the worker's next reconcile sends them, so that a failed send
/// is retried and a greeting precedes every attach.
#[derive(Debug, Default)]
pub(super) struct ConnectionPresence {
    supported: bool,
    /// When the connection last got an `activity`; none since its last `idle`.
    last_activity: Option<Instant>,
    pending: Option<Report>,
    /// The pending report answers a welcome: nothing may be attached first.
    greeting: bool,
}

impl ConnectionPresence {
    /// The server announced [`CAPABILITY`] in its latest welcome.
    pub fn supported(&self) -> bool {
        self.supported
    }

    /// A `mux.welcome`: greet it with `activity` while the window is in use
    /// on a Screen of this connection, else with `idle`.
    pub fn welcome(&mut self, supported: bool, in_use_here: bool) {
        self.supported = supported;
        self.last_activity = None;
        self.greeting = supported;
        self.pending = supported.then_some(if in_use_here {
            Report::Activity { takeover: false }
        } else {
            Report::Idle
        });
    }

    /// The user acted in the window while it showed a Screen of this connection.
    pub fn acted(&mut self, now: Instant) {
        if !self.supported || matches!(self.pending, Some(Report::Activity { .. })) {
            return;
        }
        if self.pending.is_none()
            && self
                .last_activity
                .is_some_and(|last| now.saturating_duration_since(last) < ACTIVITY_INTERVAL)
        {
            return;
        }
        // A pending `idle` gives way: the user acted after it was decided.
        self.pending = Some(Report::Activity { takeover: false });
    }

    /// The window was hidden.
    pub fn hidden(&mut self) {
        if self.supported {
            self.pending = Some(Report::Idle);
        }
    }

    /// The user asked to show this connection's sizes now.
    pub fn take_over(&mut self) -> bool {
        if self.supported {
            self.pending = Some(Report::Activity { takeover: true });
        }
        self.supported
    }

    /// Sessions may be attached: no greeting waits.
    pub fn attach_ready(&self) -> bool {
        !self.greeting
    }

    /// Send the pending report, if any; it stays pending when `send` fails.
    pub fn flush(
        &mut self,
        now: Instant,
        send: impl FnOnce(Report) -> io::Result<()>,
    ) -> io::Result<()> {
        let Some(report) = self.pending else {
            return Ok(());
        };
        send(report)?;
        self.pending = None;
        self.greeting = false;
        self.last_activity = match report {
            Report::Activity { .. } => Some(now),
            Report::Idle => None,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sent(presence: &mut ConnectionPresence, now: Instant) -> Option<Report> {
        let mut out = None;
        presence
            .flush(now, |report| {
                out = Some(report);
                Ok(())
            })
            .unwrap();
        out
    }

    #[test]
    fn a_welcome_is_greeted_before_anything_is_attached() {
        let now = Instant::now();
        let mut presence = ConnectionPresence::default();
        presence.welcome(true, false);
        assert!(!presence.attach_ready());
        assert_eq!(sent(&mut presence, now), Some(Report::Idle));
        assert!(presence.attach_ready());

        presence.welcome(true, true);
        assert!(!presence.attach_ready());
        assert_eq!(
            sent(&mut presence, now),
            Some(Report::Activity { takeover: false })
        );
        assert!(presence.attach_ready());
    }

    #[test]
    fn a_failed_greeting_stays_pending_and_keeps_attaches_back() {
        let mut presence = ConnectionPresence::default();
        presence.welcome(true, false);
        let error = presence.flush(Instant::now(), |_| Err(io::Error::other("full")));
        assert!(error.is_err());
        assert!(!presence.attach_ready());
        assert_eq!(sent(&mut presence, Instant::now()), Some(Report::Idle));
    }

    #[test]
    fn a_server_without_the_capability_gets_nothing() {
        let now = Instant::now();
        let mut presence = ConnectionPresence::default();
        presence.welcome(false, true);
        assert!(presence.attach_ready());
        presence.acted(now);
        presence.hidden();
        assert!(!presence.take_over());
        assert_eq!(sent(&mut presence, now), None);
    }

    #[test]
    fn activity_is_sent_at_most_once_per_interval() {
        let start = Instant::now();
        let mut presence = ConnectionPresence::default();
        presence.welcome(true, false);
        sent(&mut presence, start);
        presence.acted(start);
        assert_eq!(
            sent(&mut presence, start),
            Some(Report::Activity { takeover: false })
        );
        presence.acted(start + Duration::from_secs(1));
        presence.acted(start + Duration::from_millis(4999));
        assert_eq!(sent(&mut presence, start + Duration::from_secs(4)), None);
        presence.acted(start + ACTIVITY_INTERVAL);
        assert_eq!(
            sent(&mut presence, start + ACTIVITY_INTERVAL),
            Some(Report::Activity { takeover: false })
        );
    }

    #[test]
    fn idle_resets_the_interval_and_gives_way_to_a_later_action() {
        let start = Instant::now();
        let mut presence = ConnectionPresence::default();
        presence.welcome(true, true);
        sent(&mut presence, start);
        presence.hidden();
        assert_eq!(sent(&mut presence, start), Some(Report::Idle));
        // After an `idle`, the next action is sent at once.
        presence.acted(start + Duration::from_millis(10));
        assert_eq!(
            sent(&mut presence, start),
            Some(Report::Activity { takeover: false })
        );
        // An `idle` not yet sent is replaced by an action after it.
        presence.hidden();
        presence.acted(start + Duration::from_millis(20));
        assert_eq!(
            sent(&mut presence, start),
            Some(Report::Activity { takeover: false })
        );
        // And the other way round.
        presence.acted(start + ACTIVITY_INTERVAL * 2);
        presence.hidden();
        assert_eq!(sent(&mut presence, start), Some(Report::Idle));
    }

    #[test]
    fn takeover_is_sent_even_right_after_an_activity() {
        let now = Instant::now();
        let mut presence = ConnectionPresence::default();
        presence.welcome(true, true);
        sent(&mut presence, now);
        assert!(presence.take_over());
        presence.acted(now);
        assert_eq!(
            sent(&mut presence, now),
            Some(Report::Activity { takeover: true })
        );
    }

    #[test]
    fn window_presence_round_trips_through_its_cell() {
        let cell = WindowPresenceCell::default();
        assert_eq!(cell.get(), WindowPresence::default());
        assert!(!cell.get().in_use());
        for visible in [false, true] {
            for focused in [false, true] {
                let presence = WindowPresence { visible, focused };
                cell.clone().set(presence);
                assert_eq!(cell.get(), presence);
                assert_eq!(presence.in_use(), visible && focused);
            }
        }
    }

    #[test]
    fn an_interaction_is_taken_once() {
        let cell = WindowPresenceCell::default();
        assert!(!cell.take_acted());
        cell.clone().acted();
        cell.acted();
        cell.set(WindowPresence {
            visible: true,
            focused: true,
        });
        assert!(cell.take_acted());
        assert!(!cell.take_acted());
        assert!(cell.get().in_use());
    }

    #[test]
    fn size_events_out_of_range_or_with_an_unknown_policy_are_ignored() {
        let size = |cols: Value, rows: Value, policy: Value| {
            PtySize::from_event(
                &json!({"type":"size","sessionId":"s","cols":cols,"rows":rows,"policy":policy}),
            )
        };
        assert_eq!(
            size(json!(100), json!(30), json!("active")),
            Some(PtySize {
                cols: 100,
                rows: 30,
                policy: SizePolicy::Active
            })
        );
        assert_eq!(
            size(json!(1000), json!(500), json!("smallest")).map(|size| size.offers_takeover()),
            Some(false)
        );
        assert!(size(json!(1), json!(1), json!("latest")).is_some_and(PtySize::offers_takeover));
        for (cols, rows, policy) in [
            (json!(5000), json!(30), json!("active")),
            (json!(100), json!(501), json!("active")),
            (json!(0), json!(30), json!("active")),
            (json!(100), json!(0), json!("active")),
            (json!(-1), json!(30), json!("active")),
            (json!(100.5), json!(30), json!("active")),
            (json!("100"), json!(30), json!("active")),
            (json!(70000), json!(30), json!("active")),
            (json!(100), json!(30), json!("largest")),
            (json!(100), json!(30), json!(null)),
        ] {
            assert_eq!(size(cols, rows, policy), None);
        }
    }

    #[test]
    fn only_a_welcome_with_the_capability_decides_sizes() {
        assert!(decides_sizes(
            &json!({"type":"mux.welcome","capabilities":["x", CAPABILITY]})
        ));
        assert!(!decides_sizes(
            &json!({"type":"mux.welcome","capabilities":["x"]})
        ));
        assert!(!decides_sizes(&json!({"type":"mux.welcome"})));
    }
}
