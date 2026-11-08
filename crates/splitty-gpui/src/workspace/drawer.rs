//! Window-local Screen catalog presentation. The service catalog remains authoritative.
use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    Disableable, Icon, Sizable,
};
use splitty_client_core::workspace::ScreenState;

#[derive(Clone)]
pub(super) struct ScreenDrag {
    owner: SourceToken,
    screen: ScreenKey,
    view: EntityId,
}

#[derive(Clone)]
pub(super) struct ScreenDrop {
    drag: ScreenKey,
    row: ScreenKey,
    before: Option<ScreenKey>,
    after_row: bool,
    bounds: Bounds<Pixels>,
    scroll_offset: Point<Pixels>,
    order: Vec<ScreenKey>,
}

struct ScreenDragPreview {
    label: String,
    appearance: NativeAppearance,
}

#[derive(Clone)]
struct ScreenMoveOption {
    before: Option<Option<ScreenKey>>,
    enabled: bool,
}

impl Render for ScreenDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .h(chrome_rems(30.))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(self.appearance.border))
            .bg(rgb(self.appearance.raised))
            .text_color(rgb(self.appearance.text))
            .text_size(chrome_rems(12.5))
            .child(self.label.clone())
    }
}

fn screen_move_items(
    menu: PopupMenu,
    weak: WeakEntity<Workspace>,
    owner: Option<SourceToken>,
    screen: ScreenKey,
    up: ScreenMoveOption,
    down: ScreenMoveOption,
) -> PopupMenu {
    let up_weak = weak.clone();
    let up_screen = screen.clone();
    menu.item(
        PopupMenuItem::new("Move Up")
            .disabled(!up.enabled)
            .on_click(move |_, _, cx| {
                if let (Some(owner), Some(before)) = (owner, up.before.clone()) {
                    let _ = up_weak.update(cx, |this, cx| {
                        this.place_screen(owner, up_screen.clone(), before, cx)
                    });
                }
            }),
    )
    .item(
        PopupMenuItem::new("Move Down")
            .disabled(!down.enabled)
            .on_click(move |_, _, cx| {
                if let (Some(owner), Some(before)) = (owner, down.before.clone()) {
                    let _ = weak.update(cx, |this, cx| {
                        this.place_screen(owner, screen.clone(), before, cx)
                    });
                }
            }),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WheelTarget {
    Screens,
    Slot { screen: String, slot: String },
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use super::{Command, Destination, DropTarget, Workspace};
    use gpui_kit::{
        point, px, size, AppContext, Entity, Focusable, Modifiers, MouseButton, MouseDownEvent,
        MouseUpEvent, Pixels, Point, TestAppContext, VisualTestContext,
    };
    use splitty_client_core::{
        appearance::{
            test_support::{settings_snapshot_with_pin, Availability},
            DrawerVisibilityOutcome, DrawerVisibilityResult,
        },
        native_workspace::Worker,
        workspace::{MovePlacement, WindowWorkspace},
    };
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
        time::Duration,
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn with_view(run: impl FnOnce(&mut VisualTestContext, Entity<Workspace>)) {
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
        run(window, view);
    }

    fn mouse_click(
        window: &mut VisualTestContext,
        position: Point<Pixels>,
        button: MouseButton,
        count: usize,
    ) {
        window.simulate_event(MouseDownEvent {
            button,
            position,
            click_count: count,
            modifiers: Modifiers::default(),
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            button,
            position,
            click_count: count,
            modifiers: Modifiers::default(),
        });
    }

    fn redraw(window: &mut VisualTestContext) {
        window.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn advance(window: &mut VisualTestContext, duration: Duration) {
        window.executor().advance_clock(duration);
        redraw(window);
    }

    fn drag_screen(
        window: &mut VisualTestContext,
        from: &splitty_client_core::workspace::ScreenKey,
        to: &splitty_client_core::workspace::ScreenKey,
        after: bool,
    ) {
        let from_selector: &'static str =
            Box::leak(format!("drawer-select-{from}").into_boxed_str());
        let to_selector: &'static str = Box::leak(format!("drawer-block-{to}").into_boxed_str());
        let start = window.debug_bounds(from_selector).unwrap().center();
        let bounds = window.debug_bounds(to_selector).unwrap();
        let finish = point(
            bounds.center().x,
            if after {
                bounds.bottom() - px(3.)
            } else {
                bounds.top() + px(3.)
            },
        );
        window.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        window.simulate_mouse_move(
            start + point(px(9.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        window.simulate_mouse_move(finish, MouseButton::Left, Modifiers::default());
        redraw(window);
        window.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
    }

    fn hover_screen_drop(
        window: &mut VisualTestContext,
        from: &splitty_client_core::workspace::ScreenKey,
        to: &splitty_client_core::workspace::ScreenKey,
    ) -> Point<Pixels> {
        let from_selector: &'static str =
            Box::leak(format!("drawer-select-{from}").into_boxed_str());
        let to_selector: &'static str = Box::leak(format!("drawer-block-{to}").into_boxed_str());
        let start = window.debug_bounds(from_selector).unwrap().center();
        let bounds = window.debug_bounds(to_selector).unwrap();
        let finish = point(bounds.center().x, bounds.top() + px(3.));
        window.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        window.simulate_mouse_move(
            start + point(px(9.), px(0.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        window.simulate_mouse_move(finish, MouseButton::Left, Modifiers::default());
        redraw(window);
        finish
    }

    #[test]
    fn screen_drag_submits_one_relative_move_without_navigation() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                .unwrap();
            let shown = model.shown_key();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            drag_screen(window, &keys[2], &keys[0], false);
            let command = receiver.try_recv().expect("one relative placement");
            assert!(matches!(
                command.kind(),
                Command::MoveScreen { screen, before }
                    if screen == &keys[2] && before.as_ref() == Some(&keys[0])
            ));
            assert!(receiver.try_recv().is_err(), "one command per release");
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert_eq!(workspace.model.as_ref().unwrap().shown_key(), shown);
                assert!(workspace.drawer_drop.is_none());
            });
        });
    }

    #[test]
    fn screen_drag_places_before_middle_and_at_current_end() {
        for (from, target, after, expected_before) in [(2, 1, false, Some(1)), (0, 2, true, None)] {
            with_view(|window, view| {
                let mut model = WindowWorkspace::new(Uuid::new_v4());
                let keys = model
                    .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                    .unwrap();
                let (commands, mut receiver) = mpsc::channel(2);
                window.update(|window, cx| {
                    view.update(cx, |workspace, _| {
                        workspace.commands = commands;
                        workspace.model = Some(model);
                        workspace.destination = Destination::Screens;
                    });
                    window.draw(cx).clear(cx);
                });
                drag_screen(window, &keys[from], &keys[target], after);
                let sent = receiver.try_recv().expect("relative move admitted");
                assert!(matches!(
                    sent.kind(),
                    Command::MoveScreen { screen, before }
                        if screen == &keys[from]
                            && before.as_ref() == expected_before.map(|index| &keys[index])
                ));
                assert!(receiver.try_recv().is_err());
            });
        }
    }

    #[test]
    fn screen_index_and_name_keep_web_drawer_spacing() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "Screen 1"), ("b", "Screen 2")], None)
                .unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            for key in keys {
                let index_selector: &'static str =
                    Box::leak(format!("drawer-index-{key}").into_boxed_str());
                let name_selector: &'static str =
                    Box::leak(format!("drawer-select-{key}").into_boxed_str());
                let index = window.debug_bounds(index_selector).unwrap();
                let name = window.debug_bounds(name_selector).unwrap();
                assert!(
                    f32::from(name.left() - index.right()) >= 7.,
                    "visible gap separates Screen index and name"
                );
            }
        });
    }

    #[test]
    fn screen_drag_rejects_noop_cross_group_and_stale_geometry() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let local = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                .unwrap();
            let remote = model
                .ui_test_screen_order_fixture(&[("r1", "R1"), ("r2", "R2")], Some("vm.test"))
                .unwrap();
            let remote_id = remote[0].source_id();
            model.disconnect(remote_id).unwrap();
            model.finish_attempt(remote_id).unwrap();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            drag_screen(window, &local[1], &local[0], true);
            assert!(
                receiver.try_recv().is_err(),
                "already-positioned drop is a no-op"
            );
            drag_screen(window, &local[2], &remote[0], false);
            assert!(receiver.try_recv().is_err(), "cannot cross a host group");
            let finish = hover_screen_drop(window, &local[2], &local[0]);
            assert!(window.update(|_, cx| view.read(cx).drawer_drop.is_some()));
            window.simulate_resize(size(px(900.), px(620.)));
            window.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
            assert!(
                receiver.try_recv().is_err(),
                "resize invalidates measured row"
            );
            redraw(window);
            drag_screen(window, &remote[1], &remote[0], false);
            let sent = receiver
                .try_recv()
                .expect("offline remote references arrange locally");
            assert!(matches!(
                sent.kind(),
                Command::MoveScreen { screen, before }
                    if screen == &remote[1] && before.as_ref() == Some(&remote[0])
            ));
            assert!(receiver.try_recv().is_err());
            redraw(window);
            let finish = hover_screen_drop(window, &remote[1], &remote[0]);
            window.update(|_, cx| {
                view.update(cx, |workspace, _| {
                    let model = workspace.model.as_mut().unwrap();
                    let owner = model.lifecycle_token();
                    model.detach(owner, &remote[0]).unwrap();
                });
            });
            window.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
            assert!(
                receiver.try_recv().is_err(),
                "removed anchor cannot be reused"
            );
        });
    }

    #[test]
    fn screen_drag_escape_and_owner_replacement_never_send() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(2);
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            let finish = hover_screen_drop(window, &keys[2], &keys[0]);
            window.simulate_keystrokes("escape");
            window.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
            assert!(
                receiver.try_recv().is_err(),
                "Escape cancels before admission"
            );
            redraw(window);
            let finish = hover_screen_drop(window, &keys[2], &keys[0]);
            window.update(|_, cx| {
                view.update(cx, |workspace, _| {
                    workspace.model = Some(WindowWorkspace::new(Uuid::new_v4()));
                });
            });
            window.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
            assert!(
                receiver.try_recv().is_err(),
                "old owner never retargets a move"
            );
        });
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B")], None)
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(1);
            window.update(|window, cx| {
                window.activate_window();
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            let finish = hover_screen_drop(window, &keys[1], &keys[0]);
            window.deactivate_window();
            window.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
            assert!(
                receiver.try_recv().is_err(),
                "deactivation cancels captured drag"
            );
            assert!(window.update(|_, cx| view.read(cx).drawer_drop.is_none()));
        });
    }

    #[test]
    fn screen_context_menu_keyboard_moves_adjacent_identity_and_disables_old_service() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(3);
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            let selector: &'static str =
                Box::leak(format!("drawer-select-{}", keys[1]).into_boxed_str());
            let name = window.debug_bounds(selector).unwrap();
            mouse_click(window, name.center(), MouseButton::Right, 1);
            window.simulate_keystrokes("down down enter");
            let sent = receiver.try_recv().expect("keyboard Move Up admitted");
            assert!(matches!(
                sent.kind(),
                Command::MoveScreen { screen, before }
                    if screen == &keys[1] && before.as_ref() == Some(&keys[0])
            ));
            assert!(receiver.try_recv().is_err());
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace
                        .model
                        .as_mut()
                        .unwrap()
                        .source_mut(keys[1].source_id())
                        .unwrap()
                        .set_screen_placement_supported(false);
                });
                window.draw(cx).clear(cx);
            });
            drag_screen(window, &keys[2], &keys[0], false);
            assert!(
                receiver.try_recv().is_err(),
                "unsupported local drag is inert"
            );
            let name = window.debug_bounds(selector).unwrap();
            mouse_click(window, name.center(), MouseButton::Right, 1);
            window.simulate_keystrokes("down down enter");
            assert!(
                receiver.try_recv().is_err(),
                "disabled menu cannot send fallback"
            );
        });
    }

    #[test]
    fn name_double_click_f2_and_context_menu_enter_exact_local_rename_editor() {
        exercise_name_interactions();
    }

    fn exercise_name_interactions() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let screen = model.ui_test_local_screen_fixture("Work").unwrap();
            let source = model.lifecycle_token();
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            let selector: &'static str =
                Box::leak(format!("drawer-select-{screen}").into_boxed_str());
            let name = window
                .debug_bounds(selector)
                .expect("Screen name is rendered");
            mouse_click(window, name.center(), MouseButton::Left, 1);
            mouse_click(window, name.center(), MouseButton::Left, 2);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let editor = workspace.rename.as_ref().expect("double-click editor");
                    assert_eq!(editor.screen, screen);
                    assert_eq!(editor.source, source);
                    assert!(editor.input.read(cx).focus_handle(cx).is_focused(window));
                    workspace.finish_rename(false, window, cx);
                });
                window.draw(cx).clear(cx);
            });

            // Keyboard navigation must reach the same focused name control.
            for _ in 0..32 {
                window.simulate_keystrokes("tab f2");
                if window.update(|_, cx| view.read(cx).rename.is_some()) {
                    break;
                }
            }
            assert!(window.update(|_, cx| view.read(cx).rename.is_some()));
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.finish_rename(false, window, cx)
                });
                window.draw(cx).clear(cx);
            });

            let name = window
                .debug_bounds(selector)
                .expect("Screen name is rendered");
            mouse_click(window, name.center(), MouseButton::Right, 1);
            window.simulate_keystrokes("down enter");
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    assert_eq!(
                        workspace.rename.as_ref().expect("menu editor").screen,
                        screen
                    );
                    workspace.finish_rename(false, window, cx);
                    workspace.model = Some(WindowWorkspace::new(Uuid::new_v4()));
                    workspace.start_captured_rename(source, &screen, window, cx);
                    assert!(
                        workspace.rename.is_none(),
                        "old owner cannot retarget rename"
                    );
                });
            });
        });
    }

    #[test]
    fn pane_hover_tracks_whole_screen_and_only_the_pointed_pane() {
        exercise_pane_hover();
    }

    fn exercise_pane_hover() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let screen = model.ui_test_local_screen_fixture("Work").unwrap();
            let (commands, mut receiver) = mpsc::channel(2);
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            let block_selector: &'static str =
                Box::leak(format!("drawer-block-{screen}").into_boxed_str());
            let pane_selector: &'static str =
                Box::leak(format!("drawer-pane-{screen}-p0").into_boxed_str());
            let block = window.debug_bounds(block_selector).unwrap();
            let pane = window.debug_bounds(pane_selector).unwrap();
            window.simulate_mouse_move(pane.center(), None, Modifiers::default());
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert_eq!(workspace.drawer_hovered, Some(screen.clone()));
                assert_eq!(
                    workspace.drawer_pane_hovered,
                    Some((screen.clone(), "p0".into()))
                );
            });
            window.simulate_mouse_move(
                point(block.origin.x + px(4.), block.origin.y + px(8.)),
                None,
                Modifiers::default(),
            );
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert_eq!(workspace.drawer_hovered, Some(screen.clone()));
                assert!(workspace.drawer_pane_hovered.is_none());
            });
            window.simulate_mouse_move(point(px(600.), px(500.)), None, Modifiers::default());
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert!(workspace.drawer_hovered.is_none());
                assert!(workspace.drawer_pane_hovered.is_none());
            });
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.draw(cx).clear(cx);
            });
            window.simulate_mouse_move(pane.center(), None, Modifiers::default());
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert_eq!(workspace.drawer_hovered, Some(screen));
            });
            assert!(
                receiver.try_recv().is_err(),
                "hover only changes presentation"
            );
        });
    }

    #[test]
    fn pin_click_presents_immediately_and_stale_read_cannot_override_pending_intent() {
        exercise_pin_reconciliation();
    }

    fn exercise_pin_reconciliation() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(WindowWorkspace::new(owner));
                    workspace.apply_appearance(
                        &settings_snapshot_with_pin(owner, 1, Availability::Full, false),
                        window,
                        cx,
                    );
                    workspace.drawer_overlay_open = true;
                });
                window.draw(cx).clear(cx);
            });
            let pin = window
                .debug_bounds("drawer-pin")
                .expect("Pin button is rendered");
            window.simulate_click(pin.center(), Modifiers::default());
            let sent = receiver.try_recv().expect("pin request admitted");
            let request_id = match sent.kind() {
                Command::SetDrawerVisibility {
                    request_id,
                    pinned: true,
                    ..
                } => *request_id,
                _ => panic!("unexpected pin command"),
            };
            window.update(|_, cx| {
                let workspace = view.read(cx);
                assert!(workspace.drawer_visible);
                assert!(!workspace.drawer_overlay_open);
                assert_eq!(workspace.drawer_pending.unwrap().request_id, request_id);
                assert!(workspace.drawer_pending.unwrap().desired);
            });
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.apply_appearance(
                        &settings_snapshot_with_pin(owner, 2, Availability::Full, false),
                        window,
                        cx,
                    );
                    assert!(workspace.drawer_visible, "old read cannot undo the click");
                    workspace.apply_drawer_visibility(
                        DrawerVisibilityResult {
                            connection_id: owner,
                            request_id: Uuid::new_v4(),
                            outcome: DrawerVisibilityOutcome::Rejected {
                                reason: "old".into(),
                            },
                        },
                        cx,
                    );
                    assert!(
                        workspace.drawer_visible,
                        "foreign request cannot settle pin"
                    );
                    workspace.apply_drawer_visibility(
                        DrawerVisibilityResult {
                            connection_id: owner,
                            request_id,
                            outcome: DrawerVisibilityOutcome::Rejected {
                                reason: "denied".into(),
                            },
                        },
                        cx,
                    );
                    assert!(!workspace.drawer_visible, "rejection restores latest read");
                    assert!(workspace.drawer_overlay_open);
                    assert!(workspace.drawer_pending.is_none());
                    assert!(workspace.status.contains("denied"));
                });
            });
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.toggle_drawer(cx);
                    assert!(workspace.drawer_visible);
                });
            });
            let sent = receiver.try_recv().expect("retry pin request admitted");
            let retry_id = match sent.kind() {
                Command::SetDrawerVisibility {
                    request_id,
                    pinned: true,
                    ..
                } => *request_id,
                _ => panic!("unexpected retry command"),
            };
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.apply_drawer_visibility(
                        DrawerVisibilityResult {
                            connection_id: owner,
                            request_id: retry_id,
                            outcome: DrawerVisibilityOutcome::Confirmed(true),
                        },
                        cx,
                    );
                    assert!(workspace.drawer_visible);
                    assert!(workspace.drawer_pending.is_none());
                    assert!(workspace.drawer_authoritative_pinned);
                    workspace.toggle_drawer(cx);
                    assert!(!workspace.drawer_visible);
                });
            });
            let sent = receiver.try_recv().expect("unpin request admitted");
            let unpin_id = match sent.kind() {
                Command::SetDrawerVisibility {
                    request_id,
                    pinned: false,
                    ..
                } => *request_id,
                _ => panic!("unexpected unpin command"),
            };
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.apply_drawer_visibility(
                        DrawerVisibilityResult {
                            connection_id: owner,
                            request_id: unpin_id,
                            outcome: DrawerVisibilityOutcome::Rejected {
                                reason: "denied again".into(),
                            },
                        },
                        cx,
                    );
                    assert!(workspace.drawer_visible, "rollback uses confirmed baseline");
                    assert!(workspace.drawer_pending.is_none());
                });
            });
        });
    }

    #[test]
    fn drawer_overlay_and_pinned_inset_reverse_without_extra_settings_writes() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            model.ui_test_local_screen_fixture("Work").unwrap();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.apply_appearance(
                        &settings_snapshot_with_pin(owner, 1, Availability::Full, false),
                        window,
                        cx,
                    );
                });
                window.draw(cx).clear(cx);
                cx.set_reduce_motion(false);
                window.draw(cx).clear(cx);
            });
            let base = window
                .debug_bounds("workspace-viewport")
                .expect("workspace viewport");
            let base_x = f32::from(base.origin.x);
            let base_width = f32::from(base.size.width);

            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.toggle_drawer_overlay(cx);
                })
            });
            redraw(window);
            advance(window, Duration::from_millis(110));
            let slide = window.debug_bounds("drawer-slide").expect("sliding Drawer");
            let slide_x = f32::from(slide.origin.x);
            assert!(
                slide_x > -200. && slide_x < 0.,
                "overlay moves through viewport"
            );
            let unchanged = window.debug_bounds("workspace-viewport").unwrap();
            assert!((f32::from(unchanged.origin.x) - base_x).abs() < 1.);
            assert!((f32::from(unchanged.size.width) - base_width).abs() < 1.);
            advance(window, Duration::from_millis(180));

            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.toggle_drawer(cx);
                    workspace.toggle_drawer(cx);
                })
            });
            let sent = receiver.try_recv().expect("one admitted pin write");
            let request_id = match sent.kind() {
                Command::SetDrawerVisibility {
                    request_id,
                    pinned: true,
                    ..
                } => *request_id,
                _ => panic!("unexpected Drawer command"),
            };
            assert!(
                receiver.try_recv().is_err(),
                "pending toggle cannot repeat write"
            );
            redraw(window);
            let start = f32::from(window.debug_bounds("workspace-viewport").unwrap().origin.x);
            assert!((start - base_x).abs() < 1.);
            advance(window, Duration::from_millis(110));
            let middle = f32::from(window.debug_bounds("workspace-viewport").unwrap().origin.x);
            assert!(middle > base_x && middle < base_x + 200.);

            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.apply_drawer_visibility(
                        DrawerVisibilityResult {
                            connection_id: owner,
                            request_id,
                            outcome: DrawerVisibilityOutcome::Rejected {
                                reason: "denied".into(),
                            },
                        },
                        cx,
                    );
                })
            });
            redraw(window);
            let reversal = f32::from(window.debug_bounds("workspace-viewport").unwrap().origin.x);
            assert!(
                (reversal - middle).abs() < 1.,
                "reversal begins at displayed inset"
            );
            advance(window, Duration::from_millis(250));
            let restored = f32::from(window.debug_bounds("workspace-viewport").unwrap().origin.x);
            assert!((restored - base_x).abs() < 1.);

            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |workspace, cx| workspace.toggle_drawer(cx));
                window.draw(cx).clear(cx);
            });
            let settled = f32::from(window.debug_bounds("workspace-viewport").unwrap().origin.x);
            assert!(
                (settled - base_x - 200.).abs() < 1.,
                "reduced motion settles immediately"
            );
            assert!(matches!(
                receiver.try_recv().unwrap().kind(),
                Command::SetDrawerVisibility { pinned: true, .. }
            ));
            assert!(receiver.try_recv().is_err());
        });
    }

    #[test]
    fn split_drop_rect_moves_within_and_between_measured_slots_without_ops() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let screen = model.ui_test_local_screen_fixture("Work").unwrap();
            let source = model.lifecycle_token();
            model
                .source_mut(source.source_id())
                .unwrap()
                .apply_snapshot(
                    owner,
                    &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                        "id":"work","revision":2,
                        "root":{"type":"split","id":"split","axis":"horizontal",
                            "sizes":[0.5,0.5],"children":[
                                {"type":"slot","id":"left","paneIds":["p0"],"activePaneId":"p0"},
                                {"type":"slot","id":"right","paneIds":["p1"],"activePaneId":"p1"}
                            ]},
                        "panes":{"p0":{"id":"p0"},"p1":{"id":"p1"}},
                        "sessions":{}
                    }}),
                )
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(2);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.model = Some(model);
                    workspace.commands = commands;
                    workspace.destination = Destination::Screens;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let left = screen.layout_key("left");
            let right = screen.layout_key("right");
            let (left_bounds, right_bounds) = window.update(|_, cx| {
                let workspace = view.read(cx);
                (
                    *workspace.slot_bounds.get(&left).expect("left measured"),
                    *workspace.slot_bounds.get(&right).expect("right measured"),
                )
            });
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.set_drop_target(
                        Some(DropTarget::Body {
                            slot: left.clone(),
                            placement: MovePlacement::Tab,
                        }),
                        cx,
                    );
                })
            });
            redraw(window);
            let whole = window
                .debug_bounds("workspace-drop-highlight")
                .expect("whole slot");
            assert!((f32::from(whole.size.width) - f32::from(left_bounds.size.width)).abs() < 1.);
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.set_drop_target(
                        Some(DropTarget::Body {
                            slot: left.clone(),
                            placement: MovePlacement::Left,
                        }),
                        cx,
                    );
                    assert_eq!(workspace.drop_motion_duration, Duration::from_millis(70));
                })
            });
            redraw(window);
            advance(window, Duration::from_millis(35));
            let half = window.debug_bounds("workspace-drop-highlight").unwrap();
            assert!(half.size.width < whole.size.width);
            assert!(f32::from(half.size.width) > f32::from(whole.size.width) / 2.);

            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.set_drop_target(
                        Some(DropTarget::Body {
                            slot: right.clone(),
                            placement: MovePlacement::Tab,
                        }),
                        cx,
                    );
                    assert_eq!(workspace.drop_motion_duration, Duration::from_millis(300));
                })
            });
            redraw(window);
            advance(window, Duration::from_millis(150));
            let moving = window.debug_bounds("workspace-drop-highlight").unwrap();
            assert!(moving.origin.x > left_bounds.origin.x);
            assert!(moving.origin.x < right_bounds.origin.x);
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.set_drop_target(
                        Some(DropTarget::Body {
                            slot: left.clone(),
                            placement: MovePlacement::Right,
                        }),
                        cx,
                    );
                });
            });
            redraw(window);
            let reversed = window.debug_bounds("workspace-drop-highlight").unwrap();
            assert!(
                (f32::from(reversed.origin.x) - f32::from(moving.origin.x)).abs() < 1.,
                "rapid target reversal starts at the displayed rectangle"
            );
            advance(window, Duration::from_millis(150));
            let returning = window.debug_bounds("workspace-drop-highlight").unwrap();
            assert!(returning.origin.x < moving.origin.x);
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.set_drop_target(None, cx);
                })
            });
            redraw(window);
            assert!(window.debug_bounds("workspace-drop-highlight").is_none());
            assert!(
                receiver.try_recv().is_err(),
                "preview and cancel submit no layout op"
            );
        });
    }

    #[test]
    fn pinned_inset_reports_only_changed_terminal_grid_sizes() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            model
                .ui_test_remote_terminal_fixture("me@192.0.2.7")
                .expect("in-memory attached remote terminal");
            let (commands, mut receiver) = mpsc::channel(256);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model, window, cx);
                    workspace.apply_appearance(
                        &settings_snapshot_with_pin(owner, 1, Availability::Full, false),
                        window,
                        cx,
                    );
                });
                window.draw(cx).clear(cx);
                cx.set_reduce_motion(false);
                window.draw(cx).clear(cx);
            });
            assert!(!window.update(|_, cx| view.read(cx).panes.is_empty()));
            while receiver.try_recv().is_ok() {}

            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.toggle_drawer(cx);
                })
            });
            redraw(window);
            for _ in 0..15 {
                advance(window, Duration::from_millis(16));
            }
            let mut pin_writes = 0;
            let mut resizes = 0;
            while let Ok(command) = receiver.try_recv() {
                match command.kind() {
                    Command::SetDrawerVisibility { .. } => pin_writes += 1,
                    Command::ResizePane { .. } => resizes += 1,
                    Command::Input { .. } => panic!("presentation sent terminal input"),
                    _ => {}
                }
            }
            assert_eq!(pin_writes, 1);
            eprintln!("Drawer inset motion: {resizes} ResizePane commands in 15 frames");
            assert!(resizes > 0, "animated grid width is reported");
            assert!(
                resizes <= 32,
                "one 220 ms transition stays bounded by frame samples"
            );
        });
    }
}

