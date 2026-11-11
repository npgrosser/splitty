//! Window observations and Drawer presentation over the shared unread policy.
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
use super::*;
use splitty_client_core::{
    local_service::NotificationSettingsRead, notifications::Severity, Event,
};

impl Workspace {
    fn watched_notification_pane(&self) -> Option<PaneKey> {
        if !self.notification_window_active
            || !self.notification_window_visible
            || self.destination != Destination::Screens
        {
            return None;
        }
        let source = self.selected_model()?;
        let pane = source.focused_pane()?;
        if !source.is_displayed_pane(pane) {
            return None;
        }
        let key = source.pane_key(pane);
        self.panes.contains_key(&key).then_some(key)
    }

    pub(super) fn refresh_notification_watch(&mut self, cx: &mut Context<Self>) {
        if let Some(pane) = self.watched_notification_pane() {
            if self.notifications.mark_read(&pane) {
                cx.notify();
            }
        }
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
/// `notify-send`, macOS through `osascript`; elsewhere nothing is shown yet.
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
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            drop(title);
        })
        .detach();
}

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
