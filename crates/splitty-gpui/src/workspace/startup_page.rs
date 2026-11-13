//! The page a window shows until its local workspace arrives: the phase the
//! start of the local backend is in, as plain text, or why the start failed.
//! On Windows the phases include starting the WSL distribution and
//! installing the backend there; elsewhere the page passes quickly. A
//! failure stays on the page: the window keeps no retry and offers nothing
//! but closing it (provisional, see `openspec/open-decisions.md`).
use super::*;
use splitty_client_core::owned_service::StartupPhase;

/// Where the start of a window's local backend is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Startup {
    /// No phase reported yet.
    #[default]
    Opening,
    Phase(StartupPhase),
    /// The start failed: the phase it was in and the reason.
    Failed {
        during: Option<StartupPhase>,
        reason: String,
    },
}

/// What the startup page says.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct StartupText {
    pub(super) title: &'static str,
    pub(super) detail: String,
    /// The phase a failure ended, shown under its reason.
    pub(super) during: Option<String>,
    pub(super) failed: bool,
}

/// The phase as one line, the same words the page shows while it runs.
fn describe(phase: &StartupPhase) -> String {
    match phase {
        StartupPhase::FindingDistribution => "Looking for WSL distributions…".to_owned(),
        StartupPhase::StartingDistribution(distro) => {
            format!("Starting the WSL distribution '{distro}'…")
        }
        StartupPhase::InstallingBackend(distro) => {
            format!("Installing the backend in '{distro}'…")
        }
        StartupPhase::StartingBackend(Some(distro)) => {
            format!("Starting the backend in '{distro}'…")
        }
        StartupPhase::StartingBackend(None) => "Starting the backend…".to_owned(),
        StartupPhase::Connecting => "Connecting to the backend…".to_owned(),
    }
}

impl Startup {
    /// The state after `phase` was reported.
    pub(super) fn entered(&mut self, phase: StartupPhase) {
        // A failure is final; a late phase does not hide it.
        if !matches!(self, Self::Failed { .. }) {
            *self = Self::Phase(phase);
        }
    }

    /// The state after the start failed with `reason`.
    pub(super) fn failed(&mut self, reason: String) {
        let during = match std::mem::take(self) {
            Self::Phase(phase) => Some(phase),
            Self::Failed { during, .. } => during,
            Self::Opening => None,
        };
        *self = Self::Failed { during, reason };
    }

    /// The WSL target is still being prepared: no backend exists yet, so
    /// closing the window needs no answer from one.
    pub(super) fn preparing(&self) -> bool {
        matches!(
            self,
            Self::Phase(
                StartupPhase::FindingDistribution
                    | StartupPhase::StartingDistribution(_)
                    | StartupPhase::InstallingBackend(_)
            )
        )
    }

    pub(super) fn text(&self) -> StartupText {
        match self {
            Self::Opening => StartupText {
                title: "Starting Splitty",
                detail: "Starting…".to_owned(),
                during: None,
                failed: false,
            },
            Self::Phase(phase) => StartupText {
                title: "Starting Splitty",
                detail: describe(phase),
                during: None,
                failed: false,
            },
            Self::Failed { during, reason } => StartupText {
                title: "Splitty could not start",
                detail: reason.trim().to_owned(),
                during: during.as_ref().map(|phase| {
                    format!("Failed while: {}", describe(phase).trim_end_matches('…'))
                }),
                failed: true,
            },
        }
    }
}

/// A failure's reason can span several lines with commands to run; the page
/// is wider for it than for one status line.
const FAILURE_WIDTH: f32 = 520.;

