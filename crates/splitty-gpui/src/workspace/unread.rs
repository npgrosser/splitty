//! Window observations and Drawer presentation over the shared unread policy.
//!
//! What counts as seen is the window's to say: a pane is seen after it has
//! stayed in view for a moment (`ViewWatch`), and the seen pane with the
//! focus is the watched one, whose notices are read as they arrive.
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
use super::*;
use splitty_client_core::{
    local_service::NotificationSettingsRead, notifications::Severity, Event,
};

/// How long a pane stays in view before what it holds counts as seen, so
/// that passing through a Screen marks nothing read.
const VIEW_DWELL: Duration = Duration::from_millis(1000);

/// The panes in view and which of them the user has seen. A pane is seen
/// once it has stayed in view for `VIEW_DWELL`; one that leaves the view
/// starts over when it returns.
#[derive(Debug, Default)]
pub(super) struct ViewWatch {
    /// Per pane the round it came into view in, `None` once it is seen.
    panes: HashMap<PaneKey, Option<u64>>,
    round: u64,
}

impl ViewWatch {
    /// Take the panes in view now. Returns the round to settle after the
    /// dwell when a pane came into view.
    fn observe(&mut self, in_view: &[PaneKey]) -> Option<u64> {
        self.panes.retain(|pane, _| in_view.contains(pane));
        let came: Vec<_> = in_view
            .iter()
            .filter(|pane| !self.panes.contains_key(*pane))
            .cloned()
            .collect();
        if came.is_empty() {
            return None;
        }
        self.round += 1;
        for pane in came {
            self.panes.insert(pane, Some(self.round));
        }
        Some(self.round)
    }

    /// The panes that stayed in view since `round` or longer, seen from now.
    fn settle(&mut self, round: u64) -> Vec<PaneKey> {
        let mut seen = Vec::new();
        for (pane, since) in &mut self.panes {
            if since.is_some_and(|since| since <= round) {
                *since = None;
                seen.push(pane.clone());
            }
        }
        seen
    }

    fn seen(&self, pane: &PaneKey) -> bool {
        self.panes.get(pane) == Some(&None)
    }
}

impl Workspace {
    /// The panes the user can see: those displayed on the shown Screen while
    /// the window is focused, visible and shows its Screens.
    fn panes_in_view(&self) -> Vec<PaneKey> {
        if !self.notification_window_active
            || !self.notification_window_visible
            || self.destination != Destination::Screens
        {
            return Vec::new();
        }
        let Some(source) = self.selected_model() else {
            return Vec::new();
        };
        source
            .displayed_panes()
            .iter()
            .map(|pane| source.pane_key(pane))
            .filter(|key| self.panes.contains_key(key))
            .collect()
    }

    /// The seen pane with the focus: what arrives for it is read at once.
    fn watched_notification_pane(&self) -> Option<PaneKey> {
        if !self.notification_window_active
            || !self.notification_window_visible
            || self.destination != Destination::Screens
        {
            return None;
        }
        let source = self.selected_model()?;
        let key = source.pane_key(source.focused_pane()?);
        self.view_watch.seen(&key).then_some(key)
    }

