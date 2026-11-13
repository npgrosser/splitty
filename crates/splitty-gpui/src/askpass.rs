//! The SSH questions of the local service, embedded or the WSL backend, each
//! in a window of its own. The workspace reads them over the service's API
//! and publishes the list; an answer or a dismissal goes back the same way.
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::Input,
    ActiveTheme as _, Sizable as _,
};
use gpui_kit::{prelude::*, *};
use splitty_client_core::native_workspace::SshPrompt;
use std::{collections::HashMap, rc::Rc};

/// Hands an answer, or `None` to cancel, for the question with this id to
/// the local service.
pub type AnswerSink = Rc<dyn Fn(String, Option<String>)>;

struct Askpass {
    /// The question's id and where its answer goes, until it is answered.
    request: Option<(String, AnswerSink)>,
    prompt: String,
    confirm: bool,
    input: Entity<InputState>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl Drop for Askpass {
    /// A dialog closed by the window manager cancels its question.
    fn drop(&mut self) {
        if let Some((id, sink)) = self.request.take() {
            sink(id, None);
        }
    }
}

impl Askpass {
    fn finish(&mut self, answer: Option<String>, window: &mut Window) {
        if let Some((id, sink)) = self.request.take() {
            sink(id, answer);
        }
        window.remove_window();
    }

    fn submit(&mut self, window: &mut Window, cx: &App) {
        let answer = if self.confirm {
            "yes".to_owned()
        } else {
            self.input.read(cx).value().to_string()
        };
        self.finish(Some(answer), window);
    }

    fn new(
        request: (String, AnswerSink),
        prompt: String,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Render control and direction override characters visibly.
        let prompt: String = prompt.chars().map(|ch| {
            if (ch.is_control() && ch != '\n' && ch != '\t')
                || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                '\u{fffd}'
            } else { ch }
        }).collect();
        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let focus = cx.focus_handle();
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Askpass, _, event: &InputEvent, window, cx| {
                if !this.confirm && matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit(window, cx);
                }
            },
        );
        if !confirm {
            input.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            focus.focus(window, cx);
        }
        Self {
            request: Some(request),
            prompt,
            confirm,
            input,
            focus,
            _subscription: subscription,
        }
    }
}

