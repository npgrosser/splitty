//! Window-local Screen catalog presentation. The service catalog remains authoritative.
use super::popover::{MenuEntry, PopoverAnchor, PopoverKind};
use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonCustomVariant, ButtonVariants},
    Disableable, Icon, Sizable,
};
use splitty_client_core::ports::ForwardState;
use splitty_client_core::workspace::ScreenState;

// Drawer metrics from the web client's `Drawer.svelte`, in CSS pixels at zoom
// 1; every one passes through `chrome_rems`. Colors come from the theme.

/// `.drawer-sidebar`: the Drawer's own text size, independent of the theme's.
/// Its text inherits the web client's body weight, 500. Both are set on each
/// text element rather than the Drawer itself: menus opened from the Drawer
/// inherit its text style.
pub(super) const SIDEBAR_TEXT: f32 = 12.5;
/// `.drawer-head`: `padding: 10px 8px 6px 16px`.
const HEAD_PADDING: [f32; 4] = [10., 8., 6., 16.];
/// `.drawer-head-label`: 10.5px, weight 600, uppercase.
pub(super) const HEAD_LABEL_TEXT: f32 = 10.5;
/// `.drawer-icon-btn`: a 24 px square with radius 5.
const ICON_BUTTON: f32 = 24.;
pub(super) const ICON_BUTTON_RADIUS: f32 = 5.;
/// `.drawer-screens`: `padding: 2px 0 4px`. The native cards keep only their
/// top margin, so the bottom padding also takes the last card's 1 px margin.
const LIST_PADDING_TOP: f32 = 2.;
const LIST_PADDING_BOTTOM: f32 = 5.;
/// `.drawer-screen`: `margin: 1px 8px`, where the vertical margins of
/// neighboring cards collapse into one 1 px gap; `padding: 1px 0 3px`.
pub(super) const CARD_INSET: f32 = 8.;
const CARD_GAP: f32 = 1.;
const CARD_PADDING_TOP: f32 = 1.;
const CARD_PADDING_BOTTOM: f32 = 3.;
/// `.drawer-screen`: `border-radius: 6px`, fixed rather than themed.
const CARD_RADIUS: f32 = 6.;
/// `.drawer-screen.unreachable:not(.active)`: a Screen of a host that failed
/// or went offline.
const UNREACHABLE_OPACITY: f32 = 0.55;
/// `.drawer-screen-row`: `height: 30px; padding: 0 4px 0 8px; gap: 8px`.
const ROW_HEIGHT: f32 = 30.;
const ROW_PADDING_LEFT: f32 = 8.;
const ROW_PADDING_RIGHT: f32 = 4.;
const ROW_GAP: f32 = 8.;
/// `.drawer-screen-index`: 14 px wide, 10.5px monospace.
const INDEX_WIDTH: f32 = 14.;
const INDEX_TEXT: f32 = 10.5;
/// `.drawer-screen-actions`: `gap: 1px` between the 24 px buttons.
const ACTION_GAP: f32 = 1.;
/// `.drawer-screen-badge`: a 16 px pill, `padding: 0 5px`, 10px weight 600.
const BADGE_HEIGHT: f32 = 16.;
const BADGE_PADDING_X: f32 = 5.;
const BADGE_TEXT: f32 = 10.;
/// A Screen's own status ("Loading", "Offline"), sized as `.drawer-host-note`.
pub(super) const NOTE_TEXT: f32 = 10.5;
/// `.drawer-screen-input`: `padding: 2px 6px; border-radius: 4px`.
const RENAME_PADDING: [f32; 2] = [2., 6.];
const RENAME_RADIUS: f32 = 4.;
/// `.drawer-pane-item`: `height: 21px; padding: 0 8px 0 30px; gap: 7px`,
/// 11px monospace.
const PANE_HEIGHT: f32 = 21.;
const PANE_INDENT: f32 = 30.;
const PANE_PADDING_RIGHT: f32 = 8.;
const PANE_GAP: f32 = 7.;
const PANE_TEXT: f32 = 11.;
/// `.drawer-pane-dot`: a 5 px dot in a 12 px slot, which also holds the
/// leading title icon and the viewer's document icon.
const PANE_GLYPH_SLOT: f32 = 12.;
const PANE_DOT: f32 = 5.;
/// `.drawer-pane-icon`: 10px, 12px while a process runs; `.drawer-pane-doc`: 10 px.
const PANE_ICON_TEXT: f32 = 10.;
const PANE_RUNNING_ICON_TEXT: f32 = 12.;
const PANE_DOC_ICON: f32 = 10.;
/// A running pane's title icon glows in its own color, as the web client's
/// `drop-shadow(0 0 2px currentColor)`: a soft round light behind the glyph.
const PANE_GLOW_CORE: f32 = 6.;
const PANE_GLOW_BLUR: f32 = 4.;
const PANE_GLOW_OPACITY: f32 = 0.6;
/// `.drawer-pane-badge`: a 6 px unread dot.
pub(super) const PANE_BADGE: f32 = 6.;
/// `.drawer-pane-stack`: `padding-left: 9px` after the pane indent, with a
/// 1.5 px rail inset 4 px from both ends and rounded by 1 px.
const STACK_PADDING_LEFT: f32 = 9.;
const STACK_RAIL_WIDTH: f32 = 1.5;
const STACK_RAIL_INSET: f32 = 4.;
const STACK_RAIL_RADIUS: f32 = 1.;
/// `.drawer-new`: `margin: 4px 8px 0`, 32 px tall, radius 6, `gap: 8px`, and
/// a 14 px plus in a 14 px slot.
/// `.drawer-spacer`: `min-height: 8px`.
const SPACER_MIN_HEIGHT: f32 = 8.;
/// `.drawer-nav`: `padding: 6px 8px 10px; gap: 1px`.
const NAV_PADDING: [f32; 3] = [6., 8., 10.];
const NAV_GAP: f32 = 1.;
/// `.drawer-nav-item`: 30 px tall, `padding: 0 8px`, `gap: 10px`, radius 6,
/// and a 15 px icon at opacity .8 (1 while active).
const NAV_ITEM_HEIGHT: f32 = 30.;
const NAV_ITEM_PADDING_X: f32 = 8.;
const NAV_ITEM_GAP: f32 = 10.;
const NAV_ITEM_RADIUS: f32 = 6.;
const NAV_ICON: f32 = 15.;
const NAV_ICON_OPACITY: f32 = 0.8;
/// `.drawer-nav-shortcut`: 10px in the system face.
const NAV_SHORTCUT_TEXT: f32 = 10.;

/// The key of a destination as its Drawer entry shows it, from the one
/// table the command palette and the shortcut list read.
pub(super) fn nav_shortcut(shortcut: Shortcut, cx: &App) -> SharedString {
    super::palette::shortcut_label(shortcut, cx.platform_info().is_mac()).into()
}

/// One destination of the Drawer's bottom navigation.
pub(super) struct NavItem {
    pub(super) icon: IconName,
    pub(super) label: &'static str,
    /// The destination's keyboard shortcut or a short state, shown as a hint.
    pub(super) shortcut: Option<SharedString>,
    pub(super) active: bool,
}

/// A Drawer control in the web client's `.drawer-icon-btn` manner: `rest` at
/// rest; on hover the primary text color over one `hover-bg` layer, which
/// stacks over a hovered or active card as the translucent web color does.
pub(super) fn drawer_control(
    id: impl Into<ElementId>,
    rest: Hsla,
    radius: f32,
    appearance: &NativeAppearance,
    window: &Window,
    cx: &App,
) -> Button {
    let hover: Hsla = rgba(appearance.hover_overlay).into();
    Button::new(id)
        .focus_ring(focus_visible(cx))
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(rgb(appearance.text).into())
                .hover(hover)
                .active(hover),
        )
        .bg(transparent_black())
        .text_color(rest)
        .rounded(chrome_rems(radius).to_pixels(window.rem_size()))
        .cursor_pointer()
        // A ring marks keyboard focus only; a click leaves none behind.
        .focus_ring(focus_visible(cx))
}

fn pane_list_transition() -> Transition {
    Transition::new(Duration::from_millis(150)).easing(Easing::CubicBezier {
        x1: 0.25,
        y1: 0.1,
        x2: 0.25,
        y2: 1.,
    })
}

#[derive(Clone)]
pub(super) struct ScreenDrag {
    pub(super) owner: SourceToken,
    pub(super) screen: ScreenKey,
    pub(super) view: EntityId,
}

/// The platform drag's own preview stays empty: the lifted row is painted in
/// the Drawer list itself, where it follows the pointer vertically.
pub(super) struct ScreenDragPreview;

impl Render for ScreenDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WheelTarget {
    Screens,
    Slot { screen: String, slot: String },
}

fn chrome_px(base: f32, window_scale: f32) -> Pixels {
    px(base * window_scale)
}

