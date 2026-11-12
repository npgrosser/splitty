//! Service-owned process metadata and ordinary pane exit policy.
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use splitty_core::{ids::SessionId, mux::Op, protocol::ServerEvent};

const DRAIN_GRACE: Duration = Duration::from_secs(3);

/// Weak ownership lets the service and its terminals shut down normally.
pub(super) fn start(state: &Arc<super::State>) {
    let weak = Arc::downgrade(state);
    tokio::spawn(async move {
        let mut observer = Observer::default();
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
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
}

impl Observer {
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
        self.exits.retain(|id, _| {
            workspace
                .screens
                .values()
                .any(|screen| screen.sessions.contains_key(&SessionId::from(id.as_str())))
        });
        for screen in workspace.screens.values() {
            for entry in screen.sessions.values() {
                let Some(session) = backend.get(entry.id.as_str()) else {
                    continue;
                };
                let info = session.title();
                let osc = session.osc_title_watch().borrow().title.clone();
                let exited = *session.done().borrow();
                let exit_code = session
                    .process_result()
                    .and_then(|result| result.exit_code)
                    .and_then(|code| i32::try_from(code).ok());
                let close = if exited {
                    let since = *self.exits.entry(entry.id.to_string()).or_insert(now);
                    now.saturating_duration_since(since) >= DRAIN_GRACE
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
                let Ok((effects, screens)) =
                    state.mux_store.dispatch_inspect(op, |before, after| {
                        after
                            .screens
                            .values()
                            .filter(|screen| {
                                before
                                    .screens
                                    .get(&screen.id)
                                    .is_some_and(|old| old.revision != screen.revision)
                            })
                            .map(|screen| (screen.id.clone(), screen.revision))
                            .collect::<Vec<_>>()
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
            }
        }
    }
}