/// A host key needs room to be read; a password prompt is one line.
fn options(confirm: bool, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(
            size(px(520.), px(if confirm { 320. } else { 210. })),
            cx,
        )),
        titlebar: Some(TitlebarOptions {
            title: Some("Splitty · SSH".into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The open question dialogs, one per question of the local service.
#[derive(Default)]
pub struct Prompts {
    dialogs: HashMap<String, (AnyWindowHandle, Entity<Askpass>)>,
}

impl Prompts {
    /// Show `list`, the questions the service waits on now: a dialog for each
    /// new one; a dialog whose question went closes without answering it. A
    /// dialog the user closed stays closed while its cancel is on its way.
    pub fn show(&mut self, list: &[SshPrompt], sink: &AnswerSink, cx: &mut App) {
        self.dialogs.retain(|id, (handle, dialog)| {
            if list.iter().any(|prompt| &prompt.id == id) {
                return true;
            }
            dialog.update(cx, |dialog, _| dialog.request = None);
            let _ = handle.update(cx, |_, window, _| window.remove_window());
            false
        });
        for prompt in list {
            if self.dialogs.contains_key(&prompt.id) {
                continue;
            }
            let request = (prompt.id.clone(), sink.clone());
            let text = format!("{}\n\n{}", prompt.target, prompt.prompt);
            let confirm = prompt.confirm;
            let result = gpui_kit::open_window(options(confirm, cx), cx, |window, cx| {
                cx.new(|cx| Askpass::new(request, text, confirm, window, cx))
            });
            match result {
                Ok(dialog) => {
                    self.dialogs.insert(prompt.id.clone(), dialog);
                    cx.activate(true);
                }
                Err(_) => sink(prompt.id.clone(), None),
            }
        }
    }
}

impl Render for Askpass {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let confirm = self.confirm;
        // The broker's prompt names its host first, then SSH's own words.
        let (host, question) = match self.prompt.split_once("\n\n") {
            Some((host, question)) => (Some(host.to_owned()), question.to_owned()),
            None => (None, self.prompt.clone()),
        };
        let muted = cx.theme().muted_foreground;
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    this.finish(None, window);
                }
            }))
            .size_full()
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_size(px(13.))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if confirm {
                                "Trust this SSH host?"
                            } else {
                                "SSH authentication"
                            }),
                    )
                    .when_some(host, |head, host| {
                        head.child(div().text_size(px(12.)).text_color(muted).child(host))
                    }),
            )
            .child(
                div()
                    .id("ssh-prompt")
                    .debug_selector(|| "ssh-prompt".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(confirm, |prompt| {
                        // A host key reads as what it is: fixed-width text.
                        prompt
                            .p(px(10.))
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.5))
                    })
                    .line_height(relative(1.5))
                    .child(question),
            )
            .when(!confirm, |view| {
                view.child(
                    div()
                        .id("ssh-input")
                        .debug_selector(|| "ssh-input".into())
                        .flex_none()
                        .w_full()
                        .child(Input::new(&self.input).small()),
                )
            })
            .child(
                div()
                    .id("ssh-actions")
                    .debug_selector(|| "ssh-actions".into())
                    .flex_none()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .small()
                            .outline()
                            .on_click(cx.listener(|this, _, window, _| this.finish(None, window))),
                    )
                    .child(
                        Button::new("continue")
                            .label(if confirm { "Trust host" } else { "Continue" })
                            .small()
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::{AnswerSink, Askpass, Prompts};
    use gpui_kit::{px, size, AppContext, TestAppContext};
    use splitty_client_core::native_workspace::SshPrompt;
    use std::{cell::RefCell, rc::Rc};

    type Answers = Rc<RefCell<Vec<(String, Option<String>)>>>;

    fn recording() -> (AnswerSink, Answers) {
        let answers: Answers = Rc::default();
        let recorded = answers.clone();
        let sink: AnswerSink = Rc::new(move |id, answer| recorded.borrow_mut().push((id, answer)));
        (sink, answers)
    }

    fn prompt(id: char, confirm: bool) -> SshPrompt {
        SshPrompt {
            id: id.to_string().repeat(64),
            target: "demo@localhost:2223".into(),
            prompt: "demo@localhost's password:".into(),
            confirm,
        }
    }

    /// Type `value` (for a password) and press the dialog's primary action,
    /// or with `None` dismiss it.
    fn act(app: &mut TestAppContext, prompts: &Prompts, id: &str, value: Option<&str>) {
        let (handle, view) = prompts.dialogs[id].clone();
        app.update(|cx| {
            handle
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| match value {
                        Some(value) => {
                            view.input
                                .update(cx, |input, cx| input.set_value(value, window, cx));
                            view.submit(window, cx);
                        }
                        None => view.finish(None, window),
                    })
                })
                .unwrap()
        });
        app.run_until_parked();
    }

    // remote-hosts, SSH Questions Of The Local Service: each listed question
    // gets one dialog; its answer or dismissal goes to the service once, and
    // a question that went closes its dialog without an answer.
    #[test]
    fn listed_questions_open_dialogs_that_answer_once() {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let (sink, answers) = recording();
        let mut prompts = Prompts::default();
        let list = [
            prompt('a', false),
            prompt('b', true),
            prompt('c', false),
            prompt('d', false),
        ];
        let id = |index: usize| list[index].id.clone();
        for _ in 0..2 {
            app.update(|cx| prompts.show(&list, &sink, cx));
            app.run_until_parked();
            assert_eq!(app.windows().len(), 4, "one dialog per question");
        }
        act(&mut app, &prompts, &id(0), Some("private answer"));
        act(&mut app, &prompts, &id(1), Some("ignored for a host key"));
        act(&mut app, &prompts, &id(3), None);
        assert_eq!(
            *answers.borrow(),
            [
                (id(0), Some("private answer".to_owned())),
                (id(1), Some("yes".to_owned())),
                (id(3), None),
            ]
        );
        assert_eq!(app.windows().len(), 1);
        // The answered questions are still listed until the service settles
        // them; their dialogs do not come back.
        app.update(|cx| prompts.show(&list, &sink, cx));
        app.run_until_parked();
        assert_eq!(app.windows().len(), 1);
        // The last question went without an answer from this window.
        app.update(|cx| prompts.show(&[], &sink, cx));
        app.run_until_parked();
        assert!(app.windows().is_empty());
        assert_eq!(answers.borrow().len(), 3, "closing answers nothing again");
    }

    #[test]
    fn password_prompt_input_and_actions_fit_the_authentication_window() {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let (_, window) = app.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                Askpass::new(
                    ("layout-test".into(), recording().0),
                    "demo@localhost:2223\n\ndemo@localhost's password:".into(),
                    false,
                    window,
                    cx,
                )
            });
            gpui_kit::base::Root::new(view, window, cx)
        });
        window.simulate_resize(size(px(640.), px(360.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let prompt = window.debug_bounds("ssh-prompt").unwrap();
        let input = window.debug_bounds("ssh-input").unwrap();
        let actions = window.debug_bounds("ssh-actions").unwrap();
        assert!(
            prompt.size.height >= px(80.),
            "the host and question must stay visible: {prompt:?}"
        );
        assert!(
            input.size.height >= px(24.) && input.size.height <= px(48.),
            "single-line credential field: {input:?}"
        );
        assert!(input.origin.y >= prompt.origin.y + prompt.size.height);
        assert!(actions.origin.y >= input.origin.y + input.size.height);
        assert!(actions.origin.y + actions.size.height <= px(360.));
    }
}