impl Workspace {
    /// A Screen card's top or bottom padding. It is part of the highlighted
    /// card, so a click there selects the Screen like a click on its row.
    fn card_padding(
        &self,
        key: &ScreenKey,
        owner: Option<SourceToken>,
        editing: bool,
        edge: &str,
        height: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let weak = cx.entity().downgrade();
        let select = key.clone();
        div()
            .id(SharedString::from(format!("drawer-card-{edge}-{key}")))
            .flex_none()
            .w_full()
            .h(chrome_rems(height))
            .when(!editing, |padding| {
                padding.cursor_pointer().on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    if !event.standard_click() {
                        return;
                    }
                    let _ = weak.update(cx, |this, cx| {
                        if this.lifecycle_token() == owner
                            && !GlobalState::is_in_deferred_context(cx)
                        {
                            this.switch_screen_key(select.clone(), window, cx);
                            this.focus_current(window, cx);
                        }
                    });
                })
            })
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use super::{Command, Destination, DropTarget, Workspace};
    use gpui_kit::{
        point, px, size, AppContext, Entity, Focusable, Modifiers, MouseButton, MouseDownEvent,
        MouseUpEvent, Pixels, Point, ScrollDelta, ScrollEvent, TestAppContext, TouchPhase,
        VisualTestContext,
    };
    use serde_json::json;
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

    #[test]
    fn remote_screen_menu_survives_leaving_its_row() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let (key, _) = model.ui_test_remote_terminal_fixture("menu.test").unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.reconcile(model, window, cx);
                    workspace.drawer_hovered = Some(key.clone());
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let selector = Box::leak(format!("drawer-more-{key}").into_boxed_str());
            let button = window.debug_bounds(selector).unwrap();
            window.simulate_click(button.center(), Modifiers::default());
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    assert_eq!(workspace.drawer_screen_menu.as_ref(), Some(&key));
                    workspace.drawer_hovered = None;
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            assert!(
                window.debug_bounds(selector).is_some(),
                "keep the dropdown mounted while the pointer enters its menu"
            );
            window.simulate_click(point(px(700.), px(400.)), Modifiers::default());
            window.update(|_, cx| {
                view.update(cx, |workspace, _| {
                    assert!(workspace.drawer_screen_menu.is_none());
                })
            });
        });
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

    #[test]
    fn hidden_screen_title_snapshot_updates_drawer_label_and_leading_icon() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B")], None)
                .unwrap();
            let owner = model.lifecycle_token();
            for (screen, title, revision) in [("a", "Old title", 1), ("b", "Other", 1)] {
                let pane = format!("pane-{screen}");
                let mut snapshot = json!({"type":"mux.screen_snapshot","screen":{
                    "id":screen,"revision":revision,
                    "root":{"type":"slot","id":format!("slot-{screen}"),"paneIds":[pane.clone()]},
                    "panes":{},
                    "sessions":{}
                }});
                snapshot["screen"]["panes"][pane.as_str()] = json!({"id":pane,"title":title});
                model
                    .source_mut(owner.source_id())
                    .unwrap()
                    .apply_snapshot(owner.connection_id(), &snapshot)
                    .unwrap();
            }
            model.select(owner, &keys[1]).unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.reconcile(model.clone(), window, cx)
                });
                window.draw(cx).clear(cx);
            });
            assert!(window
                .debug_bounds("drawer-title-label-pane-a-Old title")
                .is_some());
            assert!(window.debug_bounds("drawer-title-icon-pane-a-✳").is_none());

            model
                .source_mut(owner.source_id())
                .unwrap()
                .apply_snapshot(
                    owner.connection_id(),
                    &json!({"type":"mux.screen_snapshot","screen":{
                        "id":"a","revision":2,
                        "root":{"type":"slot","id":"slot-a","paneIds":["pane-a"]},
                        "panes":{"pane-a":{"id":"pane-a","title":"✳ Live work"}},
                        "sessions":{}
                    }}),
                )
                .unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.reconcile(model.clone(), window, cx)
                });
                window.draw(cx).clear(cx);
            });
            assert!(window
                .debug_bounds("drawer-title-label-pane-a-Old title")
                .is_none());
            assert!(window
                .debug_bounds("drawer-title-label-pane-a-Live work")
                .is_some());
            assert!(window.debug_bounds("drawer-title-icon-pane-a-✳").is_some());
            assert_eq!(
                view.read_with(window, |workspace, _| workspace.current_screen_key()),
                Some(keys[1].clone())
            );

            model.select(owner, &keys[0]).unwrap();
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |workspace, cx| {
                    workspace.reconcile(model.clone(), window, cx)
                });
                window.draw(cx).clear(cx);
                cx.set_reduce_motion(false);
            });
            assert!(window
                .debug_bounds("tab-title-pane-a-✳ Live work")
                .is_some());

            model
                .source_mut(owner.source_id())
                .unwrap()
                .apply_snapshot(
                    owner.connection_id(),
                    &json!({"type":"mux.screen_snapshot","screen":{
                        "id":"a","revision":3,
                        "root":{"type":"slot","id":"slot-a","paneIds":["pane-a"]},
                        "panes":{"pane-a":{"id":"pane-a"}},
                        "sessions":{}
                    }}),
                )
                .unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| workspace.reconcile(model, window, cx));
                window.draw(cx).clear(cx);
            });
            assert!(window.debug_bounds("tab-title-pane-a-Terminal 1").is_some());
            assert!(window
                .debug_bounds("tab-title-pane-a-✳ Live work")
                .is_none());
            assert!(window
                .debug_bounds("drawer-title-label-pane-a-Terminal")
                .is_some());
        });
    }

    #[test]
    fn drawer_wheel_keeps_one_swipe_across_selection_updates_until_idle() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                .unwrap();
            let owner = model.lifecycle_token();
            model.select(owner, &keys[0]).unwrap();
            let (commands, mut receiver) = mpsc::channel(16);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model.clone(), window, cx);
                });
                window.draw(cx).clear(cx);
            });
            while receiver.try_recv().is_ok() {}
            let selector: &'static str =
                Box::leak(format!("drawer-select-{}", keys[0]).into_boxed_str());
            let position = window.debug_bounds(selector).unwrap().center();
            let scroll = |window: &mut VisualTestContext| {
                window.simulate_event(ScrollEvent {
                    position,
                    delta: ScrollDelta::Lines(point(0., -3.)),
                    touch_phase: TouchPhase::Moved,
                    ..Default::default()
                });
            };
            scroll(window);
            assert!(matches!(
                receiver.try_recv().unwrap().kind(),
                Command::CycleScreen {
                    step: splitty_client_core::workspace::TabStep::Next
                }
            ));
            model.select(owner, &keys[1]).unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| workspace.reconcile(model, window, cx))
            });
            while let Ok(command) = receiver.try_recv() {
                assert!(
                    matches!(command.kind(), Command::Visible { .. }),
                    "selection only updates visible routing"
                );
            }
            for _ in 0..10 {
                advance(window, Duration::from_millis(20));
                scroll(window);
            }
            assert!(
                receiver.try_recv().is_err(),
                "selection acknowledgement and momentum cannot repeat the swipe"
            );
            advance(window, Duration::from_millis(120));
            scroll(window);
            assert!(
                receiver.try_recv().is_err(),
                "idle threshold is strict and follows the last event"
            );
            advance(window, Duration::from_millis(121));
            scroll(window);
            assert!(matches!(
                receiver.try_recv().unwrap().kind(),
                Command::CycleScreen {
                    step: splitty_client_core::workspace::TabStep::Next
                }
            ));
            assert!(receiver.try_recv().is_err(), "one command per new swipe");
        });
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
                assert!(
                    workspace
                        .drawer_drag
                        .as_ref()
                        .is_some_and(|session| !session.is_dragging()),
                    "the dropped arrangement waits for the service's order"
                );
            });
        });
    }

    #[test]
    fn card_padding_selects_its_screen_like_the_row() {
        with_view(|window, view| {
            let owner = Uuid::new_v4();
            let mut model = WindowWorkspace::new(owner);
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B")], None)
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            let other = keys
                .iter()
                .find(|key| {
                    window.update(|_, cx| {
                        view.read(cx).model.as_ref().unwrap().shown_key().as_ref() != Some(*key)
                    })
                })
                .expect("a Screen that is not shown")
                .clone();
            let card = block_bounds(window, &other);
            // The bottom padding lies below the row and any pane entries.
            let padding = point(card.center().x, card.bottom() - px(1.));
            mouse_click(window, padding, MouseButton::Left, 1);
            let command = receiver.try_recv().expect("the padding selects its Screen");
            assert!(matches!(command.kind(), Command::SwitchScreen(screen) if screen == &other));
            assert!(receiver.try_recv().is_err(), "one selection per click");
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
    fn header_cards_rows_and_panes_keep_web_drawer_metrics() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B")], None)
                .unwrap();
            let owner = model.lifecycle_token();
            // A stacked group of two panes beside a single pane.
            model
                .source_mut(owner.source_id())
                .unwrap()
                .apply_snapshot(
                    owner.connection_id(),
                    &json!({"type":"mux.screen_snapshot","screen":{
                        "id":"a","revision":1,
                        "root":{"type":"split","id":"split","axis":"horizontal",
                            "sizes":[0.5,0.5],"children":[
                                {"type":"slot","id":"left","paneIds":["p0","p1"]},
                                {"type":"slot","id":"right","paneIds":["p2"]}
                            ]},
                        "panes":{"p0":{"id":"p0"},"p1":{"id":"p1"},"p2":{"id":"p2"}},
                        "sessions":{}
                    }}),
                )
                .unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, _| {
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                });
                window.draw(cx).clear(cx);
            });
            let bounds = |window: &mut VisualTestContext, selector: String| {
                window
                    .debug_bounds(Box::leak(selector.into_boxed_str()))
                    .unwrap()
            };
            let pin = window.debug_bounds("drawer-pin").unwrap();
            assert_near(pin.size.width, px(24.), "pin width");
            assert_near(pin.size.height, px(24.), "pin height");
            assert_near(pin.top(), px(10.), "header top padding");
            let first = block_bounds(window, &keys[0]);
            let second = block_bounds(window, &keys[1]);
            assert_near(
                first.top(),
                pin.bottom() + px(6. + 2. + 34. + 1.),
                "header, local group caption, list padding and card margin",
            );
            assert_near(first.left(), px(8.), "card inset");
            assert_near(first.size.width, px(200. - 1. - 16.), "card width");
            assert_near(
                first.size.height,
                px(1. + 30. + 3. * 21. + 3.),
                "card padding, row and three panes",
            );
            assert_near(second.size.height, px(1. + 30. + 3.), "card without panes");
            assert_near(second.top() - first.bottom(), px(1.), "gap between cards");
            let index = bounds(window, format!("drawer-index-{}", keys[0]));
            assert_near(index.left() - first.left(), px(8.), "row left padding");
            assert_near(index.size.width, px(14.), "index width");
            let stacked = bounds(window, format!("drawer-pane-{}-p0", keys[0]));
            let single = bounds(window, format!("drawer-pane-{}-p2", keys[0]));
            assert_near(stacked.size.height, px(21.), "pane height");
            assert_near(
                stacked.top() - first.top(),
                px(1. + 30.),
                "panes follow the row",
            );
            assert_near(
                stacked.left() - first.left(),
                px(30. + 9.),
                "a stacked pane sits past the indent and the rail",
            );
            assert_near(single.left(), first.left(), "a single pane indents inside");
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
            assert!(window.update(|_, cx| view
                .read(cx)
                .drawer_drag
                .as_ref()
                .is_some_and(|session| session.is_lifted())));
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
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| workspace.focus.focus(window, cx));
            });
            window.simulate_keystrokes("escape");
            assert!(
                window.update(|_, cx| !cx.has_active_drag() && view.read(cx).drawer_drag.is_none())
            );
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
            assert!(window.update(|_, cx| view.read(cx).drawer_drag.is_none()));
        });
    }

    fn block_bounds(
        window: &mut VisualTestContext,
        screen: &splitty_client_core::workspace::ScreenKey,
    ) -> gpui_kit::Bounds<Pixels> {
        let selector: &'static str = Box::leak(format!("drawer-block-{screen}").into_boxed_str());
        window.debug_bounds(selector).unwrap()
    }

    fn assert_near(actual: Pixels, expected: Pixels, what: &str) {
        assert!(
            (f32::from(actual) - f32::from(expected)).abs() < 1.,
            "{what}: {actual:?} is not {expected:?}"
        );
    }

    fn sent_moves(
        receiver: &mut mpsc::Receiver<splitty_client_core::native_workspace::Command>,
    ) -> Vec<(
        splitty_client_core::workspace::ScreenKey,
        Option<splitty_client_core::workspace::ScreenKey>,
    )> {
        let mut moves = Vec::new();
        while let Ok(command) = receiver.try_recv() {
            if let Command::MoveScreen { screen, before } = command.kind() {
                moves.push((screen.clone(), before.clone()));
            }
        }
        moves
    }

    #[test]
    fn screen_drag_rows_make_room_with_web_motion_and_cancel_returns_them_at_once() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(4);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.model = Some(model);
                    workspace.destination = Destination::Screens;
                    workspace.focus.focus(window, cx);
                });
                window.draw(cx).clear(cx);
            });
            let before: Vec<_> = keys.iter().map(|key| block_bounds(window, key)).collect();
            let room = before[1].top() - before[0].top();
            let select: &'static str =
                Box::leak(format!("drawer-select-{}", keys[2]).into_boxed_str());
            let press = window.debug_bounds(select).unwrap().center();
            let finish = hover_screen_drop(window, &keys[2], &keys[0]);
            let lift = finish.y - press.y;
            assert_near(
                block_bounds(window, &keys[2]).top(),
                before[2].top() + lift,
                "the lifted row keeps the grabbed point under the pointer",
            );
            assert_near(
                block_bounds(window, &keys[0]).top(),
                before[0].top(),
                "neighbors start from their slots",
            );
            advance(window, Duration::from_millis(100));
            let halfway = block_bounds(window, &keys[0]).top() - before[0].top();
            assert!(
                halfway > room * 0.5 && halfway < room,
                "the shift eases out: {halfway:?} of {room:?} at half time"
            );
            advance(window, Duration::from_millis(150));
            for index in 0..2 {
                assert_near(
                    block_bounds(window, &keys[index]).top(),
                    before[index].top() + room,
                    "passed rows make room by one row pitch",
                );
            }
            assert_near(
                block_bounds(window, &keys[2]).top(),
                before[2].top() + lift,
                "the lifted row does not ease",
            );
            window.simulate_keystrokes("escape");
            redraw(window);
            for (key, slot) in keys.iter().zip(&before) {
                assert_near(
                    block_bounds(window, key).top(),
                    slot.top(),
                    "a cancel returns every row to its slot at once",
                );
            }
            window.simulate_mouse_up(finish, MouseButton::Left, Modifiers::default());
            assert!(sent_moves(&mut receiver).is_empty());
        });
    }

    #[test]
    fn screen_drop_holds_the_landed_order_until_the_service_order_arrives() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let keys = model
                .ui_test_screen_order_fixture(&[("a", "A"), ("b", "B"), ("c", "C")], None)
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(32);
            let reconcile = |window: &mut VisualTestContext, model: &WindowWorkspace| {
                window.update(|window, cx| {
                    view.update(cx, |workspace, cx| {
                        workspace.reconcile(model.clone(), window, cx)
                    });
                    window.draw(cx).clear(cx);
                });
            };
            window.update(|_, cx| {
                view.update(cx, |workspace, _| {
                    workspace.commands = commands;
                    workspace.destination = Destination::Screens;
                });
            });
            reconcile(window, &model);
            let slots: Vec<_> = keys
                .iter()
                .map(|key| block_bounds(window, key).top())
                .collect();
            let tops = |window: &mut VisualTestContext| {
                keys.iter()
                    .map(|key| block_bounds(window, key).top())
                    .collect::<Vec<_>>()
            };
            let assert_tops = |actual: Vec<Pixels>, expected: [Pixels; 3], what: &str| {
                for (actual, expected) in actual.into_iter().zip(expected) {
                    assert_near(actual, expected, what);
                }
            };
            sent_moves(&mut receiver);

            drag_screen(window, &keys[0], &keys[2], true);
            assert_eq!(sent_moves(&mut receiver), [(keys[0].clone(), None)]);
            redraw(window);
            assert_tops(
                tops(window),
                [slots[2], slots[0], slots[1]],
                "the dropped row sits in its gap while the order is submitted",
            );
            reconcile(window, &model);
            assert_tops(
                tops(window),
                [slots[2], slots[0], slots[1]],
                "an update with the old order keeps the landed presentation",
            );

            model
                .ui_test_screen_order_fixture(&[("b", "B"), ("c", "C"), ("a", "A")], None)
                .unwrap();
            reconcile(window, &model);
            assert!(window.update(|_, cx| view.read(cx).drawer_drag.is_none()));
            assert_tops(
                tops(window),
                [slots[2], slots[0], slots[1]],
                "the service's order takes over without a visible jump",
            );

            drag_screen(window, &keys[1], &keys[0], true);
            assert_eq!(sent_moves(&mut receiver), [(keys[1].clone(), None)]);
            redraw(window);
            assert_tops(
                tops(window),
                [slots[1], slots[2], slots[0]],
                "a second drop is held as well",
            );
            advance(window, Duration::from_millis(999));
            assert_tops(
                tops(window),
                [slots[1], slots[2], slots[0]],
                "the hold lasts while an answer may still come",
            );
            advance(window, Duration::from_millis(2));
            assert!(window.update(|_, cx| view.read(cx).drawer_drag.is_none()));
            assert_tops(
                tops(window),
                [slots[2], slots[0], slots[1]],
                "an unanswered drop returns to the model's order",
            );
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
                !std::iter::from_fn(|| receiver.try_recv().ok())
                    .any(|command| !matches!(command.kind(), Command::AssessClose { .. })),
                "disabled menu cannot send fallback"
            );
        });
    }

    #[test]
    fn name_double_click_f2_and_context_menu_enter_exact_local_rename_editor() {
        exercise_name_interactions();
    }

    #[test]
    fn remote_name_editor_keeps_its_source_when_selected_ids_collide() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let local = model
                .ui_test_screen_order_fixture(&[("same", "Local")], None)
                .unwrap()[0]
                .clone();
            let remote = model
                .ui_test_screen_order_fixture(
                    &[("same", "A longer live remote screen title")],
                    Some("vm.test"),
                )
                .unwrap()[0]
                .clone();
            let owner = model.lifecycle_token();
            let remote_source = model.execution_token(remote.source_id()).unwrap();
            assert!(model
                .source(remote.source_id())
                .unwrap()
                .validate_screen_rename(remote.screen_id())
                .is_ok());
            assert!(super::super::rename_binding_current(
                &model,
                owner,
                remote_source,
                &remote
            ));
            model.select(owner, &local).unwrap();
            let (commands, mut receiver) = mpsc::channel(16);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model.clone(), window, cx);
                    workspace.drawer_visible = true;
                });
                window.draw(cx).clear(cx);
            });
            window.run_until_parked();
            redraw(window);
            while receiver.try_recv().is_ok() {}
            let selector: &'static str =
                Box::leak(format!("drawer-select-{remote}").into_boxed_str());
            let bounds = window.debug_bounds(selector).unwrap();
            let click_point = bounds.center();
            window.simulate_mouse_move(click_point, None, Modifiers::default());
            redraw(window);
            window.update(|_, cx| assert_eq!(view.read(cx).drawer_hovered, Some(remote.clone())));
            assert_eq!(
                window.debug_bounds(selector),
                Some(bounds),
                "hover keeps the rename/select hitbox stable"
            );
            let more_selector: &'static str =
                Box::leak(format!("drawer-more-{remote}").into_boxed_str());
            let more = window.debug_bounds(more_selector).unwrap();
            assert!(
                bounds.right() < more.origin.x,
                "the complete title remains available in its tooltip without drawing into the action lane"
            );
            let state_selector: &'static str =
                Box::leak(format!("drawer-state-{remote}-Loading").into_boxed_str());
            let state = window.debug_bounds(state_selector).unwrap();
            assert!(
                state.right() < more.origin.x,
                "loading status stays outside the action lane"
            );
            window.simulate_mouse_down(click_point, MouseButton::Left, Modifiers::default());
            window.simulate_mouse_up(click_point, MouseButton::Left, Modifiers::default());
            assert!(matches!(
                receiver.try_recv().expect("first click selects row").kind(),
                Command::SwitchScreen(screen) if screen == &remote
            ));
            mouse_click(window, click_point, MouseButton::Left, 2);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    let editor = workspace.rename.as_ref().unwrap();
                    assert_eq!(editor.owner, owner);
                    assert_eq!(editor.source, remote_source);
                    assert_eq!(editor.screen, remote);
                    assert!(editor.input.read(cx).focus_handle(cx).is_focused(window));
                    workspace.reconcile(model.clone(), window, cx);
                    let editor = workspace.rename.as_ref().unwrap();
                    assert!(editor.input.read(cx).focus_handle(cx).is_focused(window));
                    editor.input.update(cx, |input, cx| {
                        input.set_value("Renamed Remote", window, cx)
                    });
                });
            });
            window.simulate_keystrokes("enter");
            let command = receiver.try_recv().expect("one exact rename");
            assert_eq!(command.source_token(), Some(remote_source));
            assert!(matches!(
                command.kind(),
                Command::RenameScreen { screen, connection, name }
                    if screen == "same"
                        && *connection == remote_source.connection_id()
                        && name == "Renamed Remote"
            ));
            assert!(receiver.try_recv().is_err());
            window.update(|_, cx| {
                let workspace = view.read(cx);
                let model = workspace.model.as_ref().unwrap();
                assert_eq!(model.shown_key(), Some(local.clone()));
                assert_eq!(
                    model.local_source().screens()[0].name.as_deref(),
                    Some("Local")
                );
                assert_eq!(
                    model.source(remote.source_id()).unwrap().screens()[0]
                        .name
                        .as_deref(),
                    Some("A longer live remote screen title"),
                    "queue admission does not optimistically change the name"
                );
            });
        });
    }

    #[test]
    fn remote_name_editor_refuses_invalid_full_queue_and_lost_connection() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let remote = model
                .ui_test_screen_order_fixture(&[("work", "Original")], Some("vm.test"))
                .unwrap()[0]
                .clone();
            let owner = model.lifecycle_token();
            let source = model.execution_token(remote.source_id()).unwrap();
            let (commands, mut receiver) = mpsc::channel(1);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.reconcile(model.clone(), window, cx);
                    workspace.commands = commands.clone();
                    workspace.start_captured_rename(owner, source, &remote, window, cx);
                    workspace
                        .rename
                        .as_ref()
                        .unwrap()
                        .input
                        .update(cx, |input, cx| input.set_value("x".repeat(257), window, cx));
                    workspace.finish_rename(true, window, cx);
                    assert!(workspace.status.contains("Rename unavailable"));
                    assert!(workspace.status.contains("screen name too long"));
                    assert!(workspace.rename.is_none());
                });
            });
            assert!(receiver.try_recv().is_err());

            commands
                .try_send(super::super::WorkerCommand::new(
                    owner,
                    Command::SwitchScreen(remote.clone()),
                ))
                .unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.start_captured_rename(owner, source, &remote, window, cx);
                    workspace
                        .rename
                        .as_ref()
                        .unwrap()
                        .input
                        .update(cx, |input, cx| {
                            input.set_value("Valid new name", window, cx)
                        });
                    workspace.finish_rename(true, window, cx);
                    assert!(workspace.status.contains("Action not sent"));
                    assert_eq!(
                        workspace
                            .model
                            .as_ref()
                            .unwrap()
                            .source(remote.source_id())
                            .unwrap()
                            .screens()[0]
                            .name
                            .as_deref(),
                        Some("Original")
                    );
                });
            });
            assert!(matches!(
                receiver.try_recv().unwrap().kind(),
                Command::SwitchScreen(_)
            ));
            assert!(
                receiver.try_recv().is_err(),
                "full queue does not replay rename"
            );

            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.start_captured_rename(owner, source, &remote, window, cx);
                    let editor = workspace.rename.as_ref().unwrap();
                    assert!(editor.input.read(cx).focus_handle(cx).is_focused(window));
                    model.disconnect(remote.source_id()).unwrap();
                    workspace.reconcile(model.clone(), window, cx);
                    assert!(
                        workspace.rename.is_none(),
                        "lost source discards exact editor"
                    );
                    workspace.start_captured_rename(owner, source, &remote, window, cx);
                    assert!(
                        workspace.rename.is_none(),
                        "old capture cannot revive offline"
                    );
                });
            });
            assert!(receiver.try_recv().is_err());
        });
    }

    #[test]
    fn remote_name_context_and_f2_share_editor_and_escape_cancels() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let remote = model
                .ui_test_screen_order_fixture(&[("work", "Work")], Some("vm.test"))
                .unwrap()[0]
                .clone();
            let source = model.execution_token(remote.source_id()).unwrap();
            let (commands, mut receiver) = mpsc::channel(2);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model, window, cx);
                    workspace.drawer_visible = true;
                });
                window.draw(cx).clear(cx);
            });
            while receiver.try_recv().is_ok() {}
            window.update(|window, _| window.activate_window());
            window.run_until_parked();
            let selector: &'static str =
                Box::leak(format!("drawer-select-{remote}").into_boxed_str());
            let name = window.debug_bounds(selector).unwrap();
            mouse_click(window, name.center(), MouseButton::Right, 1);
            window.simulate_keystrokes("down enter");
            window.update(|window, cx| {
                let workspace = view.read(cx);
                let editor = workspace.rename.as_ref().expect("remote menu editor");
                assert_eq!(editor.screen, remote);
                assert_eq!(editor.source, source);
                assert!(editor.input.read(cx).focus_handle(cx).is_focused(window));
            });
            window.simulate_keystrokes("escape");
            assert!(window.update(|_, cx| view.read(cx).rename.is_none()));
            assert!(receiver.try_recv().is_err());

            for _ in 0..48 {
                window.simulate_keystrokes("tab f2");
                if window.update(|_, cx| view.read(cx).rename.is_some()) {
                    break;
                }
            }
            redraw(window);
            window.run_until_parked();
            window.update(|window, cx| {
                let workspace = view.read(cx);
                let editor = workspace.rename.as_ref().expect("remote F2 editor");
                assert_eq!(editor.screen, remote);
                assert_eq!(editor.source, source);
                assert!(editor.input.read(cx).focus_handle(cx).is_focused(window));
            });
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace
                        .rename
                        .as_ref()
                        .unwrap()
                        .input
                        .update(cx, |input, cx| input.set_value("From Blur", window, cx));
                });
                window.blur(cx);
            });
            window.run_until_parked();
            assert!(window.update(|_, cx| view.read(cx).rename.is_none()));
            let sent = receiver.try_recv().expect("blur submits one rename");
            assert_eq!(sent.source_token(), Some(source));
            assert!(
                matches!(sent.kind(), Command::RenameScreen { name, .. } if name == "From Blur")
            );
            assert!(receiver.try_recv().is_err());
        });
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
                    workspace.start_captured_rename(source, source, &screen, window, cx);
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

    #[test]
    fn pane_list_collapse_reverses_from_displayed_height_and_respects_reduced_motion() {
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
                    &json!({"type":"mux.screen_snapshot","screen":{
                        "id":"work","revision":2,
                        "root":{"type":"split","id":"split","axis":"horizontal",
                            "sizes":[0.5,0.5],"children":[
                                {"type":"slot","id":"left","paneIds":["p0"]},
                                {"type":"slot","id":"right","paneIds":["p1"]}
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
            let selector: &'static str =
                Box::leak(format!("drawer-panes-{screen}").into_boxed_str());
            let height = |window: &mut VisualTestContext| {
                f32::from(window.debug_bounds(selector).unwrap().size.height)
            };
            let full = height(window);
            assert!(full > 0.);
            let pane_selector: &'static str =
                Box::leak(format!("drawer-pane-{screen}-p0").into_boxed_str());
            let second_pane_selector: &'static str =
                Box::leak(format!("drawer-pane-{screen}-p1").into_boxed_str());
            let pane_position = window.debug_bounds(pane_selector).unwrap().center();
            let row_distance = |window: &mut VisualTestContext| {
                let first = window.debug_bounds(pane_selector).unwrap();
                let second = window.debug_bounds(second_pane_selector).unwrap();
                f32::from(second.origin.y - first.origin.y)
            };
            let full_row_distance = row_distance(window);
            assert!(full_row_distance > 0.);

            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.drawer_collapsed.insert(screen.clone());
                    cx.notify();
                });
            });
            redraw(window);
            assert!((height(window) - full).abs() < 1.);
            mouse_click(window, pane_position, MouseButton::Left, 1);
            assert!(
                receiver.try_recv().is_err(),
                "a closing pane cannot be selected"
            );
            advance(window, Duration::from_millis(50));
            let midway = height(window);
            assert!(midway > 0. && midway < full);
            assert!(
                (row_distance(window) - full_row_distance).abs() < 1.,
                "pane groups keep their row positions while the outer list clips"
            );

            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.drawer_collapsed.remove(&screen);
                    cx.notify();
                });
            });
            redraw(window);
            assert!((height(window) - midway).abs() < 1.);
            advance(window, Duration::from_millis(150));
            assert!((height(window) - full).abs() < 1.);

            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                view.update(cx, |workspace, cx| {
                    workspace.drawer_collapsed.insert(screen.clone());
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            assert!(height(window) < 1.);
            assert!(
                receiver.try_recv().is_err(),
                "pane motion sends no commands"
            );

            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.drawer_collapsed.clear();
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            let block_selector: &'static str =
                Box::leak(format!("drawer-block-{screen}").into_boxed_str());
            let block = window.debug_bounds(block_selector).unwrap();
            window.simulate_mouse_move(block.center(), None, Modifiers::default());
            redraw(window);
            let collapse_selector: &'static str =
                Box::leak(format!("drawer-collapse-{screen}").into_boxed_str());
            let collapse_position = window.debug_bounds(collapse_selector).unwrap().center();
            mouse_click(window, collapse_position, MouseButton::Left, 1);
            assert!(window.update(|window, cx| view.read(cx).focus.is_focused(window)));
            assert!(receiver.try_recv().is_err());
        });
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
    screen_switched: bool,
    screen_last_scroll: Option<Instant>,
}

