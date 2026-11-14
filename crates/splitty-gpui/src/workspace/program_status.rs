//! Program status marks over the records each connection reports: a pane's
//! tab and its row in the Drawer show its program's state before the title, a
//! Screen and a host group show that a pane in them waits for the user. A `done` or failed mark goes once the pane is
//! watched, as an unread notification does.
use super::*;
use gpui_kit::{pulsating_between, Animation, AnimationExt};
use splitty_client_core::{
    notifications::Severity,
    workspace::{ProgramState, ProgramStatus},
};

/// How the host lists word a Screen with a waiting pane.
pub(super) const WAITING: &str = "Waiting";
/// The marks have the size of the Drawer's pane badge.
const TAB_STATUS_DOT: f32 = super::drawer::PANE_BADGE;

/// The glyphs of a program's state, all in the terminal face the app carries,
/// so they are monochrome and present on every platform.
const GLYPH_PROGRAM: &str = "\u{2736}";
const GLYPH_DONE: &str = "\u{2713}";
const GLYPH_ERROR: &str = "\u{2715}";
/// One breath of a working program's glyph, and how far it fades.
const WORKING_PULSE: std::time::Duration = std::time::Duration::from_millis(1600);
const WORKING_PULSE_LOW: f32 = 0.35;

/// What the icon place before a pane's title shows, on its tab and on its
/// row in the Drawer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TitleMark {
    /// A program that works, waits, or has an unseen `done` or failed record.
    Status(ProgramState),
    /// A program that reports its status and is at rest.
    Idle,
    /// The title's own leading icon.
    Icon(char),
}

/// The mark of a pane whose session holds a record in `record`, `unseen`
/// when that record still shows, and whose title starts with `icon`. A
/// state the user has to notice takes the place; at rest the title's own
/// icon keeps it.
pub(super) fn title_mark(
    record: Option<ProgramState>,
    unseen: bool,
    icon: Option<char>,
) -> Option<TitleMark> {
    match record {
        Some(state @ (ProgramState::Working | ProgramState::Blocked)) => {
            Some(TitleMark::Status(state))
        }
        Some(state @ (ProgramState::Done | ProgramState::Error)) if unseen => {
            Some(TitleMark::Status(state))
        }
        Some(_) => Some(icon.map_or(TitleMark::Idle, TitleMark::Icon)),
        None => icon.map(TitleMark::Icon),
    }
}

/// A mark's glyph in its font and color. An icon of the title keeps the
/// color of its surroundings.
pub(super) fn title_mark_glyph(mark: TitleMark, cx: &App) -> AnyElement {
    let (glyph, color) = match mark {
        TitleMark::Icon(icon) => {
            return div()
                .when_some(super::drawer::title_icon_family(), |icon, family| {
                    icon.font_family(family)
                })
                .child(icon.to_string())
                .into_any_element();
        }
        TitleMark::Idle => (GLYPH_PROGRAM, cx.theme().muted_foreground),
        TitleMark::Status(ProgramState::Working) => (GLYPH_PROGRAM, cx.theme().foreground),
        TitleMark::Status(state) => (
            match state {
                ProgramState::Done => GLYPH_DONE,
                ProgramState::Error => GLYPH_ERROR,
                _ => GLYPH_PROGRAM,
            },
            super::unread::severity_color(severity(state), cx),
        ),
    };
    let glyph = div()
        .font_family(crate::terminal::terminal_family(cx))
        .text_color(color)
        .child(glyph);
    if mark == TitleMark::Status(ProgramState::Working) {
        glyph
            .with_animation(
                "program-working",
                Animation::new(WORKING_PULSE)
                    .repeat()
                    .with_easing(pulsating_between(WORKING_PULSE_LOW, 1.)),
                |glyph, shown| glyph.opacity(shown),
            )
            .into_any_element()
    } else {
        glyph.into_any_element()
    }
}

fn severity(state: ProgramState) -> Severity {
    match state {
        ProgramState::Idle | ProgramState::Working => Severity::Info,
        ProgramState::Blocked => Severity::Warning,
        ProgramState::Done => Severity::Success,
        ProgramState::Error => Severity::Error,
    }
}

impl Workspace {
    /// The status a pane shows as one the user has to notice: none for an
    /// idle program, none for a `done` or failed record the user saw.
    pub(super) fn tab_program_status(&self, pane: &PaneKey) -> Option<&ProgramStatus> {
        self.shown_program_status(pane.source_id(), pane.pane_id())
    }

    fn shown_program_status(&self, source: SourceId, pane: &str) -> Option<&ProgramStatus> {
        let source = self.model.as_ref()?.source(source)?;
        self.program_status_seen
            .shown(source, pane)
            .filter(|status| status.state() != ProgramState::Idle)
    }

