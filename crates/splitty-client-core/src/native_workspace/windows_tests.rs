//! Several windows of one client on one worker: each reads its own model
//! from the one stream, a Screen moves between them without a detach, and a
//! closed window hands its Screens over.
use super::*;
use crate::workspace::ScreenKey;
use std::time::Duration;

const SECOND: WindowId = WindowId::new(1);

/// What the windows read so far, each from its own part of the stream.
#[derive(Default)]
struct Seen {
    models: HashMap<WindowId, WindowWorkspace>,
    /// Every model in the order it was read, with its window and the number
    /// of Screens it lists; a closed window as `None`.
    order: Vec<(WindowId, Option<usize>)>,
    attached: Vec<(WindowId, String)>,
    output: HashMap<WindowId, String>,
    statuses: Vec<(WindowId, String)>,
    elsewhere: Vec<(WindowId, ScreenKey, WindowId)>,
    close_settled: Vec<(WindowId, u64)>,
    close_rejected: Vec<(WindowId, u64, String)>,
}
impl Seen {
    fn listed(&self, window: WindowId) -> Vec<String> {
        self.models.get(&window).map_or_else(Vec::new, |model| {
            model
                .opened()
                .iter()
                .map(|entry| entry.key().screen_id().to_owned())
                .collect()
        })
    }
    /// The Screens window `window` lists, each with an attached terminal.
    fn ready(&self, window: WindowId, screens: usize) -> bool {
        self.models.get(&window).is_some_and(|model| {
            let local = model.local_source();
            local.catalog_initialized()
                && model.opened().len() == screens
                && local.routes().len() == screens
                && local.routes().iter().all(|route| {
                    local.session_ready(&route.session_id)
                        && self
                            .attached
                            .iter()
                            .any(|(_, session)| session == &route.session_id)
                })
        })
    }
    fn attaches(&self, session: &str) -> usize {
        self.attached
            .iter()
            .filter(|(_, attached)| attached == session)
            .count()
    }
    fn wrote(&self, window: WindowId, text: &str) -> bool {
        self.output
            .get(&window)
            .is_some_and(|output| output.contains(text))
    }
    fn position(&self, entry: (WindowId, Option<usize>), from: usize) -> Option<usize> {
        self.order[from..]
            .iter()
            .position(|seen| *seen == entry)
            .map(|index| index + from)
    }
}

async fn until(
    updates: &mut mpsc::Receiver<Update>,
    seen: &mut Seen,
    done: impl Fn(&Seen) -> bool,
) {
    while !done(seen) {
        let update = tokio::time::timeout(Duration::from_secs(20), updates.recv())
            .await
            .expect("window update timeout")
            .expect("worker alive");
        let (window, update) = match update {
            Update::ForWindow { window, update } => (window, *update),
            update => (WindowId::FIRST, update),
        };
        match update {
            Update::Workspace(model) => {
                assert_eq!(model.window(), window, "a window reads its own model");
                assert!(model.single_window(), "a window's model is one window's");
                seen.order.push((window, Some(model.opened().len())));
                seen.models.insert(window, *model);
            }
            Update::WindowClosed { window: closed } => {
                assert_eq!(closed, window);
                seen.order.push((window, None));
                seen.models.remove(&window);
            }
            Update::Attached { session, .. } => seen.attached.push((window, session)),
            Update::Event { event, .. } => {
                if let Some(output) = event.output() {
                    seen.output
                        .entry(window)
                        .or_default()
                        .push_str(&String::from_utf8_lossy(output.bytes));
                }
            }
            Update::Status(status) => seen.statuses.push((window, status)),
            Update::ScreenElsewhere {
                screen,
                window: holder,
            } => seen.elsewhere.push((window, screen, holder)),
            Update::CloseSettled { request_id, .. } => {
                seen.close_settled.push((window, request_id))
            }
            Update::CloseRejected {
                request_id, reason, ..
            } => seen.close_rejected.push((window, request_id, reason)),
            _ => {}
        }
    }
}

fn typed(model: &WindowWorkspace, screen: &ScreenKey, text: &str) -> CommandKind {
    let local = model.local_source();
    let route = local
        .routes()
        .into_iter()
        .find(|route| local.screen_for(&route.pane_id) == Some(screen.screen_id()))
        .expect("terminal of the Screen");
    CommandKind::Input {
        route: TerminalToken::new(local.source_token(), &route.pane_id, &route.session_id),
        bytes: format!("printf '%s_%s\\n' {text} DONE\n").into_bytes(),
    }
}