impl WheelNavigation {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn screen_changed(&mut self) {
        // A navigation acknowledgement must not turn its remaining momentum
        // into another gesture. Tab distance still belongs to its old group.
        if self.target != Some(WheelTarget::Screens) {
            self.reset();
        }
    }

    fn admit(
        &mut self,
        target: WheelTarget,
        event: &ScrollEvent,
        blocked: bool,
        now: Instant,
    ) -> i32 {
        if blocked {
            self.reset();
            return 0;
        }
        self.steps(target, event, now)
    }

    /// Drawer gestures follow the web swipe gate. Tabs retain their separate
    /// distance-based scrolling; terminal content never uses this recognizer.
    fn steps(&mut self, target: WheelTarget, event: &ScrollEvent, now: Instant) -> i32 {
        let (x, y) = match event.delta {
            ScrollDelta::Lines(delta) => (delta.x, delta.y),
            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
        };
        if event.touch_phase == TouchPhase::Cancelled {
            self.reset();
            return 0;
        }
        if target == WheelTarget::Screens
            && event.touch_phase == TouchPhase::Ended
            && x == 0.
            && y == 0.
        {
            // Trackpad momentum may continue after finger contact ends.
            return 0;
        }
        if !Self::vertical(event) {
            if target != WheelTarget::Screens || event.modifiers.shift {
                self.reset();
            }
            return 0;
        }
        if !x.is_finite() || !y.is_finite() || x.abs() >= y.abs() || y == 0. {
            if target != WheelTarget::Screens {
                self.reset();
            }
            return 0;
        }
        if self.target.as_ref() != Some(&target) {
            self.reset();
        }
        self.target = Some(target.clone());
        if target == WheelTarget::Screens {
            let magnitude = y.abs();
            if self.screen_last_scroll.is_some_and(|last| {
                now.saturating_duration_since(last) > Duration::from_millis(120)
            }) {
                self.screen_switched = false;
            }
            self.screen_last_scroll = Some(now);
            // DOM wheel deltas use pixels in the web action. Native line events
            // already represent wheel movement and need no pixel noise floor.
            let below_threshold = matches!(event.delta, ScrollDelta::Pixels(_)) && magnitude < 2.;
            if self.screen_switched || below_threshold {
                return 0;
            }
            self.screen_switched = true;
            return if y > 0. { 1 } else { -1 };
        }
        if event.touch_phase == TouchPhase::Started {
            self.pixels = 0.;
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

const EDGE_WIDTH: f32 = 18.;
const EDGE_DWELL: Duration = Duration::from_millis(200);
const EDGE_INTENT_SPEED: f32 = 800.;

fn workspace_left_inset(window: &Window) -> Pixels {
    let padding = gpui_kit::component::window_paddings(window).left;
    if padding > px(0.) {
        padding + px(1.)
    } else {
        px(0.)
    }
}

fn edge_motion(
    previous: Option<(Point<Pixels>, Instant)>,
    now: Instant,
    position: Point<Pixels>,
) -> bool {
    let x = f32::from(position.x);
    if !(0. ..=EDGE_WIDTH).contains(&x) {
        return false;
    }
    previous.is_none_or(|(point, then)| {
        let prev_x = f32::from(point.x);
        let dx = x - prev_x;
        let dy = f32::from(position.y - point.y);
        if prev_x > EDGE_WIDTH || (dx <= 0. && dy.abs() <= -dx) {
            return true;
        }
        let elapsed = now.saturating_duration_since(then).as_secs_f32();
        elapsed == 0. || dx.hypot(dy) / elapsed < EDGE_INTENT_SPEED
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
    // A leading non-ASCII character followed by a plain space is an icon.
    if !first.is_ascii() && !first.is_whitespace() && chars.next() == Some(' ') {
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

    /// A Screen's commands, the same from its row and its More button.
    fn screen_menu_entries(&self, key: &ScreenKey) -> Vec<MenuEntry> {
        let Some(model) = self.model.as_ref() else {
            return Vec::new();
        };
        let owner = model.lifecycle_token();
        let remote = key.source_id() != model.local_source().source_id();
        let source = model.source(key.source_id());
        let mut entries = Vec::new();
        if let Some(rename) = source
            .filter(|source| source.validate_screen_rename(key.screen_id()).is_ok())
            .map(SourceWorkspace::source_token)
        {
            let screen = key.clone();
            entries.push(MenuEntry::new(
                "rename",
                "Rename Screen…",
                move |this, window, cx| {
                    this.start_captured_rename(owner, rename, &screen, window, cx)
                },
            ));
        }
        for (id, label, down) in [
            ("move-up", "Move Up", false),
            ("move-down", "Move Down", true),
        ] {
            let before = self.screen_menu_before(key, down);
            let enabled = before
                .as_ref()
                .is_some_and(|before| self.can_place_screen(owner, key, before.as_ref()));
            let screen = key.clone();
            entries.push(
                MenuEntry::new(id, label, move |this, _, cx| {
                    if let Some(before) = before.clone() {
                        this.place_screen(owner, screen.clone(), before, cx);
                    }
                })
                .enabled(enabled),
            );
        }
        let screen = key.clone();
        let close = MenuEntry::new(
            "close",
            if remote {
                "Detach from This Window"
            } else {
                "Close Screen"
            },
            move |this, window, cx| this.request_close_screen(screen.clone(), window, cx),
        )
        .separated();
        entries.push(if remote {
            close.detail("Keeps running on the host")
        } else {
            close
        });
        if remote {
            let ready = model.opened().iter().any(|entry| {
                entry.key() == key
                    && entry.state() == splitty_client_core::workspace::OpenedScreenState::Ready
            });
            let token = model.execution_token(key.source_id()).filter(|_| ready);
            let screen = key.clone();
            entries.push(
                MenuEntry::new(
                    "terminate",
                    "Terminate Screen…",
                    move |this, window, cx| {
                        if let Some(token) = token {
                            this.request_terminate_screen(token, screen.clone(), window, cx);
                        }
                    },
                )
                .detail("Ends its terminals on the host")
                .danger()
                .enabled(token.is_some()),
            );
        }
        entries
    }

    fn open_screen_menu(
        &mut self,
        key: &ScreenKey,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .model
            .as_ref()
            .is_some_and(|model| model.opened().iter().any(|entry| entry.key() == key))
        {
            return;
        }
        let entries = self.screen_menu_entries(key);
        self.open_drawer_popover(
            PopoverKind::Actions(entries),
            PopoverAnchor::Pointer(position),
            Some(key.clone()),
            window,
            cx,
        );
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

    /// Submits one relative placement; returns whether it was admitted.
    pub(super) fn place_screen(
        &mut self,
        owner: SourceToken,
        screen: ScreenKey,
        before: Option<ScreenKey>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.lifecycle_token() != Some(owner)
            || !self.can_place_screen(owner, &screen, before.as_ref())
        {
            return false;
        }
        self.send_captured(owner, Command::MoveScreen { screen, before }, cx);
        true
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

    pub(super) fn open_drawer_overlay(&mut self, cx: &mut Context<Self>) {
        self.drawer_edge_task = None;
        self.set_drawer_overlay(true, cx);
    }

    pub(super) fn close_drawer_overlay(&mut self, cx: &mut Context<Self>) {
        self.drawer_edge_task = None;
        self.set_drawer_overlay(false, cx);
    }

    pub(super) fn drawer_edge_hover(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let inset = workspace_left_inset(window);
        let position = point(event.position.x - inset, event.position.y);
        let eligible = !self.drawer_visible
            && !self.drawer_overlay_open
            && self.drawer_edge_reveal
            && !event.dragging()
            && !GlobalState::is_in_deferred_context(cx)
            && edge_motion(self.drawer_edge_last, now, position);
        self.drawer_edge_last = Some((position, now));
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
                        .is_some_and(|(point, _)| (0. ..=EDGE_WIDTH).contains(&f32::from(point.x)))
                    && !GlobalState::is_in_deferred_context(cx)
                {
                    this.set_drawer_overlay(true, cx);
                }
                this.drawer_edge_task = None;
            });
        }));
    }

    /// `.drawer-nav-item`: one destination of the Drawer's bottom navigation,
    /// with its look for `active`; the caller adds identity and actions.
    pub(super) fn drawer_nav_item(
        &self,
        id: impl Into<ElementId>,
        NavItem {
            icon,
            label,
            shortcut,
            active,
        }: NavItem,
        window: &Window,
        cx: &App,
    ) -> Button {
        drawer_control(
            id,
            rgb(if active {
                self.appearance.text
            } else {
                self.appearance.secondary
            })
            .into(),
            NAV_ITEM_RADIUS,
            &self.appearance,
            window,
            cx,
        )
        .when(active, |item| item.bg(rgb(self.appearance.active)))
        .small()
        .w_full()
        .h(chrome_rems(NAV_ITEM_HEIGHT))
        .px(chrome_rems(NAV_ITEM_PADDING_X))
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(chrome_rems(NAV_ITEM_GAP))
                .text_size(chrome_rems(SIDEBAR_TEXT))
                .font_weight(FontWeight::MEDIUM)
                .child(
                    div()
                        .w(chrome_rems(NAV_ICON))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .opacity(if active { 1. } else { NAV_ICON_OPACITY })
                        .child(Icon::new(icon).size(chrome_rems(NAV_ICON))),
                )
                .child(label)
                .when_some(shortcut, |row, shortcut| {
                    row.child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .font(font(self.ui_family.clone()))
                            .text_size(chrome_rems(NAV_SHORTCUT_TEXT))
                            .text_color(rgb(self.appearance.muted))
                            .child(shortcut),
                    )
                }),
        )
    }

    fn render_local_group(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.lifecycle_token();
        div()
            .flex()
            .items_center()
            .gap_2()
            .px(chrome_rems(CARD_INSET + 4.))
            .h(chrome_rems(34.))
            .text_size(chrome_rems(SIDEBAR_TEXT))
            .text_color(rgb(self.appearance.secondary))
            .child(Icon::new(IconName::Monitor).size(chrome_rems(14.)))
            .child(div().flex_1().child("This computer"))
            .child(
                drawer_control(
                    "drawer-create",
                    rgb(self.appearance.secondary).into(),
                    CARD_RADIUS,
                    &self.appearance,
                    window,
                    cx,
                )
                .debug_selector(|| "drawer-create".into())
                .small()
                .w(chrome_rems(26.))
                .h(chrome_rems(26.))
                .icon(Icon::new(IconName::Plus).size(chrome_rems(14.)))
                .tooltip("New screen on this computer")
                .accessibility_label("Create local Screen")
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.lifecycle_token() == owner {
                        this.create_screen(window, cx);
                    }
                })),
            )
            .into_any_element()
    }

    pub(super) fn render_drawer(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.lifecycle_token();
        let pin_label = match self.drawer_pending {
            Some(pending) if pending.desired => "Pinning drawer…",
            Some(_) => "Unpinning drawer…",
            None if self.drawer_visible => "Unpin drawer",
            None => "Pin drawer",
        };
        let muted: Hsla = rgb(self.appearance.muted).into();
        let mono = terminal::terminal_family(cx);
        let mut list = div()
            .id("drawer-screen-list")
            .flex_shrink_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.drawer_scroll)
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ScreenDrag>, _, cx| {
                    this.track_screen_drag(event, cx);
                }),
            )
            .on_drag_move(cx.listener(
                |this, event: &DragMoveEvent<super::host_drag::HostDrag>, _, cx| {
                    this.track_host_drag(event, cx)
                },
            ))
            .pt(chrome_rems(LIST_PADDING_TOP))
            .pb(chrome_rems(LIST_PADDING_BOTTOM));
        if let Some(window_model) = &self.model {
            // Per host target: its known ports and how many are forwarded.
            let mut port_counts = HashMap::<String, (usize, usize)>::new();
            for row in self.ports.rows(Some(window_model)) {
                let counts = port_counts
                    .entry(row.key().target().to_owned())
                    .or_default();
                counts.0 += 1;
                counts.1 += usize::from(row.state() == ForwardState::Up);
            }
            let shown = window_model.shown_key();
            let opened = window_model.opened();
            let local = window_model.local_source().source_id();
            let mut group = Some(local);
            let mut group_header = self.render_local_group(window, cx);
            let mut group_rows = div()
                .id(SharedString::from(format!("drawer-group-screens-{local}")))
                .debug_selector(move || format!("drawer-group-screens-{local}"))
                .relative()
                .flex()
                .flex_col()
                .overflow_hidden();
            let mut remote_groups = div()
                .id("drawer-remote-groups")
                .relative()
                .flex()
                .flex_col()
                .overflow_hidden();
            for (index, entry) in opened.iter().enumerate() {
                let key = entry.key().clone();
                let remote = key.source_id() != window_model.local_source().source_id();
                let rename_source = window_model
                    .source(key.source_id())
                    .map(SourceWorkspace::source_token);
                if group != Some(key.source_id()) {
                    if let Some(previous) = group {
                        let block = self.render_drawer_group(
                            previous,
                            group_header,
                            group_rows,
                            window,
                            cx,
                        );
                        if previous == local {
                            list = list.child(block);
                        } else {
                            remote_groups = remote_groups.child(block);
                        }
                    }
                    group = Some(key.source_id());
                    let source = key.source_id();
                    group_rows = div()
                        .id(SharedString::from(format!("drawer-group-screens-{source}")))
                        .debug_selector(move || format!("drawer-group-screens-{source}"))
                        .relative()
                        .flex()
                        .flex_col()
                        .overflow_hidden();
                    group_header = div().into_any_element();
                    if remote {
                        let title = window_model
                            .source_target(key.source_id())
                            .map(|target| {
                                owner
                                    .and_then(|owner| self.hosts.accepted_alias(owner, target))
                                    .unwrap_or_else(|| {
                                        target.rsplit_once('@').map_or(target, |(_, host)| host)
                                    })
                            })
                            .map(super::hosts::display)
                            .unwrap_or_else(|| "Host".into());
                        if let Some(group) =
                            super::hosts::HostGroupTarget::capture(window_model, key.source_id())
                        {
                            let counts =
                                port_counts.get(group.target()).copied().unwrap_or_default();
                            group_header =
                                self.render_drawer_host(group, title, counts, window, cx);
                        }
                    }
                }
                let active = shown.as_ref() == Some(&key);
                let unreachable = remote
                    && super::drawer_ports::host_indicator(window_model.host(key.source_id()))
                        .unreachable();
                let editing = self
                    .rename
                    .as_ref()
                    .is_some_and(|editor| editor.screen == key);
                // GPUI reports no hover while a drag is active; the pressed row
                // keeps its hover look, as the web drawer's dragged row stays
                // under the pointer.
                let held = self
                    .drawer_drag
                    .as_ref()
                    .is_some_and(|session| session.holds(&key));
                let actions_visible = (self.drawer_hovered.as_ref() == Some(&key)
                    || self.drawer_screen_menu.as_ref() == Some(&key)
                    || held)
                    && !editing;
                let hover_transition =
                    Transition::new(Duration::from_millis(80)).easing(Easing::Ease);
                // Never a label by position: it could repeat a real Screen's
                // name (`mux-protocol`, Server-Chosen Screen Names).
                let label = entry.name().map(super::hosts::display).unwrap_or_else(|| {
                    if !remote {
                        let creating = window_model
                            .source(key.source_id())
                            .is_some_and(|source| source.is_creating(key.screen_id()));
                        return if creating {
                            "Creating Screen…".into()
                        } else {
                            "Unnamed Screen".into()
                        };
                    }
                    match window_model
                        .source(key.source_id())
                        .and_then(SourceWorkspace::remote_screen_creation)
                        .filter(|creation| creation.screen_id() == key.screen_id())
                        .map(|creation| creation.status())
                    {
                        Some(splitty_client_core::workspace::RemoteScreenCreateStatus::Pending) => {
                            "Creating Screen…".into()
                        }
                        Some(
                            splitty_client_core::workspace::RemoteScreenCreateStatus::Uncertain,
                        ) => "Unconfirmed Screen".into(),
                        _ => "Unnamed Screen".into(),
                    }
                });
                let unread_summary = self.screen_unread_summary(&key);
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
                let can_collapse = screen_snapshot.is_some_and(|screen| {
                    drawer_pane_groups(screen)
                        .iter()
                        .map(Vec::len)
                        .sum::<usize>()
                        > 1
                });
                // The row actions are 24 px buttons 1 px apart: close or
                // detach, a remote Screen's more menu and, with several
                // panes, the collapse toggle. Their lane stays reserved while
                // they are hidden, so hovering never moves the name's
                // rename/select target.
                let actions = if editing {
                    0
                } else {
                    1 + usize::from(remote) + usize::from(can_collapse)
                };
                let action_lane = if actions == 0 {
                    0.
                } else {
                    actions as f32 * ICON_BUTTON + (actions - 1) as f32 * ACTION_GAP + ROW_GAP
                };
                let mut row = div()
                    .id(SharedString::from(format!("drawer-screen-{key}")))
                    .flex()
                    .relative()
                    .items_center()
                    .min_w_0()
                    .h(chrome_rems(ROW_HEIGHT))
                    .pl(chrome_rems(ROW_PADDING_LEFT))
                    .pr(chrome_rems(ROW_PADDING_RIGHT + action_lane))
                    .gap(chrome_rems(ROW_GAP))
                    .when(!editing, |row| {
                        let menu_key = key.clone();
                        row.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                if this.lifecycle_token() == owner
                                    && !GlobalState::is_in_deferred_context(cx)
                                {
                                    this.open_screen_menu(&menu_key, event.position, window, cx);
                                }
                            }),
                        )
                    })
                    .when(!editing, |row| {
                        let select = key.clone();
                        let rename_on_click = key.clone();
                        let weak = cx.entity().downgrade();
                        row.cursor_pointer().on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            let _ = weak.update(cx, |this, cx| {
                                if this.lifecycle_token() == owner
                                    && !GlobalState::is_in_deferred_context(cx)
                                {
                                    if event.click_count() == 2 {
                                        if let (Some(owner), Some(source)) = (owner, rename_source)
                                        {
                                            this.start_captured_rename(
                                                owner,
                                                source,
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
                        })
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("drawer-index-{key}")))
                            .when(cfg!(all(test, feature = "ui-test-support")), |index| {
                                index.debug_selector(|| format!("drawer-index-{key}"))
                            })
                            .w(chrome_rems(INDEX_WIDTH))
                            .flex_none()
                            .text_size(chrome_rems(INDEX_TEXT))
                            .font_weight(FontWeight::MEDIUM)
                            .font(font(mono.clone()))
                            .text_align(TextAlign::Right)
                            .text_color(if active {
                                rgb(self.appearance.accent)
                            } else {
                                muted.into()
                            })
                            .child(format!("{}", index + 1)),
                    );
                let mut row_actions = div()
                    .id(SharedString::from(format!("drawer-row-actions-{key}")))
                    .absolute()
                    .right(chrome_rems(ROW_PADDING_RIGHT))
                    .top_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(chrome_rems(ACTION_GAP))
                    .on_click(|_, _, cx| cx.stop_propagation());
                if editing {
                    if let Some(editor) = &self.rename {
                        row = row.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .py(chrome_rems(RENAME_PADDING[0]))
                                .px(chrome_rems(RENAME_PADDING[1]))
                                .border_1()
                                .border_color(rgb(self.appearance.accent))
                                .rounded(chrome_rems(RENAME_RADIUS))
                                .bg(rgb(self.appearance.surface))
                                .text_color(rgb(self.appearance.text))
                                .text_size(chrome_rems(SIDEBAR_TEXT))
                                .font_weight(FontWeight::MEDIUM)
                                .line_height(relative(1.2))
                                .child(Input::new(&editor.input)),
                        );
                    }
                } else {
                    let select = key.clone();
                    let weak = cx.entity().downgrade();
                    let rename_on_click = key.clone();
                    let rename_on_key = key.clone();
                    let key_weak = cx.entity().downgrade();
                    let select_button =
                        Button::new(SharedString::from(format!("drawer-select-{key}")))
                            .accessibility_label(match &unread_summary {
                                Some(summary) => format!("Select {label}, {summary}"),
                                None => format!("Select {label}"),
                            })
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
                            .font(font(self.ui_family.clone()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(chrome_rems(SIDEBAR_TEXT))
                            .tooltip(label.clone())
                            .child(
                                div()
                                    .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                                        view.debug_selector(|| {
                                            format!("drawer-screen-label-{key}-{label}")
                                        })
                                    })
                                    .w_full()
                                    .min_w_0()
                                    // Kit sizes button content itself; the
                                    // label restates the Drawer's size.
                                    .font(font(self.ui_family.clone()))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(chrome_rems(SIDEBAR_TEXT))
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
                                if event.keystroke.key == "f2"
                                    && event.keystroke.modifiers == Modifiers::default()
                                {
                                    cx.stop_propagation();
                                    window.prevent_default();
                                    if let (Some(owner), Some(source)) = (owner, rename_source) {
                                        let _ = key_weak.update(cx, |this, cx| {
                                            this.start_captured_rename(
                                                owner,
                                                source,
                                                &rename_on_key,
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            })
                            .on_click(move |event, window, cx| {
                                cx.stop_propagation();
                                let _ = weak.update(cx, |this, cx| {
                                    if this.lifecycle_token() == owner
                                        && !GlobalState::is_in_deferred_context(cx)
                                    {
                                        if event.click_count() == 2 {
                                            if let (Some(owner), Some(source)) =
                                                (owner, rename_source)
                                            {
                                                this.start_captured_rename(
                                                    owner,
                                                    source,
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
                    row = row.child(div().flex_1().min_w_0().child(select_button));
                }
                if self.screen_blocked(&key) {
                    row = row.child(self.render_blocked_mark(format!("drawer-blocked-{key}"), cx));
                }
                if let Some(summary) = unread_summary {
                    let count = self.notifications.unread_count(&key);
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("drawer-unread-{key}")))
                            .when(cfg!(all(test, feature = "ui-test-support")), |badge| {
                                badge.debug_selector(|| format!("drawer-unread-{key}-{count}"))
                            })
                            .flex_none()
                            .h(chrome_rems(BADGE_HEIGHT))
                            .min_w(chrome_rems(BADGE_HEIGHT))
                            .px(chrome_rems(BADGE_PADDING_X))
                            .rounded(chrome_rems(BADGE_HEIGHT / 2.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(self.appearance.accent))
                            .text_color(rgb(self.appearance.surface))
                            .text_size(chrome_rems(BADGE_TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(chrome_rems(BADGE_HEIGHT))
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(summary.clone())
                                    .build(window, cx)
                            })
                            .child(count.to_string()),
                    );
                }
                if let Some(state) = state {
                    row = row.child(
                        div()
                            .when(cfg!(all(test, feature = "ui-test-support")), |label| {
                                label.debug_selector(|| format!("drawer-state-{key}-{state}"))
                            })
                            .flex_none()
                            .text_size(chrome_rems(NOTE_TEXT))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(state),
                    );
                }
                if actions_visible && can_collapse {
                    let collapsed = self.drawer_collapsed.contains(&key);
                    let collapse_key = key.clone();
                    let weak = cx.entity().downgrade();
                    row_actions = row_actions.child(
                        drawer_control(
                            SharedString::from(format!("drawer-collapse-{key}")),
                            muted,
                            ICON_BUTTON_RADIUS,
                            &self.appearance,
                            window,
                            cx,
                        )
                        .accessibility_label(if collapsed {
                            format!("Show panes in {label}")
                        } else {
                            format!("Hide panes in {label}")
                        })
                        .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                            button.debug_selector(|| format!("drawer-collapse-{key}"))
                        })
                        .xsmall()
                        .w(chrome_rems(ICON_BUTTON))
                        .h(chrome_rems(ICON_BUTTON))
                        .icon(Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        }))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let _ = weak.update(cx, |this, cx| {
                                if this.lifecycle_token() != owner {
                                    return;
                                }
                                if !this.drawer_collapsed.insert(collapse_key.clone()) {
                                    this.drawer_collapsed.remove(&collapse_key);
                                } else {
                                    if this
                                        .drawer_pane_hovered
                                        .as_ref()
                                        .is_some_and(|(screen, _)| screen == &collapse_key)
                                    {
                                        this.drawer_pane_hovered = None;
                                    }
                                    if this.destination == Destination::Screens {
                                        this.focus_current(window, cx);
                                    }
                                }
                                cx.notify();
                            });
                        }),
                    );
                }
                if actions_visible && remote {
                    let menu_key = key.clone();
                    let menu_open = self.drawer_popover_screen() == Some(&key);
                    row_actions = row_actions.child(
                        drawer_control(
                            SharedString::from(format!("drawer-more-{key}")),
                            muted,
                            ICON_BUTTON_RADIUS,
                            &self.appearance,
                            window,
                            cx,
                        )
                        .accessibility_label(format!("More actions for {label}"))
                        .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                            button.debug_selector(|| format!("drawer-more-{key}"))
                        })
                        .icon(Icon::new(IconName::Ellipsis))
                        .xsmall()
                        .w(chrome_rems(ICON_BUTTON))
                        .h(chrome_rems(ICON_BUTTON))
                        .selected(menu_open)
                        .when(menu_open, |button| {
                            button.bg(rgba(self.appearance.hover_overlay))
                        })
                        .on_click(cx.listener(
                            move |this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                if this.lifecycle_token() == owner {
                                    this.open_screen_menu(&menu_key, event.position(), window, cx);
                                }
                            },
                        )),
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
                    row_actions = row_actions.child(
                        drawer_control(
                            SharedString::from(format!("drawer-close-{key}")),
                            muted,
                            ICON_BUTTON_RADIUS,
                            &self.appearance,
                            window,
                            cx,
                        )
                        .accessibility_label(if remote {
                            format!("Detach {label}: keeps running on the host")
                        } else {
                            format!("Close {label}")
                        })
                        .xsmall()
                        .w(chrome_rems(ICON_BUTTON))
                        .h(chrome_rems(ICON_BUTTON))
                        .tooltip(close_tooltip)
                        .icon(Icon::new(if remote {
                            IconName::Unplug
                        } else {
                            IconName::X
                        }))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let _ = weak.update(cx, |this, cx| {
                                if this.lifecycle_token() == owner
                                    && this.model.as_ref().is_some_and(|model| {
                                        model.opened().iter().any(|entry| entry.key() == &close)
                                    })
                                {
                                    this.request_close_screen(close.clone(), window, cx);
                                }
                            });
                        }),
                    );
                }
                if actions_visible {
                    row = row.child(row_actions);
                }
                // `active-bg` and `hover-bg` composited over the Drawer, which
                // is the card's own color at rest.
                let block_background: Hsla = if active {
                    rgb(self.appearance.active).into()
                } else if actions_visible {
                    rgb(self.appearance.hover).into()
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
                // The row's part in a Screen drag: the lifted row follows the
                // pointer, the rows it passes make room, and a cancel or a
                // landed drop returns everything to its slot at once.
                let motion = self
                    .drawer_drag
                    .as_ref()
                    .map(|session| session.motion(&key))
                    .unwrap_or_default();
                let offset = transition(
                    format!("drawer-drag-offset-{key}"),
                    motion.offset,
                    if motion.animate {
                        screen_drag::screen_shift_transition()
                    } else {
                        Transition::new(Duration::ZERO)
                    },
                    window,
                    cx,
                );
                let opacity = transition(
                    format!("drawer-drag-opacity-{key}"),
                    if motion.lifted {
                        screen_drag::SCREEN_DRAG_OPACITY
                    } else {
                        1.
                    },
                    screen_drag::screen_lift_transition(),
                    window,
                    cx,
                );
                let opacity = if unreachable && !active {
                    opacity * UNREACHABLE_OPACITY
                } else {
                    opacity
                };
                let measured = self.drawer_row_slots.clone();
                let measured_scroll = self.drawer_scroll.clone();
                let measured_key = key.clone();
                let drag_owner = owner.filter(|_| window_model.can_move_screen(&key) && !editing);
                let mut block = div()
                    .id(SharedString::from(format!("drawer-block-{key}")))
                    .when(cfg!(all(test, feature = "ui-test-support")), |block| {
                        block.debug_selector(|| format!("drawer-block-{key}"))
                    })
                    .mx(chrome_rems(CARD_INSET))
                    .mt(chrome_rems(CARD_GAP))
                    .relative()
                    .top(px(offset))
                    .when(opacity < 1., |block| block.opacity(opacity))
                    .rounded(chrome_rems(CARD_RADIUS))
                    .bg(block_background)
                    .when(!editing, |block| {
                        let select = key.clone();
                        let rename_on_click = key.clone();
                        let weak = cx.entity().downgrade();
                        block.cursor_pointer().on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            if !event.standard_click() {
                                return;
                            }
                            let _ = weak.update(cx, |this, cx| {
                                if this.lifecycle_token() == owner
                                    && !GlobalState::is_in_deferred_context(cx)
                                {
                                    if event.click_count() == 2 {
                                        if let (Some(owner), Some(source)) = (owner, rename_source)
                                        {
                                            this.start_captured_rename(
                                                owner,
                                                source,
                                                &rename_on_click,
                                                window,
                                                cx,
                                            );
                                        }
                                    } else {
                                        this.switch_screen_key(select.clone(), window, cx);
                                        this.focus_current(window, cx);
                                    }
                                }
                            });
                        })
                    })
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                measured.borrow_mut().insert(
                                    measured_key,
                                    RowSlot {
                                        top: f32::from(bounds.top() - measured_scroll.offset().y)
                                            - offset,
                                        height: f32::from(bounds.size.height),
                                    },
                                );
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .when_some(drag_owner, |block, owner| {
                        let press_key = key.clone();
                        let weak = cx.entity().downgrade();
                        block
                            .capture_any_mouse_down(cx.listener(
                                move |this, event: &MouseDownEvent, _, _| {
                                    if event.button == MouseButton::Left {
                                        this.press_screen_row(&press_key, event.position);
                                    }
                                },
                            ))
                            .on_drag(
                                ScreenDrag {
                                    owner,
                                    screen: key.clone(),
                                    view: cx.entity().entity_id(),
                                },
                                move |drag, _, window, cx| {
                                    let position = window.mouse_position();
                                    let _ = weak.update(cx, |this, cx| {
                                        this.begin_screen_drag(drag.owner, &drag.screen, position);
                                        cx.notify();
                                    });
                                    cx.new(|_| ScreenDragPreview)
                                },
                            )
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
                    .child(self.card_padding(&key, owner, editing, "top", CARD_PADDING_TOP, cx))
                    .child(row);
                if let Some(screen) = screen_snapshot {
                    let groups = drawer_pane_groups(screen);
                    let multiple_groups = groups.len() > 1;
                    let pane_source = window_model.execution_token(key.source_id());
                    let collapsed = self.drawer_collapsed.contains(&key);
                    let pane_count = groups.iter().map(Vec::len).sum::<usize>();
                    let pane_height = transition(
                        format!("drawer-panes-height-{key}"),
                        if collapsed {
                            0.
                        } else {
                            PANE_HEIGHT * pane_count as f32
                        },
                        pane_list_transition(),
                        window,
                        cx,
                    );
                    let mut pane_list = div()
                        .id(SharedString::from(format!("drawer-panes-{key}")))
                        .when(cfg!(all(test, feature = "ui-test-support")), |list| {
                            list.debug_selector(|| format!("drawer-panes-{key}"))
                        })
                        .flex()
                        .flex_col()
                        .h(chrome_rems(pane_height))
                        .overflow_hidden();
                    for group in groups {
                        let stacked = multiple_groups && group.len() > 1;
                        let mut group_view = div().flex().flex_col().flex_none();
                        if stacked {
                            group_view = group_view
                                .relative()
                                .ml(chrome_rems(PANE_INDENT))
                                .pl(chrome_rems(STACK_PADDING_LEFT))
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top(chrome_rems(STACK_RAIL_INSET))
                                        .bottom(chrome_rems(STACK_RAIL_INSET))
                                        .w(chrome_rems(STACK_RAIL_WIDTH))
                                        .rounded(chrome_rems(STACK_RAIL_RADIUS))
                                        .bg(rgb(self.appearance.border)),
                                );
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
                            let unread_severity =
                                window_model.source(key.source_id()).and_then(|source| {
                                    self.notifications.severity(&source.pane_key(pane_id))
                                });
                            let is_viewer = pane.viewer.is_some();
                            // "Running" as the web Drawer has it: the
                            // foreground process is not the session's own
                            // shell. The service derives `busy` from exactly
                            // that and clears it when the process exits.
                            let busy = pane
                                .session_id
                                .as_ref()
                                .and_then(|id| screen.sessions.get(id))
                                .is_some_and(|session| session.busy);
                            let glows = busy && !is_viewer && leading_icon.is_some();
                            let glyph: AnyElement = if is_viewer {
                                Icon::new(IconName::FileText)
                                    .size(chrome_rems(PANE_DOC_ICON))
                                    .into_any_element()
                            } else if let Some(icon) = leading_icon {
                                div()
                                    .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                                        view.debug_selector(|| {
                                            format!("drawer-title-icon-{pane_id}-{icon}")
                                        })
                                    })
                                    .text_size(chrome_rems(if busy {
                                        PANE_RUNNING_ICON_TEXT
                                    } else {
                                        PANE_ICON_TEXT
                                    }))
                                    .when(busy, |icon| icon.text_color(rgb(self.appearance.text)))
                                    .child(icon.to_string())
                                    .into_any_element()
                            } else {
                                div()
                                    .size(chrome_rems(PANE_DOT))
                                    .rounded_full()
                                    .bg(if busy {
                                        rgb(self.appearance.text).into()
                                    } else {
                                        muted
                                    })
                                    .into_any_element()
                            };
                            group_view = group_view.child(
                                div().id(SharedString::from(format!("drawer-pane-hover-{key}-{pane_id}")))
                                .on_hover(move |hovered, _, cx| {
                                    let _ = hover_weak.update(cx, |this, cx| {
                                        if this.lifecycle_token() != owner {
                                            return;
                                        }
                                        let identity = (hover_screen.clone(), hover_pane.clone());
                                        if *hovered && !this.drawer_collapsed.contains(&hover_screen) {
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
                                .child(
                                Button::new(SharedString::from(format!(
                                    "drawer-pane-{key}-{pane_id}"
                                )))
                                .accessibility_label(match unread_severity {
                                    Some(severity) => format!("Open {title} in {label}, unread {} notification", super::unread::severity_label(severity)),
                                    None => format!("Open {title} in {label}"),
                                })
                                .tooltip(match unread_severity {
                                    Some(severity) => format!("{title}\nUnread {} notification", super::unread::severity_label(severity)),
                                    None => title.clone(),
                                })
                                .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                                    button.debug_selector(|| format!("drawer-pane-{key}-{pane_id}"))
                                })
                                .disabled(collapsed || pane_source.is_none())
                                .text()
                                .when(pane_source.is_some(), |button| button.cursor_pointer())
                                .compact()
                                .xsmall()
                                .w_full()
                                .h(chrome_rems(PANE_HEIGHT))
                                .pl(chrome_rems(if stacked { 0. } else { PANE_INDENT }))
                                .pr(chrome_rems(PANE_PADDING_RIGHT))
                                .justify_start()
                                .font(font(mono.clone()))
                                .font_weight(FontWeight::NORMAL)
                                .text_size(chrome_rems(PANE_TEXT))
                                .child(
                                    div()
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(chrome_rems(PANE_GAP))
                                        .text_size(chrome_rems(PANE_TEXT))
                                        .text_color(transition::<Hsla>(
                                            format!("drawer-pane-text-{key}-{pane_id}"),
                                            // A viewer keeps its color on
                                            // hover, as the web rule order
                                            // has it.
                                            rgb(if is_viewer {
                                                self.appearance.secondary
                                            } else if pane_hovered {
                                                self.appearance.text
                                            } else if busy {
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
                                                .relative()
                                                .w(chrome_rems(PANE_GLYPH_SLOT))
                                                .h_full()
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .when(glows, |slot| {
                                                    slot.child(
                                                        div()
                                                            .absolute()
                                                            .left(chrome_rems(
                                                                (PANE_GLYPH_SLOT - PANE_GLOW_CORE) / 2.,
                                                            ))
                                                            .top(chrome_rems(
                                                                (PANE_HEIGHT - PANE_GLOW_CORE) / 2.,
                                                            ))
                                                            .size(chrome_rems(PANE_GLOW_CORE))
                                                            .rounded_full()
                                                            .shadow(vec![BoxShadow {
                                                                color: Hsla::from(rgb(self.appearance.text))
                                                                    .opacity(PANE_GLOW_OPACITY),
                                                                offset: point(px(0.), px(0.)),
                                                                blur_radius: chrome_px(PANE_GLOW_BLUR, self.appearance.zoom.window_scale()),
                                                                spread_radius: px(0.),
                                                                inset: false,
                                                            }]),
                                                    )
                                                })
                                                .child(glyph),
                                        )
                                        .child(
                                            div()
                                                .when(cfg!(all(test, feature = "ui-test-support")), |view| {
                                                    view.debug_selector(|| format!("drawer-title-label-{pane_id}-{pane_label}"))
                                                })
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(pane_label.to_owned()),
                                        )
                                        .when_some(unread_severity, |view, severity| view.child(
                                            div()
                                                .id(SharedString::from(format!("drawer-pane-unread-{key}-{pane_id}")))
                                                .when(cfg!(all(test, feature = "ui-test-support")), |badge| {
                                                    badge.debug_selector(|| format!("drawer-pane-unread-{key}-{pane_id}-{}", super::unread::severity_label(severity)))
                                                })
                                                .flex_none()
                                                .size(chrome_rems(PANE_BADGE))
                                                .rounded_full()
                                                .bg(super::unread::severity_color(severity, cx))
                                        )),
                                )
                                .on_click(
                                    move |_, window, cx| {
                                        cx.stop_propagation();
                                        let _ = weak.update(cx, |this, cx| {
                                            if this.lifecycle_token() != owner
                                                || this.drawer_collapsed.contains(&select_key)
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
                                ),
                            );
                        }
                        pane_list = pane_list.child(group_view);
                    }
                    block = block.child(pane_list);
                }
                block = block.child(self.card_padding(
                    &key,
                    owner,
                    editing,
                    "bottom",
                    CARD_PADDING_BOTTOM,
                    cx,
                ));
                if motion.lifted {
                    group_rows = group_rows.child(screen_drag::RaisedInClip::new(block));
                } else {
                    group_rows = group_rows.child(block);
                }
            }
            if let Some(source) = group {
                let block = self.render_drawer_group(source, group_header, group_rows, window, cx);
                if source == local {
                    list = list.child(block);
                } else {
                    remote_groups = remote_groups.child(block);
                }
            }
            list = list.child(remote_groups);
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
        let templates_active = self.template_dialog.is_some();
        let templates = self
            .drawer_nav_item(
                "drawer-templates",
                NavItem {
                    icon: IconName::LayoutGrid,
                    label: "Templates",
                    shortcut: Some(nav_shortcut(Shortcut::Templates, cx)),
                    active: templates_active,
                },
                window,
                cx,
            )
            .accessibility_label("Open local Templates")
            .selected(templates_active)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
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
            .on_click(cx.listener(|this, _, window, cx| {
                if this.has_drawer_panel() && !GlobalState::is_in_deferred_context(cx) {
                    this.close_drawer_panel(window, cx);
                }
            }))
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered
                    && this.drawer_overlay_open
                    && !this.has_drawer_panel()
                    && this.rename.is_none()
                    && this.tab_context_menu.is_none()
                    && this.profile_picker.is_none()
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
                    .flex_none()
                    .pt(chrome_rems(HEAD_PADDING[0]))
                    .pr(chrome_rems(HEAD_PADDING[1]))
                    .pb(chrome_rems(HEAD_PADDING[2]))
                    .pl(chrome_rems(HEAD_PADDING[3]))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        // GPUI has no letter spacing; the web label adds .06em.
                        div()
                            .text_size(chrome_rems(HEAD_LABEL_TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child("SCREENS"),
                    )
                    .child(
                        // One pin icon in both states, as the web Drawer has
                        // it: pinned shows in the secondary text color.
                        drawer_control(
                            "drawer-pin",
                            if self.drawer_visible {
                                rgb(self.appearance.secondary).into()
                            } else {
                                muted
                            },
                            ICON_BUTTON_RADIUS,
                            &self.appearance,
                            window,
                            cx,
                        )
                        .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                            button.debug_selector(|| "drawer-pin".into())
                        })
                        .accessibility_label(pin_label)
                        .tooltip(pin_label)
                        .selected(self.drawer_visible)
                        .disabled(self.drawer_pending.is_some() || owner.is_none())
                        .when(self.drawer_pending.is_some() || owner.is_none(), |button| {
                            button.cursor_default()
                        })
                        .small()
                        .w(chrome_rems(ICON_BUTTON))
                        .h(chrome_rems(ICON_BUTTON))
                        .icon(Icon::new(IconName::Pin))
                        .on_click({
                            let weak = cx.entity().downgrade();
                            move |_, _, cx| {
                                cx.stop_propagation();
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
            .child(div().flex_1().min_h(chrome_rems(SPACER_MIN_HEIGHT)))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(chrome_rems(NAV_GAP))
                    .pt(chrome_rems(NAV_PADDING[0]))
                    .px(chrome_rems(NAV_PADDING[1]))
                    .pb(chrome_rems(NAV_PADDING[2]))
                    .border_t_1()
                    .border_color(rgb(self.appearance.subtle_border))
                    .child(self.render_hosts_navigation(window, cx))
                    .child(templates)
                    .child(
                        self.drawer_nav_item(
                            "drawer-settings",
                            NavItem {
                                icon: IconName::SlidersHorizontal,
                                label: "Settings",
                                shortcut: Some(nav_shortcut(Shortcut::Settings, cx)),
                                active: self.destination == Destination::Settings,
                            },
                            window,
                            cx,
                        )
                        .accessibility_label("Open Settings")
                        .track_focus(&self.settings_trigger)
                        .selected(self.destination == Destination::Settings)
                        .on_click(cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            if this.destination == Destination::Settings {
                                this.close_settings(window, cx);
                            } else {
                                this.show_settings(window, cx);
                            }
                        })),
                    ),
            )
            .children(
                (self
                    .drawer_drag
                    .as_ref()
                    .is_some_and(|session| session.is_dragging())
                    || self
                        .drawer_host_drag
                        .as_ref()
                        .is_some_and(|session| session.is_dragging()))
                .then(|| {
                    // A drag ends wherever the button is released, inside the
                    // Drawer or not, as the web list's window listener does.
                    let weak = cx.entity().downgrade();
                    canvas(
                        |_, _, _| {},
                        move |_, (), window, _| {
                            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture
                                    && event.button == MouseButton::Left
                                {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.finish_screen_drag(cx);
                                        this.finish_host_drag(cx);
                                    });
                                }
                            });
                        },
                    )
                    .absolute()
                    .size_0()
                }),
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
                || self.tab_context_menu.is_some()
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
            self.wheel_navigation
                .admit(target, event, true, cx.background_executor().now());
            if WheelNavigation::vertical(event) {
                cx.stop_propagation();
            }
            return;
        }
        let vertical = WheelNavigation::vertical(event);
        let steps = self.wheel_navigation.admit(
            target.clone(),
            event,
            false,
            cx.background_executor().now(),
        );
        if vertical {
            cx.stop_propagation();
        }
        if steps == 0 {
            return;
        }
        if matches!(&target, WheelTarget::Screens) {
            self.close_drawer_panel(window, cx);
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
    fn tab_target() -> WheelTarget {
        WheelTarget::Slot {
            screen: "s".into(),
            slot: "tabs".into(),
        }
    }

    #[test]
    fn tab_line_steps_are_discrete_and_bounded() {
        assert_eq!(wheel_step(1), TabStep::Previous);
        assert_eq!(wheel_step(-1), TabStep::Next);
        let mut wheel = WheelNavigation::default();
        for y in [1., 1., -8., 3., -0.5] {
            let event = event(ScrollDelta::Lines(point(0., y)), TouchPhase::Moved);
            assert_eq!(
                wheel.steps(tab_target(), &event, Instant::now()),
                if y > 0. { 1 } else { -1 }
            );
        }
    }

    #[test]
    fn tab_pixels_accumulate_and_reset_on_target_or_phase() {
        let mut wheel = WheelNavigation::default();
        let mut part = event(
            ScrollDelta::Pixels(point(px(0.), px(25.))),
            TouchPhase::Moved,
        );
        let now = Instant::now();
        assert_eq!(wheel.steps(tab_target(), &part, now), 0);
        assert_eq!(wheel.steps(tab_target(), &part, now), 1);
        let other = WheelTarget::Slot {
            screen: "s".into(),
            slot: "other".into(),
        };
        assert_eq!(wheel.steps(other, &part, now), 0);
        part.touch_phase = TouchPhase::Started;
        assert_eq!(wheel.steps(tab_target(), &part, now), 0);
        part.touch_phase = TouchPhase::Cancelled;
        assert_eq!(wheel.steps(tab_target(), &part, now), 0);
        part.touch_phase = TouchPhase::Moved;
        assert_eq!(wheel.steps(tab_target(), &part, now), 0);
        wheel.screen_changed();
        assert_eq!(wheel.steps(tab_target(), &part, now), 0);
    }

    #[test]
    fn drawer_swipe_waits_for_idle_after_last_event_even_after_screen_changes() {
        let mut wheel = WheelNavigation::default();
        let down = event(ScrollDelta::Lines(point(0., -3.)), TouchPhase::Moved);
        let start = Instant::now();
        assert_eq!(wheel.steps(WheelTarget::Screens, &down, start), -1);
        for ms in (20..=300).step_by(20) {
            wheel.screen_changed();
            assert_eq!(
                wheel.steps(
                    WheelTarget::Screens,
                    &down,
                    start + Duration::from_millis(ms)
                ),
                0
            );
        }
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &down,
                start + Duration::from_millis(420)
            ),
            0
        );
        assert_eq!(
            wheel.steps(
                WheelTarget::Screens,
                &down,
                start + Duration::from_millis(541)
            ),
            -1
        );
    }

    #[test]
    fn drawer_swipe_matches_web_noise_floor_and_idle_debounce() {
        let mut wheel = WheelNavigation::default();
        let start = Instant::now();
        for (ms, y, expected) in [
            (0, -1., 0),
            (10, -2., -1),
            (20, -2., 0),
            (30, -1., 0),
            (40, -4., 0),
            (50, -12., 0),
            (60, 3., 0),
            (181, 3., 1),
        ] {
            let event = event(ScrollDelta::Pixels(point(px(0.), px(y))), TouchPhase::Moved);
            assert_eq!(
                wheel.steps(
                    WheelTarget::Screens,
                    &event,
                    start + Duration::from_millis(ms)
                ),
                expected,
                "sample at {ms}ms"
            );
        }
    }

    #[test]
    fn drawer_contact_end_does_not_rearm_momentum_but_cancel_or_target_change_does() {
        let mut wheel = WheelNavigation::default();
        let now = Instant::now();
        let swipe = event(
            ScrollDelta::Pixels(point(px(0.), px(20.))),
            TouchPhase::Moved,
        );
        assert_eq!(wheel.steps(WheelTarget::Screens, &swipe, now), 1);
        let mut end = event(
            ScrollDelta::Pixels(point(px(0.), px(0.))),
            TouchPhase::Ended,
        );
        assert_eq!(wheel.steps(WheelTarget::Screens, &end, now), 0);
        assert_eq!(wheel.steps(WheelTarget::Screens, &swipe, now), 0);
        end.touch_phase = TouchPhase::Cancelled;
        assert_eq!(wheel.steps(WheelTarget::Screens, &end, now), 0);
        assert_eq!(wheel.steps(WheelTarget::Screens, &swipe, now), 1);
        assert_eq!(wheel.steps(tab_target(), &swipe, now), 0);
        assert_eq!(wheel.steps(WheelTarget::Screens, &swipe, now), 1);
    }

    #[test]
    fn horizontal_has_priority() {
        let mut wheel = WheelNavigation::default();
        let horizontal = event(
            ScrollDelta::Pixels(point(px(60.), px(20.))),
            TouchPhase::Moved,
        );
        assert_eq!(
            wheel.steps(WheelTarget::Screens, &horizontal, Instant::now()),
            0
        );
        let vertical = event(
            ScrollDelta::Pixels(point(px(0.), px(48.))),
            TouchPhase::Ended,
        );
        assert_eq!(
            wheel.steps(WheelTarget::Screens, &vertical, Instant::now()),
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
        let now = Instant::now();
        assert_eq!(wheel.admit(tab_target(), &part, false, now), 0);
        assert_eq!(wheel.admit(tab_target(), &part, true, now), 0);
        assert_eq!(wheel.admit(tab_target(), &part, false, now), 0);
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, true, now), 0);
        assert_eq!(wheel.admit(WheelTarget::Screens, &part, false, now), 1);
    }

    #[test]
    fn edge_reveal_requires_slow_unpressed_motion_inside_edge_strip() {
        let now = Instant::now();
        assert!(edge_motion(None, now, point(px(12.), px(20.))));
        assert!(!edge_motion(None, now, point(px(19.), px(20.))));
        assert!(edge_motion(
            Some((point(px(80.), px(20.)), now)),
            now + Duration::from_millis(10),
            point(px(12.), px(20.)),
        ));
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
        assert_eq!(split_title_icon("Ä offen"), (Some('Ä'), "offen"));
        assert_eq!(split_title_icon("🙂 Running"), (Some('🙂'), "Running"));
        assert_eq!(split_title_icon("Terminal"), (None, "Terminal"));
        assert_eq!(split_title_icon("Änderung"), (None, "Änderung"));
        assert_eq!(split_title_icon("⚡Build"), (None, "⚡Build"));
    }
}