    /// Whether a pane on `screen` waits for the user.
    pub(super) fn screen_blocked(&self, screen: &ScreenKey) -> bool {
        self.model
            .as_ref()
            .and_then(|model| model.source(screen.source_id()))
            .is_some_and(|source| source.blocked_screens().contains(screen.screen_id()))
    }

    /// Whether a pane on a Screen of `source` waits for the user, also on a
    /// Screen this window has not opened.
    pub(super) fn source_blocked(&self, source: SourceId) -> bool {
        self.model
            .as_ref()
            .and_then(|model| model.source(source))
            .is_some_and(|source| !source.blocked_screens().is_empty())
    }

    /// Whether a pane on a host's Screen waits for the user, opened or not.
    pub(super) fn host_screen_blocked(&self, target: &str, screen: &str) -> bool {
        self.model
            .as_ref()
            .and_then(|model| model.source(model.source_for_target(target)?))
            .is_some_and(|source| source.blocked_screens().contains(screen))
    }

    /// The watched pane's `done` or failed record counts as seen.
    pub(super) fn see_program_status(&mut self, pane: &PaneKey) -> bool {
        let Some(source) = self
            .model
            .as_ref()
            .and_then(|model| model.source(pane.source_id()))
        else {
            return false;
        };
        self.program_status_seen.see(source, pane.pane_id())
    }

    /// The mark before the title of a pane of `source`, whose title starts
    /// with `icon`.
    pub(super) fn pane_title_mark(
        &self,
        source: SourceId,
        pane: &str,
        icon: Option<char>,
    ) -> Option<TitleMark> {
        let record = self
            .model
            .as_ref()
            .and_then(|model| model.source(source))
            .and_then(|source| source.session_program_status(source.session_for(pane)?))
            .map(|status| status.state());
        title_mark(
            record,
            self.shown_program_status(source, pane).is_some(),
            icon,
        )
    }

    /// The mark before a tab's title; a program's state explains itself on
    /// hover.
    pub(super) fn render_tab_mark(
        &self,
        pane: &PaneKey,
        icon: Option<char>,
        cx: &App,
    ) -> Option<AnyElement> {
        let mark = self.pane_title_mark(pane.source_id(), pane.pane_id(), icon)?;
        let glyph = title_mark_glyph(mark, cx);
        let (TitleMark::Status(_) | TitleMark::Idle) = mark else {
            return Some(glyph);
        };
        let summary = self
            .tab_program_status(pane)
            .map_or_else(|| "Idle".into(), |status| status.summary());
        let state = match mark {
            TitleMark::Status(state) => state.as_str(),
            _ => "rest",
        };
        let pane_id = pane.pane_id().to_owned();
        Some(
            div()
                .id(SharedString::from(format!("tab-status-{pane}")))
                .when(cfg!(all(test, feature = "ui-test-support")), |mark| {
                    mark.debug_selector(move || format!("tab-status-{pane_id}-{state}"))
                })
                .child(glyph)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(summary.clone()).build(window, cx)
                })
                .into_any_element(),
        )
    }

    /// The Drawer's mark for a Screen or a host group with a waiting pane.
    pub(super) fn render_blocked_mark(&self, id: String, cx: &App) -> AnyElement {
        div()
            .id(SharedString::from(id.clone()))
            .when(cfg!(all(test, feature = "ui-test-support")), |mark| {
                mark.debug_selector(move || id.clone())
            })
            .flex_none()
            .size(chrome_rems(TAB_STATUS_DOT))
            .rounded_full()
            .bg(super::unread::severity_color(Severity::Warning, cx))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("A program is waiting for you")
                    .build(window, cx)
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{title_mark, ProgramState, TitleMark};

    #[test]
    fn a_state_to_notice_takes_the_icon_place_and_rest_leaves_it_to_the_title() {
        for state in [ProgramState::Working, ProgramState::Blocked] {
            for unseen in [true, false] {
                assert_eq!(
                    title_mark(Some(state), unseen, Some('⚡')),
                    Some(TitleMark::Status(state))
                );
            }
        }
        for state in [ProgramState::Done, ProgramState::Error] {
            assert_eq!(
                title_mark(Some(state), true, Some('⚡')),
                Some(TitleMark::Status(state))
            );
            assert_eq!(title_mark(Some(state), false, None), Some(TitleMark::Idle));
            assert_eq!(
                title_mark(Some(state), false, Some('⚡')),
                Some(TitleMark::Icon('⚡'))
            );
        }
        assert_eq!(
            title_mark(Some(ProgramState::Idle), true, None),
            Some(TitleMark::Idle)
        );
        assert_eq!(
            title_mark(Some(ProgramState::Idle), true, Some('⚡')),
            Some(TitleMark::Icon('⚡'))
        );
        assert_eq!(
            title_mark(None, false, Some('⚡')),
            Some(TitleMark::Icon('⚡'))
        );
        assert_eq!(title_mark(None, false, None), None);
    }
}
