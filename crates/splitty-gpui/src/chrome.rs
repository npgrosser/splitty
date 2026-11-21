//! Window chrome renders resolved appearance and delegates actions to GPUI.
//! It never reads app themes, terminal settings or desktop configuration itself.
use crate::appearance::edges::{bevel_lines, Relief, Sides};
use crate::appearance::titlebar::TitleBarAppearance;
use crate::appearance::{CaptionButtons, TitleAlign};
use gpui_kit::base::{Button, InteractiveElementExt as _};
use gpui_kit::component::{Icon, IconName, TitleBar};
use gpui_kit::{prelude::*, *};
use std::rc::Rc;

type CloseWindow = Rc<dyn Fn(&mut Window, &mut App)>;

/// Windows routes caption buttons through the native window procedure, by
/// their `window_control_area`; elsewhere the buttons act themselves.
const NATIVE_CAPTION: bool = cfg!(target_os = "windows");

/// The close cell of wide caption buttons under the pointer.
const CLOSE_HOVER: u32 = 0xc42b1c;
const CLOSE_PRESSED: u32 = 0xc42b1ccc;

/// The buttons of the Windows bar, all three on the right.
const WINDOWS_BUTTONS: [Option<WindowButton>; 3] = [
    Some(WindowButton::Minimize),
    Some(WindowButton::Maximize),
    Some(WindowButton::Close),
];

