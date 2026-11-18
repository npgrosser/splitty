//! Window chrome renders resolved appearance and delegates actions to GPUI.
//! It never reads app themes, terminal settings or desktop configuration itself.
use crate::appearance::titlebar::TitleBarAppearance;
use gpui_kit::base::{Button, InteractiveElementExt as _};
use gpui_kit::component::{Icon, IconName, TitleBar};
use gpui_kit::{prelude::*, *};
use std::rc::Rc;

type CloseWindow = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct WindowChrome {
    pub title: String,
    pub appearance: TitleBarAppearance,
    pub on_close: CloseWindow,
}

struct DragState {
    pending: bool,
}
impl Render for DragState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl WindowChrome {
    fn controls(&self, buttons: &[Option<WindowButton>], window: &Window) -> AnyElement {
        let supported = window.window_controls();
        let colors = &self.appearance;
        let metrics = colors.profile.metrics();
        div()
            .flex()
            .gap(px(metrics.button_gap))
            .items_center()
            .opacity(if window.is_window_active() {
                1.
            } else {
                metrics.inactive_controls_opacity
            })
            .children(buttons.iter().flatten().copied().map(|button| {
                let (id, icon) = match button {
                    WindowButton::Minimize => ("minimize", IconName::WindowMinimize),
                    WindowButton::Maximize => (
                        "maximize",
                        if window.is_maximized() {
                            IconName::WindowRestore
                        } else {
                            IconName::WindowMaximize
                        },
                    ),
                    WindowButton::Close => ("close", IconName::WindowClose),
                };
                let on_close = self.on_close.clone();
                let enabled = match button {
                    WindowButton::Minimize => supported.minimize,
                    WindowButton::Maximize => supported.maximize,
                    WindowButton::Close => true,
                };
                let mut icon = Icon::new(icon).size(px(metrics.icon_size));
                if let Some(svg) = colors.icons.svg(button, window.is_maximized()) {
                    icon = icon.data(svg);
                }
                Button::new(id)
                    .disabled(!enabled)
                    .group("window-control")
                    .when(cfg!(feature = "ui-test-support"), |view| {
                        view.debug_selector(move || format!("window-{id}"))
                    })
                    .accessibility_label(match button {
                        WindowButton::Minimize => "Minimize window",
                        WindowButton::Maximize if window.is_maximized() => "Restore window",
                        WindowButton::Maximize => "Maximize window",
                        WindowButton::Close => "Close window",
                    })
                    .tab_stop(false)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(metrics.button_size))
                    .rounded(px(if metrics.round_buttons {
                        metrics.button_size / 2.
                    } else {
                        4.
                    }))
                    .text_color(rgb(colors.foreground))
                    .when(!metrics.round_buttons, |view| {
                        view.hover(|style| style.bg(rgb(colors.hover)))
                    })
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        match button {
                            WindowButton::Minimize => window.minimize_window(),
                            WindowButton::Maximize => window.zoom_window(),
                            WindowButton::Close => on_close(window, cx),
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(metrics.face_size))
                            .rounded(px(if metrics.round_buttons {
                                metrics.face_size / 2.
                            } else {
                                4.
                            }))
                            .when(metrics.round_buttons, |view| view.bg(rgb(colors.button)))
                            .group_hover("window-control", |view| view.bg(rgb(colors.hover)))
                            .child(icon),
                    )
            }))
            .into_any_element()
    }
}

impl RenderOnce for WindowChrome {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let a = &self.appearance;
        let metrics = a.profile.metrics();
        let title = div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .font_family(a.font.family.clone())
            .text_size(px(a.font.size))
            .font_weight(metrics.title_weight)
            .text_color(rgb(a.foreground))
            .opacity(if window.is_window_active() {
                1.
            } else {
                metrics.inactive_title_opacity
            })
            .child(self.title.clone());
        if !cfg!(target_os = "linux") {
            // AppKit keeps the actual traffic lights; Windows routes the Kit
            // caption hit regions through the native window procedure.
            return div()
                .relative()
                .flex_shrink_0()
                .child(
                    TitleBar::new()
                        .h(px(36.))
                        .bg(rgb(a.background))
                        .border_b_0(),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .px(px(108.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(title),
                )
                .into_any_element();
        }
        let layout = a.buttons.layout(cx.button_layout(), a.desktop_gnome);
        let left = self.controls(&layout.left, window);
        let right = self.controls(&layout.right, window);
        let inset = metrics.title_inset(
            layout
                .left
                .iter()
                .flatten()
                .count()
                .max(layout.right.iter().flatten().count()),
        );
        let state = window.use_state(cx, |_, _| DragState { pending: false });
        let border = gpui_kit::component::window_border_widths(window);
        div()
            .id("title-bar")
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(px(a.height))
            .pl(a.profile.control_padding(border.left))
            .pr(a.profile.control_padding(border.right))
            .bg(rgb(a.background))
            .on_double_click(|_, window, _| {
                if window.window_controls().maximize {
                    window.zoom_window();
                }
            })
            .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| state.pending = false))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&state, |state, _, _, _| state.pending = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&state, |state, _, _, _| state.pending = false),
            )
            .on_mouse_move(window.listener_for(
                &state,
                |state, event: &MouseMoveEvent, window, _| {
                    if state.pending {
                        state.pending = false;
                        if event.pressed_button == Some(MouseButton::Left) {
                            window.start_window_move();
                        }
                    }
                },
            ))
            .on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position)
            })
            .child(left)
            .child(right)
            // Passive title overlay leaves the whole empty bar draggable.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .px(px(inset))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(title),
            )
            .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test-support", target_os = "linux"))]
mod tests {
    use super::{TitleBarAppearance, WindowChrome};
    use crate::appearance::{
        titlebar::{SystemFont, TitleBarStyle},
        NativeAppearance,
    };
    use gpui_kit::{
        div, Context, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
        Window, WindowAppearance,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    struct Header {
        closed: Rc<Cell<usize>>,
    }
    impl Render for Header {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let closed = self.closed.clone();
            div().size_full().child(WindowChrome {
                title: "A title long enough to exercise the passive centered overlay".into(),
                appearance: TitleBarAppearance::resolve(
                    TitleBarStyle::System,
                    &NativeAppearance::fallback("UI"),
                    WindowAppearance::Dark,
                    true,
                    SystemFont {
                        family: ".SystemUIFont".into(),
                        size: 15.,
                    },
                ),
                on_close: Rc::new(move |_, _| closed.set(closed.get() + 1)),
            })
        }
    }

    // In-process: pointer activation must reach the application's close policy,
    // not remove the window or leak a click to the titlebar's drag behavior.
    #[test]
    fn close_button_routes_through_the_owner_and_survives_redraw() {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let closed = Rc::new(Cell::new(0));
        let (_, window) = app.add_window_view(|_, _| Header {
            closed: closed.clone(),
        });
        for count in 1..=2 {
            window.update(|window, cx| window.draw(cx).clear(cx));
            assert!(window.debug_bounds("window-minimize").is_some());
            assert!(window.debug_bounds("window-maximize").is_some());
            let close = window.debug_bounds("window-close").expect("close control");
            window.simulate_click(close.center(), Modifiers::default());
            assert_eq!(closed.get(), count);
        }
    }
}
