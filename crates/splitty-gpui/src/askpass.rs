//! Native presentation of the shared SSH authentication broker. Unix embedded
//! backends share its connection-scoped credentials with the browser frontend.
//! The standalone askpass window remains available for the Windows WSL helper.
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    input::Input,
    ActiveTheme as _, Sizable as _,
};
use gpui_kit::{prelude::*, *};
use std::{io::Write, time::Duration};

struct Askpass {
    request: Option<String>,
    prompt: String,
    confirm: bool,
    input: Entity<InputState>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl Drop for Askpass {
    fn drop(&mut self) {
        if let Some(id) = &self.request {
            splitty_client_core::answer_ssh_prompt(id, None);
        }
    }
}

impl Askpass {
    fn finish(&self, answer: Option<String>, window: &mut Window) {
        if let Some(id) = &self.request {
            splitty_client_core::answer_ssh_prompt(id, answer);
            window.remove_window();
        } else {
            let Some(value) = answer else {
                std::process::exit(1)
            };
            let mut stdout = std::io::stdout().lock();
            let result = writeln!(stdout, "{value}").and_then(|_| stdout.flush());
            std::process::exit(if result.is_ok() { 0 } else { 1 });
        }
    }

    fn submit(&self, window: &mut Window, cx: &App) {
        let answer = if self.confirm {
            "yes".to_owned()
        } else {
            self.input.read(cx).value().to_string()
        };
        self.finish(Some(answer), window);
    }

    fn new(
        request: Option<String>,
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
            request,
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

/// One application-level presenter, so multiple workspace windows cannot answer
/// the same prompt. Dropped/cancelled SSH attempts also close their dialog.
#[cfg(not(target_os = "windows"))]
pub fn install(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut dialogs: std::collections::HashMap<String, AnyWindowHandle> = Default::default();
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let prompts = splitty_client_core::ssh_prompts();
            cx.update(|cx| {
                dialogs.retain(|id, handle| {
                    if prompts.iter().any(|prompt| &prompt.id == id) {
                        return true;
                    }
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                    false
                });
                for prompt in prompts {
                    if dialogs.contains_key(&prompt.id) {
                        continue;
                    }
                    let id = prompt.id.clone();
                    let result =
                        gpui_kit::open_window(options(prompt.confirm, cx), cx, |window, cx| {
                            cx.new(|cx| {
                                Askpass::new(
                                    Some(prompt.id),
                                    format!("{}\n\n{}", prompt.target, prompt.prompt),
                                    prompt.confirm,
                                    window,
                                    cx,
                                )
                            })
                        });
                    match result {
                        Ok((handle, _)) => {
                            dialogs.insert(id, handle);
                            cx.activate(true);
                        }
                        Err(_) => {
                            splitty_client_core::answer_ssh_prompt(&id, None);
                        }
                    }
                }
            });
        }
    })
    .detach();
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

pub fn run() -> anyhow::Result<()> {
    splitty_client_core::watch_askpass_parent();
    let prompt = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Missing SSH prompt"))?;
    anyhow::ensure!(prompt.len() <= 16 * 1024, "SSH prompt too long");
    let confirm = splitty_client_core::ssh_prompt_is_confirmation(
        &prompt,
        std::env::var("SSH_ASKPASS_PROMPT").ok().as_deref(),
    );
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    std::process::exit(1);
                }
            })
            .detach();
            // Do not leave an abandoned credential dialog alive indefinitely.
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(110))
                    .await;
                std::process::exit(1);
            })
            .detach();
            let result = gpui_kit::open_window(options(confirm, cx), cx, |window, cx| {
                cx.new(|cx| Askpass::new(None, prompt, confirm, window, cx))
            });
            if let Err(error) = result {
                eprintln!("Could not open SSH dialog: {error}");
                std::process::exit(1);
            }
            cx.activate(true);
        });
    Ok(())
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::Askpass;
    use gpui_kit::{px, size, AppContext, TestAppContext};

    #[test]
    fn password_prompt_input_and_actions_fit_the_authentication_window() {
        let mut app = TestAppContext::single();
        app.update(gpui_kit::init);
        let (_, window) = app.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                Askpass::new(
                    Some("layout-test".into()),
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
