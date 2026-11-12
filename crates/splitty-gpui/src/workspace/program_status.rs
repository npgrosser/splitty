//! Program status marks over the records each connection reports: a pane's
//! tab shows its program's state, a Screen and a host group show that a pane
//! in them waits for the user. A `done` or failed mark goes once the pane is
//! watched, as an unread notification does.
use super::*;
use splitty_client_core::{
    notifications::Severity,
    workspace::{ProgramState, ProgramStatus},
};

/// How the host lists word a Screen with a waiting pane.
pub(super) const WAITING: &str = "Waiting";
/// The marks have the size of the Drawer's pane badge.
const TAB_STATUS_DOT: f32 = super::drawer::PANE_BADGE;
/// Space between the tab title and the mark.
const TAB_STATUS_GAP: f32 = 6.;

fn severity(state: ProgramState) -> Severity {
    match state {
        ProgramState::Idle | ProgramState::Working => Severity::Info,
        ProgramState::Blocked => Severity::Warning,
        ProgramState::Done => Severity::Success,
        ProgramState::Error => Severity::Error,
    }
}

impl Workspace {
    /// The status a pane's tab shows: none for an idle program, none for a
    /// `done` or failed record the user saw.
    pub(super) fn tab_program_status(&self, pane: &PaneKey) -> Option<&ProgramStatus> {
        let source = self.model.as_ref()?.source(pane.source_id())?;
        self.program_status_seen
            .shown(source, pane.pane_id())
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

    pub(super) fn render_tab_status(&self, pane: &PaneKey, cx: &App) -> Option<AnyElement> {
        let status = self.tab_program_status(pane)?;
        let summary = status.summary();
        let state = status.state().as_str();
        let pane_id = pane.pane_id().to_owned();
        Some(
            div()
                .id(SharedString::from(format!("tab-status-{pane}")))
                .when(cfg!(all(test, feature = "ui-test-support")), |mark| {
                    mark.debug_selector(move || format!("tab-status-{pane_id}-{state}"))
                })
                .flex_none()
                .ml(chrome_rems(TAB_STATUS_GAP))
                .size(chrome_rems(TAB_STATUS_DOT))
                .rounded_full()
                .bg(super::unread::severity_color(severity(status.state()), cx))
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
