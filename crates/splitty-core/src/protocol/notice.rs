//! What a window shows for a notification event (`mux-protocol`,
//! Classified Notification Events): the server classifies each event once,
//! and windows keep only unread state, which pane they show, their own
//! settings and presentation.

use serde::{Deserialize, Serialize};

use crate::pty::status::{BlockKind, ProgramStatus, StatusState};

/// The longest title in characters; longer ones are cut.
pub const MAX_TITLE_CHARS: usize = 500;
/// The longest record text a program status title shows, in characters.
const STATUS_TEXT_CHARS: usize = 120;

/// A notification event's classification, as `Notice` in `docs/protocol.ts`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub kind: NoticeKind,
    pub severity: Severity,
    /// Plain text, at most [`MAX_TITLE_CHARS`].
    pub title: String,
    /// The `notifications.*` switch, besides `enabled`, that governs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<Setting>,
    /// Why no window notifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppressed: Option<Suppressed>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeKind {
    #[serde(rename = "process.finished")]
    ProcessFinished,
    #[serde(rename = "process.idle")]
    ProcessIdle,
    #[serde(rename = "session.exited")]
    SessionExited,
    #[serde(rename = "session.crashed")]
    SessionCrashed,
    #[serde(rename = "program-status")]
    ProgramStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

/// A field of the `notifications` settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Setting {
    ProcessFinished,
    ProcessExited,
    ProcessCrashed,
    ProgramStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Suppressed {
    /// The session ended because it was closed (`userInitiated`).
    UserInitiated,
    /// The session's program reports its status, which tells instead.
    ProgramStatus,
}

impl Notice {
    fn new(kind: NoticeKind, severity: Severity, title: String, setting: Option<Setting>) -> Self {
        Self {
            kind,
            severity,
            title: plain_text(&title, MAX_TITLE_CHARS),
            setting,
            suppressed: None,
        }
    }

    fn suppressed_by(mut self, reason: Suppressed, applies: bool) -> Self {
        if applies {
            self.suppressed = Some(reason);
        }
        self
    }

    /// `process` ended and the shell is in the foreground again.
    pub fn process_finished(process: &str) -> Self {
        Self::new(
            NoticeKind::ProcessFinished,
            Severity::Info,
            format!("{process} finished"),
            Some(Setting::ProcessFinished),
        )
    }

    /// The shell integration reported a finished command; `reports_status`
    /// when the session holds a root program status record.
    pub fn process_idle(process: &str, reports_status: bool) -> Self {
        let title = if process.is_empty() {
            "Command finished".to_owned()
        } else {
            format!("{process} is waiting")
        };
        Self::new(NoticeKind::ProcessIdle, Severity::Info, title, None)
            .suppressed_by(Suppressed::ProgramStatus, reports_status)
    }

    /// The session's program exited with `exit_code`.
    pub fn session_exited(exit_code: i32, process: Option<&str>, user_initiated: bool) -> Self {
        let process = process
            .filter(|process| !process.is_empty())
            .unwrap_or("session");
        let notice = if exit_code == 0 {
            Self::new(
                NoticeKind::SessionExited,
                Severity::Info,
                format!("{process} exited"),
                Some(Setting::ProcessExited),
            )
        } else {
            Self::new(
                NoticeKind::SessionCrashed,
                Severity::Error,
                format!("{process} failed"),
                Some(Setting::ProcessCrashed),
            )
        };
        notice.suppressed_by(Suppressed::UserInitiated, user_initiated)
    }

    /// A root record marked `notify` of a session on the screen `screen`
    /// (its label); none for a state that does not notify.
    pub fn program_status(screen: Option<&str>, record: &ProgramStatus) -> Option<Self> {
        let severity = match record.state {
            StatusState::Blocked => Severity::Warning,
            StatusState::Done => Severity::Success,
            StatusState::Error => Severity::Error,
            StatusState::Idle | StatusState::Working => return None,
        };
        let summary = status_summary(record);
        let title = match screen {
            Some(screen) => format!("{screen}: {summary}"),
            None => summary,
        };
        Some(Self::new(
            NoticeKind::ProgramStatus,
            severity,
            title,
            Some(Setting::ProgramStatus),
        ))
    }
}

/// A screen's label in a notice: its name, else "Unnamed Screen", never a
/// label by position that could repeat a real screen's name (`mux-protocol`,
/// Server-Chosen Screen Names).
pub fn screen_label(name: Option<&str>) -> String {
    name.map(|name| plain_text(name, MAX_TITLE_CHARS))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Unnamed Screen".to_owned())
}

