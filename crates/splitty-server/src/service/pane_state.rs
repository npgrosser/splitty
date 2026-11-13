//! Service-owned process metadata and ordinary pane exit policy.
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use splitty_core::{ids::SessionId, mux::Op, protocol::ServerEvent, pty::SessionHandle};
use tokio::sync::Notify;

/// How often the observer reads process metadata.
const INTERVAL: Duration = Duration::from_millis(500);
/// How long an exited pane waits for its output to end before the exit
/// policy applies anyway, as when a process that outlived the program still
/// holds the terminal open.
const DRAIN_GRACE: Duration = Duration::from_secs(3);

/// Weak ownership lets the service and its terminals shut down normally.
pub(super) fn start(state: &Arc<super::State>) {
    start_every(state, INTERVAL);
}

/// [`start`], reading metadata every `interval`.
pub(crate) fn start_every(state: &Arc<super::State>, interval: Duration) {
    let weak = Arc::downgrade(state);
    let wake = Arc::new(Notify::new());
    tokio::spawn(async move {
        let mut observer = Observer::waking(Arc::clone(&wake));
        let mut interval = tokio::time::interval(interval);
        loop {
            // A program's exit and the end of its output are applied at once,
            // not at the next metadata read.
            tokio::select! {
                _ = interval.tick() => {}
                () = wake.notified() => {}
            }
            let Some(state) = weak.upgrade() else {
                break;
            };
            observer.refresh(&state, Instant::now()).await;
        }
    });
}

#[derive(Default)]
pub(crate) struct Observer {
    exits: HashMap<String, Instant>,
    /// Wakes the observer's loop; without it the observer refreshes only
    /// when called.
    wake: Option<Arc<Notify>>,
    /// The sessions seen, whose end wakes the observer.
    seen: HashMap<String, Seen>,
}

struct Seen {
    session: Weak<dyn SessionHandle>,
    /// Whether the observer saw the program run: then it sees the exit
    /// happen. A program that ended unobserved, as one that exited while no
    /// daemon ran, keeps its pane through the grace, for a client to read
    /// its result.
    ran: bool,
}

/// Wake the observer once `session`'s program has ended and again once its
/// output has, holding neither the session nor the observer's service.
fn watch_end(session: &Arc<dyn SessionHandle>, wake: Arc<Notify>) {
    let mut done = session.done();
    let mut output = session.output_watch();
    let session = Arc::downgrade(session);
    tokio::spawn(async move {
        // An error means the exit watcher is gone, which it only is once the
        // process has exited.
        let _ = done.wait_for(|done| *done).await;
        wake.notify_one();
        while session
            .upgrade()
            .is_some_and(|session| !session.output_complete())
        {
            if output.changed().await.is_err() {
                break;
            }
        }
        wake.notify_one();
    });
}

impl Observer {
    /// An observer whose sessions' ends notify `wake`.
    pub(crate) fn waking(wake: Arc<Notify>) -> Self {
        Self {
            wake: Some(wake),
            ..Self::default()
        }
    }

    /// Note `session` under `id`, `exited` or not, watching its end from the
    /// first time it is seen; whether the observer has seen it run.
    fn see(&mut self, id: &str, session: &Arc<dyn SessionHandle>, exited: bool) -> bool {
        let current = Arc::downgrade(session);
        if !self
            .seen
            .get(id)
            .is_some_and(|seen| Weak::ptr_eq(&seen.session, &current))
        {
            if let Some(wake) = &self.wake {
                watch_end(session, Arc::clone(wake));
            }
            let seen = Seen {
                session: current,
                ran: false,
            };
            self.seen.insert(id.to_owned(), seen);
        }
        let seen = self.seen.get_mut(id).expect("seen session");
        seen.ran |= !exited;
        seen.ran
    }

