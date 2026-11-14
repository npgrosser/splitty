//! In-process tests of the tab context menu and tab dragging through the real
//! workspace view.
use super::{TabMenuAction, TransferChoice, Workspace};
use crate::workspace::{answer_close_assessment as assess, CloseTarget, Command, Destination};
use gpui_kit::{
    point, px, size, AppContext, Bounds, Entity, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, Point, TestAppContext, VisualTestContext,
};
use splitty_client_core::{
    native_workspace::{Update, Worker},
    workspace::{ClosePlan, MovePlacement, ScreenTransferTarget, WindowWorkspace},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{atomic::AtomicBool, Arc},
};
use tokio::sync::mpsc;
use uuid::Uuid;

type Commands = mpsc::Receiver<splitty_client_core::native_workspace::Command>;

const CWD: &str = "/home/demo/project";

/// One Screen "work" whose group holds `tabs`, each `(pane, busy)`; every
/// session reports `cwd`. A second Screen "other" is a move destination.
fn local_model(owner: Uuid, tabs: &[(&str, bool)], cwd: Option<&str>) -> WindowWorkspace {
    let mut model = WindowWorkspace::new(owner);
    model
        .ui_test_screen_order_fixture(&[("work", "Work"), ("other", "Other")], None)
        .unwrap();
    let token = model.lifecycle_token();
    model
        .source_mut(token.source_id())
        .unwrap()
        .apply_snapshot(owner, &snapshot(tabs, cwd))
        .unwrap();
    model
}

fn snapshot(tabs: &[(&str, bool)], cwd: Option<&str>) -> serde_json::Value {
    let ids: Vec<_> = tabs.iter().map(|(pane, _)| *pane).collect();
    let mut panes = serde_json::Map::new();
    let mut sessions = serde_json::Map::new();
    for (pane, busy) in tabs {
        let session = format!("s-{pane}");
        panes.insert(
            (*pane).into(),
            serde_json::json!({"id": pane, "sessionId": session, "title": format!("tab {pane}")}),
        );
        let mut entry = serde_json::json!({"id": session, "status": "running", "busy": busy});
        if let Some(cwd) = cwd {
            entry["cwd"] = cwd.into();
        }
        sessions.insert(session, entry);
    }
    serde_json::json!({"type":"mux.screen_snapshot","screen":{
        "id":"work","revision":2,
        "root":{"type":"slot","id":"slot","paneIds":ids,"activePaneId":ids[0]},
        "panes":panes,
        "sessions":sessions
    }})
}

fn with_model(
    model: WindowWorkspace,
    run: impl FnOnce(&mut VisualTestContext, Entity<Workspace>, Commands),
) {
    let mut app = TestAppContext::single();
    app.update(gpui_kit::init);
    let content = Rc::new(RefCell::new(None));
    let (_, window) = app.add_window_view({
        let content = content.clone();
        move |window, cx| {
            let view = cx.new(|cx| {
                let (workspace, worker) = Workspace::new_with_worker(
                    false,
                    Arc::new(AtomicBool::new(false)),
                    window,
                    cx,
                    |_, _, _| Worker::ui_test_fixture(),
                );
                worker.shutdown().unwrap();
                workspace
            });
            *content.borrow_mut() = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        }
    });
    let view = content.borrow_mut().take().unwrap();
    window.simulate_resize(size(px(900.), px(600.)));
    let (commands, receiver) = mpsc::channel(32);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.model = Some(model);
            workspace.commands = commands;
            workspace.destination = Destination::Screens;
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });
    run(window, view, receiver);
}

/// Opens the menu of local tab `pane` at `position`.
fn open(window: &mut VisualTestContext, view: &Entity<Workspace>, pane: &str, at: Point<Pixels>) {
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            let model = workspace.model.as_ref().unwrap();
            let token = model.lifecycle_token();
            let screen = model.local_source().screen_key("work");
            workspace.open_tab_context_menu_captured(token, &screen, pane, at, window, cx);
        });
    });
    draw(window);
}

fn draw(window: &mut VisualTestContext) {
    window.update(|window, cx| window.draw(cx).clear(cx));
}

