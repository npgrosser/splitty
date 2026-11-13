//! Requests of a pane's process to open a file or a folder in the user's
//! editor (`control-api`, Open In Client Editor). A request is untrusted
//! data: the window shows it, and only the user's own confirmation launches
//! anything. Preferences choose an editor; they never allow a launch.
use super::*;
use crate::Client;
use serde_json::{json, Value};
pub use splitty_core::config::EditorSettings;
use std::{
    io,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// A request is shown for at most this long, whatever its service says.
const MAX_SHOWN: Duration = Duration::from_secs(25);
/// Between two requests of one connection that are shown.
const BETWEEN: Duration = Duration::from_secs(2);
/// How long a request's ID is remembered, and how many are.
const REMEMBERED: Duration = Duration::from_secs(300);
const MAX_REMEMBERED: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppEditor {
    VsCode,
    Zed,
}
impl AppEditor {
    fn named(name: &str) -> Option<Self> {
        match name {
            "vscode" => Some(Self::VsCode),
            "zed" => Some(Self::Zed),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    File,
    Folder,
}

/// What the window tells the service about a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorOutcome {
    Dispatched,
    Denied,
    Failed,
    Expired,
    Disconnected,
}
impl EditorOutcome {
    fn wire(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::Denied => "denied",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Disconnected => "disconnected",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorRequest {
    pub source: SourceToken,
    pub request_id: String,
    pub path: String,
    pub kind: PathKind,
    /// The editor the process named; the preferences choose otherwise.
    pub editor: Option<AppEditor>,
    /// How long the request may be shown.
    pub expires_in: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorUpdate {
    Requested(EditorRequest),
    /// The service withdrew a request, or its terminal ended.
    Cancelled {
        source: SourceToken,
        request_id: String,
    },
}

/// The editor for a path: the one the request names, else the rule of the
/// longest matching project folder of this connection (`local` or
/// `host:` and the SSH target), else the rule of a file's extension, else
/// the default of its kind.
pub fn preferred_editor(
    settings: &EditorSettings,
    connection: &str,
    path: &str,
    kind: PathKind,
    explicit: Option<AppEditor>,
) -> AppEditor {
    if let Some(editor) = explicit {
        return editor;
    }
    let project = settings
        .projects
        .iter()
        .filter_map(|rule| {
            let prefix = rule.path.trim_end_matches('/');
            let inside = rule.connection == connection
                && (prefix.is_empty()
                    || path == prefix
                    || path
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('/')));
            (inside).then(|| Some((prefix.len(), AppEditor::named(&rule.editor)?)))?
        })
        // The first of the longest ones, as the rules are listed.
        .fold(None, |best: Option<(usize, AppEditor)>, rule| match best {
            Some(held) if held.0 >= rule.0 => Some(held),
            _ => Some(rule),
        });
    if let Some((_, editor)) = project {
        return editor;
    }
    if kind == PathKind::File {
        let name = path.rsplit('/').next().unwrap_or(path);
        if let Some((_, suffix)) = name.rsplit_once('.') {
            let rule = settings.extensions.iter().find(|rule| {
                rule.extension
                    .trim_start_matches('.')
                    .eq_ignore_ascii_case(suffix)
            });
            if let Some(editor) = rule.and_then(|rule| AppEditor::named(&rule.editor)) {
                return editor;
            }
        }
    }
    AppEditor::named(match kind {
        PathKind::File => &settings.file,
        PathKind::Folder => &settings.folder,
    })
    .unwrap_or(AppEditor::VsCode)
}

fn send(client: &Client, message: &Value) -> io::Result<()> {
    client
        .sender()
        .json(splitty_transport::wire::Kind::Text, message)
}

/// The editor requests of one connection: at most one is shown at a time.
#[derive(Default)]
pub(super) struct EditorRequests {
    registered: bool,
    seen: HashMap<String, Instant>,
    last_shown: Option<Instant>,
    pending: Option<(String, String)>,
}
impl EditorRequests {
    /// Ask a service that knows editor requests to send them here.
    pub(super) fn welcome(&mut self, client: &Client, json: &Value) {
        // A request of the previous connection cannot be answered any more.
        self.pending = None;
        self.registered = capability::offered(json, capability::EDITOR_OPEN)
            && send(client, &json!({"type":"editor.register"})).is_ok();
    }

    fn result(&self, client: &Client, request_id: &str, outcome: EditorOutcome) {
        let _ = send(
            client,
            &json!({"type":"editor.result","requestId":request_id,"outcome":outcome.wire()}),
        );
    }

    /// The user's answer, or the window's, to the request shown.
    pub(super) fn answer(&mut self, client: &Client, request_id: &str, outcome: EditorOutcome) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(pending, _)| pending == request_id)
        {
            self.pending = None;
            self.result(client, request_id, outcome);
        }
    }

    /// A terminal ended: a request made from it has nobody to answer to.
    pub(super) fn session_closed(
        &mut self,
        client: &Client,
        source: SourceToken,
        session: &str,
        out: &mut Vec<Update>,
    ) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, pending)| pending == session)
        {
            let (request_id, _) = self.pending.take().expect("pending request");
            self.result(client, &request_id, EditorOutcome::Disconnected);
            out.push(Update::Editor(EditorUpdate::Cancelled {
                source,
                request_id,
            }));
        }
    }

    pub(super) fn event(
        &mut self,
        client: &Client,
        source: SourceToken,
        attached: &HashSet<String>,
        json: &Value,
        out: &mut Vec<Update>,
    ) {
        match MessageType::of(json) {
            MessageType::EditorCancelled => {
                if let Some(id) = json["requestId"].as_str() {
                    if self
                        .pending
                        .as_ref()
                        .is_some_and(|(pending, _)| pending == id)
                    {
                        self.pending = None;
                        out.push(Update::Editor(EditorUpdate::Cancelled {
                            source,
                            request_id: id.to_owned(),
                        }));
                    }
                }
            }
            MessageType::EditorRequest if self.registered => {
                self.requested(client, source, attached, json, out)
            }
            _ => {}
        }
    }

    fn requested(
        &mut self,
        client: &Client,
        source: SourceToken,
        attached: &HashSet<String>,
        json: &Value,
        out: &mut Vec<Update>,
    ) {
        let now = Instant::now();
        let Some(id) = json["requestId"].as_str().filter(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) else {
            return;
        };
        self.seen.retain(|_, until| now < *until);
        if self.seen.contains_key(id) {
            return;
        }
        if self.seen.len() >= MAX_REMEMBERED {
            return self.result(client, id, EditorOutcome::Denied);
        }
        self.seen.insert(id.to_owned(), now + REMEMBERED);
        if self.pending.is_some()
            || self
                .last_shown
                .is_some_and(|shown| now.duration_since(shown) < BETWEEN)
        {
            return self.result(client, id, EditorOutcome::Denied);
        }
        let kind = match json["kind"].as_str() {
            Some("file") => Some(PathKind::File),
            Some("folder") => Some(PathKind::Folder),
            _ => None,
        };
        let editor = match &json["editor"] {
            Value::Null => Some(None),
            Value::String(name) => AppEditor::named(name).map(Some),
            _ => None,
        };
        let (Some(path), Some(session), Some(kind), Some(editor), Some(expires_at)) = (
            json["path"].as_str().filter(|path| path.len() <= 4096),
            json["sessionId"].as_str().filter(|id| id.len() <= 256),
            kind,
            editor,
            json["expiresAt"].as_f64().filter(|at| at.is_finite()),
        ) else {
            return self.result(client, id, EditorOutcome::Failed);
        };
        if !attached.contains(session) {
            return self.result(client, id, EditorOutcome::Disconnected);
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64;
        let left = expires_at - now_ms;
        if left <= 0. {
            return self.result(client, id, EditorOutcome::Expired);
        }
        self.last_shown = Some(now);
        self.pending = Some((id.to_owned(), session.to_owned()));
        out.push(Update::Editor(EditorUpdate::Requested(EditorRequest {
            source,
            request_id: id.to_owned(),
            path: path.to_owned(),
            kind,
            editor,
            expires_in: Duration::from_millis(left as u64).min(MAX_SHOWN),
        })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_core::config::{EditorExtension, EditorProject};

    #[test]
    fn the_editor_is_the_named_one_then_a_project_an_extension_or_the_default() {
        let settings = EditorSettings {
            file: "vscode".into(),
            folder: "zed".into(),
            extensions: vec![EditorExtension {
                extension: ".RS".into(),
                editor: "zed".into(),
            }],
            projects: vec![
                EditorProject {
                    connection: "local".into(),
                    path: "/work".into(),
                    editor: "zed".into(),
                },
                EditorProject {
                    connection: "local".into(),
                    path: "/work/site/".into(),
                    editor: "vscode".into(),
                },
                EditorProject {
                    connection: "host:dev".into(),
                    path: "/".into(),
                    editor: "zed".into(),
                },
            ],
        };
        let choose =
            |connection, path, kind| preferred_editor(&settings, connection, path, kind, None);
        assert_eq!(
            choose("local", "/tmp/a.txt", PathKind::File),
            AppEditor::VsCode
        );
        assert_eq!(choose("local", "/tmp", PathKind::Folder), AppEditor::Zed);
        assert_eq!(
            choose("local", "/tmp/main.rs", PathKind::File),
            AppEditor::Zed
        );
        // The longest project folder wins, and only whole path parts match.
        assert_eq!(
            choose("local", "/work/a.txt", PathKind::File),
            AppEditor::Zed
        );
        assert_eq!(
            choose("local", "/work/site/a.rs", PathKind::File),
            AppEditor::VsCode
        );
        assert_eq!(
            choose("local", "/workshop/a.txt", PathKind::File),
            AppEditor::VsCode
        );
        // A rule belongs to its connection.
        assert_eq!(
            choose("host:dev", "/srv/a.txt", PathKind::File),
            AppEditor::Zed
        );
        assert_eq!(
            choose("host:other", "/work/a.txt", PathKind::File),
            AppEditor::VsCode
        );
        assert_eq!(
            preferred_editor(
                &settings,
                "local",
                "/work/a.txt",
                PathKind::File,
                Some(AppEditor::VsCode)
            ),
            AppEditor::VsCode
        );
    }

    /// A request with a field this build does not know is still one; a
    /// message of a type it does not know is no request.
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn requests_are_read_past_fields_and_types_this_build_does_not_know() {
        use splitty_core::protocol::session::{self, SessionMessage};
        let (runtime, path) = crate::appearance::local::tests::fixture();
        let service = crate::EmbeddedService::from_runtime(runtime);
        let client = service.connect().await.unwrap();
        let source = crate::workspace::SourceWorkspace::new(uuid::Uuid::new_v4()).source_token();
        let attached = HashSet::from(["s1".to_owned()]);
        let mut editors = EditorRequests {
            registered: true,
            ..Default::default()
        };
        let mut out = Vec::new();
        let id = "0123456789abcdef0123456789abcdef";
        editors.event(
            &client,
            source,
            &attached,
            &json!({"type":"editor.prompt","requestId":id,"sessionId":"s1","path":"/a"}),
            &mut out,
        );
        assert!(out.is_empty() && editors.pending.is_none());
        let expires_at = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            + 60_000;
        let mut request = serde_json::to_value(SessionMessage::EditorRequest {
            request_id: id.into(),
            session_id: "s1".into(),
            path: "/work/a.rs".into(),
            kind: session::PathKind::File,
            editor: Some(session::Editor::Zed),
            expires_at,
        })
        .unwrap();
        request["futureField"] = json!({"x": 1});
        editors.event(&client, source, &attached, &request, &mut out);
        let [Update::Editor(EditorUpdate::Requested(asked))] = out.as_slice() else {
            panic!("expected a request");
        };
        assert_eq!(asked.request_id, id);
        assert_eq!(asked.kind, PathKind::File);
        assert_eq!(asked.editor, Some(AppEditor::Zed));
        out.clear();
        let mut cancelled = serde_json::to_value(SessionMessage::EditorCancelled {
            request_id: id.into(),
        })
        .unwrap();
        cancelled["futureField"] = json!(true);
        editors.event(&client, source, &attached, &cancelled, &mut out);
        assert!(matches!(
            out.as_slice(),
            [Update::Editor(EditorUpdate::Cancelled { request_id, .. })] if request_id == id
        ));
        client.finish().await.unwrap();
        service.close().await.unwrap();
        let _ = std::fs::remove_dir_all(path);
    }
}