#[tokio::test]
async fn windows_share_one_connection_and_a_moved_screen_stays_attached() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
    let worker = Worker::start_isolated(receiver, sender);
    let mut seen = Seen::default();
    until(&mut updates, &mut seen, |seen| {
        seen.ready(WindowId::FIRST, 1)
    })
    .await;
    let first = seen.models[&WindowId::FIRST].clone();
    let scope = first.lifecycle_token();
    let work = first.shown_key().expect("first Screen");
    let session = first.local_source().routes()[0].session_id.clone();

    // A second window opens with a Screen of its own on the same connection.
    let presence = WindowPresenceCell::default();
    presence.set(WindowPresence {
        visible: true,
        focused: true,
    });
    commands
        .send(Command::new(
            scope,
            CommandKind::OpenWindow {
                window: SECOND,
                presence,
                start: WindowStart::NewScreen,
            },
        ))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| seen.ready(SECOND, 1)).await;
    let second = seen.models[&SECOND].clone();
    let notes = second.shown_key().expect("the new window's Screen");
    assert_ne!(notes, work);
    assert_eq!(second.lifecycle_token(), scope, "no second connection");
    assert_eq!(seen.listed(WindowId::FIRST), [work.screen_id()]);
    assert_eq!(
        seen.models[&WindowId::FIRST].shown_key(),
        Some(work.clone())
    );

    // A Screen another window holds is not shown here; its window is named.
    commands
        .send(Command::new(
            scope,
            CommandKind::SwitchScreen(notes.clone()),
        ))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| !seen.elsewhere.is_empty()).await;
    assert_eq!(seen.elsewhere, [(WindowId::FIRST, notes.clone(), SECOND)]);

    // Output of a Screen reaches the window that holds it, and no other.
    commands
        .send(Command::new(scope, typed(&first, &work, "ONE")))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.wrote(WindowId::FIRST, "ONE_DONE")
    })
    .await;
    assert!(!seen.wrote(SECOND, "ONE_DONE"));

    // The Screen moves: the window it leaves reads its model first, the
    // session is not attached again, and its output follows the Screen.
    let moved_from = seen.order.len();
    commands
        .send(Command::new(
            scope,
            CommandKind::MoveScreenToWindow {
                screen: work.clone(),
                target: SECOND,
            },
        ))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.listed(WindowId::FIRST).is_empty() && seen.listed(SECOND).len() == 2
    })
    .await;
    let left = seen
        .position((WindowId::FIRST, Some(0)), moved_from)
        .expect("the window without the Screen");
    let arrived = seen
        .position((SECOND, Some(2)), moved_from)
        .expect("the window with the Screen");
    assert!(left < arrived, "the giver reads its model first");
    assert_eq!(seen.models[&SECOND].shown_key(), Some(work.clone()));
    assert_eq!(seen.models[&WindowId::FIRST].shown_key(), None);
    commands
        .send(Command::new(scope, typed(&second, &notes, "TWO")).in_window(SECOND))
        .await
        .unwrap();
    commands
        .send(Command::new(scope, typed(&first, &work, "THREE")).in_window(SECOND))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.wrote(SECOND, "TWO_DONE") && seen.wrote(SECOND, "THREE_DONE")
    })
    .await;
    assert!(!seen.wrote(WindowId::FIRST, "THREE_DONE"));
    assert_eq!(seen.attaches(&session), 1, "a move attaches nothing");

    // The second window closes: it hears of it before the first window reads
    // that it holds the Screens, and nothing ends.
    let closed_from = seen.order.len();
    commands
        .send(Command::new(scope, CommandKind::CloseWindow).in_window(SECOND))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.listed(WindowId::FIRST).len() == 2
    })
    .await;
    let closed = seen
        .position((SECOND, None), closed_from)
        .expect("the closed window");
    let inherited = seen
        .position((WindowId::FIRST, Some(2)), closed_from)
        .expect("the heir");
    assert!(closed < inherited, "the closed window hears of it first");
    assert!(seen.models[&WindowId::FIRST].shown_key().is_some());
    commands
        .send(Command::new(scope, typed(&first, &work, "FOUR")))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.wrote(WindowId::FIRST, "FOUR_DONE")
    })
    .await;
    assert_eq!(
        seen.attaches(&session),
        1,
        "a closed window detaches nothing"
    );

    // The last window does not close this way, and a window that is gone
    // commands nothing.
    commands
        .send(Command::new(scope, CommandKind::CloseWindow))
        .await
        .unwrap();
    commands
        .send(
            Command::new(
                scope,
                CommandKind::CycleScreen {
                    step: TabStep::Next,
                },
            )
            .in_window(SECOND),
        )
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.statuses
            .iter()
            .any(|(window, status)| *window == SECOND && status.contains("not open"))
            && seen.statuses.iter().any(|(window, status)| {
                *window == WindowId::FIRST && status.contains("last window")
            })
    })
    .await;
    assert_eq!(seen.listed(WindowId::FIRST).len(), 2);
    worker.shutdown().unwrap();
}