#[derive(Default)]
pub(super) struct WheelNavigation {
    target: Option<WheelTarget>,
    pixels: f32,
}

impl WheelNavigation {
    pub(super) fn reset(&mut self) {
        self.target = None;
        self.pixels = 0.;
    }

    fn admit(&mut self, target: WheelTarget, event: &ScrollEvent, blocked: bool) -> i32 {
        if blocked {
            self.reset();
            return 0;
        }
        self.steps(target, event)
    }

    /// Returns bounded relative steps. A phase-less pixel stream still advances
    /// as distance accrues; a real boundary discards the prior partial step.
    fn steps(&mut self, target: WheelTarget, event: &ScrollEvent) -> i32 {
        if !Self::vertical(event) {
            self.reset();
            return 0;
        }
        let (x, y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
        };
        if !x.is_finite() || !y.is_finite() || x.abs() >= y.abs() || y == 0. {
            self.reset();
            return 0;
        }
        if self.target.as_ref() != Some(&target) || event.touch_phase == TouchPhase::Started {
            self.pixels = 0.;
        }
        self.target = Some(target);
        if event.touch_phase == TouchPhase::Cancelled {
            self.reset();
            return 0;
        }
        let result = match event.delta {
            // The line value is an OS scroll setting, not a detent count.
            ScrollDelta::Lines(_) => {
                if y > 0. {
                    1
                } else {
                    -1
                }
            }
            ScrollDelta::Pixels(_) => {
                if self.pixels.signum() != y.signum() {
                    self.pixels = 0.;
                }
                self.pixels = (self.pixels + y).clamp(-192., 192.);
                let steps = (self.pixels / 48.).trunc().clamp(-1., 1.) as i32;
                self.pixels -= steps as f32 * 48.;
                steps
            }
        };
        if matches!(event.touch_phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.reset();
        }
        result
    }

