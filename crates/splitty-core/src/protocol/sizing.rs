//! The PTY size of sessions that several connections show.
//!
//! Every connection reports the size at which it shows a session (`resize`)
//! and tells when its user acts (`activity`) or stops using it (`idle`). The
//! arbiter derives each session's PTY size from those reports with the
//! configured policy, resizes the PTY when that size changes and publishes it,
//! so that every connection streaming the session can send it a `size` event.

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tracing::debug;

/// Announced in `mux.welcome.capabilities` by servers that arbitrate sizes.
pub const CAPABILITY: &str = "pty-size-v1";

/// How often the arbiter looks for connections that have become idle.
pub const TICK: Duration = Duration::from_secs(1);

/// Which connections' reports decide a session's PTY size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizePolicy {
    /// The active connections; when none is, as [`SizePolicy::Latest`].
    Active,
    /// The connection whose user acted most recently.
    Latest,
    /// Every connection that shows the session.
    Smallest,
}

impl SizePolicy {
    /// The name in the settings and in `size` events.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Latest => "latest",
            Self::Smallest => "smallest",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "active" => Some(Self::Active),
            "latest" => Some(Self::Latest),
            "smallest" => Some(Self::Smallest),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeConfig {
    pub policy: SizePolicy,
    /// How long after its last activity a connection still counts as active.
    pub idle: Duration,
}

impl SizeConfig {
    /// From the settings `terminal.size_policy` and
    /// `terminal.size_idle_seconds`; an unknown policy is the default one.
    pub fn from_settings(policy: &str, idle_seconds: u32) -> Self {
        Self {
            policy: SizePolicy::parse(policy).unwrap_or(SizePolicy::Active),
            idle: Duration::from_secs(u64::from(idle_seconds.max(1))),
        }
    }
}

impl Default for SizeConfig {
    fn default() -> Self {
        Self {
            policy: SizePolicy::Active,
            idle: Duration::from_secs(120),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

/// What a session's `size` event carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeState {
    pub size: PtySize,
    pub policy: SizePolicy,
}

/// A connection, as the arbiter knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClientKey(pub u64);

/// Resizes a session's PTY.
pub type ResizeFn = Box<dyn Fn(&str, PtySize) -> io::Result<()> + Send + Sync>;

/// The answer to a connection's report.
pub struct Reported {
    /// The session's size after the report.
    pub state: SizeState,
    /// Whether the report changed it, which publishes it to every stream.
    pub changed: bool,
    /// The outcome of resizing the PTY, if it was resized.
    pub result: io::Result<()>,
}

#[derive(Clone, Copy, Debug)]
struct Client {
    /// When its user last acted.
    last_activity: Option<Instant>,
    /// The order of that activity among all connections; 0 for none yet.
    order: u64,
    /// Hidden, or another connection took the size over, until its next
    /// activity.
    idle: bool,
    /// Has never sent `activity` or `idle`: a client that predates them,
    /// whose every report counts as an activity.
    legacy: bool,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            last_activity: None,
            order: 0,
            idle: false,
            legacy: true,
        }
    }
}

impl Client {
    fn is_active(&self, now: Instant, idle: Duration) -> bool {
        !self.idle
            && self
                .last_activity
                .is_some_and(|at| now.saturating_duration_since(at) < idle)
    }
}

struct Session {
    /// The size at which each connection shows the session.
    reports: HashMap<ClientKey, PtySize>,
    /// The size the PTY was last set to by the arbiter.
    applied: Option<PtySize>,
    published: watch::Sender<Option<SizeState>>,
}

impl Session {
    fn new() -> Self {
        Self {
            reports: HashMap::new(),
            applied: None,
            published: watch::channel(None).0,
        }
    }
}

struct State {
    config: SizeConfig,
    clients: HashMap<ClientKey, Client>,
    sessions: HashMap<String, Session>,
    /// The order of the latest activity.
    order: u64,
}

impl State {
    /// `client`'s user acted at `now`.
    fn touch(&mut self, client: ClientKey, now: Instant) {
        self.order += 1;
        let entry = self.clients.entry(client).or_default();
        entry.last_activity = Some(now);
        entry.order = self.order;
        entry.idle = false;
    }