fn press(window: &mut VisualTestContext, position: Point<Pixels>, button: MouseButton) {
    window.simulate_event(MouseDownEvent {
        button,
        position,
        click_count: 1,
        modifiers: Modifiers::default(),
        first_mouse: false,
    });
    window.simulate_event(MouseUpEvent {
        button,
        position,
        click_count: 1,
        modifiers: Modifiers::default(),
    });
    draw(window);
}

fn bounds(window: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    window
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is shown"))
}

fn click(window: &mut VisualTestContext, selector: &'static str) {
    let center = bounds(window, selector).center();
    press(window, center, MouseButton::Left);
}

fn hover(window: &mut VisualTestContext, selector: &'static str) {
    let center = bounds(window, selector).center();
    window.simulate_mouse_move(center, None, Modifiers::default());
    draw(window);
}

fn keys(window: &mut VisualTestContext, keys: &str) {
    window.simulate_keystrokes(keys);
    draw(window);
}

fn menu_open(window: &mut VisualTestContext, view: &Entity<Workspace>) -> bool {
    window.update(|_, cx| view.read(cx).tab_context_menu.is_some())
}

fn next_close(receiver: &mut Commands) -> (ClosePlan, bool) {
    loop {
        let command = receiver.try_recv().expect("assessed close sent");
        if let Command::RequestClose { plan, confirmed } = command.kind() {
            return (plan.clone(), *confirmed);
        }
    }
}

fn no_close(receiver: &mut Commands) {
    while let Ok(command) = receiver.try_recv() {
        assert!(
            !matches!(command.kind(), Command::RequestClose { .. }),
            "no close may be sent"
        );
    }
}

fn closed_pane(plan: &ClosePlan) -> &str {
    match &plan.target {
        CloseTarget::Pane { screen, pane } => {
            assert_eq!(screen, "work");
            pane
        }
        target => panic!("unexpected close target {target:?}"),
    }
}

fn deliver(window: &mut VisualTestContext, view: &Entity<Workspace>, update: Update) {
    let (_sender, mut updates) = mpsc::channel(1);
    window.update(|window, cx| {
        view.update(cx, |workspace, cx| {
            workspace.poll(Some(update), &mut updates, window, cx);
        });
    });
    draw(window);
}

fn settle(window: &mut VisualTestContext, view: &Entity<Workspace>, plan: &ClosePlan) {
    deliver(
        window,
        view,
        Update::CloseSettled {
            source: plan.source_token(),
            request_id: plan.request_id,
        },
    );
}

fn status(window: &mut VisualTestContext, view: &Entity<Workspace>) -> String {
    window.update(|_, cx| view.read(cx).status.clone())
}

#[test]
fn inactive_tab_close_pointer_uses_assessed_captured_pane() {
    let owner = Uuid::new_v4();
    with_model(
        local_model(owner, &[("p0", false), ("p1", false)], None),
        |window, view, mut receiver| {
            open(window, &view, "p1", point(px(120.), px(80.)));
            click(window, "tab-context-close");
            assess(window, &view, &mut receiver);
            let (plan, _) = next_close(&mut receiver);
            assert_eq!(plan.connection_id, Some(owner));
            assert_eq!(closed_pane(&plan), "p1");
            assert!(!menu_open(window, &view));
        },
    );
}

#[test]
fn tab_close_keyboard_and_stale_owner_do_not_redirect() {
    let owner = Uuid::new_v4();
    with_model(
        local_model(owner, &[("p0", false), ("p1", false)], None),
        |window, view, mut receiver| {
            open(window, &view, "p1", point(px(120.), px(80.)));
            keys(window, "enter");
            assess(window, &view, &mut receiver);
            let (plan, _) = next_close(&mut receiver);
            assert_eq!(closed_pane(&plan), "p1");
            deliver(
                window,
                &view,
                Update::CloseSettled {
                    source: plan.source_token(),
                    request_id: plan.request_id,
                },
            );
            open(window, &view, "p1", point(px(120.), px(80.)));
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(local_model(
                        Uuid::new_v4(),
                        &[("p0", false), ("p1", false)],
                        None,
                    ));
                    workspace.choose_close_tab("p1", window, cx);
                });
            });
            no_close(&mut receiver);
        },
    );
}