    fn vertical(event: &ScrollEvent) -> bool {
        if event.modifiers.shift {
            return false;
        }
        let (x, y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
        };
        x.is_finite() && y.is_finite() && y != 0. && y.abs() > x.abs()
    }
}

fn wheel_step(direction: i32) -> TabStep {
    if direction > 0 {
        TabStep::Previous
    } else {
        TabStep::Next
    }
}

const EDGE_WIDTH: f32 = 6.;
const EDGE_DWELL: Duration = Duration::from_millis(200);
const EDGE_INTENT_SPEED: f32 = 800.;

fn edge_motion(
    previous: Option<(Point<Pixels>, Instant)>,
    now: Instant,
    position: Point<Pixels>,
) -> bool {
    if f32::from(position.x) > EDGE_WIDTH {
        return false;
    }
    previous.is_none_or(|(point, then)| {
        let elapsed = now.saturating_duration_since(then).as_secs_f32();
        elapsed == 0. || {
            let dx = f32::from(position.x - point.x);
            let dy = f32::from(position.y - point.y);
            dx.hypot(dy) / elapsed < EDGE_INTENT_SPEED
        }
    })
}

fn collect_pane_groups<'a>(node: &'a LayoutNode, out: &mut Vec<Vec<&'a str>>) {
    match node {
        LayoutNode::Slot { pane_ids, .. } => {
            out.push(pane_ids.iter().map(|id| id.as_str()).collect())
        }
        LayoutNode::Split { children, .. } => {
            for child in children {
                collect_pane_groups(child, out);
            }
        }
    }
}

