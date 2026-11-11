//! Bounded window-local notification facts, independent of transport readiness.
use crate::{
    connection::Event,
    workspace::{
        PaneKey, ScreenKey, SourceId, SourceKind, SourceToken, SourceWorkspace, WindowWorkspace,
    },
};
use serde_json::Value;
use splitty_core::config::NotificationSettings;
use std::collections::VecDeque;

const MAX_RECORDS: usize = 100;
/// Facts kept for the window between two reads; a burst shows its first ones.
const MAX_FRESH: usize = 16;
const MAX_ID_BYTES: usize = 128;
const MAX_TITLE_CHARS: usize = 500;
const MAX_HOST_CHARS: usize = 200;

/// Ascending priority for an unread pane indicator.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy)]
enum Kind {
    Finished,
    Idle,
    Exited,
    Crashed,
    Generic,
}
impl Kind {
    fn enabled(self, settings: &NotificationSettings) -> bool {
        settings.enabled
            && match self {
                Self::Finished => settings.process_finished,
                Self::Exited => settings.process_exited,
                Self::Crashed => settings.process_crashed,
                Self::Idle | Self::Generic => true,
            }
    }
}
struct Captured {
    pane: PaneKey,
    session: String,
}
#[derive(PartialEq)]
struct Binding {
    owner: SourceToken,
    screen: ScreenKey,
}
struct Record {
    source: SourceId,
    id: String,
    kind: Kind,
    severity: Severity,
    title: String,
    captured: Option<Captured>,
    binding: Option<Binding>,
    read: bool,
}

/// One window's last 100 admitted facts. No event, JSON or transport lease is retained.
#[derive(Default)]
pub struct NotificationCenter {
    owner: Option<SourceToken>,
    settings: Option<NotificationSettings>,
    records: VecDeque<Record>,
    fresh: Vec<FreshNotification>,
}

/// A fact admitted since the last `take_fresh`, for a notice in the window
/// or from the system. Its Screen is absent when no pane of the window holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshNotification {
    pub title: String,
    pub severity: Severity,
    pub pane: Option<PaneKey>,
    pub screen: Option<ScreenKey>,
}
impl NotificationCenter {
    /// Apply only an already owner/generation-accepted settings result. A failed
    /// read does not call this method and therefore preserves the last good value.
    pub fn apply_settings(&mut self, settings: &NotificationSettings) -> bool {
        let changed = self.settings.as_ref() != Some(settings);
        self.settings = Some(settings.clone());
        let before = self.records.len();
        self.records.retain(|record| record.kind.enabled(settings));
        changed || before != self.records.len()
    }

    /// Reconcile before projecting a new workspace. Empty reconnect observations
    /// hide bindings but preserve bounded history until absence is conclusive.
    pub fn reconcile(&mut self, window: &WindowWorkspace) -> bool {
        let owner = window.lifecycle_token();
        let mut changed = false;
        if self.owner != Some(owner) {
            if self.owner.is_some() {
                self.records.clear();
                self.fresh.clear();
                self.settings = None;
            }
            self.owner = Some(owner);
            changed = true;
        }
        self.records.retain_mut(|record| {
            let Some(source) = window.source(record.source) else {
                changed = true;
                return false;
            };
            let Some(captured) = &record.captured else {
                return true;
            };
            let screen = canonical_screen(source, captured.pane.pane_id(), &captured.session);
            let binding = screen.map(|screen| Binding {
                owner: source.source_token(),
                screen,
            });
            if binding.is_none() && source.route_observation_complete() {
                changed = true;
                return false;
            }
            if record.binding != binding {
                record.binding = binding;
                changed = true;
            }
            true
        });
        changed
    }

    /// Consume borrowed, already-charged event data. `watched` is the UI's exact
    /// pane in a focused, visible window; the core never infers platform focus.
    pub fn observe(
        &mut self,
        window: &WindowWorkspace,
        source: SourceToken,
        event: &Event,
        watched: Option<&PaneKey>,
        host_label: &str,
    ) -> bool {
        let changed = self.reconcile(window);
        if event.connection_id != source.connection_id() {
            return changed;
        }
        event
            .json()
            .is_some_and(|value| self.observe_json(window, source, value, watched, host_label))
            || changed
    }