#[test]
fn entries_follow_the_web_order_with_one_submenu_row() {
    with_model(
        local_model(
            Uuid::new_v4(),
            &[("p0", false), ("p1", false), ("p2", false)],
            Some(CWD),
        ),
        |window, view, _| {
            open(window, &view, "p1", point(px(120.), px(80.)));
            let order = [
                "tab-context-close",
                "tab-context-close-others",
                "tab-context-close-right",
                "tab-context-copy-cwd",
                "tab-context-move",
            ];
            let tops: Vec<_> = order.iter().map(|row| bounds(window, row).top()).collect();
            assert!(tops.windows(2).all(|pair| pair[0] < pair[1]), "{tops:?}");
            assert!(
                window.debug_bounds("tab-context-copy-ssh").is_none(),
                "a local tab has no SSH path"
            );
            assert!(
                window.debug_bounds("tab-context-move-menu").is_none(),
                "destinations stay in the closed submenu"
            );
            assert!(window.debug_bounds("tab-context-transfer-new").is_none());
            let enabled = window.update(|_, cx| {
                let workspace = view.read(cx);
                workspace.tab_menu_enabled(workspace.tab_context_menu.as_ref().unwrap())
            });
            assert_eq!(
                enabled,
                [
                    TabMenuAction::Close,
                    TabMenuAction::CloseOthers,
                    TabMenuAction::CloseRight,
                    TabMenuAction::CopyDirectory,
                    TabMenuAction::MoveToScreen,
                ]
            );
        },
    );
}

#[test]
fn unknown_cwd_and_single_tab_leave_out_and_disable_entries() {
    with_model(
        local_model(Uuid::new_v4(), &[("p0", false)], None),
        |window, view, mut receiver| {
            open(window, &view, "p0", point(px(120.), px(80.)));
            assert!(window.debug_bounds("tab-context-copy-cwd").is_none());
            for disabled in ["tab-context-close-others", "tab-context-close-right"] {
                click(window, disabled);
                assert!(menu_open(window, &view), "{disabled} does nothing");
            }
            no_close(&mut receiver);
            let enabled = window.update(|_, cx| {
                let workspace = view.read(cx);
                workspace.tab_menu_enabled(workspace.tab_context_menu.as_ref().unwrap())
            });
            assert_eq!(enabled, [TabMenuAction::Close, TabMenuAction::MoveToScreen]);
        },
    );
}

#[test]
fn keyboard_skips_disabled_rows_wraps_and_copies_the_directory() {
    with_model(
        local_model(Uuid::new_v4(), &[("p0", false), ("p1", false)], Some(CWD)),
        |window, view, mut receiver| {
            // The last tab has nothing to its right.
            open(window, &view, "p1", point(px(120.), px(80.)));
            let selected = |window: &mut VisualTestContext| {
                window.update(|_, cx| {
                    let menu = view.read(cx).tab_context_menu.as_ref().unwrap();
                    (menu.selected, menu.focus_visible)
                })
            };
            assert_eq!(
                selected(window),
                (Some(TabMenuAction::Close), false),
                "focus starts on the first enabled entry without a visible highlight"
            );
            keys(window, "up");
            assert_eq!(selected(window), (Some(TabMenuAction::MoveToScreen), true));
            keys(window, "home down");
            assert_eq!(selected(window).0, Some(TabMenuAction::CloseOthers));
            keys(window, "down");
            assert_eq!(
                selected(window).0,
                Some(TabMenuAction::CopyDirectory),
                "Close Tabs to the Right is disabled for the last tab"
            );
            keys(window, "end down");
            assert_eq!(selected(window).0, Some(TabMenuAction::Close));
            keys(window, "end up enter");
            assert!(!menu_open(window, &view));
            assert_eq!(
                window.read_from_clipboard().and_then(|item| item.text()),
                Some(CWD.to_owned())
            );
            no_close(&mut receiver);
        },
    );
}