/// The state in words, followed by the record's `msg`, else its `title`.
fn status_summary(record: &ProgramStatus) -> String {
    let label = match (record.state, record.kind) {
        (StatusState::Blocked, Some(BlockKind::Permission)) => "Waiting for permission",
        (StatusState::Blocked, Some(BlockKind::Question)) => "Waiting for an answer",
        (StatusState::Blocked, Some(BlockKind::Auth)) => "Waiting for sign-in",
        (StatusState::Blocked, None) => "Waiting",
        (StatusState::Done, _) => "Done",
        (StatusState::Error, _) => "Failed",
        (StatusState::Idle, _) => "Idle",
        (StatusState::Working, _) => "Working",
    };
    let text = record
        .msg
        .as_deref()
        .or(record.title.as_deref())
        .map(|text| plain_text(text, STATUS_TEXT_CHARS))
        .filter(|text| !text.is_empty());
    match text {
        Some(text) => format!("{label}: {text}"),
        None => label.to_owned(),
    }
}

/// `value` without control and bidirectional formatting characters, at most
/// `limit` characters.
fn plain_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| {
            !(c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        })
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(state: StatusState, kind: Option<BlockKind>, msg: Option<&str>) -> ProgramStatus {
        ProgramStatus {
            state,
            kind,
            progress: None,
            app: None,
            title: Some("title".into()),
            msg: msg.map(str::to_owned),
            updated_at: 0,
        }
    }

    #[test]
    fn process_events_are_classified_with_their_switch() {
        let finished = Notice::process_finished("cargo");
        assert_eq!(finished.kind, NoticeKind::ProcessFinished);
        assert_eq!(finished.severity, Severity::Info);
        assert_eq!(finished.title, "cargo finished");
        assert_eq!(finished.setting, Some(Setting::ProcessFinished));
        assert_eq!(finished.suppressed, None);

        assert_eq!(Notice::process_idle("", false).title, "Command finished");
        let idle = Notice::process_idle("claude", true);
        assert_eq!(idle.title, "claude is waiting");
        assert_eq!(idle.setting, None);
        assert_eq!(idle.suppressed, Some(Suppressed::ProgramStatus));
    }

    #[test]
    fn exits_are_split_by_their_code_and_closing_suppresses_them() {
        let exited = Notice::session_exited(0, Some("vim"), false);
        assert_eq!(
            (exited.kind, exited.severity, exited.title.as_str()),
            (NoticeKind::SessionExited, Severity::Info, "vim exited")
        );
        assert_eq!(exited.setting, Some(Setting::ProcessExited));
        let failed = Notice::session_exited(1, None, false);
        assert_eq!(
            (failed.kind, failed.severity, failed.title.as_str()),
            (
                NoticeKind::SessionCrashed,
                Severity::Error,
                "session failed"
            )
        );
        assert_eq!(failed.setting, Some(Setting::ProcessCrashed));
        assert_eq!(failed.suppressed, None);
        assert_eq!(
            Notice::session_exited(129, Some("zsh"), true).suppressed,
            Some(Suppressed::UserInitiated)
        );
    }

    #[test]
    fn program_status_names_the_screen_and_the_message() {
        let blocked = record(
            StatusState::Blocked,
            Some(BlockKind::Permission),
            Some("Run tests?"),
        );
        let notice = Notice::program_status(Some("build"), &blocked).expect("notice");
        assert_eq!(notice.kind, NoticeKind::ProgramStatus);
        assert_eq!(notice.severity, Severity::Warning);
        assert_eq!(notice.title, "build: Waiting for permission: Run tests?");
        assert_eq!(notice.setting, Some(Setting::ProgramStatus));

        let done =
            Notice::program_status(None, &record(StatusState::Done, None, None)).expect("notice");
        assert_eq!(
            (done.severity, done.title.as_str()),
            (Severity::Success, "Done: title")
        );
        let failed = Notice::program_status(None, &record(StatusState::Error, None, Some("x")))
            .expect("notice");
        assert_eq!(failed.severity, Severity::Error);
        assert!(Notice::program_status(None, &record(StatusState::Working, None, None)).is_none());
        assert!(Notice::program_status(None, &record(StatusState::Idle, None, None)).is_none());
    }

    #[test]
    fn titles_are_plain_and_bounded() {
        let long = "x".repeat(2 * MAX_TITLE_CHARS);
        assert_eq!(
            Notice::process_finished(&long).title.chars().count(),
            MAX_TITLE_CHARS
        );
        assert_eq!(
            Notice::process_finished("a\u{1b}[31m\u{202e}b").title,
            "a[31mb finished"
        );
        assert_eq!(screen_label(None), "Unnamed Screen");
        assert_eq!(screen_label(Some("")), "Unnamed Screen");
        assert_eq!(screen_label(Some("api")), "api");
    }

    #[test]
    fn the_wire_shape_matches_the_protocol() {
        let notice = Notice::session_exited(1, Some("node"), true);
        assert_eq!(
            serde_json::to_value(&notice).expect("JSON"),
            serde_json::json!({
                "kind": "session.crashed",
                "severity": "error",
                "title": "node failed",
                "setting": "processCrashed",
                "suppressed": "user-initiated",
            })
        );
        let idle = serde_json::to_value(Notice::process_idle("", false)).expect("JSON");
        assert_eq!(
            idle,
            serde_json::json!({"kind": "process.idle", "severity": "info", "title": "Command finished"})
        );
    }
}