/// Space between the title and the controls before it.
const TITLE_START_INSET: f32 = 8.;

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
        let a = &self.appearance;
        let metrics = a.caption_metrics();
        let active = window.is_window_active();
        let count = buttons.iter().flatten().count();
        div()
            .flex()
            .flex_shrink_0()
            .gap(px(metrics.gap))
            .items_center()
            .h_full()
            .when(a.caption == CaptionButtons::Plain, |row| {
                row.opacity(a.control_color(active).1)
            })
            .children(
                buttons
                    .iter()
                    .flatten()
                    .copied()
                    .enumerate()
                    .map(|(index, button)| {
                        let control = self.control(button, window);
                        let set_off = button == WindowButton::Close && count > 1;
                        div()
                            .flex()
                            .h_full()
                            .items_center()
                            .when(set_off && index > 0, |cell| cell.pl(px(metrics.close_gap)))
                            .when(set_off && index == 0, |cell| cell.pr(px(metrics.close_gap)))
                            .child(control)
                    }),
            )
            .into_any_element()
    }

    /// One caption button with its routing, label and test selector; its
    /// look comes from the bar's caption style.
    fn control(&self, button: WindowButton, window: &Window) -> AnyElement {
        let supported = window.window_controls();
        let maximized = window.is_maximized();
        let id = match button {
            WindowButton::Minimize => "minimize",
            WindowButton::Maximize => "maximize",
            WindowButton::Close => "close",
        };
        let enabled = match button {
            WindowButton::Minimize => supported.minimize,
            WindowButton::Maximize => supported.maximize,
            WindowButton::Close => true,
        };
        let on_close = self.on_close.clone();
        let base = Button::new(id)
            .disabled(!enabled)
            .group("window-control")
            .when(cfg!(feature = "ui-test-support"), |view| {
                view.debug_selector(move || format!("window-{id}"))
            })
            .accessibility_label(match button {
                WindowButton::Minimize => "Minimize window",
                WindowButton::Maximize if maximized => "Restore window",
                WindowButton::Maximize => "Maximize window",
                WindowButton::Close => "Close window",
            })
            .tab_stop(false)
            .flex_shrink_0()
            .when(NATIVE_CAPTION, |view| {
                view.focusable(false).window_control_area(match button {
                    WindowButton::Minimize => WindowControlArea::Min,
                    WindowButton::Maximize => WindowControlArea::Max,
                    WindowButton::Close => WindowControlArea::Close,
                })
            })
            .when(!NATIVE_CAPTION, |view| {
                view.on_mouse_down(MouseButton::Left, |_, window, cx| {
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
            });
        match self.appearance.caption {
            CaptionButtons::Plain => self.plain(base, button, window),
            CaptionButtons::Boxed => self.boxed(base, button, enabled, window),
            CaptionButtons::Wide => self.wide(base, button, window),
        }
    }

    /// Symbols with a hover background, round on GNOME.
    fn plain(&self, base: Button, button: WindowButton, window: &Window) -> AnyElement {
        let colors = &self.appearance;
        let metrics = colors.profile.metrics();
        let size = colors.caption_metrics().height;
        let face = metrics.face_size.min(size);
        let mut icon =
            Icon::new(window_icon(button, window.is_maximized())).size(px(metrics.icon_size));
        if let Some(svg) = colors.icons.svg(button, window.is_maximized()) {
            icon = icon.data(svg);
        }
        base.flex()
            .items_center()
            .justify_center()
            .size(px(size))
            .rounded(px(if metrics.round_buttons { size / 2. } else { 4. }))
            .text_color(rgb(colors.control_color(window.is_window_active()).0))
            .when(!metrics.round_buttons, |view| {
                view.hover(|style| style.bg(rgb(colors.hover)))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(face))
                    .rounded(px(if metrics.round_buttons { face / 2. } else { 4. }))
                    .when(metrics.round_buttons, |view| view.bg(rgb(colors.button)))
                    .group_hover("window-control", |view| view.bg(rgb(colors.hover)))
                    .child(icon),
            )
            .into_any_element()
    }

    /// A small button with the theme's raised edge, sunken while pressed,
    /// and a pixel glyph that moves with the press.
    fn boxed(
        &self,
        base: Button,
        button: WindowButton,
        enabled: bool,
        window: &Window,
    ) -> AnyElement {
        let a = &self.appearance;
        let metrics = a.caption_metrics();
        let unit = ((metrics.height - 4.) / 10.).floor().max(1.);
        let glyph_color = match a.bevel {
            Some(bevel) if !enabled => bevel.shadow,
            _ => a.glyph,
        };
        let edge = |relief: Relief, shown: bool| {
            let layer = div()
                .id(match relief {
                    Relief::Sunken => "window-control-sunken",
                    _ => "window-control-raised",
                })
                .absolute()
                .inset_0()
                .when(!shown, |layer| layer.invisible());
            let layer = if shown {
                layer.group_active("window-control", |style| style.invisible())
            } else {
                layer.group_active("window-control", |style| style.visible())
            };
            a.bevel
                .map(|bevel| layer.child(bevel_lines(bevel, relief, Sides::ALL)))
        };
        base.relative()
            .flex()
            .items_center()
            .justify_center()
            .w(px(metrics.width))
            .h(px(metrics.height))
            .bg(rgb(a.face))
            .map(|view| match a.bevel {
                Some(_) => view.border_2(),
                None => view
                    .border_1()
                    .border_color(rgb(a.edge))
                    .active(|style| style.bg(rgb(a.hover))),
            })
            .when(enabled, |view| {
                view.children(edge(Relief::Raised, true))
                    .children(edge(Relief::Sunken, false))
            })
            .when(!enabled, |view| view.children(edge(Relief::Raised, true)))
            .child(
                div()
                    .id("window-control-glyph")
                    .relative()
                    .when(enabled, |glyph| {
                        glyph.group_active("window-control", |style| style.top(px(1.)).left(px(1.)))
                    })
                    .child(pixel_glyph(
                        button,
                        window.is_maximized(),
                        glyph_color,
                        unit,
                    )),
            )
            .into_any_element()
    }

    /// A cell as tall as the bar; the close cell turns red under the pointer.
    fn wide(&self, base: Button, button: WindowButton, window: &Window) -> AnyElement {
        let a = &self.appearance;
        let metrics = a.caption_metrics();
        let close = button == WindowButton::Close;
        let icon = Icon::new(window_icon(button, window.is_maximized())).size(px(14.));
        base.flex()
            .items_center()
            .justify_center()
            .w(px(metrics.width))
            .h(px(metrics.height))
            .text_color(rgb(a.control_color(window.is_window_active()).0))
            .map(|view| {
                if close {
                    view.hover(|style| style.bg(rgb(CLOSE_HOVER)).text_color(rgb(0xffffff)))
                        .active(|style| style.bg(rgba(CLOSE_PRESSED)).text_color(rgb(0xffffff)))
                } else {
                    view.hover(|style| style.bg(rgb(a.button)))
                        .active(|style| style.bg(rgb(a.hover)))
                }
            })
            .child(icon)
            .into_any_element()
    }

    fn title(&self, window: &Window) -> Div {
        let a = &self.appearance;
        let (color, opacity) = a.title_color(window.is_window_active());
        div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .font_family(a.font.family.clone())
            .text_size(px(a.font.size))
            .font_weight(a.weight)
            .text_color(rgb(color))
            .opacity(opacity)
            .child(self.title.clone())
    }
}

fn window_icon(button: WindowButton, maximized: bool) -> IconName {
    match button {
        WindowButton::Minimize => IconName::WindowMinimize,
        WindowButton::Maximize if maximized => IconName::WindowRestore,
        WindowButton::Maximize => IconName::WindowMaximize,
        WindowButton::Close => IconName::WindowClose,
    }
}

/// The rectangles of a boxed caption glyph on a 9 by 9 grid, as
/// `(x, y, width, height)`.
fn glyph_rects(button: WindowButton, maximized: bool) -> &'static [(f32, f32, f32, f32)] {
    match button {
        // A short bar at the bottom.
        WindowButton::Minimize => &[(1., 7., 6., 2.)],
        // Two overlapped boxes, each with a thick top line; the front box
        // hides what the back one has behind it.
        WindowButton::Maximize if maximized => &[
            (3., 0., 6., 2.),
            (3., 2., 1., 1.),
            (8., 2., 1., 4.),
            (6., 5., 3., 1.),
            (0., 3., 6., 2.),
            (0., 5., 1., 4.),
            (5., 5., 1., 4.),
            (0., 8., 6., 1.),
        ],
        // A box with a thick top line.
        WindowButton::Maximize => &[
            (0., 0., 9., 2.),
            (0., 2., 1., 7.),
            (8., 2., 1., 7.),
            (0., 8., 9., 1.),
        ],
        // An X of two-pixel strokes.
        WindowButton::Close => &[
            (0., 1., 2., 1.),
            (6., 1., 2., 1.),
            (1., 2., 2., 1.),
            (5., 2., 2., 1.),
            (2., 3., 4., 1.),
            (3., 4., 2., 1.),
            (2., 5., 4., 1.),
            (1., 6., 2., 1.),
            (5., 6., 2., 1.),
            (0., 7., 2., 1.),
            (6., 7., 2., 1.),
        ],
    }
}