#[test]
fn close_other_tabs_closes_idle_tabs_one_after_another() {
    with_model(
        local_model(
            Uuid::new_v4(),
            &[("p0", false), ("p1", false), ("p2", false)],
            None,
        ),
        |window, view, mut receiver| {
            open(window, &view, "p1", point(px(120.), px(80.)));
            click(window, "tab-context-close-others");
            assess(window, &view, &mut receiver);
            assert!(
                !window.has_pending_prompt(),
                "idle tabs need no confirmation"
            );
            let (first, confirmed) = next_close(&mut receiver);
            assert_eq!(
                closed_pane(&first),
                "p2",
                "right to left, as the web client"
            );
            assert!(!confirmed);
            no_close(&mut receiver);
            settle(window, &view, &first);
            let (second, confirmed) = next_close(&mut receiver);
            assert_eq!(closed_pane(&second), "p0");
            assert!(!confirmed);
            assert!(second.request_id > first.request_id);
            assert_eq!(second.source_token(), first.source_token());
            settle(window, &view, &second);
            no_close(&mut receiver);
            assert!(window.update(|_, cx| view.read(cx).pending_close.is_none()));
        },
    );
}

#[test]
fn close_tabs_to_the_right_leaves_earlier_tabs() {
    with_model(
        local_model(
            Uuid::new_v4(),
            &[("p0", false), ("p1", false), ("p2", false)],
            None,
        ),
        |window, view, mut receiver| {
            open(window, &view, "p0", point(px(120.), px(80.)));
            click(window, "tab-context-close-right");
            assess(window, &view, &mut receiver);
            let (first, _) = next_close(&mut receiver);
            assert_eq!(closed_pane(&first), "p2");
            settle(window, &view, &first);
            let (second, _) = next_close(&mut receiver);
            assert_eq!(closed_pane(&second), "p1");
            settle(window, &view, &second);
            no_close(&mut receiver);
        },
    );
}

#[test]
fn busy_tabs_get_one_confirmation_before_any_close() {
    with_model(
        local_model(
            Uuid::new_v4(),
            &[("p0", true), ("p1", false), ("p2", true)],
            None,
        ),
        |window, view, mut receiver| {
            open(window, &view, "p1", point(px(120.), px(80.)));
            click(window, "tab-context-close-others");
            assess(window, &view, &mut receiver);
            let (title, detail) = window.pending_prompt().expect("one confirmation");
            assert_eq!(title, "Close 2 tabs?");
            assert!(detail.contains("Running sessions: 2"), "{detail}");
            assert!(
                detail.contains("tab p0") && detail.contains("tab p2"),
                "{detail}"
            );
            no_close(&mut receiver);
            window.simulate_prompt_answer("Keep open");
            window.run_until_parked();
            no_close(&mut receiver);
            assert!(window.update(|_, cx| view.read(cx).pending_close.is_none()));

            open(window, &view, "p1", point(px(120.), px(80.)));
            click(window, "tab-context-close-others");
            assess(window, &view, &mut receiver);
            window.simulate_prompt_answer("Close");
            window.run_until_parked();
            let (first, confirmed) = next_close(&mut receiver);
            assert_eq!(closed_pane(&first), "p2");
            assert!(confirmed);
            no_close(&mut receiver);
            settle(window, &view, &first);
            let (second, confirmed) = next_close(&mut receiver);
            assert_eq!(closed_pane(&second), "p0");
            assert!(confirmed, "the one confirmation covers every queued tab");
            assert!(!window.has_pending_prompt());
        },
    );
}

#[test]
fn a_rejected_close_leaves_the_remaining_tabs_open() {
    with_model(
        local_model(
            Uuid::new_v4(),
            &[("p0", false), ("p1", false), ("p2", false)],
            None,
        ),
        |window, view, mut receiver| {
            open(window, &view, "p1", point(px(120.), px(80.)));
            click(window, "tab-context-close-others");
            assess(window, &view, &mut receiver);
            let (first, _) = next_close(&mut receiver);
            deliver(
                window,
                &view,
                Update::CloseRejected {
                    source: first.source_token(),
                    request_id: first.request_id,
                    reason: "close impact changed; request close again".into(),
                },
            );
            no_close(&mut receiver);
            assert!(window.update(|_, cx| view.read(cx).pending_close.is_none()));
            assert!(status(window, &view).ends_with("1 tab stayed open"));
        },
    );
}