    fn observe_json(
        &mut self,
        window: &WindowWorkspace,
        owner: SourceToken,
        value: &Value,
        watched: Option<&PaneKey>,
        host_label: &str,
    ) -> bool {
        let Some(source) = window
            .source(owner.source_id())
            .filter(|source| source.source_token() == owner)
        else {
            return false;
        };
        let Some(settings) = &self.settings else {
            return false;
        };
        let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| valid_id(id))
        else {
            return false;
        };
        if self
            .records
            .iter()
            .any(|record| record.source == owner.source_id() && record.id == id)
        {
            return false;
        }
        let Some((kind, severity, title)) = classify(value) else {
            return false;
        };
        if !kind.enabled(settings) {
            return false;
        }
        let title = if source.kind() == SourceKind::Remote {
            let host = plain_text(host_label, MAX_HOST_CHARS);
            let host = if host.is_empty() {
                "Remote host"
            } else {
                &host
            };
            plain_text(&format!("{host}: {title}"), MAX_TITLE_CHARS)
        } else {
            title
        };
        let captured = value
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|id| valid_id(id))
            .and_then(|session| {
                let pane = source.pane_for(session)?;
                if !valid_id(pane) {
                    return None;
                }
                let screen = canonical_screen(source, pane, session)?;
                Some((
                    Captured {
                        pane: source.pane_key(pane),
                        session: session.into(),
                    },
                    Binding { owner, screen },
                ))
            });
        let read = captured
            .as_ref()
            .is_some_and(|(captured, _)| watched == Some(&captured.pane));
        let (captured, binding) = match captured {
            Some((captured, binding)) => (Some(captured), Some(binding)),
            None => (None, None),
        };
        if self.records.len() == MAX_RECORDS {
            self.records.pop_front();
        }
        // What happens in the watched pane is seen as it happens.
        if !read && self.fresh.len() < MAX_FRESH {
            self.fresh.push(FreshNotification {
                title: title.clone(),
                severity,
                pane: captured.as_ref().map(|captured| captured.pane.clone()),
                screen: binding.as_ref().map(|binding| binding.screen.clone()),
            });
        }
        self.records.push_back(Record {
            source: owner.source_id(),
            id: id.into(),
            kind,
            severity,
            title,
            captured,
            binding,
            read,
        });
        true
    }

    /// The facts admitted since the last call, oldest first.
    pub fn take_fresh(&mut self) -> Vec<FreshNotification> {
        std::mem::take(&mut self.fresh)
    }

    /// Whether new facts show a notice in the window (`notifications.toasts`).
    pub fn toasts(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|settings| settings.toasts)
    }

    /// Whether new facts reach the system while the window is in the
    /// background (`notifications.desktop`).
    pub fn desktop(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|settings| settings.desktop)
    }

    /// Read only currently bound records; stale presentation must reconcile first.
    pub fn mark_read(&mut self, pane: &PaneKey) -> bool {
        let mut changed = false;
        for record in &mut self.records {
            if !record.read
                && record.binding.is_some()
                && record
                    .captured
                    .as_ref()
                    .is_some_and(|captured| &captured.pane == pane)
            {
                record.read = true;
                changed = true;
            }
        }
        changed
    }
    pub fn unread_count(&self, screen: &ScreenKey) -> usize {
        self.unread_titles(screen).count()
    }
    pub fn unread_titles<'a>(&'a self, screen: &'a ScreenKey) -> impl Iterator<Item = &'a str> {
        self.records
            .iter()
            .rev()
            .filter(move |record| {
                !record.read
                    && record
                        .binding
                        .as_ref()
                        .is_some_and(|binding| &binding.screen == screen)
            })
            .map(|record| record.title.as_str())
    }
    pub fn severity(&self, pane: &PaneKey) -> Option<Severity> {
        self.records
            .iter()
            .filter(|record| {
                !record.read
                    && record.binding.is_some()
                    && record
                        .captured
                        .as_ref()
                        .is_some_and(|captured| &captured.pane == pane)
            })
            .map(|record| record.severity)
            .max()
    }
}

/// Snapshot proof is deliberately separate from attachment/send readiness.
fn canonical_screen(source: &SourceWorkspace, pane: &str, session: &str) -> Option<ScreenKey> {
    if source.session_for(pane) != Some(session) || source.pane_for(session) != Some(pane) {
        return None;
    }
    let screen = source.screen_for(pane)?;
    if !valid_id(screen) {
        return None;
    }
    let snapshot = source.screen(screen)?.snapshot()?;
    // IDs in canonical maps are typed; compare without allocating a lookup key.
    snapshot
        .panes
        .values()
        .any(|entry| {
            entry.id.as_str() == pane
                && entry
                    .session_id
                    .as_ref()
                    .is_some_and(|id| id.as_str() == session)
        })
        .then(|| source.screen_key(screen))
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_BYTES && !value.chars().any(hidden)
}
fn hidden(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}
fn plain_text(value: &str, limit: usize) -> String {
    value.chars().filter(|c| !hidden(*c)).take(limit).collect()
}
fn classify(value: &Value) -> Option<(Kind, Severity, String)> {
    let text = |field| {
        plain_text(
            value.get(field).and_then(Value::as_str).unwrap_or_default(),
            MAX_TITLE_CHARS,
        )
    };
    let (kind, severity, title) = match value.get("type")?.as_str()? {
        "process.finished" => (
            Kind::Finished,
            Severity::Info,
            format!("{} finished", text("process")),
        ),
        "process.idle" => {
            let process = text("process");
            let title = if process.is_empty() {
                "Command finished".into()
            } else {
                format!("{process} is waiting")
            };
            (Kind::Idle, Severity::Info, title)
        }
        "session.exited" => {
            let code = i32::try_from(value.get("exitCode")?.as_i64()?).ok()?;
            let process = text("process");
            let process = if process.is_empty() {
                "session"
            } else {
                &process
            };
            if code == 0 {
                (Kind::Exited, Severity::Info, format!("{process} exited"))
            } else {
                (Kind::Crashed, Severity::Error, format!("{process} failed"))
            }
        }
        "notification" => {
            let severity = match value.get("severity").and_then(Value::as_str) {
                Some("error") => Severity::Error,
                Some("warning") => Severity::Warning,
                Some("success") => Severity::Success,
                _ => Severity::Info,
            };
            (Kind::Generic, severity, text("title"))
        }
        _ => return None,
    };
    Some((kind, severity, plain_text(&title, MAX_TITLE_CHARS)))
}

#[cfg(test)]
mod tests;