impl Workspace {
    pub(super) fn render_startup_page(&self) -> Div {
        let text = self.startup.text();
        let appearance = &self.appearance;
        let icon = if text.failed {
            Icon::new(IconName::TriangleAlert)
                .size(chrome_rems(empty_page::ICON))
                .text_color(rgb(appearance.muted))
                .opacity(empty_page::ICON_OPACITY)
                .into_any_element()
        } else {
            gpui_kit::component::spinner::Spinner::new()
                .icon(Icon::new(IconName::Loader))
                .with_size(px(empty_page::ICON))
                .color(rgb(appearance.muted).into())
                .into_any_element()
        };
        let (frame, column) = self.render_page_column(empty_page::PageColumn {
            page_id: if text.failed {
                "startup-failed-page"
            } else {
                "startup-page"
            },
            icon,
            title: text.title.into(),
            detail: text.detail.into(),
            max_width: if text.failed {
                FAILURE_WIDTH
            } else {
                empty_page::MAX_WIDTH
            },
        });
        frame.child(column.when_some(text.during, |column, during| {
            column.child(
                div()
                    .mt(chrome_rems(empty_page::ACTION_GAP))
                    .text_size(chrome_rems(11.))
                    .text_color(rgb(appearance.muted))
                    .text_center()
                    .child(during),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{Startup, StartupPhase};

    #[test]
    fn each_phase_is_one_plain_line_under_one_title() {
        let mut startup = Startup::default();
        assert_eq!(startup.text().title, "Starting Splitty");
        for (phase, line) in [
            (
                StartupPhase::FindingDistribution,
                "Looking for WSL distributions…",
            ),
            (
                StartupPhase::StartingDistribution("Ubuntu 24.04".into()),
                "Starting the WSL distribution 'Ubuntu 24.04'…",
            ),
            (
                StartupPhase::InstallingBackend("Debian".into()),
                "Installing the backend in 'Debian'…",
            ),
            (
                StartupPhase::StartingBackend(Some("Debian".into())),
                "Starting the backend in 'Debian'…",
            ),
            (StartupPhase::StartingBackend(None), "Starting the backend…"),
            (StartupPhase::Connecting, "Connecting to the backend…"),
        ] {
            let preparing = matches!(
                phase,
                StartupPhase::FindingDistribution
                    | StartupPhase::StartingDistribution(_)
                    | StartupPhase::InstallingBackend(_)
            );
            startup.entered(phase);
            let text = startup.text();
            assert_eq!(text.title, "Starting Splitty");
            assert_eq!(text.detail, line);
            assert!(!text.failed && text.during.is_none());
            assert_eq!(startup.preparing(), preparing, "{line}");
        }
        assert!(!Startup::default().preparing());
    }

    #[test]
    fn a_failure_names_its_phase_and_stays() {
        let mut startup = Startup::default();
        startup.entered(StartupPhase::InstallingBackend("Ubuntu".into()));
        startup.failed("The backend could not be installed.\n".into());
        let text = startup.text();
        assert!(text.failed);
        assert_eq!(text.title, "Splitty could not start");
        assert_eq!(text.detail, "The backend could not be installed.");
        assert_eq!(
            text.during.as_deref(),
            Some("Failed while: Installing the backend in 'Ubuntu'")
        );

        // A later phase or failure keeps the phase that failed.
        startup.entered(StartupPhase::Connecting);
        assert!(startup.text().failed);
        startup.failed("second".into());
        assert_eq!(
            startup.text().during.as_deref(),
            Some("Failed while: Installing the backend in 'Ubuntu'")
        );
        assert_eq!(startup.text().detail, "second");

        // Before any phase there is no phase to name.
        let mut early = Startup::default();
        early.failed("Windows workspace requires a WSL backend".into());
        assert!(early.text().during.is_none());
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use crate::workspace::{chrome::view_tests::with_view, Update, Workspace};
    use gpui_kit::{px, size, Entity, VisualTestContext};
    use splitty_client_core::{owned_service::StartupPhase, workspace::WindowWorkspace};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn deliver(window: &mut VisualTestContext, view: &Entity<Workspace>, update: Update) {
        let (_, mut updates) = mpsc::channel(1);
        window.update(|window, cx| {
            view.update(cx, |workspace, cx| {
                workspace.poll(Some(update), &mut updates, window, cx)
            });
            window.draw(cx).clear(cx);
        });
    }

    #[test]
    fn the_window_shows_the_phase_until_the_workspace_arrives_and_a_failure_stays() {
        with_view(|window, view| {
            window.simulate_resize(size(px(900.), px(560.)));
            window.update(|window, cx| window.draw(cx).clear(cx));
            assert!(window.debug_bounds("startup-page").is_some());
            deliver(
                window,
                &view,
                Update::Startup(StartupPhase::StartingDistribution("Ubuntu".into())),
            );
            assert!(window.debug_bounds("startup-page").is_some());
            assert!(window.debug_bounds("workspace-status").is_none());
            window.update(|_, cx| {
                assert_eq!(
                    view.read(cx).startup.text().detail,
                    "Starting the WSL distribution 'Ubuntu'…"
                );
            });

            deliver(
                window,
                &view,
                Update::LocalServiceFailed("WSL is not installed.".into()),
            );
            assert!(window.debug_bounds("startup-page").is_none());
            assert!(window.debug_bounds("startup-failed-page").is_some());
            assert!(
                window.debug_bounds("workspace-status").is_none(),
                "the failure is on the page, not in the status line"
            );
        });
    }

    #[test]
    fn a_failure_after_the_workspace_arrived_is_a_status_line() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            model
                .ui_test_screen_order_fixture(&[], None)
                .expect("empty local catalog");
            deliver(window, &view, Update::Workspace(Box::new(model)));
            assert!(window.debug_bounds("startup-page").is_none());
            deliver(window, &view, Update::LocalServiceFailed("ended".into()));
            assert!(window.debug_bounds("startup-failed-page").is_none());
            assert!(window.debug_bounds("workspace-status").is_some());
        });
    }
}