/// session-scratchpad, Confirmation Before Text Is Lost: the native window asks
/// from the server's assessment, so an idle tab with scratchpad text asks.
#[test]
fn an_idle_tab_with_scratchpad_text_asks_before_it_closes() {
    let owner = Uuid::new_v4();
    let mut model = local_model(owner, &[("p0", false), ("p1", false)], None);
    let mut notes = snapshot(&[("p0", false), ("p1", false)], None);
    notes["screen"]["revision"] = 3.into();
    notes["screen"]["sessions"]["s-p1"]["scratchpad"] = "next: run the migration".into();
    let token = model.lifecycle_token();
    model
        .source_mut(token.source_id())
        .unwrap()
        .apply_snapshot(owner, &notes)
        .unwrap();
    with_model(model, |window, view, mut receiver| {
        open(window, &view, "p1", point(px(120.), px(80.)));
        click(window, "tab-context-close");
        assess(window, &view, &mut receiver);
        let (_, detail) = window.pending_prompt().expect("a confirmation");
        assert!(detail.contains("1 scratchpad holds text"), "{detail}");
        no_close(&mut receiver);
        window.simulate_prompt_answer("Close");
        window.run_until_parked();
        let (plan, confirmed) = next_close(&mut receiver);
        assert_eq!(closed_pane(&plan), "p1");
        assert!(confirmed);
    });
}

/// client-connections, Native Close Consent Scope: a queued tab whose
/// session changed after the unconfirmed decision is not closed.
#[test]
fn a_tab_whose_session_changed_after_an_unconfirmed_decision_stays_open() {
    let owner = Uuid::new_v4();
    with_model(
        local_model(owner, &[("p0", false), ("p1", false), ("p2", false)], None),
        |window, view, mut receiver| {
            open(window, &view, "p1", point(px(120.), px(80.)));
            click(window, "tab-context-close-others");
            assess(window, &view, &mut receiver);
            let (first, _) = next_close(&mut receiver);
            window.update(|_, cx| {
                view.update(cx, |workspace, _| {
                    let model = workspace.model.as_mut().unwrap();
                    let token = model.lifecycle_token();
                    let mut busy = snapshot(&[("p0", true), ("p1", false)], None);
                    busy["screen"]["revision"] = 3.into();
                    busy["screen"]["panes"]["p0"]["sessionId"] = "s-p0-replaced".into();
                    busy["screen"]["sessions"]["s-p0-replaced"] =
                        busy["screen"]["sessions"]["s-p0"].clone();
                    busy["screen"]["sessions"]["s-p0-replaced"]["id"] = "s-p0-replaced".into();
                    model
                        .source_mut(token.source_id())
                        .unwrap()
                        .apply_snapshot(owner, &busy)
                        .unwrap();
                });
            });
            settle(window, &view, &first);
            no_close(&mut receiver);
            assert!(status(window, &view).ends_with("1 tab stayed open"));
        },
    );
}

#[test]
fn submenu_opens_on_hover_beside_its_row_and_closes_on_other_rows() {
    with_model(
        local_model(Uuid::new_v4(), &[("p0", false), ("p1", false)], Some(CWD)),
        |window, view, mut receiver| {
            open(window, &view, "p0", point(px(120.), px(80.)));
            let panel = bounds(window, "tab-context-menu");
            assert_eq!(
                panel.origin,
                point(px(120.), px(80.)),
                "opens at the pointer"
            );
            hover(window, "tab-context-move");
            let row = bounds(window, "tab-context-move");
            let submenu = bounds(window, "tab-context-move-menu");
            assert_eq!(submenu.left(), panel.right() + px(4.));
            assert_eq!(submenu.top(), row.top());
            let other = bounds(window, "tab-context-transfer-other");
            let new = bounds(window, "tab-context-transfer-new");
            assert!(other.top() < new.top(), "existing Screens come first");
            hover(window, "tab-context-transfer-new");
            assert!(window.debug_bounds("tab-context-move-menu").is_some());
            hover(window, "tab-context-copy-cwd");
            assert!(
                window.debug_bounds("tab-context-move-menu").is_none(),
                "a row without a submenu closes it"
            );
            hover(window, "tab-context-move");
            click(window, "tab-context-transfer-other");
            assert!(!menu_open(window, &view));
            let command = receiver.try_recv().expect("transfer sent");
            match command.kind() {
                Command::TransferPane { source, target, .. } => {
                    assert_eq!(source.pane_id, "p0");
                    assert!(matches!(
                        target,
                        ScreenTransferTarget::Existing { screen } if screen.screen_id() == "other"
                    ));
                }
                _ => panic!("expected a Screen transfer"),
            }
        },
    );
}