#[tokio::test]
async fn a_screen_from_outside_goes_to_the_window_that_was_active_last() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let embedded = crate::EmbeddedService::isolated();
    let outside = embedded.connect().await.unwrap();
    let service = crate::owned_service::OwnedService::from(embedded);
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        service,
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let mut seen = Seen::default();
    until(&mut updates, &mut seen, |seen| {
        seen.ready(WindowId::FIRST, 1)
    })
    .await;
    let scope = seen.models[&WindowId::FIRST].lifecycle_token();
    // The second window is the active one; the first never was.
    let presence = WindowPresenceCell::default();
    presence.set(WindowPresence {
        visible: true,
        focused: true,
    });
    commands
        .send(Command::new(
            scope,
            CommandKind::OpenWindow {
                window: SECOND,
                presence,
                start: WindowStart::Empty,
            },
        ))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.models.contains_key(&SECOND)
    })
    .await;
    assert!(seen.listed(SECOND).is_empty());

    // Another client creates a Screen; no window here asked for it.
    let created = outside.create_screen("outside").unwrap();
    until(&mut updates, &mut seen, |seen| seen.ready(SECOND, 1)).await;
    assert_eq!(seen.listed(SECOND), [created.screen_id.as_str()]);
    assert_eq!(seen.listed(WindowId::FIRST).len(), 1);
    assert_ne!(seen.listed(WindowId::FIRST), seen.listed(SECOND));

    stop.store(true, Ordering::Release);
    drop(commands);
    let _ = worker.await;
    drop(outside);
}

#[tokio::test]
async fn a_whole_group_of_screens_moves_in_one_step_and_keeps_its_order() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
    let worker = Worker::start_isolated(receiver, sender);
    let mut seen = Seen::default();
    until(&mut updates, &mut seen, |seen| {
        seen.ready(WindowId::FIRST, 1)
    })
    .await;
    let scope = seen.models[&WindowId::FIRST].lifecycle_token();
    for screens in 2..=3 {
        commands
            .send(Command::new(scope, CommandKind::CreateScreen(None)))
            .await
            .unwrap();
        until(&mut updates, &mut seen, |seen| {
            seen.ready(WindowId::FIRST, screens)
        })
        .await;
    }
    let group: Vec<ScreenKey> = seen.models[&WindowId::FIRST]
        .opened()
        .iter()
        .map(|entry| entry.key().clone())
        .collect();
    let listed = seen.listed(WindowId::FIRST);
    let attached = seen.attached.len();

    // The group opens a new window that holds all of it, in its order, and
    // shows its first Screen; the window that gave it lists nothing.
    let from = seen.order.len();
    commands
        .send(Command::new(
            scope,
            CommandKind::OpenWindow {
                window: SECOND,
                presence: WindowPresenceCell::default(),
                start: WindowStart::Screens(group.clone()),
            },
        ))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.listed(SECOND).len() == 3
    })
    .await;
    assert_eq!(seen.listed(SECOND), listed);
    assert_eq!(seen.models[&SECOND].shown_key(), Some(group[0].clone()));
    assert!(seen.listed(WindowId::FIRST).is_empty());
    // One model for each window, not one per Screen.
    assert!(seen.position((SECOND, Some(1)), from).is_none());
    assert!(seen.position((SECOND, Some(2)), from).is_none());

    // And back to the open window, with a Screen that is gone left out.
    let mut back = group.clone();
    back.push(
        seen.models[&SECOND]
            .local_source()
            .screen_key("no-such-screen"),
    );
    commands
        .send(
            Command::new(
                scope,
                CommandKind::MoveScreensToWindow {
                    screens: back,
                    target: WindowId::FIRST,
                },
            )
            .in_window(SECOND),
        )
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.listed(WindowId::FIRST).len() == 3 && seen.listed(SECOND).is_empty()
    })
    .await;
    assert_eq!(seen.listed(WindowId::FIRST), listed);
    assert_eq!(
        seen.models[&WindowId::FIRST].shown_key(),
        Some(group[0].clone())
    );
    assert_eq!(
        seen.attached.len(),
        attached,
        "a group move attaches nothing"
    );
    worker.shutdown().unwrap();
}

