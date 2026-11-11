//! The confirmation dialog in the window's theme. GPUI's own prompt is the
//! system's or a plain white fallback; this one replaces both.
use super::*;
use gpui::{PromptButton, PromptHandle, PromptResponse, RenderablePromptHandle};

const WIDTH: f32 = 380.;
const PADDING: f32 = 18.;
const TITLE_TEXT: f32 = 13.5;
const DETAIL_TEXT: f32 = 12.;
const BUTTON_HEIGHT: f32 = 28.;
const BUTTON_TEXT: f32 = 12.;

/// The colors the next prompt is drawn in: those of the window that asks.
struct PromptTheme(NativeAppearance);
impl Global for PromptTheme {}

/// Draw confirmations in the theme from now on.
pub(super) fn install(cx: &mut App) {
    cx.set_prompt_builder(build);
}

pub(super) fn set_theme(appearance: &NativeAppearance, cx: &mut App) {
    cx.set_global(PromptTheme(appearance.clone()));
}

fn build(
    level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    actions: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let appearance = cx
        .try_global::<PromptTheme>()
        .map(|theme| theme.0.clone())
        .unwrap_or_else(|| NativeAppearance::fallback(".SystemUIFont"));
    let view = cx.new(|cx| ThemedPrompt {
        danger: !matches!(level, PromptLevel::Info),
        message: message.to_owned().into(),
        detail: detail.map(|detail| detail.to_owned().into()),
        actions: actions
            .iter()
            .map(|action| action.label().clone())
            .collect(),
        appearance,
        focus: cx.focus_handle(),
    });
    handle.with_view(view, window, cx)
}

struct ThemedPrompt {
    /// The last answer goes through with something that cannot be undone.
    danger: bool,
    message: SharedString,
    detail: Option<SharedString>,
    actions: Vec<SharedString>,
    appearance: NativeAppearance,
    focus: FocusHandle,
}

impl EventEmitter<PromptResponse> for ThemedPrompt {}
impl Focusable for ThemedPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ThemedPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.appearance;
        let last = self.actions.len().saturating_sub(1);
        let hover: Hsla = rgba(colors.hover_overlay).into();
        let confirm = if self.danger {
            colors.danger
        } else {
            colors.accent
        };
        let confirm_hover: Hsla = if self.danger {
            rgba(colors.danger_hover).into()
        } else {
            hover
        };
        let buttons = self.actions.iter().enumerate().map(|(index, label)| {
            let primary = index == last && last > 0;
            div()
                .id(("prompt-answer", index))
                .flex_none()
                .h(chrome_rems(BUTTON_HEIGHT))
                .px(chrome_rems(12.))
                .flex()
                .items_center()
                .rounded(chrome_rems(5.))
                .border_1()
                .border_color(rgb(if primary {
                    confirm
                } else {
                    colors.subtle_border
                }))
                .text_size(chrome_rems(BUTTON_TEXT))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(if primary { confirm } else { colors.secondary }))
                .cursor_pointer()
                .hover(move |button| button.bg(if primary { confirm_hover } else { hover }))
                .child(label.clone())
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(PromptResponse(index));
                }))
        });
        let panel = div()
            .w(chrome_rems(WIDTH))
            .max_w(relative(0.9))
            .flex()
            .flex_col()
            .gap(chrome_rems(6.))
            .p(chrome_rems(PADDING))
            .bg(rgb(colors.overlay))
            .border_1()
            .border_color(rgb(colors.border))
            .rounded(chrome_rems(colors.radius.max(6.)))
            .shadow(vec![BoxShadow {
                color: rgba(colors.shadow).into(),
                offset: point(px(0.), px(12.)),
                blur_radius: px(32.),
                spread_radius: px(0.),
                inset: false,
            }])
            .occlude()
            .child(
                div()
                    .text_size(chrome_rems(TITLE_TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(colors.text))
                    .child(self.message.clone()),
            )
            .children(self.detail.clone().map(|detail| {
                div()
                    .text_size(chrome_rems(DETAIL_TEXT))
                    .line_height(relative(1.45))
                    .text_color(rgb(colors.secondary))
                    .child(detail)
            }))
            .child(
                div()
                    .mt(chrome_rems(12.))
                    .flex()
                    .justify_end()
                    .gap(chrome_rems(8.))
                    .children(buttons),
            );
        div()
            .id("prompt-backdrop")
            .track_focus(&self.focus)
            .size_full()
            .absolute()
            .top_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .font_family(SharedString::from(colors.font_family.clone()))
            .bg(rgba(0x00000080))
            .occlude()
            // Escape keeps things as they are; Enter gives the last answer.
            .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                let answer = match event.keystroke.key.as_str() {
                    "escape" => 0,
                    "enter" => last,
                    _ => return,
                };
                cx.stop_propagation();
                cx.emit(PromptResponse(answer));
            }))
            .child(panel)
    }
}