    pub(crate) async fn refresh(&mut self, state: &super::State, now: Instant) {
        let _publication = state.client_state.effect_runner.publication.lock().await;
        let workspace = state.mux_store.get_workspace_state();
        let backend = state.client_state.dispatcher.backend();
        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let policy = state
            .config
            .read()
            .expect("settings")
            .terminal
            .close_on_exit
            .clone();
        let known = |id: &String| {
            workspace
                .screens
                .values()
                .any(|screen| screen.sessions.contains_key(&SessionId::from(id.as_str())))
        };
        self.exits.retain(|id, _| known(id));
        self.seen.retain(|id, _| known(id));
        for screen in workspace.screens.values() {
            for entry in screen.sessions.values() {
                let Some(session) = backend.get(entry.id.as_str()) else {
                    continue;
                };
                let info = session.title();
                let osc = session.osc_title_watch().borrow().title.clone();
                let exited = *session.done().borrow();
                let ran = self.see(entry.id.as_str(), &session, exited);
                let exit_code = session
                    .process_result()
                    .and_then(|result| result.exit_code)
                    .and_then(|code| i32::try_from(code).ok());
                let close = if exited {
                    let since = *self.exits.entry(entry.id.to_string()).or_insert(now);
                    // An exit seen to happen closes once the output has ended,
                    // which it does after its last byte is recorded.
                    ((ran && session.output_complete())
                        || now.saturating_duration_since(since) >= DRAIN_GRACE)
                        && (policy == "always" || (policy == "graceful" && exit_code == Some(0)))
                        && entry.job_id.is_none()
                        && entry.restart_policy != splitty_core::mux::jobs::RestartPolicy::Never
                } else {
                    false
                };
                let display_title = info.display_title(&osc, &home);
                let busy = !exited && info.busy();
                if entry.busy && !busy {
                    // The shell is the foreground process again: the program
                    // that ran ended, and its working, blocked and idle status
                    // records end with it (program-status).
                    session.end_program_status();
                }
                let status = if exited {
                    splitty_core::mux::SessionStatus::Exited
                } else {
                    splitty_core::mux::SessionStatus::Running
                };
                let managed = entry.job_id.is_some()
                    || entry.restart_policy == splitty_core::mux::jobs::RestartPolicy::Never;
                if !close
                    && entry.display_title.as_deref() == Some(&display_title)
                    && (info.cwd.is_empty() || entry.cwd.as_deref() == Some(&info.cwd))
                    && entry.busy == busy
                    && (managed || (entry.status == status && entry.exit_code == exit_code))
                {
                    continue;
                }
                let op = Op::UpdateSessionMetadata {
                    session_id: entry.id.clone(),
                    display_title,
                    cwd: info.cwd.clone(),
                    busy,
                    exited,
                    exit_code,
                    close,
                };
                let Ok((effects, (screens, removed))) =
                    state.mux_store.dispatch_inspect(op, |before, after| {
                        let changed = after
                            .screens
                            .values()
                            .filter(|screen| {
                                before
                                    .screens
                                    .get(&screen.id)
                                    .is_some_and(|old| old.revision != screen.revision)
                            })
                            .map(|screen| (screen.id.clone(), screen.revision))
                            .collect::<Vec<_>>();
                        // A screen whose last pane closed on exit went with it.
                        let removed = before
                            .screens
                            .keys()
                            .any(|id| !after.screens.contains_key(id));
                        (changed, removed)
                    })
                else {
                    continue;
                };
                state
                    .client_state
                    .effect_runner
                    .execute(effects, &state.client_state.env)
                    .await;
                if exited {
                    let mut event = ServerEvent::title_changed(
                        entry.id.to_string(),
                        info.proc,
                        info.cwd,
                        Some(info.cmdline),
                        info.base_shell,
                        Some(osc),
                    );
                    if let ServerEvent::TitleChanged { busy, .. } = &mut event {
                        *busy = false;
                    }
                    state.client_state.notify_tx.send(&event);
                }
                // Job lifecycle already publishes its own ordered spawn/nudge batch.
                for (screen, _) in screens.into_iter().filter(|_| !managed) {
                    state.client_state.notify_tx.send(&ServerEvent::mux_nudge(
                        "session_metadata".into(),
                        Some(screen.to_string()),
                    ));
                }
                if removed {
                    // Without a screen ID: the workspace's screens changed, and
                    // a client reads them again instead of a screen that is gone.
                    state
                        .client_state
                        .notify_tx
                        .send(&ServerEvent::mux_nudge("session_metadata".into(), None));
                }
            }
        }
    }
}