fn drawer_pane_groups(screen: &ScreenState) -> Vec<Vec<&str>> {
    let mut groups = Vec::new();
    collect_pane_groups(&screen.root, &mut groups);
    for floating in &screen.floating {
        collect_pane_groups(&floating.root, &mut groups);
    }
    groups
}

fn split_title_icon(title: &str) -> (Option<char>, &str) {
    let mut chars = title.chars();
    let Some(first) = chars.next() else {
        return (None, title);
    };
    if !first.is_ascii() && !first.is_alphanumeric() && chars.next() == Some(' ') {
        (Some(first), &title[first.len_utf8() + 1..])
    } else {
        (None, title)
    }
}

impl Workspace {
    fn screen_menu_before(&self, screen: &ScreenKey, down: bool) -> Option<Option<ScreenKey>> {
        let opened: Vec<_> = self
            .model
            .as_ref()?
            .opened()
            .into_iter()
            .filter(|entry| entry.key().source_id() == screen.source_id())
            .map(|entry| entry.key().clone())
            .collect();
        let index = opened.iter().position(|key| key == screen)?;
        if down {
            (index + 1 < opened.len()).then(|| opened.get(index + 2).cloned())
        } else {
            index
                .checked_sub(1)
                .map(|previous| Some(opened[previous].clone()))
        }
    }

    fn can_place_screen(
        &self,
        owner: SourceToken,
        screen: &ScreenKey,
        before: Option<&ScreenKey>,
    ) -> bool {
        self.model.as_ref().is_some_and(|model| {
            model.can_move_screen(screen)
                && matches!(model.validate_screen_move(owner, screen, before), Ok(true))
        })
    }

    fn place_screen(
        &mut self,
        owner: SourceToken,
        screen: ScreenKey,
        before: Option<ScreenKey>,
        cx: &mut Context<Self>,
    ) {
        if self.lifecycle_token() != Some(owner)
            || !self.can_place_screen(owner, &screen, before.as_ref())
        {
            return;
        }
        self.send_captured(owner, Command::MoveScreen { screen, before }, cx);
    }