#[test]
fn submenu_keyboard_enters_leaves_and_chooses() {
    with_model(
        local_model(Uuid::new_v4(), &[("p0", false), ("p1", false)], None),
        |window, view, mut receiver| {
            open(window, &view, "p0", point(px(120.), px(80.)));
            let state = |window: &mut VisualTestContext| {
                window.update(|_, cx| {
                    let menu = view.read(cx).tab_context_menu.as_ref().unwrap();
                    (
                        menu.selected,
                        menu.transfer
                            .as_ref()
                            .map(|transfer| transfer.focused.clone()),
                    )
                })
            };
            keys(window, "right");
            assert_eq!(state(window).1, None, "Close Tab has no submenu");
            keys(window, "end");
            assert_eq!(state(window), (Some(TabMenuAction::MoveToScreen), None));
            keys(window, "enter");
            assert_eq!(
                state(window).1,
                Some(None),
                "Enter opens the submenu and keeps the focus on its row"
            );
            keys(window, "right");
            assert_eq!(
                state(window).1,
                Some(Some(TransferChoice::Existing("other".into())))
            );
            keys(window, "down");
            assert_eq!(state(window).1, Some(Some(TransferChoice::New)));
            keys(window, "down");
            assert_eq!(
                state(window).1,
                Some(Some(TransferChoice::Existing("other".into()))),
                "arrows wrap inside the submenu"
            );
            keys(window, "left");
            assert_eq!(state(window), (Some(TabMenuAction::MoveToScreen), None));
            assert!(window.debug_bounds("tab-context-move-menu").is_none());
            keys(window, "right escape");
            assert_eq!(state(window), (Some(TabMenuAction::MoveToScreen), None));
            assert!(
                menu_open(window, &view),
                "Escape in a submenu returns to its parent"
            );
            keys(window, "right up");
            assert_eq!(state(window).1, Some(Some(TransferChoice::New)));
            keys(window, "up");
            keys(window, "end");
            assert_eq!(state(window).1, Some(Some(TransferChoice::New)));
            keys(window, "enter");
            assert!(!menu_open(window, &view));
            let command = receiver.try_recv().expect("transfer sent");
            assert!(matches!(
                command.kind(),
                Command::TransferPane {
                    target: ScreenTransferTarget::New { .. },
                    ..
                }
            ));
        },
    );
}

#[test]
fn moving_the_parent_focus_off_its_row_closes_the_submenu() {
    with_model(
        local_model(Uuid::new_v4(), &[("p0", false), ("p1", false)], None),
        |window, view, _| {
            open(window, &view, "p0", point(px(120.), px(80.)));
            hover(window, "tab-context-move");
            assert!(window.debug_bounds("tab-context-move-menu").is_some());
            keys(window, "end");
            assert!(
                window.debug_bounds("tab-context-move-menu").is_some(),
                "focus onto the submenu's own row keeps it open"
            );
            keys(window, "up");
            assert!(window.debug_bounds("tab-context-move-menu").is_none());
        },
    );
}

#[test]
fn menus_shift_inside_the_window_and_submenus_flip_left() {
    with_model(
        local_model(Uuid::new_v4(), &[("p0", false), ("p1", false)], None),
        |window, view, _| {
            open(window, &view, "p0", point(px(880.), px(590.)));
            let panel = bounds(window, "tab-context-menu");
            assert_eq!(panel.right(), px(892.), "shifted 8px inside the right edge");
            assert_eq!(
                panel.bottom(),
                px(592.),
                "shifted 8px inside the bottom edge"
            );
            hover(window, "tab-context-move");
            let submenu = bounds(window, "tab-context-move-menu");
            assert_eq!(
                submenu.right(),
                panel.left() - px(4.),
                "without room on the right it opens on the left"
            );
            assert!(submenu.bottom() <= px(592.));
        },
    );
}