    pub(super) fn refresh_notification_watch(&mut self, cx: &mut Context<Self>) {
        let in_view = self.panes_in_view();
        if let Some(round) = self.view_watch.observe(&in_view) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(VIEW_DWELL).await;
                let _ = this.update(cx, |this, cx| {
                    let mut changed = false;
                    for pane in this.view_watch.settle(round) {
                        changed |= this.see_pane(&pane);
                    }
                    if changed {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        if let Some(pane) = self.watched_notification_pane() {
            if self.see_pane(&pane) {
                cx.notify();
            }
        }
    }

    /// Mark what a pane holds as read and its `done` or failed record as seen.
    fn see_pane(&mut self, pane: &PaneKey) -> bool {
        let read = self.notifications.mark_read(pane);
        read | self.see_program_status(pane)
    }

    pub(super) fn apply_notification_settings(
        &mut self,
        owner: SourceToken,
        read: NotificationSettingsRead,
        cx: &mut Context<Self>,
    ) {
        if self.lifecycle_token() != Some(owner)
            || read.connection_id() != owner.connection_id()
            || read.generation() <= self.notification_settings_generation
        {
            return;
        }
        self.notification_settings_generation = read.generation();
        self.settings.update(cx, |settings, cx| {
            settings.set_notification_read(read.clone(), cx)
        });
        if let Some(model) = &self.model {
            self.notifications.reconcile(model);
        }
        match read.result() {
            Ok(settings) => {
                self.notifications.apply_settings(settings);
                self.notification_settings_error = None;
            }
            Err(error) => self.notification_settings_error = Some(error.to_owned()),
        }
        cx.notify();
    }

    pub(super) fn observe_notification(
        &mut self,
        source: SourceToken,
        event: &Event,
        cx: &mut Context<Self>,
    ) {
        if event.json().is_none() {
            return;
        }
        let watched = self.watched_notification_pane();
        let owner = self.lifecycle_token();
        let Some(model) = &self.model else {
            return;
        };
        let host = model.source_target(source.source_id()).map(|target| {
            owner
                .and_then(|owner| self.hosts.accepted_alias(owner, target))
                .unwrap_or(target)
        });
        let changed = self.notifications.observe(
            model,
            source,
            event,
            watched.as_ref(),
            host.unwrap_or("Local"),
        );
        let toasts = self.notifications.toasts();
        let desktop = self.notifications.desktop() && !self.notification_window_active;
        for fresh in self.notifications.take_fresh() {
            if toasts {
                self.activity_notice(&fresh, cx);
            }
            if desktop {
                system_notification(&fresh.title, cx);
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Go to the terminal a notice told about.
    pub(super) fn show_notified(
        &mut self,
        screen: ScreenKey,
        pane: Option<PaneKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.destination != Destination::Screens {
            self.show_screens(window, cx);
        }
        if self.current_screen_key().as_ref() == Some(&screen) {
            if let Some(pane) = pane {
                self.select(pane.pane_id(), window, cx);
            }
            return;
        }
        // The pane is chosen once its Screen is the shown one.
        self.pending_notified = pane.map(|pane| (screen.clone(), pane));
        self.switch_screen_key(screen, window, cx);
    }

    /// Select the pane of a followed notice when its Screen has come up.
    pub(super) fn settle_notified(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((screen, pane)) = self.pending_notified.take() else {
            return;
        };
        if self.current_screen_key().as_ref() != Some(&screen) {
            return;
        }
        if self
            .selected_model()
            .is_some_and(|model| model.screen_for(pane.pane_id()) == Some(screen.screen_id()))
        {
            self.select(pane.pane_id(), window, cx);
        }
    }

    pub(super) fn screen_unread_summary(&self, screen: &ScreenKey) -> Option<String> {
        let count = self.notifications.unread_count(screen);
        if count == 0 {
            return None;
        }
        let titles = self
            .notifications
            .unread_titles(screen)
            .take(5)
            .collect::<Vec<_>>()
            .join(" · ");
        Some(format!("{count} unread: {titles}"))
    }
}

/// Hand a notification to the system. Linux desktops take it through
/// `notify-send`, macOS through `osascript`, Windows through a PowerShell toast.
fn system_notification(title: &str, cx: &App) {
    let title = title.to_owned();
    cx.background_executor()
        .spawn(async move {
            #[cfg(target_os = "linux")]
            let _ = std::process::Command::new("notify-send")
                .args(["--app-name", "Splitty", "--", "Splitty", &title])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("osascript")
                .args([
                    "-e",
                    "on run argv",
                    "-e",
                    "display notification (item 1 of argv) with title \"Splitty\"",
                    "-e",
                    "end run",
                    &title,
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
                let powershell = std::path::Path::new(&root)
                    .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
                let _ = std::process::Command::new(powershell)
                    .args(["-NoProfile", "-NonInteractive", "-Command", WINDOWS_TOAST])
                    .env("SPLITTY_NOTICE", &title)
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW, no console flashing up
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            drop(title);
        })
        .detach();
}

/// Shows a toast through Windows PowerShell's own application identity: an
/// executable without an installer has no identity of its own to show one
/// under. The notice's text travels in the environment and becomes a text
/// node, so nothing in it is read as script or markup. The script has no
/// double quotes, which keeps it one unambiguous argument.
#[cfg(target_os = "windows")]
const WINDOWS_TOAST: &str = "\
$m = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]; \
$x = $m::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType, Windows.UI.Notifications, ContentType = WindowsRuntime]::ToastText02); \
$t = $x.GetElementsByTagName('text'); \
[void]$t.Item(0).AppendChild($x.CreateTextNode('Splitty')); \
[void]$t.Item(1).AppendChild($x.CreateTextNode($env:SPLITTY_NOTICE)); \
$n = [Windows.UI.Notifications.ToastNotification, Windows.UI.Notifications, ContentType = WindowsRuntime]::new($x); \
$m::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe').Show($n)";

pub(super) fn severity_color(severity: Severity, cx: &App) -> Hsla {
    match severity {
        Severity::Info => cx.theme().primary,
        Severity::Success => cx.theme().success,
        Severity::Warning => cx.theme().warning,
        Severity::Error => cx.theme().danger,
    }
}

pub(super) fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Success => "success",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

#[cfg(test)]
mod view_watch_tests {
    use super::ViewWatch;
    use splitty_client_core::workspace::{PaneKey, WindowWorkspace};
    use uuid::Uuid;

    fn panes<const N: usize>(ids: [&str; N]) -> [PaneKey; N] {
        let window = WindowWorkspace::new(Uuid::new_v4());
        ids.map(|id| window.local_source().pane_key(id))
    }

    #[test]
    fn a_pane_is_seen_after_its_own_dwell_and_starts_over_when_it_returns() {
        let [a, b, c] = panes(["a", "b", "c"]);
        let mut watch = ViewWatch::default();
        let first = watch.observe(&[a.clone(), b.clone()]).unwrap();
        assert!(!watch.seen(&a) && !watch.seen(&b));
        assert_eq!(watch.observe(&[a.clone(), b.clone()]), None);

        // `c` comes into view later and `b` leaves before its dwell ends.
        let second = watch.observe(&[a.clone(), c.clone()]).unwrap();
        assert_eq!(watch.settle(first), std::slice::from_ref(&a));
        assert!(watch.seen(&a) && !watch.seen(&b) && !watch.seen(&c));
        assert!(watch.settle(first).is_empty(), "a pane is seen once");

        // `b` returns: the dwell of its first stay does not count.
        let third = watch.observe(&[a.clone(), b.clone(), c.clone()]).unwrap();
        assert_eq!(watch.settle(second), std::slice::from_ref(&c));
        assert!(!watch.seen(&b));
        assert_eq!(watch.settle(third), std::slice::from_ref(&b));

        // Out of view nothing is seen, and a later dwell covers earlier ones.
        assert_eq!(watch.observe(&[]), None);
        assert!(!watch.seen(&a));
        let fourth = watch.observe(std::slice::from_ref(&a)).unwrap();
        let fifth = watch.observe(&[a.clone(), b.clone()]).unwrap();
        let mut seen = watch.settle(fifth);
        seen.sort_by(|x, y| x.pane_id().cmp(y.pane_id()));
        assert_eq!(seen, [a, b]);
        assert!(watch.settle(fourth).is_empty());
    }
}