#[tokio::test]
async fn closing_from_a_newer_window_does_not_make_an_earlier_windows_closes_stale() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
    let worker = Worker::start_isolated(receiver, sender);
    let mut seen = Seen::default();
    until(&mut updates, &mut seen, |seen| {
        seen.ready(WindowId::FIRST, 1)
    })
    .await;
    let first = seen.models[&WindowId::FIRST].clone();
    let scope = first.lifecycle_token();
    let first_screen = first.shown_key().expect("first Screen");

    // Add a second tab in the first window so closing a pane leaves its Screen open.
    let initial_pane = first
        .local_source()
        .snapshot()
        .expect("first Screen snapshot")
        .panes
        .keys()
        .next()
        .expect("initial pane")
        .to_string();
    commands
        .send(Command::new(
            scope,
            CommandKind::Tab {
                screen: first_screen.clone(),
                pane: initial_pane,
            },
        ))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.models.get(&WindowId::FIRST).is_some_and(|model| {
            model
                .local_source()
                .snapshot()
                .is_some_and(|screen| screen.panes.len() == 2)
        })
    })
    .await;

    // Open a second window with a Screen of its own, then close that Screen
    // using the second window's higher ID space (`SECOND.ordinal() << 32`).
    commands
        .send(Command::new(
            scope,
            CommandKind::OpenWindow {
                window: SECOND,
                presence: WindowPresenceCell::default(),
                start: WindowStart::NewScreen,
            },
        ))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| seen.ready(SECOND, 1)).await;
    let second_model = seen.models[&SECOND].clone();
    let second_screen = second_model.shown_key().expect("second Screen");
    let second_request_id = (SECOND.ordinal() << 32) + 1;
    let second_plan = second_model
        .local_source()
        .assess_close(
            second_request_id,
            CloseTarget::Screen {
                screen: second_screen.screen_id().to_owned(),
            },
        )
        .unwrap();
    commands
        .send(Command::close(second_plan, true).in_window(SECOND))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.close_settled.contains(&(SECOND, second_request_id))
    })
    .await;
    assert!(seen.close_rejected.is_empty());

    // Now the first window closes a pane with its own low ID space (`1`):
    // the second window's higher ID must not make the first window's close stale.
    let first_model = seen.models[&WindowId::FIRST].clone();
    let pane_to_close = first_model
        .local_source()
        .snapshot()
        .expect("first Screen snapshot")
        .panes
        .keys()
        .next()
        .expect("a pane")
        .to_string();
    let first_request_id = (WindowId::FIRST.ordinal() << 32) + 1;
    let first_plan = first_model
        .local_source()
        .assess_close(
            first_request_id,
            CloseTarget::Pane {
                screen: first_screen.screen_id().to_owned(),
                pane: pane_to_close.clone(),
            },
        )
        .unwrap();
    commands
        .send(Command::close(first_plan, true).in_window(WindowId::FIRST))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.close_settled
            .contains(&(WindowId::FIRST, first_request_id))
    })
    .await;
    assert!(
        seen.close_rejected.is_empty(),
        "unexpected close rejections: {:?}",
        seen.close_rejected
    );
    assert!(
        !seen.models[&WindowId::FIRST]
            .local_source()
            .snapshot()
            .expect("first Screen snapshot")
            .panes
            .contains_key(&splitty_core::ids::PaneId::from(pane_to_close.as_str())),
        "the closed pane must be removed"
    );

    // A genuinely stale close ID from the same window is still rejected.
    let remaining_pane = seen.models[&WindowId::FIRST]
        .local_source()
        .snapshot()
        .expect("first Screen snapshot")
        .panes
        .keys()
        .next()
        .expect("remaining pane")
        .to_string();
    let stale_plan = seen.models[&WindowId::FIRST]
        .local_source()
        .assess_close(
            first_request_id,
            CloseTarget::Pane {
                screen: first_screen.screen_id().to_owned(),
                pane: remaining_pane,
            },
        )
        .unwrap();
    commands
        .send(Command::close(stale_plan, true).in_window(WindowId::FIRST))
        .await
        .unwrap();
    until(&mut updates, &mut seen, |seen| {
        seen.close_rejected.iter().any(|(win, id, reason)| {
            *win == WindowId::FIRST && *id == first_request_id && reason.contains("stale")
        })
    })
    .await;

    worker.shutdown().unwrap();
}