#[test]
fn outside_presses_escape_and_tab_close_the_menu() {
    with_model(
        local_model(Uuid::new_v4(), &[("p0", false), ("p1", false)], None),
        |window, view, mut receiver| {
            open(window, &view, "p0", point(px(120.), px(80.)));
            press(window, point(px(600.), px(400.)), MouseButton::Left);
            assert!(!menu_open(window, &view));
            open(window, &view, "p0", point(px(120.), px(80.)));
            press(window, point(px(600.), px(400.)), MouseButton::Right);
            assert!(
                !menu_open(window, &view),
                "a right press outside only closes it"
            );
            open(window, &view, "p0", point(px(120.), px(80.)));
            let panel = bounds(window, "tab-context-menu");
            press(
                window,
                panel.origin + point(px(2.), px(2.)),
                MouseButton::Left,
            );
            assert!(
                menu_open(window, &view),
                "a press on the panel itself keeps it"
            );
            keys(window, "escape");
            assert!(!menu_open(window, &view));
            open(window, &view, "p0", point(px(120.), px(80.)));
            keys(window, "tab");
            assert!(!menu_open(window, &view));
            open(window, &view, "p0", point(px(120.), px(80.)));
            keys(window, "shift-tab");
            assert!(!menu_open(window, &view));
            no_close(&mut receiver);
        },
    );
}

#[test]
fn remote_tab_copies_its_ssh_path_and_can_move_to_a_screen() {
    with_model(
        WindowWorkspace::new(Uuid::new_v4()),
        |window, view, mut receiver| {
            let mut remote = WindowWorkspace::new(Uuid::new_v4());
            let (screen, terminal) = remote
                .ui_test_remote_terminal_fixture("demo@192.0.2.9")
                .unwrap();
            let source = terminal.source_token();
            remote
                .source_mut(source.source_id())
                .unwrap()
                .apply_snapshot(
                    source.connection_id(),
                    &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                        "id":"work","revision":2,
                        "root":{"type":"slot","id":"slot","paneIds":["p0"],"activePaneId":"p0"},
                        "panes":{"p0":{"id":"p0","sessionId":"s-p0"}},
                        "sessions":{"s-p0":{"id":"s-p0","status":"running","cwd":"/srv/app"}}
                    }}),
                )
                .unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(remote);
                    workspace.open_tab_context_menu_captured(
                        source,
                        &screen,
                        "p0",
                        point(px(120.), px(80.)),
                        window,
                        cx,
                    );
                });
            });
            draw(window);
            assert!(window.debug_bounds("tab-context-move").is_some());
            assert!(
                bounds(window, "tab-context-copy-cwd").top()
                    < bounds(window, "tab-context-copy-ssh").top()
            );
            click(window, "tab-context-copy-ssh");
            assert_eq!(
                window.read_from_clipboard().and_then(|item| item.text()),
                Some("demo@192.0.2.9:/srv/app".to_owned())
            );
            no_close(&mut receiver);

            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.open_tab_context_menu_captured(
                        source,
                        &screen,
                        "p0",
                        point(px(120.), px(80.)),
                        window,
                        cx,
                    );
                });
            });
            draw(window);
            click(window, "tab-context-close");
            assess(window, &view, &mut receiver);
            let (plan, _) = next_close(&mut receiver);
            assert_eq!(plan.source_token(), Some(source));
            assert_eq!(closed_pane(&plan), "p0");
        },
    );
}