    fn screen_drag_hover(
        &mut self,
        row: &ScreenKey,
        event: &DragMoveEvent<ScreenDrag>,
        cx: &mut Context<Self>,
    ) {
        let drag = event.drag(cx);
        if drag.view != cx.entity().entity_id()
            || self.lifecycle_token() != Some(drag.owner)
            || !event.bounds.contains(&event.event.position)
        {
            return;
        }
        let Some(model) = self.model.as_ref() else {
            return;
        };
        let order: Vec<_> = model
            .opened()
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        let Some(index) = order.iter().position(|key| key == row) else {
            return;
        };
        if row.source_id() != drag.screen.source_id() || !model.can_move_screen(&drag.screen) {
            return;
        }
        let after_row = event.event.position.y >= event.bounds.center().y;
        let before = if after_row {
            order
                .get(index + 1)
                .filter(|next| next.source_id() == row.source_id())
                .cloned()
        } else {
            Some(row.clone())
        };
        if !matches!(
            model.validate_screen_move(drag.owner, &drag.screen, before.as_ref()),
            Ok(true)
        ) {
            if self.drawer_drop.take().is_some() {
                cx.notify();
            }
            return;
        }
        let changed = self.drawer_drop.as_ref().is_none_or(|target| {
            target.drag != drag.screen
                || target.row != *row
                || target.after_row != after_row
                || target.before != before
        });
        self.drawer_drop = Some(ScreenDrop {
            drag: drag.screen.clone(),
            row: row.clone(),
            before,
            after_row,
            bounds: event.bounds,
            scroll_offset: self.drawer_scroll.offset(),
            order,
        });
        if changed {
            cx.notify();
        }
    }

    fn screen_drop(&mut self, drag: &ScreenDrag, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.drawer_drop.take() else {
            return;
        };
        cx.notify();
        if drag.view != cx.entity().entity_id()
            || self.lifecycle_token() != Some(drag.owner)
            || target.drag != drag.screen
            || !target.bounds.contains(&window.mouse_position())
            || target.scroll_offset != self.drawer_scroll.offset()
            || self.model.as_ref().is_none_or(|model| {
                model
                    .opened()
                    .iter()
                    .map(|entry| entry.key().clone())
                    .collect::<Vec<_>>()
                    != target.order
            })
        {
            return;
        }
        self.place_screen(drag.owner, drag.screen.clone(), target.before, cx);
    }

    fn set_drawer_overlay(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.drawer_visible || self.drawer_overlay_open == open {
            return;
        }
        self.drawer_overlay_open = open;
        self.wheel_navigation.reset();
        cx.notify();
    }

    pub(super) fn toggle_drawer_overlay(&mut self, cx: &mut Context<Self>) {
        if self.drawer_visible {
            return;
        }
        self.drawer_edge_task = None;
        self.set_drawer_overlay(!self.drawer_overlay_open, cx);
    }

    pub(super) fn close_drawer_overlay(&mut self, cx: &mut Context<Self>) {
        self.drawer_edge_task = None;
        self.set_drawer_overlay(false, cx);
    }