/// A boxed caption glyph painted as quads on whole pixels, so it stays
/// crisp at the size of a small button. `unit` is the size of one grid cell.
fn pixel_glyph(button: WindowButton, maximized: bool, color: u32, unit: f32) -> impl IntoElement {
    let rects = glyph_rects(button, maximized);
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, (), window, _| {
            let left = f32::from(bounds.left()).round();
            let top = f32::from(bounds.top()).round();
            for &(x, y, width, height) in rects {
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(left + x * unit), px(top + y * unit)),
                        size(px(width * unit), px(height * unit)),
                    ),
                    rgb(color),
                ));
            }
        },
    )
    .size(px(9. * unit))
}

/// Windows 11 rounds the corners of every window that does not ask it not
/// to. A theme whose frame is square asks; any other leaves the system's
/// choice. Windows 10 knows no such attribute and refuses it, which changes
/// nothing there.
#[cfg(target_os = "windows")]
mod corners {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{ffi::c_void, sync::Mutex};

    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_DEFAULT: u32 = 0;
    const DWMWCP_DONOTROUND: u32 = 1;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            window: *mut c_void,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    /// What each window was last asked for, so that drawing the title bar
    /// again asks the system nothing.
    static ASKED: Mutex<Vec<(isize, bool)>> = Mutex::new(Vec::new());

