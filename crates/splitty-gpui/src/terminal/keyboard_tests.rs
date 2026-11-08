use super::{init, Command, LinkOrigin, Terminal};
use gpui_kit::{
    div, prelude::*, px, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement,
    Modifiers, TestAppContext, Window,
};
use std::{cell::RefCell, rc::Rc};

struct Fixture {
    terminal: Entity<Terminal>,
    control: FocusHandle,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("control")
                    .track_focus(&self.control)
                    .h(px(24.))
                    .child("Focusable control"),
            )
            .child(div().flex_1().min_h_0().child(self.terminal.clone()))
    }
}

#[test]
fn focused_pty_keeps_tab_bytes_while_find_and_controls_traverse() {
    let mut app = TestAppContext::single();
    app.update(|cx| {
        gpui_kit::init(cx);
        init(cx);
    });
    let commands = Rc::new(RefCell::new(Vec::new()));
    let content = Rc::new(RefCell::new(None));
    let (_, window) = app.add_window_view({
        let commands = commands.clone();
        let content = content.clone();
        move |window, cx| {
            let terminal = cx.new(|cx| {
                Terminal::new(
                    move |command| {
                        commands.borrow_mut().push(command);
                        Ok(())
                    },
                    LinkOrigin::Local,
                    cx,
                )
            });
            let view = cx.new(|cx| Fixture {
                terminal,
                control: cx.focus_handle().tab_stop(true),
            });
            view.read(cx).control.clone().focus(window, cx);
            *content.borrow_mut() = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        }
    });
    let view = content.borrow_mut().take().unwrap();
    window.update(|window, cx| window.draw(cx).clear(cx));
    let terminal = window.update(|_, cx| view.read(cx).terminal.clone());
    let position = window.update(|_, cx| terminal.read(cx).bounds.center());
    // Enter through the real pointer handler, so focus invalidation must update
    // the dispatch context without the test manually redrawing the terminal.
    window.simulate_click(position, Modifiers::default());
    commands.borrow_mut().clear();
    for (key, expected) in [("tab", b"\t".as_slice()), ("shift-tab", b"\x1b[Z")] {
        window.simulate_keystrokes(key);
        window.update(|window, cx| assert!(terminal.read(cx).focus.is_focused(window)));
        let inputs = commands
            .borrow()
            .iter()
            .filter_map(|command| match command {
                Command::Input { bytes, .. } => Some(bytes.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(inputs, vec![expected.to_vec()]);
        commands.borrow_mut().clear();
    }

    window.simulate_keystrokes("ctrl-f");
    let search_focus = window.update(|window, cx| {
        let terminal = terminal.read(cx);
        assert!(terminal.search_input_focused(window, cx));
        terminal
            .search
            .as_ref()
            .unwrap()
            .input
            .read(cx)
            .focus_handle(cx)
    });
    window.simulate_keystrokes("tab");
    window.update(|window, cx| {
        assert!(!search_focus.is_focused(window));
        assert!(!terminal.read(cx).focus.is_focused(window));
    });
    window.simulate_keystrokes("shift-tab");
    window.update(|window, _| assert!(search_focus.is_focused(window)));
    assert!(commands
        .borrow()
        .iter()
        .all(|command| !matches!(command, Command::Input { .. })));
}