    pub(super) fn drawer_edge_hover(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let now = Instant::now();
        let eligible = !self.drawer_visible
            && !self.drawer_overlay_open
            && self.drawer_edge_reveal
            && !event.dragging()
            && !GlobalState::is_in_deferred_context(cx)
            && edge_motion(self.drawer_edge_last, now, event.position);
        self.drawer_edge_last = Some((event.position, now));
        if !eligible {
            self.drawer_edge_task = None;
            return;
        }
        if self.drawer_edge_task.is_some() {
            return;
        }
        let owner = self.lifecycle_token();
        self.drawer_edge_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EDGE_DWELL).await;
            let _ = this.update(cx, |this, cx| {
                if this.lifecycle_token() == owner
                    && !this.drawer_visible
                    && !this.drawer_overlay_open
                    && this.drawer_edge_reveal
                    && this
                        .drawer_edge_last
                        .is_some_and(|(point, _)| f32::from(point.x) <= EDGE_WIDTH)
                    && !GlobalState::is_in_deferred_context(cx)
                {
                    this.set_drawer_overlay(true, cx);
                }
                this.drawer_edge_task = None;
            });
        }));
    }

    pub(super) fn render_drawer(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.lifecycle_token();
        let pin_label = match self.drawer_pending {
            Some(pending) if pending.desired => "Pinning drawer…",
            Some(_) => "Unpinning drawer…",
            None if self.drawer_visible => "Unpin drawer",
            None => "Pin drawer",
        };
        let muted = cx.theme().muted_foreground;
        let mut list = div()
            .id("drawer-screen-list")
            .flex_shrink_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.drawer_scroll)
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ScreenDrag>, _, cx| {
                    if this
                        .drawer_drop
                        .as_ref()
                        .is_some_and(|target| !target.bounds.contains(&event.event.position))
                    {
                        this.drawer_drop = None;
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(Self::screen_drop))
            .on_scroll(cx.listener(|this, _: &ScrollEvent, _, cx| {
                if this.drawer_drop.take().is_some() {
                    cx.notify();
                }
            }))
            .py_1();
        if let Some(window_model) = &self.model {
            let shown = window_model.shown_key();
            let opened = window_model.opened();
            let mut group = None;
            for (index, entry) in opened.iter().enumerate() {
                let key = entry.key().clone();
                let remote = key.source_id() != window_model.local_source().source_id();
                if group != Some(key.source_id()) {
                    group = Some(key.source_id());
                    if remote {
                        let title = window_model
                            .source_target(key.source_id())
                            .map(|target| {
                                owner
                                    .and_then(|owner| self.hosts.accepted_alias(owner, target))
                                    .unwrap_or(target)
                            })
                            .map(super::hosts::display)
                            .unwrap_or_else(|| "Host".into());
                        list = list.child(
                            div()
                                .mx_2()
                                .mt_2()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .flex()
                                .items_center()
                                .gap(chrome_rems(7.))
                                .border_t_1()
                                .border_color(rgb(self.appearance.subtle_border))
                                .text_size(chrome_rems(11.5))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(muted)
                                .child(Icon::new(IconName::Server).size_3())
                                .child(title),
                        );
                    }
                }
                let active = shown.as_ref() == Some(&key);
                let actions_visible = self.drawer_hovered.as_ref() == Some(&key);
                let hover_transition =
                    Transition::new(Duration::from_millis(80)).easing(Easing::Ease);
                let label = entry
                    .name()
                    .map(super::hosts::display)
                    .unwrap_or_else(|| format!("Screen {}", index + 1));
                let state = match entry.state() {
                    splitty_client_core::workspace::OpenedScreenState::Ready => None,
                    splitty_client_core::workspace::OpenedScreenState::Loading => Some("Loading"),
                    splitty_client_core::workspace::OpenedScreenState::Missing => Some("Missing"),
                    splitty_client_core::workspace::OpenedScreenState::Offline => Some("Offline"),
                };
                let screen_snapshot = window_model
                    .source(key.source_id())
                    .and_then(|source| source.screen(key.screen_id()))
                    .and_then(ScreenModel::snapshot);
                let mut row = div()
                    .id(SharedString::from(format!("drawer-screen-{key}")))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .h(chrome_rems(30.))
                    .px_2()
                    .gap(chrome_rems(8.))
                    .child(
                        div()
                            .id(SharedString::from(format!("drawer-index-{key}")))
                            .when(cfg!(all(test, feature = "ui-test-support")), |index| {
                                index.debug_selector(|| format!("drawer-index-{key}"))
                            })
                            .w(chrome_rems(14.))
                            .flex_none()
                            .text_size(chrome_rems(10.5))
                            .font(font(self.mono_family.clone()))
                            .text_align(TextAlign::Right)
                            .text_color(if active {
                                rgb(self.appearance.accent)
                            } else {
                                muted.into()
                            })
                            .child(format!("{}", index + 1)),
                    );
                if !remote
                    && self
                        .rename
                        .as_ref()
                        .is_some_and(|editor| editor.screen == key)
                {
                    if let Some(editor) = &self.rename {
                        row = row.child(div().flex_1().min_w_0().child(Input::new(&editor.input)));
                    }
                } else {
                    let select = key.clone();
                    let weak = cx.entity().downgrade();
                    let rename_on_click = key.clone();
                    let rename_on_key = key.clone();
                    let key_weak = cx.entity().downgrade();
                    let select_button =
                        Button::new(SharedString::from(format!("drawer-select-{key}")))
                            .accessibility_label(format!("Select {label}"))
                            .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                                button.debug_selector(|| format!("drawer-select-{key}"))
                            })
                            .text()
                            .compact()
                            .small()
                            .flex_1()
                            .min_w_0()
                            .justify_start()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(chrome_rems(12.5))
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_color(transition::<Hsla>(
                                        format!("drawer-screen-text-{key}"),
                                        rgb(if active || actions_visible {
                                            self.appearance.text
                                        } else {
                                            self.appearance.secondary
                                        })
                                        .into(),
                                        hover_transition.clone(),
                                        window,
                                        cx,
                                    ))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(label.clone()),
                            )
                            .on_key_down(move |event, window, cx| {
                                if !remote
                                    && event.keystroke.key == "f2"
                                    && event.keystroke.modifiers == Modifiers::default()
                                {
                                    cx.stop_propagation();
                                    window.prevent_default();
                                    if let Some(owner) = owner {
                                        let _ = key_weak.update(cx, |this, cx| {
                                            this.start_captured_rename(
                                                owner,
                                                &rename_on_key,
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            })
                            .on_click(move |event, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.lifecycle_token() == owner
                                        && !GlobalState::is_in_deferred_context(cx)
                                    {
                                        if event.click_count() == 2 && !remote {
                                            if let Some(owner) = owner {
                                                this.start_captured_rename(
                                                    owner,
                                                    &rename_on_click,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        } else if event.standard_click() {
                                            this.switch_screen_key(select.clone(), window, cx);
                                            this.focus_current(window, cx);
                                        }
                                    }
                                });
                            });
                    let up = self.screen_menu_before(&key, false);
                    let down = self.screen_menu_before(&key, true);
                    let up_enabled = owner.is_some_and(|owner| {
                        up.as_ref().is_some_and(|before| {
                            self.can_place_screen(owner, &key, before.as_ref())
                        })
                    });
                    let down_enabled = owner.is_some_and(|owner| {
                        down.as_ref().is_some_and(|before| {
                            self.can_place_screen(owner, &key, before.as_ref())
                        })
                    });
                    let up = ScreenMoveOption {
                        before: up,
                        enabled: up_enabled,
                    };
                    let down = ScreenMoveOption {
                        before: down,
                        enabled: down_enabled,
                    };
                    let menu_weak = cx.entity().downgrade();
                    let menu_screen = key.clone();
                    let select_button = select_button.context_menu(move |menu, _, _| {
                        let menu = if remote {
                            menu
                        } else {
                            let weak = menu_weak.clone();
                            let screen = menu_screen.clone();
                            menu.item(PopupMenuItem::new("Rename Screen…").on_click(
                                move |_, window, cx| {
                                    if let Some(owner) = owner {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.start_captured_rename(owner, &screen, window, cx);
                                        });
                                    }
                                },
                            ))
                        };
                        screen_move_items(
                            menu,
                            menu_weak.clone(),
                            owner,
                            menu_screen.clone(),
                            up.clone(),
                            down.clone(),
                        )
                    });
                    let drag_enabled = window_model.can_move_screen(&key) && owner.is_some();
                    let drag_label = label.clone();
                    let drag_appearance = self.appearance.clone();
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("drawer-drag-{key}")))
                            .flex_1()
                            .min_w_0()
                            .when(drag_enabled, |view| {
                                view.on_drag(
                                    ScreenDrag {
                                        owner: owner.expect("checked drag owner"),
                                        screen: key.clone(),
                                        view: cx.entity().entity_id(),
                                    },
                                    move |_, _, _, cx| {
                                        cx.new(|_| ScreenDragPreview {
                                            label: drag_label.clone(),
                                            appearance: drag_appearance.clone(),
                                        })
                                    },
                                )
                            })
                            .child(select_button),
                    );
                }
                if let Some(state) = state {
                    row = row.child(div().flex_none().text_sm().text_color(muted).child(state));
                }
                if actions_visible
                    && screen_snapshot.is_some_and(|screen| {
                        drawer_pane_groups(screen)
                            .iter()
                            .map(Vec::len)
                            .sum::<usize>()
                            > 1
                    })
                {
                    let collapsed = self.drawer_collapsed.contains(&key);
                    let collapse_key = key.clone();
                    let weak = cx.entity().downgrade();
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-collapse-{key}")))
                            .accessibility_label(if collapsed {
                                format!("Show panes in {label}")
                            } else {
                                format!("Hide panes in {label}")
                            })
                            .ghost()
                            .cursor_pointer()
                            .compact()
                            .xsmall()
                            .w(chrome_rems(24.))
                            .icon(Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            }))
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.lifecycle_token() != owner {
                                        return;
                                    }
                                    if !this.drawer_collapsed.insert(collapse_key.clone()) {
                                        this.drawer_collapsed.remove(&collapse_key);
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                if actions_visible && remote {
                    let terminate = key.clone();
                    let weak = cx.entity().downgrade();
                    let up = self.screen_menu_before(&key, false);
                    let down = self.screen_menu_before(&key, true);
                    let up_enabled = owner.is_some_and(|owner| {
                        up.as_ref().is_some_and(|before| {
                            self.can_place_screen(owner, &key, before.as_ref())
                        })
                    });
                    let down_enabled = owner.is_some_and(|owner| {
                        down.as_ref().is_some_and(|before| {
                            self.can_place_screen(owner, &key, before.as_ref())
                        })
                    });
                    let up = ScreenMoveOption {
                        before: up,
                        enabled: up_enabled,
                    };
                    let down = ScreenMoveOption {
                        before: down,
                        enabled: down_enabled,
                    };
                    let token = window_model
                        .execution_token(key.source_id())
                        .filter(|_| state.is_none());
                    row = row.child(
                        Button::new(SharedString::from(format!("drawer-more-{key}")))
                            .accessibility_label(format!("More actions for {label}"))
                            .tooltip("More Screen actions")
                            .icon(Icon::new(IconName::Ellipsis))
                            .ghost()
                            .cursor_pointer()
                            .compact()
                            .xsmall()
                            .w(chrome_rems(27.))
                            .dropdown_menu(move |menu, _, _| {
                                let weak = weak.clone();
                                let terminate = terminate.clone();
                                screen_move_items(
                                    menu,
                                    weak.clone(),
                                    owner,
                                    terminate.clone(),
                                    up.clone(),
                                    down.clone(),
                                )
                                .item(
                                    PopupMenuItem::new("Terminate Screen…")
                                        .disabled(token.is_none())
                                        .on_click(move |_, window, cx| {
                                            if let Some(token) = token {
                                                let _ = weak.update(cx, |this, cx| {
                                                    this.request_terminate_screen(
                                                        token,
                                                        terminate.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }
                                        }),
                                )
                            }),
                    );
                }
                if actions_visible {
                    let close = key.clone();
                    let weak = cx.entity().downgrade();
                    let close_tooltip = if remote {
                        "Detach screen: keeps running on the host"
                    } else {
                        "Close screen"
                    };
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("drawer-close-hover-{key}")))
                            .rounded_md()
                            .hover(|style| style.bg(cx.theme().list_hover))
                            .child(
                                Button::new(SharedString::from(format!("drawer-close-{key}")))
                                    .accessibility_label(if remote {
                                        format!("Detach {label}: keeps running on the host")
                                    } else {
                                        format!("Close {label}")
                                    })
                                    .ghost()
                                    .cursor_pointer()
                                    .compact()
                                    .xsmall()
                                    .w(chrome_rems(27.))
                                    .tooltip(close_tooltip)
                                    .icon(Icon::new(if remote {
                                        IconName::Unplug
                                    } else {
                                        IconName::X
                                    }))
                                    .on_click(move |_, window, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            if this.lifecycle_token() == owner
                                                && this.model.as_ref().is_some_and(|model| {
                                                    model
                                                        .opened()
                                                        .iter()
                                                        .any(|entry| entry.key() == &close)
                                                })
                                            {
                                                this.request_close_screen(
                                                    close.clone(),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        });
                                    }),
                            ),
                    );
                }
                let block_background: Hsla = if active {
                    rgb(self.appearance.active).into()
                } else if actions_visible {
                    cx.theme().list_hover
                } else {
                    rgb(self.appearance.raised).into()
                };
                let block_background = transition(
                    format!("drawer-screen-background-{key}"),
                    block_background,
                    hover_transition.clone(),
                    window,
                    cx,
                );
                let mut block = div()
                    .id(SharedString::from(format!("drawer-block-{key}")))
                    .when(cfg!(all(test, feature = "ui-test-support")), |block| {
                        block.debug_selector(|| format!("drawer-block-{key}"))
                    })
                    .mx_2()
                    .my_0p5()
                    .relative()
                    .rounded_md()
                    .bg(block_background)
                    .on_drag_move({
                        let row_key = key.clone();
                        cx.listener(move |this, event: &DragMoveEvent<ScreenDrag>, _, cx| {
                            this.screen_drag_hover(&row_key, event, cx);
                        })
                    })
                    .on_hover({
                        let hover_key = key.clone();
                        cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.drawer_hovered = Some(hover_key.clone());
                                cx.notify();
                            } else if this.drawer_hovered.as_ref() == Some(&hover_key) {
                                this.drawer_hovered = None;
                                cx.notify();
                            }
                        })
                    })
                    .child(row);
                if let Some(target) = self.drawer_drop.as_ref().filter(|target| target.row == key) {
                    block = block.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .when(target.after_row, |line| line.bottom_0())
                            .when(!target.after_row, |line| line.top_0())
                            .h(px(2.))
                            .bg(rgb(self.appearance.accent)),
                    );
                }
                if let Some(screen) =
                    screen_snapshot.filter(|_| !self.drawer_collapsed.contains(&key))
                {
                    let groups = drawer_pane_groups(screen);
                    let multiple_groups = groups.len() > 1;
                    let pane_source = window_model.execution_token(key.source_id());
                    for group in groups {
                        let stacked = multiple_groups && group.len() > 1;
                        let mut group_view = div().flex().flex_col();
                        if stacked {
                            group_view = group_view
                                .ml(chrome_rems(30.))
                                .pl(chrome_rems(9.))
                                .border_l_1()
                                .border_color(rgb(self.appearance.border));
                        }
                        for pane_id in group {
                            let Some((_, pane)) =
                                screen.panes.iter().find(|(id, _)| id.as_str() == pane_id)
                            else {
                                continue;
                            };
                            let pane_key = pane_id.to_owned();
                            let select_key = key.clone();
                            let weak = cx.entity().downgrade();
                            let hover_weak = cx.entity().downgrade();
                            let hover_screen = key.clone();
                            let hover_pane = pane_key.clone();
                            let pane_hovered = self
                                .drawer_pane_hovered
                                .as_ref()
                                .is_some_and(|(screen, pane)| screen == &key && pane == pane_id);
                            let title = pane.title.as_deref().unwrap_or("Terminal");
                            let title = super::hosts::display(title);
                            let (leading_icon, pane_label) = split_title_icon(&title);
                            let is_viewer = pane.viewer.is_some();
                            let busy = pane
                                .session_id
                                .as_ref()
                                .and_then(|id| screen.sessions.get(id))
                                .is_some_and(|session| session.busy);
                            let glyph: AnyElement = if is_viewer {
                                Icon::new(IconName::FileText).size_3().into_any_element()
                            } else if let Some(icon) = leading_icon {
                                div()
                                    .text_size(chrome_rems(10.))
                                    .child(icon.to_string())
                                    .into_any_element()
                            } else {
                                div()
                                    .size(chrome_rems(5.))
                                    .rounded_full()
                                    .bg(if busy {
                                        rgb(self.appearance.text)
                                    } else {
                                        muted.into()
                                    })
                                    .into_any_element()
                            };
                            group_view = group_view.child(
                                Button::new(SharedString::from(format!(
                                    "drawer-pane-{key}-{pane_id}"
                                )))
                                .accessibility_label(format!("Open {title} in {label}"))
                                .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                                    button.debug_selector(|| format!("drawer-pane-{key}-{pane_id}"))
                                })
                                .disabled(pane_source.is_none())
                                .text()
                                .when(pane_source.is_some(), |button| button.cursor_pointer())
                                .compact()
                                .xsmall()
                                .w_full()
                                .h(chrome_rems(21.))
                                .pl(chrome_rems(if stacked { 8. } else { 30. }))
                                .justify_start()
                                .font(font(self.mono_family.clone()))
                                .text_size(chrome_rems(11.))
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(chrome_rems(7.))
                                        .text_color(transition::<Hsla>(
                                            format!("drawer-pane-text-{key}-{pane_id}"),
                                            rgb(if pane_hovered {
                                                self.appearance.text
                                            } else if busy || is_viewer {
                                                self.appearance.secondary
                                            } else {
                                                self.appearance.muted
                                            })
                                            .into(),
                                            hover_transition.clone(),
                                            window,
                                            cx,
                                        ))
                                        .child(
                                            div()
                                                .w(chrome_rems(12.))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(glyph),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(pane_label.to_owned()),
                                        ),
                                )
                                .on_hover(move |hovered, _, cx| {
                                    let _ = hover_weak.update(cx, |this, cx| {
                                        if this.lifecycle_token() != owner {
                                            return;
                                        }
                                        let identity = (hover_screen.clone(), hover_pane.clone());
                                        if *hovered {
                                            if this.drawer_pane_hovered.as_ref() != Some(&identity)
                                            {
                                                this.drawer_pane_hovered = Some(identity);
                                                cx.notify();
                                            }
                                        } else if this.drawer_pane_hovered.as_ref()
                                            == Some(&identity)
                                        {
                                            this.drawer_pane_hovered = None;
                                            cx.notify();
                                        }
                                    });
                                })
                                .on_click(
                                    move |_, window, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            if this.lifecycle_token() != owner
                                                || !pane_source.is_some_and(|token| {
                                                    this.model.as_ref().is_some_and(|model| {
                                                        model
                                                            .execution_token(select_key.source_id())
                                                            == Some(token)
                                                            && model
                                                                .source(select_key.source_id())
                                                                .and_then(|source| {
                                                                    source.screen(
                                                                        select_key.screen_id(),
                                                                    )
                                                                })
                                                                .and_then(ScreenModel::snapshot)
                                                                .is_some_and(|screen| {
                                                                    screen.panes.keys().any(|id| {
                                                                        id.as_str() == pane_key
                                                                    })
                                                                })
                                                    })
                                                })
                                            {
                                                return;
                                            }
                                            let token = pane_source
                                                .expect("checked captured pane authority");
                                            this.switch_screen_key(select_key.clone(), window, cx);
                                            this.send_captured(
                                                token,
                                                Command::Select {
                                                    screen: select_key.clone(),
                                                    pane: pane_key.clone(),
                                                },
                                                cx,
                                            );
                                        });
                                    },
                                ),
                            );
                        }
                        block = block.child(group_view);
                    }
                }
                list = list.child(block);
            }
            if let Some(pending) =
                window_model
                    .local_source()
                    .pending_template()
                    .filter(|pending| {
                        !window_model
                            .local_source()
                            .screens()
                            .iter()
                            .any(|info| info.id.as_str() == pending.screen_id())
                    })
            {
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(rgb(self.appearance.secondary))
                        .child(format!("{} · Applying…", pending.display_name())),
                );
            }
            if opened.is_empty() && window_model.local_source().pending_template().is_none() {
                list = list.child(
                    div()
                        .px_2()
                        .py_3()
                        .text_color(muted)
                        .child("No Screens yet"),
                );
            }
        } else {
            list = list.child(div().px_2().py_3().text_color(muted).child("Connecting…"));
        }
        let weak = cx.entity().downgrade();
        let create = Button::new("drawer-create")
            .accessibility_label("Create local Screen")
            .small()
            .w_full()
            .h(chrome_rems(32.))
            .ghost()
            .cursor_pointer()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(chrome_rems(8.))
                    .text_color(rgb(self.appearance.secondary))
                    .child(
                        div()
                            .w(chrome_rems(14.))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .child(Icon::new(IconName::Plus).size_3()),
                    )
                    .child("New screen"),
            )
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.lifecycle_token() == owner {
                        this.create_screen(window, cx);
                    }
                });
            });
        let weak = cx.entity().downgrade();
        let templates = Button::new("drawer-templates")
            .accessibility_label("Open local Templates")
            .ghost()
            .cursor_pointer()
            .small()
            .w_full()
            .h(chrome_rems(30.))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(chrome_rems(10.))
                    .text_color(rgb(self.appearance.secondary))
                    .child(
                        div()
                            .w(chrome_rems(15.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(Icon::new(IconName::LayoutGrid).size_3()),
                    )
                    .child("Templates"),
            )
            .on_click(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.lifecycle_token() == owner {
                        this.toggle_templates(window, cx);
                    }
                });
            });
        div()
            .id("screen-drawer")
            .flex_none()
            .flex()
            .flex_col()
            .w(chrome_rems(self.drawer_width as f32))
            .min_h_0()
            .h_full()
            .bg(rgb(self.appearance.raised))
            .occlude()
            .border_r_1()
            .border_color(rgb(self.appearance.subtle_border))
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered
                    && this.drawer_overlay_open
                    && this.rename.is_none()
                    && this.transfer_menu.is_none()
                    && this.profile_picker.is_none()
                    && this.template_dialog.is_none()
                    && this
                        .pending_close
                        .as_ref()
                        .is_none_or(|close| !close.prompting)
                    && this.drag.is_none()
                    && this.float_gesture.is_none()
                    && !GlobalState::is_in_deferred_context(cx)
                    && !cx.has_active_drag()
                {
                    this.close_drawer_overlay(cx);
                }
            }))
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, window, cx| {
                if let Some(source) = owner {
                    this.chrome_wheel(source, WheelTarget::Screens, event, window, cx);
                } else if WheelNavigation::vertical(event) {
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("drawer-header")
                    .h(chrome_rems(32.))
                    .pl_4()
                    .pr_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_color(muted)
                    .child(
                        div()
                            .text_size(chrome_rems(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("SCREENS"),
                    )
                    .child(
                        Button::new("drawer-pin")
                            .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                                button.debug_selector(|| "drawer-pin".into())
                            })
                            .accessibility_label(pin_label)
                            .tooltip(pin_label)
                            .selected(self.drawer_visible)
                            .disabled(self.drawer_pending.is_some() || owner.is_none())
                            .ghost()
                            .when(self.drawer_pending.is_none() && owner.is_some(), |button| {
                                button.cursor_pointer()
                            })
                            .compact()
                            .xsmall()
                            .icon(Icon::new(if self.drawer_visible {
                                IconName::PinOff
                            } else {
                                IconName::Pin
                            }))
                            .on_click({
                                let weak = cx.entity().downgrade();
                                move |_, _, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        if this.lifecycle_token() == owner {
                                            this.toggle_drawer(cx);
                                        }
                                    });
                                }
                            }),
                    ),
            )
            .child(list)
            .child(div().px_2().py_1().child(create))
            .child(div().flex_1().min_h(chrome_rems(8.)))
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .pb_2()
                    .border_t_1()
                    .border_color(rgb(self.appearance.subtle_border))
                    .child(self.render_hosts_navigation(cx))
                    .child(templates)
                    .child(
                        Button::new("drawer-settings")
                            .accessibility_label("Open Settings")
                            .track_focus(&self.settings_trigger)
                            .ghost()
                            .cursor_pointer()
                            .small()
                            .w_full()
                            .h(chrome_rems(30.))
                            .selected(self.destination == Destination::Settings)
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap(chrome_rems(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        div()
                                            .w(chrome_rems(15.))
                                            .flex_none()
                                            .flex()
                                            .justify_center()
                                            .child(Icon::new(IconName::SlidersHorizontal).size_3()),
                                    )
                                    .child("Settings"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.destination == Destination::Settings {
                                    this.close_settings(window, cx);
                                } else {
                                    this.show_settings(window, cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn chrome_wheel(
        &mut self,
        source: SourceToken,
        target: WheelTarget,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if match &target {
            WheelTarget::Screens => self.lifecycle_token() != Some(source),
            WheelTarget::Slot { .. } => !self
                .selected_model()
                .is_some_and(|model| model.source_token() == source),
        } {
            if WheelNavigation::vertical(event) {
                cx.stop_propagation();
            }
            return;
        }
        let blocked =
            (self.destination != Destination::Screens && !matches!(&target, WheelTarget::Screens))
                || self.rename.is_some()
                || self.template_dialog.is_some()
                || self.profile_picker.is_some()
                || self.transfer_menu.is_some()
                || self.pending_template_apply.is_some()
                || self.search_owner.as_ref().is_some_and(|(_, terminal)| {
                    terminal.read(cx).search_input_focused(window, cx)
                })
                || self
                    .pending_close
                    .as_ref()
                    .is_some_and(|close| close.prompting)
                || self.drag.is_some()
                || self.float_gesture.is_some()
                || GlobalState::is_in_deferred_context(cx)
                || cx.has_active_drag();
        if blocked {
            self.wheel_navigation.admit(target, event, true);
            if WheelNavigation::vertical(event) {
                cx.stop_propagation();
            }
            return;
        }
        let vertical = WheelNavigation::vertical(event);
        let steps = self.wheel_navigation.admit(target.clone(), event, false);
        if vertical {
            cx.stop_propagation();
        }
        if steps == 0 {
            return;
        }
        if matches!(&target, WheelTarget::Screens) {
            self.show_screens(window, cx);
            if self.destination != Destination::Screens {
                return;
            }
        }
        for _ in 0..steps.unsigned_abs() {
            match &target {
                WheelTarget::Screens => self.send_captured(
                    source,
                    Command::CycleScreen {
                        step: wheel_step(steps),
                    },
                    cx,
                ),
                WheelTarget::Slot { screen, slot } => {
                    let connection = source.connection_id();
                    self.send_captured(
                        source,
                        Command::CycleTabInSlot {
                            connection,
                            screen: screen.clone(),
                            slot: slot.clone(),
                            step: wheel_step(steps),
                        },
                        cx,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{edge_motion, split_title_icon, wheel_step, WheelNavigation, WheelTarget};
    use gpui_kit::{point, px, ScrollDelta, ScrollEvent, TouchPhase};
    use splitty_client_core::workspace::TabStep;
    use std::time::{Duration, Instant};

    fn event(delta: ScrollDelta, phase: TouchPhase) -> ScrollEvent {
        ScrollEvent {
            delta,
            touch_phase: phase,
            ..Default::default()
        }
    }
    #[test]
    fn line_steps_are_discrete_and_bounded() {
        assert_eq!(wheel_step(1), TabStep::Previous);
        assert_eq!(wheel_step(-1), TabStep::Next);
        let mut wheel = WheelNavigation::default();
        let down = event(ScrollDelta::Lines(point(0., 1.)), TouchPhase::Moved);
        assert_eq!(wheel.steps(WheelTarget::Screens, &down), 1);
        assert_eq!(wheel.steps(WheelTarget::Screens, &down), 1);
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(ScrollDelta::Lines(point(0., -8.)), TouchPhase::Moved)
            ),
            -1
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(ScrollDelta::Lines(point(0., 3.)), TouchPhase::Moved)
            ),
            1
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(ScrollDelta::Lines(point(0., -0.5)), TouchPhase::Moved)
            ),
            -1
        );
    }
    #[test]
    fn pixels_accumulate_and_reset_on_target_or_phase() {
        let mut wheel = WheelNavigation::default();
        let part = event(
            ScrollDelta::Pixels(point(px(0.), px(25.))),
            TouchPhase::Moved,
        );
        assert_eq!(wheel.steps(WheelTarget::Screens, &part), 0);
        assert_eq!(wheel.steps(WheelTarget::Screens, &part), 1);
        assert_eq!(
            wheel.steps(
                WheelTarget::Slot {
                    screen: "s".into(),
                    slot: "g".into()
                },
                &part
            ),
            0
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(0.), px(25.))),
                    TouchPhase::Started
                )
            ),
            0
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(0.), px(25.))),
                    TouchPhase::Cancelled
                )
            ),
            0
        );
        assert_eq!(wheel.steps(WheelTarget::Screens, &part), 0);
    }
    #[test]
    fn horizontal_has_priority() {
        let mut wheel = WheelNavigation::default();
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(60.), px(20.))),
                    TouchPhase::Moved
                )
            ),
            0
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &event(
                    ScrollDelta::Pixels(point(px(0.), px(48.))),
                    TouchPhase::Ended
                )
            ),
            1
        );
    }

    #[test]
    fn blocked_navigation_clears_partial_distance() {
        let mut wheel = WheelNavigation::default();
        let part = event(
            ScrollDelta::Pixels(point(px(0.), px(30.))),
            TouchPhase::Moved,
        );
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, false), 0);
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, true), 0);
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, false), 0);
    }

    #[test]
    fn edge_reveal_requires_slow_unpressed_motion_inside_six_pixels() {
        let now = Instant::now();
        assert!(edge_motion(None, now, point(px(3.), px(20.))));
        assert!(!edge_motion(None, now, point(px(7.), px(20.))));
        assert!(!edge_motion(
            Some((point(px(1.), px(0.)), now)),
            now + Duration::from_millis(10),
            point(px(3.), px(20.)),
        ));
        assert!(edge_motion(
            Some((point(px(1.), px(0.)), now)),
            now + Duration::from_millis(200),
            point(px(3.), px(20.)),
        ));
    }

    #[test]
    fn leading_pane_icon_requires_a_separate_non_ascii_symbol() {
        assert_eq!(split_title_icon("⚡ Build"), (Some('⚡'), "Build"));
        assert_eq!(split_title_icon("Terminal"), (None, "Terminal"));
        assert_eq!(split_title_icon("Änderung"), (None, "Änderung"));
        assert_eq!(split_title_icon("⚡Build"), (None, "⚡Build"));
    }
}