    /// Settle every session; errors are only logged, nobody asked for them.
    fn settle_all(&mut self, now: Instant, resize: &ResizeFn) {
        let config = self.config;
        for (id, session) in &mut self.sessions {
            if let Some(Err(error)) = settle(session, id, &self.clients, config, now, resize) {
                debug!(session = %id, %error, "PTY resize failed");
            }
        }
    }
}

/// Derives each session's PTY size from what its connections report.
pub struct SizeArbiter {
    state: Mutex<State>,
    resize: ResizeFn,
}

impl SizeArbiter {
    pub fn new(config: SizeConfig, resize: ResizeFn) -> Self {
        Self {
            state: Mutex::new(State {
                config,
                clients: HashMap::new(),
                sessions: HashMap::new(),
                order: 0,
            }),
            resize,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn config(&self) -> SizeConfig {
        self.lock().config
    }

    /// Apply a new policy or idle time to every session. Each session with a
    /// size publishes it again, carrying the new policy.
    pub fn configure(&self, config: SizeConfig, now: Instant) {
        let mut state = self.lock();
        state.config = config;
        state.settle_all(now, &self.resize);
        for session in state.sessions.values() {
            if let Some(size) = session.applied {
                session.published.send_replace(Some(SizeState {
                    size,
                    policy: config.policy,
                }));
            }
        }
    }

    /// The session's sizes from now on; the current one is marked as seen.
    pub fn subscribe(&self, session: &str) -> watch::Receiver<Option<SizeState>> {
        let mut state = self.lock();
        let entry = state
            .sessions
            .entry(session.to_string())
            .or_insert_with(Session::new);
        let mut receiver = entry.published.subscribe();
        receiver.borrow_and_update();
        receiver
    }

    /// `client` shows `session` at `size`. With `redraw`, a report that does
    /// not change the size resizes the PTY by one row and back, so the
    /// session's processes redraw, without publishing the intermediate size.
    pub fn report(
        &self,
        session: &str,
        client: ClientKey,
        size: PtySize,
        redraw: bool,
        now: Instant,
    ) -> Reported {
        let mut guard = self.lock();
        let state = &mut *guard;
        if state.clients.get(&client).is_none_or(|c| c.legacy) {
            state.touch(client, now);
        }
        let config = state.config;
        let entry = state
            .sessions
            .entry(session.to_string())
            .or_insert_with(Session::new);
        entry.reports.insert(client, size);
        let outcome = settle(entry, session, &state.clients, config, now, &self.resize);
        let current = SizeState {
            size: entry.applied.unwrap_or(size),
            policy: config.policy,
        };
        match outcome {
            Some(result) => Reported {
                state: current,
                changed: true,
                result,
            },
            None => Reported {
                state: current,
                changed: false,
                result: if redraw {
                    nudge(&self.resize, session, current.size)
                } else {
                    Ok(())
                },
            },
        }
    }

    /// `client`'s user acted. With `takeover`, every other connection is idle
    /// until its own next activity.
    pub fn activity(&self, client: ClientKey, takeover: bool, now: Instant) {
        let mut state = self.lock();
        state.touch(client, now);
        if let Some(entry) = state.clients.get_mut(&client) {
            entry.legacy = false;
        }
        if takeover {
            for (key, other) in &mut state.clients {
                if *key != client {
                    other.idle = true;
                }
            }
        }
        state.settle_all(now, &self.resize);
    }

    /// `client` is not in use (its window is hidden) until its next activity.
    pub fn idle(&self, client: ClientKey, now: Instant) {
        let mut state = self.lock();
        let entry = state.clients.entry(client).or_default();
        entry.idle = true;
        entry.legacy = false;
        state.settle_all(now, &self.resize);
    }

    /// `client` no longer shows `session`: it hid, detached or closed it.
    pub fn withdraw(&self, session: &str, client: ClientKey, now: Instant) {
        let mut guard = self.lock();
        let state = &mut *guard;
        let config = state.config;
        let Some(entry) = state.sessions.get_mut(session) else {
            return;
        };
        if entry.reports.remove(&client).is_none() {
            return;
        }
        if let Some(Err(error)) = settle(entry, session, &state.clients, config, now, &self.resize)
        {
            debug!(session, %error, "PTY resize failed");
        }
    }

    /// `client` disconnected: it shows nothing any more.
    pub fn disconnect(&self, client: ClientKey, now: Instant) {
        let mut state = self.lock();
        state.clients.remove(&client);
        for session in state.sessions.values_mut() {
            session.reports.remove(&client);
        }
        state.settle_all(now, &self.resize);
        prune(&mut state);
    }

    /// Settle sessions whose connections became idle since; forget sessions
    /// nobody shows or streams.
    pub fn tick(&self, now: Instant) {
        let mut state = self.lock();
        state.settle_all(now, &self.resize);
        prune(&mut state);
    }
}

/// Forget sessions without reports and without streams.
fn prune(state: &mut State) {
    state
        .sessions
        .retain(|_, session| !session.reports.is_empty() || session.published.receiver_count() > 0);
}

/// Set the session's PTY to the size the policy derives, if that changed it.
/// Returns the outcome of the resize then, `None` when nothing changed.
fn settle(
    session: &mut Session,
    id: &str,
    clients: &HashMap<ClientKey, Client>,
    config: SizeConfig,
    now: Instant,
    resize: &ResizeFn,
) -> Option<io::Result<()>> {
    let reports: Vec<(Client, PtySize)> = session
        .reports
        .iter()
        .map(|(key, size)| (clients.get(key).copied().unwrap_or_default(), *size))
        .collect();
    let size = choose(config, now, &reports)?;
    if session.applied == Some(size) {
        return None;
    }
    let result = resize(id, size);
    session.applied = Some(size);
    session.published.send_replace(Some(SizeState {
        size,
        policy: config.policy,
    }));
    Some(result)
}

/// Resize the PTY one row up (down at the largest size) and back.
fn nudge(resize: &ResizeFn, id: &str, size: PtySize) -> io::Result<()> {
    let rows = if size.rows < u16::MAX {
        size.rows + 1
    } else {
        size.rows - 1
    };
    resize(
        id,
        PtySize {
            cols: size.cols,
            rows,
        },
    )?;
    resize(id, size)
}

/// The PTY size the policy derives from the connections' reports at `now`:
/// the smallest columns and the smallest rows of the chosen reports, taken
/// separately; `None` without reports.
fn choose(config: SizeConfig, now: Instant, reports: &[(Client, PtySize)]) -> Option<PtySize> {
    let latest = || {
        let newest = reports.iter().map(|(client, _)| client.order).max();
        reports
            .iter()
            .filter(|(client, _)| newest == Some(0) || Some(client.order) == newest)
            .map(|(_, size)| *size)
            .collect::<Vec<_>>()
    };
    let chosen = match config.policy {
        SizePolicy::Smallest => reports.iter().map(|(_, size)| *size).collect(),
        SizePolicy::Latest => latest(),
        SizePolicy::Active => {
            let active: Vec<PtySize> = reports
                .iter()
                .filter(|(client, _)| client.is_active(now, config.idle))
                .map(|(_, size)| *size)
                .collect();
            if active.is_empty() {
                latest()
            } else {
                active
            }
        }
    };
    Some(PtySize {
        cols: chosen.iter().map(|size| size.cols).min()?,
        rows: chosen.iter().map(|size| size.rows).min()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const A: ClientKey = ClientKey(1);
    const B: ClientKey = ClientKey(2);
    const SID: &str = "sess-1";
    const WIDE: PtySize = PtySize {
        cols: 120,
        rows: 40,
    };
    const NARROW: PtySize = PtySize {
        cols: 100,
        rows: 30,
    };

    /// The PTY resizes an arbiter made, by session.
    type Resizes = Arc<Mutex<Vec<(String, PtySize)>>>;

    /// An arbiter whose resizes are recorded.
    fn arbiter(policy: SizePolicy) -> (SizeArbiter, Resizes) {
        let resizes = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&resizes);
        let arbiter = SizeArbiter::new(
            SizeConfig {
                policy,
                idle: Duration::from_secs(120),
            },
            Box::new(move |id, size| {
                recorded.lock().unwrap().push((id.to_string(), size));
                Ok(())
            }),
        );
        (arbiter, resizes)
    }

    fn last_resize(resizes: &Mutex<Vec<(String, PtySize)>>) -> Option<PtySize> {
        resizes.lock().unwrap().last().map(|(_, size)| *size)
    }

    /// A and B both active, showing the session at `a` and `b`.
    fn two_active(arbiter: &SizeArbiter, a: PtySize, b: PtySize, now: Instant) {
        arbiter.activity(A, false, now);
        arbiter.activity(B, false, now);
        arbiter.report(SID, A, a, false, now);
        arbiter.report(SID, B, b, false, now);
    }

    #[test]
    fn the_smaller_active_client_decides() {
        let (arbiter, resizes) = arbiter(SizePolicy::Active);
        let now = Instant::now();
        two_active(&arbiter, WIDE, NARROW, now);
        assert_eq!(last_resize(&resizes), Some(NARROW));
    }

    #[test]
    fn columns_and_rows_are_taken_separately() {
        let (arbiter, resizes) = arbiter(SizePolicy::Smallest);
        let now = Instant::now();
        arbiter.report(SID, A, PtySize { cols: 80, rows: 50 }, false, now);
        arbiter.report(
            SID,
            B,
            PtySize {
                cols: 120,
                rows: 20,
            },
            false,
            now,
        );
        assert_eq!(last_resize(&resizes), Some(PtySize { cols: 80, rows: 20 }));
    }

    #[test]
    fn an_idle_client_drops_out() {
        let (arbiter, resizes) = arbiter(SizePolicy::Active);
        let start = Instant::now();
        two_active(&arbiter, WIDE, NARROW, start);
        let later = start + Duration::from_secs(100);
        arbiter.activity(A, false, later);
        assert_eq!(last_resize(&resizes), Some(NARROW), "B is still active");

        arbiter.tick(start + Duration::from_secs(121));
        assert_eq!(last_resize(&resizes), Some(WIDE));
    }

    #[test]
    fn nobody_active_leaves_the_latest_one_deciding() {
        let (arbiter, resizes) = arbiter(SizePolicy::Active);
        let start = Instant::now();
        arbiter.activity(A, false, start);
        arbiter.report(SID, A, WIDE, false, start);
        arbiter.activity(B, false, start + Duration::from_secs(10));
        arbiter.report(SID, B, NARROW, false, start + Duration::from_secs(10));
        arbiter.tick(start + Duration::from_secs(600));
        assert_eq!(last_resize(&resizes), Some(NARROW));
    }

    #[test]
    fn latest_activity_decides() {
        let (arbiter, resizes) = arbiter(SizePolicy::Latest);
        let now = Instant::now();
        two_active(&arbiter, WIDE, NARROW, now);
        assert_eq!(last_resize(&resizes), Some(NARROW), "B acted last");
        arbiter.activity(A, false, now);
        assert_eq!(last_resize(&resizes), Some(WIDE));
    }

    #[test]
    fn smallest_ignores_activity() {
        let (arbiter, resizes) = arbiter(SizePolicy::Smallest);
        let now = Instant::now();
        arbiter.activity(A, false, now);
        arbiter.report(SID, A, WIDE, false, now);
        arbiter.idle(B, now);
        arbiter.report(SID, B, NARROW, false, now);
        assert_eq!(last_resize(&resizes), Some(NARROW));
    }

    #[test]
    fn a_client_that_leaves_gives_the_size_back() {
        let (arbiter, resizes) = arbiter(SizePolicy::Smallest);
        let now = Instant::now();
        arbiter.report(SID, A, WIDE, false, now);
        arbiter.report(SID, B, NARROW, false, now);
        arbiter.disconnect(B, now);
        assert_eq!(last_resize(&resizes), Some(WIDE));
    }

    #[test]
    fn older_clients_count_by_their_resizes() {
        let now = Instant::now();
        let small = PtySize { cols: 90, rows: 25 };
        let (latest, resizes) = arbiter(SizePolicy::Latest);
        latest.report(SID, A, WIDE, false, now);
        latest.report(SID, B, small, false, now);
        assert_eq!(last_resize(&resizes), Some(small));
        latest.report(SID, A, WIDE, false, now);
        assert_eq!(last_resize(&resizes), Some(WIDE), "the last writer wins");

        // Under `active`, an older client takes part for the idle time after
        // its last resize.
        let (active, resizes) = arbiter(SizePolicy::Active);
        active.report(SID, A, WIDE, false, now);
        active.report(SID, B, small, false, now + Duration::from_secs(60));
        assert_eq!(last_resize(&resizes), Some(small));
        active.report(SID, A, WIDE, false, now + Duration::from_secs(200));
        assert_eq!(last_resize(&resizes), Some(WIDE), "B's resize is too old");
    }

    #[test]
    fn a_new_client_does_not_take_the_size_by_reporting() {
        let (arbiter, resizes) = arbiter(SizePolicy::Latest);
        let now = Instant::now();
        arbiter.activity(A, false, now);
        arbiter.report(SID, A, WIDE, false, now);
        arbiter.idle(B, now);
        arbiter.report(SID, B, NARROW, true, now);
        assert_eq!(last_resize(&resizes), Some(WIDE));
    }

    #[test]
    fn takeover() {
        let (arbiter, resizes) = arbiter(SizePolicy::Active);
        let now = Instant::now();
        two_active(&arbiter, WIDE, NARROW, now);
        arbiter.activity(A, true, now);
        assert_eq!(last_resize(&resizes), Some(WIDE));
        arbiter.activity(B, false, now);
        assert_eq!(last_resize(&resizes), Some(NARROW));
    }

    #[test]
    fn a_hidden_window_drops_out() {
        let (arbiter, resizes) = arbiter(SizePolicy::Active);
        let now = Instant::now();
        two_active(&arbiter, WIDE, NARROW, now);
        arbiter.idle(B, now);
        assert_eq!(last_resize(&resizes), Some(WIDE));
    }

    #[test]
    fn a_session_not_shown_drops_out() {
        let (arbiter, resizes) = arbiter(SizePolicy::Smallest);
        let now = Instant::now();
        arbiter.report(SID, A, WIDE, false, now);
        arbiter.report(SID, B, NARROW, false, now);
        arbiter.withdraw(SID, B, now);
        assert_eq!(last_resize(&resizes), Some(WIDE));
    }

    #[test]
    fn a_session_nobody_shows_keeps_its_size() {
        let (arbiter, resizes) = arbiter(SizePolicy::Smallest);
        let now = Instant::now();
        arbiter.report(SID, A, WIDE, false, now);
        arbiter.withdraw(SID, A, now);
        arbiter.tick(now);
        assert_eq!(resizes.lock().unwrap().len(), 1);
        assert_eq!(last_resize(&resizes), Some(WIDE));
    }

    #[test]
    fn redraw_without_a_size_change() {
        let (arbiter, resizes) = arbiter(SizePolicy::Active);
        let now = Instant::now();
        two_active(&arbiter, WIDE, NARROW, now);
        let sizes = arbiter.subscribe(SID);
        resizes.lock().unwrap().clear();

        let reported = arbiter.report(SID, A, WIDE, true, now);
        assert!(!reported.changed);
        assert_eq!(reported.state.size, NARROW);
        assert_eq!(
            resizes.lock().unwrap().as_slice(),
            [
                (
                    SID.to_string(),
                    PtySize {
                        cols: 100,
                        rows: 31
                    }
                ),
                (SID.to_string(), NARROW)
            ]
        );
        assert!(!sizes.has_changed().unwrap(), "nothing published");
    }

    #[test]
    fn a_changing_report_publishes_once() {
        let (arbiter, _) = arbiter(SizePolicy::Active);
        let now = Instant::now();
        let mut sizes = arbiter.subscribe(SID);
        arbiter.activity(A, false, now);
        let reported = arbiter.report(SID, A, WIDE, true, now);
        assert!(reported.changed);
        assert!(sizes.has_changed().unwrap());
        assert_eq!(
            *sizes.borrow_and_update(),
            Some(SizeState {
                size: WIDE,
                policy: SizePolicy::Active
            })
        );
    }

    #[test]
    fn a_reconfigured_policy_is_published() {
        let (arbiter, resizes) = arbiter(SizePolicy::Smallest);
        let now = Instant::now();
        arbiter.activity(B, false, now);
        arbiter.report(SID, B, NARROW, false, now);
        arbiter.activity(A, false, now);
        arbiter.report(SID, A, WIDE, false, now);
        let mut sizes = arbiter.subscribe(SID);

        arbiter.configure(
            SizeConfig {
                policy: SizePolicy::Latest,
                idle: Duration::from_secs(120),
            },
            now,
        );
        assert_eq!(last_resize(&resizes), Some(WIDE));
        assert_eq!(
            *sizes.borrow_and_update(),
            Some(SizeState {
                size: WIDE,
                policy: SizePolicy::Latest
            })
        );

        arbiter.configure(
            SizeConfig {
                policy: SizePolicy::Active,
                idle: Duration::from_secs(120),
            },
            now,
        );
        assert!(
            sizes.has_changed().unwrap(),
            "the same size with a new policy"
        );
        assert_eq!(
            sizes.borrow_and_update().map(|state| state.policy),
            Some(SizePolicy::Active)
        );
    }

    #[test]
    fn settings_names() {
        assert_eq!(
            SizeConfig::from_settings("latest", 30),
            SizeConfig {
                policy: SizePolicy::Latest,
                idle: Duration::from_secs(30)
            }
        );
        assert_eq!(
            SizeConfig::from_settings("bogus", 120).policy,
            SizePolicy::Active
        );
        for policy in [SizePolicy::Active, SizePolicy::Latest, SizePolicy::Smallest] {
            assert_eq!(SizePolicy::parse(policy.as_str()), Some(policy));
        }
    }

    #[test]
    fn sessions_nobody_shows_or_streams_are_forgotten() {
        let (arbiter, _) = arbiter(SizePolicy::Smallest);
        let now = Instant::now();
        let sizes = arbiter.subscribe(SID);
        arbiter.report(SID, A, WIDE, false, now);
        arbiter.disconnect(A, now);
        assert!(arbiter.lock().sessions.contains_key(SID), "still streamed");
        drop(sizes);
        arbiter.tick(now);
        assert!(arbiter.lock().sessions.is_empty());
    }
}