#[test]
fn dragged_tab_preview_starts_just_below_the_pointer() {
    let owner = Uuid::new_v4();
    with_model(
        local_model(owner, &[("p0", false), ("p1", false)], None),
        |window, _, _| {
            // Grabbed off-center, the preview still keeps the same place
            // beside the pointer, clear of the marked drop target under it.
            let tab = bounds(window, "tab-title-p0-tab p0");
            let start = point(tab.left() + px(3.), tab.top() + px(2.));
            let to = start + point(px(40.), px(60.));
            window.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            window.simulate_mouse_move(
                start + point(px(9.), px(0.)),
                MouseButton::Left,
                Modifiers::default(),
            );
            window.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            draw(window);
            let preview = bounds(window, "tab-drag-preview");
            let expected = to + point(px(-30.), px(4.));
            assert!(
                (preview.left() - expected.x).abs() <= px(1.)
                    && (preview.top() - expected.y).abs() <= px(1.),
                "preview at {:?}, expected {expected:?}",
                preview.origin
            );
            window.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        },
    );
}

/// Screen "work" with two groups side by side: `left` holding p0 and p1,
/// showing p1, and `right` holding p2.
fn two_groups(owner: Uuid) -> WindowWorkspace {
    let mut model = WindowWorkspace::new(owner);
    model
        .ui_test_screen_order_fixture(&[("work", "Work"), ("other", "Other")], None)
        .unwrap();
    let token = model.lifecycle_token();
    let pane = |id: &str| serde_json::json!({"id": id, "sessionId": format!("s-{id}"), "title": format!("tab {id}")});
    let session = |id: &str| serde_json::json!({"id": format!("s-{id}"), "status": "running"});
    model
        .source_mut(token.source_id())
        .unwrap()
        .apply_snapshot(
            owner,
            &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                "id":"work","revision":2,
                "root":{"type":"split","id":"root","axis":"horizontal","sizes":[0.5,0.5],"children":[
                    {"type":"slot","id":"left","paneIds":["p0","p1"],"activePaneId":"p1"},
                    {"type":"slot","id":"right","paneIds":["p2"],"activePaneId":"p2"}]},
                "panes":{"p0":pane("p0"),"p1":pane("p1"),"p2":pane("p2")},
                "sessions":{"s-p0":session("p0"),"s-p1":session("p1"),"s-p2":session("p2")}
            }}),
        )
        .unwrap();
    model
}

fn drag(window: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
    window.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    window.simulate_mouse_move(
        from + point(px(9.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    window.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    draw(window);
    window.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    draw(window);
}

/// workspace-model, Native Tab Group Move: the empty area of a tab bar drags
/// the whole group; a drop into another group sends the move once, and a
/// drop onto its own group or a press without a drag sends nothing.
#[test]
fn the_empty_tab_bar_area_drags_the_whole_group() {
    with_model(two_groups(Uuid::new_v4()), |window, _, mut receiver| {
        let grip = bounds(window, "group-grip-left");
        let right = bounds(window, "group-grip-right");
        let from = grip.center();
        press(window, from, MouseButton::Left);
        assert!(receiver.try_recv().is_err(), "a press sends nothing");
        drag(
            window,
            from,
            point(grip.center().x, grip.bottom() + px(200.)),
        );
        assert!(receiver.try_recv().is_err(), "its own group is no target");
        let body = point(right.center().x, right.bottom() + px(200.));
        drag(window, from, body);
        let command = receiver.try_recv().expect("group move sent");
        match command.kind() {
            Command::MoveGroup {
                drag,
                target,
                placement,
                before,
            } => {
                assert_eq!(drag.slot_id(), "left");
                assert_eq!(drag.shown_pane(), "p1");
                assert_eq!(target, "right");
                assert_eq!(*placement, MovePlacement::Tab);
                assert_eq!(*before, None);
            }
            _ => panic!("expected a group move"),
        }
        assert!(receiver.try_recv().is_err(), "sent once");
    });
}

/// A group dropped on another group's right edge zone goes right of it.
#[test]
fn a_group_dropped_on_an_edge_zone_goes_beside_that_group() {
    with_model(two_groups(Uuid::new_v4()), |window, _, mut receiver| {
        let grip = bounds(window, "group-grip-left");
        let right = bounds(window, "group-grip-right");
        let edge = point(right.right() - px(10.), right.bottom() + px(200.));
        drag(window, grip.center(), edge);
        let command = receiver.try_recv().expect("group move sent");
        assert!(matches!(
            command.kind(),
            Command::MoveGroup {
                target,
                placement: MovePlacement::Right,
                ..
            } if target == "right"
        ));
    });
}
