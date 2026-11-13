//! The page shown where a Screen's terminals would be when there is nothing
//! to show: the window holds no Screen, or the shown Screen holds no pane.
//! One icon, a short title, one muted line and the action that fills the
//! page, with its keyboard shortcut. Colors come from the theme
//! (`NativeAppearance`); the action is the toolkit's primary button. The
//! startup page (`startup_page`) shares the centered column without an action.
use super::*;

/// The icon's edge, the text sizes and the spacing between the parts.
pub(super) const ICON: f32 = 28.;
pub(super) const ICON_OPACITY: f32 = 0.7;
const TITLE_TEXT: f32 = 14.;
const DETAIL_TEXT: f32 = 12.;
const TEXT_GAP: f32 = 6.;
pub(super) const ACTION_GAP: f32 = 18.;
const SHORTCUT_GAP: f32 = 8.;
pub(super) const MAX_WIDTH: f32 = 320.;

/// What an empty page says and offers.
pub(super) struct EmptyPage {
    /// The page's element ID, and its selector in view tests.
    pub(super) id: &'static str,
    pub(super) icon: IconName,
    pub(super) title: &'static str,
    pub(super) detail: &'static str,
    /// The action's element ID, and its selector in view tests.
    pub(super) action_id: &'static str,
    pub(super) action: &'static str,
    pub(super) shortcut: Shortcut,
    /// The action cannot run now, for example while it is already under way.
    pub(super) disabled: bool,
}

/// The parts of a centered page; `page_id` is also its selector in view tests.
pub(super) struct PageColumn {
    pub(super) page_id: &'static str,
    pub(super) icon: AnyElement,
    pub(super) title: SharedString,
    pub(super) detail: SharedString,
    pub(super) max_width: f32,
}

impl Workspace {
    /// The centered column of an icon, a title and one muted line.
    pub(super) fn render_page_column(&self, column: PageColumn) -> (Div, Stateful<Div>) {
        let appearance = &self.appearance;
        let page_id = column.page_id;
        let frame = div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(appearance.surface));
        let page = div()
            .id(page_id)
            .when(cfg!(all(test, feature = "ui-test-support")), |page| {
                page.debug_selector(move || page_id.into())
            })
            .flex()
            .flex_col()
            .items_center()
            .max_w(chrome_rems(column.max_width))
            .px_4()
            .child(column.icon)
            .child(
                div()
                    .mt(chrome_rems(ACTION_GAP))
                    .text_size(chrome_rems(TITLE_TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(appearance.text))
                    .text_center()
                    .child(column.title),
            )
            .child(
                div()
                    .mt(chrome_rems(TEXT_GAP))
                    .text_size(chrome_rems(DETAIL_TEXT))
                    .text_color(rgb(appearance.muted))
                    .text_center()
                    .child(column.detail),
            );
        (frame, page)
    }

    pub(super) fn render_empty_page(
        &self,
        page: EmptyPage,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Div {
        let action_id = page.action_id;
        let shortcut = palette::shortcut_label(page.shortcut, cx.platform_info().is_mac());
        let (frame, column) = self.render_page_column(PageColumn {
            page_id: page.id,
            icon: Icon::new(page.icon)
                .size(chrome_rems(ICON))
                .text_color(rgb(self.appearance.muted))
                .opacity(ICON_OPACITY)
                .into_any_element(),
            title: page.title.into(),
            detail: page.detail.into(),
            max_width: MAX_WIDTH,
        });
        let appearance = &self.appearance;
        frame.child(
            column.child(
                div()
                    .mt(chrome_rems(ACTION_GAP))
                    .flex()
                    .items_center()
                    .gap(chrome_rems(SHORTCUT_GAP))
                    .child(
                        Button::new(action_id)
                            .debug_selector(move || action_id.into())
                            .label(page.action)
                            .icon(Icon::new(IconName::Plus))
                            .primary()
                            .small()
                            .focus_ring(focus_visible(cx))
                            .disabled(page.disabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                run(this, window, cx);
                            })),
                    )
                    .child(menu_style::shortcut(appearance, shortcut)),
            ),
        )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::{Command, WindowWorkspace, WorkerCommand, Workspace};
    use gpui_kit::{px, size, AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
    use splitty_client_core::native_workspace::Worker;
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{atomic::AtomicBool, Arc},
    };
    use tokio::sync::mpsc;
    use uuid::Uuid;

    /// A drawn workspace window over `model`, with the commands it sends.
    fn with_window(
        model: WindowWorkspace,
        run: impl FnOnce(&mut VisualTestContext, Entity<Workspace>, mpsc::Receiver<WorkerCommand>),
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
                    worker.shutdown().expect("owned fixture worker stops");
                    workspace
                });
                *content.borrow_mut() = Some(view.clone());
                gpui_kit::base::Root::new(view, window, cx)
            }
        });
        let view = content.borrow_mut().take().expect("workspace view exists");
        let (commands, receiver) = mpsc::channel(8);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.commands = commands;
                workspace.model = Some(model);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        });
        window.simulate_resize(size(px(900.), px(560.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        run(window, view, receiver);
    }

    #[test]
    fn a_window_without_screens_offers_one_action_that_creates_a_screen() {
        // Before the local catalog arrived the window is still connecting.
        with_window(WindowWorkspace::new(Uuid::new_v4()), |window, _, _| {
            assert!(window.debug_bounds("no-screens-page").is_none());
            assert!(window.debug_bounds("new-screen-empty").is_none());
        });

        let mut model = WindowWorkspace::new(Uuid::new_v4());
        model
            .ui_test_screen_order_fixture(&[], None)
            .expect("empty local catalog");
        let local = model.lifecycle_token();
        with_window(model, |window, _, mut receiver| {
            let page = window
                .debug_bounds("no-screens-page")
                .expect("the page of a window without Screens");
            let action = window
                .debug_bounds("new-screen-empty")
                .expect("its one action");
            assert!(page.contains(&action.center()));
            assert!(
                window.debug_bounds("new-terminal-empty").is_none(),
                "no empty Screen is shown"
            );
            window.simulate_click(action.center(), Modifiers::default());
            let command = receiver.try_recv().expect("a Screen is requested");
            assert_eq!(command.source_token(), Some(local));
            assert!(matches!(command.kind(), Command::CreateScreen(name) if name == "Screen 1"));
            assert!(receiver.try_recv().is_err(), "requested once");
        });
    }

    #[test]
    fn a_screen_without_panes_offers_a_new_terminal_on_its_own_source() {
        for target in [None, Some("empty.test")] {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let key = model
                .ui_test_empty_screen_fixture(target)
                .expect("empty Screen");
            let source = model
                .source(key.source_id())
                .expect("its source")
                .source_token();
            assert_eq!(source == model.lifecycle_token(), target.is_none());
            with_window(model, |window, _, mut receiver| {
                assert!(
                    window.debug_bounds("no-screens-page").is_none(),
                    "{target:?}: the window has a Screen"
                );
                assert!(window.debug_bounds("empty-screen-page").is_some());
                let action = window
                    .debug_bounds("new-terminal-empty")
                    .expect("the empty Screen's action");
                window.simulate_click(action.center(), Modifiers::default());
                let command = receiver.try_recv().expect("a terminal is requested");
                assert_eq!(
                    command.source_token(),
                    Some(source),
                    "{target:?}: sent to the Screen's own source"
                );
                assert!(matches!(
                    command.kind(),
                    Command::NewTerminal { screen, connection }
                        if screen == "kept" && *connection == source.connection_id()
                ));
                assert!(receiver.try_recv().is_err(), "requested once");
            });
        }
    }
}