    pub(super) fn apply(window: &gpui_kit::Window, square: bool) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return;
        };
        let id = handle.hwnd.get();
        let mut asked = ASKED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match asked.iter_mut().find(|(window, _)| *window == id) {
            Some((_, last)) if *last == square => return,
            Some((_, last)) => *last = square,
            None => asked.push((id, square)),
        }
        let preference = if square {
            DWMWCP_DONOTROUND
        } else {
            DWMWCP_DEFAULT
        };
        // SAFETY: the handle is this live window's, the value is a `u32` that
        // outlives the call, and its size is passed with it.
        unsafe {
            DwmSetWindowAttribute(
                id as *mut c_void,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                std::ptr::from_ref(&preference).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

impl RenderOnce for WindowChrome {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let a = &self.appearance;
        let active = window.is_window_active();
        let title = self.title(window);
        // The frame is the system's on Windows; only its corners are ours to
        // ask for.
        #[cfg(target_os = "windows")]
        corners::apply(window, a.radius == 0.);
        if !cfg!(target_os = "linux") {
            // AppKit keeps the actual traffic lights; Windows routes the
            // caption hit regions through the native window procedure.
            let macos = cfg!(target_os = "macos");
            let layout = WINDOWS_BUTTONS;
            // Plain controls on Windows are the toolkit's own cells.
            let custom = !macos && a.caption != CaptionButtons::Plain;
            let controls_width = if macos {
                0.
            } else if custom {
                a.controls_width(&layout) + a.caption_metrics().side_padding
            } else {
                3. * 34.
            };
            // Room for the traffic lights before the title.
            let lead: f32 = if macos { 80. } else { 12. };
            let (left, right) = match a.align {
                TitleAlign::Center => {
                    let side = lead.max(controls_width).max(3. * 34.) + 6.;
                    (side, side)
                }
                TitleAlign::Start => (lead, controls_width + TITLE_START_INSET),
            };
            return div()
                .relative()
                .flex_shrink_0()
                .child(
                    TitleBar::new()
                        .h(px(a.height))
                        .bg(a.fill(active).background())
                        // The toolkit's own controls draw in the bar's color.
                        .text_color(rgb(a.control_color(active).0))
                        .border_b_0()
                        .when(custom, |bar| {
                            bar.window_controls(
                                div()
                                    .flex()
                                    .flex_shrink_0()
                                    .h_full()
                                    .items_center()
                                    .pr(px(a.caption_metrics().side_padding))
                                    .child(self.controls(&layout, window)),
                            )
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .pl(px(left))
                        .pr(px(right))
                        .flex()
                        .items_center()
                        .map(|view| match a.align {
                            TitleAlign::Center => view.justify_center(),
                            TitleAlign::Start => view.justify_start(),
                        })
                        .child(title),
                )
                .into_any_element();
        }
        let layout = a.buttons.layout(cx.button_layout(), a.desktop_gnome);
        let left = self.controls(&layout.left, window);
        let right = self.controls(&layout.right, window);
        let left_count = layout.left.iter().flatten().count();
        let state = window.use_state(cx, |_, _| DragState { pending: false });
        let border = gpui_kit::component::window_border_widths(window);
        let (pad_left, pad_right) = match a.caption {
            CaptionButtons::Plain => (
                a.profile.control_padding(border.left),
                a.profile.control_padding(border.right),
            ),
            _ => {
                let padding = px(a.caption_metrics().side_padding);
                (padding, padding)
            }
        };
        let inset = match a.caption {
            CaptionButtons::Plain => a
                .profile
                .metrics()
                .title_inset(left_count.max(layout.right.iter().flatten().count())),
            _ => {
                a.caption_metrics().side_padding
                    + a.controls_width(&layout.left)
                        .max(a.controls_width(&layout.right))
                    + 12.
            }
        };
        div()
            .id("title-bar")
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .h(px(a.height))
            .pl(pad_left)
            .pr(pad_right)
            .bg(a.fill(active).background())
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
            .map(|bar| match a.align {
                // Passive title overlay leaves the whole empty bar draggable.
                TitleAlign::Center => bar.child(right).child(
                    div()
                        .absolute()
                        .inset_0()
                        .px(px(inset))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(title),
                ),
                // A passive title in the row, after the left controls.
                TitleAlign::Start => bar
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .px(px(TITLE_START_INSET))
                            .child(title),
                    )
                    .child(right),
            })
            .into_any_element()
    }
}

#[cfg(all(test, feature = "ui-test-support", target_os = "linux"))]
mod tests {
    use super::{glyph_rects, TitleBarAppearance, WindowChrome};
    use crate::appearance::{
        titlebar::{SystemFont, TitleBarStyle, WindowDecoration},
        Bevel, CaptionButtons, NativeAppearance, TitleAlign,
    };
    use gpui_kit::{
        div, Context, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
        Window, WindowAppearance, WindowButton,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    struct Header {
        closed: Rc<Cell<usize>>,
        caption: CaptionButtons,
        align: TitleAlign,
    }
    impl Render for Header {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let closed = self.closed.clone();
            let mut app = NativeAppearance::fallback("UI");
            if self.caption != CaptionButtons::Plain {
                app.window_decoration = WindowDecoration::Splitty;
                app.title_bar.buttons = self.caption;
                app.title_bar.align = self.align;
                app.title_bar.height = Some(22.);
                app.bevel = Some(Bevel {
                    highlight: 0xffffff,
                    light: 0xdfdfdf,
                    shadow: 0x808080,
                    dark_shadow: 0x000000,
                });
            }
            div().size_full().child(WindowChrome {
                title: "A title long enough to exercise the passive centered overlay".into(),
                appearance: TitleBarAppearance::resolve(
                    if self.caption == CaptionButtons::Plain {
                        TitleBarStyle::System
                    } else {
                        TitleBarStyle::Theme
                    },
                    &app,
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
        let styles = [
            (CaptionButtons::Plain, TitleAlign::Center),
            (CaptionButtons::Boxed, TitleAlign::Start),
            (CaptionButtons::Wide, TitleAlign::Center),
        ];
        for (caption, align) in styles {
            let mut app = TestAppContext::single();
            app.update(gpui_kit::init);
            let closed = Rc::new(Cell::new(0));
            let (_, window) = app.add_window_view(|_, _| Header {
                closed: closed.clone(),
                caption,
                align,
            });
            for count in 1..=2 {
                window.update(|window, cx| window.draw(cx).clear(cx));
                assert!(window.debug_bounds("window-minimize").is_some());
                assert!(window.debug_bounds("window-maximize").is_some());
                let close = window.debug_bounds("window-close").expect("close control");
                if caption == CaptionButtons::Wide {
                    assert_eq!(f32::from(close.size.width), 46., "{caption:?}");
                }
                window.simulate_click(close.center(), Modifiers::default());
                assert_eq!(closed.get(), count, "{caption:?}");
            }
        }
    }

    #[test]
    fn boxed_glyphs_stay_on_their_grid() {
        for (button, maximized) in [
            (WindowButton::Minimize, false),
            (WindowButton::Maximize, false),
            (WindowButton::Maximize, true),
            (WindowButton::Close, false),
        ] {
            let rects = glyph_rects(button, maximized);
            assert!(!rects.is_empty());
            for &(x, y, width, height) in rects {
                assert!(x >= 0. && y >= 0. && x + width <= 9. && y + height <= 9.);
            }
        }
    }
}
