//! Explicit mapping between the browser API and native UI requests.
//!
//! The assigned role is trusted adapter state, never a request field. Arguments
//! carry operation data, not URLs, HTTP methods, headers or socket addresses.
//! Domain validation is performed by the shared operation after admission.

pub mod directory;
pub mod remembered;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io::{self, Write};
use std::time::Duration;

pub const MAX_API_BODY: usize = 1024 * 1024;
pub const MAX_JOB_BODY: usize = 128 * 1024;
pub const MAX_SESSION_FRAME: usize = 16 * 1024 * 1024;

/// Announced in `mux.welcome` on local main connections by a local service
/// whose hub writes remembered remote Screens per host
/// (`remoteScreens.updateHost`) and takes directory observations without
/// pane counts.
pub const HOST_STATE_CAPABILITY: &str = "hub-host-state-v1";

/// Assigned by the service connector/broker, not deserialized from the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Main,
    Remote,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    /// The service to which the connection was assigned.
    Backend,
    /// A backend operation that the owner control socket serves as well, from
    /// this catalog. Job operations reach that socket through the job router
    /// it shares with the agent grant sockets instead.
    Control,
    /// Explicit local host-management authority, unavailable to remote windows.
    LocalHub,
}

/// How an operation answers a request it cannot decode, and the shape of its
/// other errors. Only the dispatcher's own decoding uses it; the shapes are
/// those the browser API has always answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorStyle {
    /// `{"error"}`; an argument or body that does not decode is 422.
    Plain,
    /// Managed-job errors `{"error","code"}`; one that does not decode is 400
    /// with code `invalid_request`.
    Job,
    /// Viewer errors `{"code","error"}`; one that does not decode is 400 with
    /// code `invalid`.
    Viewer,
}

/// What an argument carries. Every non-body argument is a string on the wire,
/// as path and query arguments of the browser API are.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgumentType {
    Text,
    /// A non-negative integer, written as a decimal string; the operation
    /// receives it as a number.
    Count,
    /// The operation's JSON body.
    Body,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Argument {
    pub name: &'static str,
    pub optional: bool,
    pub kind: ArgumentType,
}

impl Argument {
    /// `spec` is the name with a `?` suffix when optional. `body` is the body;
    /// any other argument is text unless `kind` says otherwise.
    const fn declare(spec: &'static str, kind: &[ArgumentType]) -> Self {
        let (name, optional) = match spec.as_bytes() {
            [.., b'?'] => (spec.split_at(spec.len() - 1).0, true),
            _ => (spec, false),
        };
        let kind = match kind {
            [kind] => *kind,
            _ if same(name, "body") => ArgumentType::Body,
            _ => ArgumentType::Text,
        };
        Self {
            name,
            optional,
            kind,
        }
    }
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut index = 0;
    while index < a.len() {
        if a[index] != b[index] {
            return false;
        }
        index += 1;
    }
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cancellation {
    Read,
    /// Cancels this caller's wait. An already admitted effect may have happened.
    Mutation,
    /// Cancels a file/editor registration as well as its response wait.
    Registration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Deadline {
    Ordinary,
    JobLaunch,
    JobLogs,
    Editor,
    Forward,
    /// The file service enforces acceptance and read-idle deadlines; a large
    /// download may legitimately outlive a fixed total request timeout.
    FileTransfer,
    /// A wait for the user's review lasts until the user answers, the viewer
    /// pane closes or the caller drops the request.
    Review,
}

impl Deadline {
    pub fn duration(self) -> Option<Duration> {
        match self {
            Self::Ordinary => Some(Duration::from_secs(30)),
            Self::JobLaunch => Some(Duration::from_secs(15)),
            Self::JobLogs | Self::Editor => Some(Duration::from_secs(35)),
            Self::Forward => Some(Duration::from_secs(180)),
            Self::FileTransfer | Self::Review => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Description {
    pub operation: Operation,
    pub name: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub scope: Scope,
    /// Fixed path/query/body arguments.
    pub arguments: &'static [Argument],
    pub body_limit: usize,
    pub cancellation: Cancellation,
    pub deadline: Deadline,
    pub errors: ErrorStyle,
}

impl Description {
    /// Whether the operation takes a JSON body.
    pub fn takes_body(&self) -> bool {
        self.arguments
            .iter()
            .any(|argument| argument.kind == ArgumentType::Body)
    }

    pub fn argument(&self, name: &str) -> Option<&'static Argument> {
        self.arguments.iter().find(|argument| argument.name == name)
    }
}

macro_rules! operations {
    ($( $variant:ident, $name:literal, $method:literal, $path:literal,
        $scope:ident, [$($arg:literal $(as $kind:ident)?),*], $limit:ident, $cancel:ident,
        $deadline:ident, $errors:ident; )*) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
        pub enum Operation {
            $(#[serde(rename = $name)] $variant,)*
        }
        pub const OPERATIONS: &[Description] = &[
            $(Description {
                operation: Operation::$variant, name: $name, method: $method,
                path: $path, scope: Scope::$scope,
                arguments: &[$(Argument::declare($arg, &[$(ArgumentType::$kind)?])),*],
                body_limit: $limit, cancellation: Cancellation::$cancel,
                deadline: Deadline::$deadline, errors: ErrorStyle::$errors,
            },)*
        ];
        impl Operation {
            pub fn description(self) -> &'static Description {
                // The macro constructs both sides of this exhaustive mapping.
                match self {
                    $(Self::$variant => &OPERATIONS[Self::$variant as usize],)*
                }
            }
        }
    };
}

operations! {
    HomeGet, "home.get", "GET", "/api/home/settings", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    HomeMetadata, "home.metadata", "GET", "/api/home/settings/metadata", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    HomeUpdate, "home.update", "PATCH", "/api/home/settings", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    ThemeGet, "theme.get", "GET", "/api/home/theme", Backend, ["name?"], MAX_API_BODY, Read, Ordinary, Plain;
    ThemeList, "theme.list", "GET", "/api/home/themes", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    ShellProfilesGet, "shellProfiles.get", "GET", "/api/shell-profiles", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    ShellProfilesUpdate, "shellProfiles.update", "PUT", "/api/shell-profiles", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    ServerSettingsGet, "serverSettings.get", "GET", "/api/server/settings", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    ServerSettingsUpdate, "serverSettings.update", "PATCH", "/api/server/settings", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    TemplateExtract, "template.extract", "POST", "/api/templates/extract/screen", Backend, ["body"], MAX_API_BODY, Read, Ordinary, Plain;
    TemplateList, "template.list", "GET", "/api/templates", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    TemplateGet, "template.get", "GET", "/api/templates/{slug}", Backend, ["slug"], MAX_API_BODY, Read, Ordinary, Plain;
    TemplateSave, "template.save", "POST", "/api/templates/{slug}", Backend, ["slug", "body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    TemplateCreate, "template.create", "POST", "/api/templates", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    TemplateDelete, "template.delete", "DELETE", "/api/templates/{slug}", Backend, ["slug"], MAX_API_BODY, Mutation, Ordinary, Plain;
    BuildGet, "build.get", "GET", "/api/build", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    StatsGet, "stats.get", "GET", "/api/stats", Backend, [], MAX_API_BODY, Read, Ordinary, Plain;
    CommandSend, "command.send", "POST", "/api/command", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    MuxState, "mux.state", "GET", "/api/mux/state", Control, [], MAX_API_BODY, Read, Ordinary, Plain;
    MuxScreens, "mux.screens", "GET", "/api/mux/screens", Control, [], MAX_API_BODY, Read, Ordinary, Plain;
    MuxScreen, "mux.screen", "GET", "/api/mux/screen/{id}", Control, ["id"], MAX_API_BODY, Read, Ordinary, Plain;
    MuxOp, "mux.op", "POST", "/api/mux/op", Control, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    MuxNeighbor, "mux.neighbor", "GET", "/api/mux/neighbor", Control, ["screen?", "pane?", "dir?"], MAX_API_BODY, Read, Ordinary, Plain;
    MuxStatus, "mux.status", "GET", "/api/mux/status", Control, [], MAX_API_BODY, Read, Ordinary, Plain;
    SessionSend, "session.send", "POST", "/api/session/{id}/send", Control, ["id", "body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    EditorOpen, "editor.open", "POST", "/api/editor/open", Control, ["body"], MAX_API_BODY, Registration, Editor, Plain;
    FileOffer, "file.offer", "POST", "/api/file/offer", Control, ["body"], MAX_API_BODY, Registration, FileTransfer, Plain;
    ViewerComments, "viewer.comments", "GET", "/api/viewer/comments", Control, ["screenId?", "paneId?", "document?", "all?"], MAX_API_BODY, Read, Ordinary, Viewer;
    ViewerCommentReply, "viewer.commentReply", "POST", "/api/viewer/comments/reply", Control, ["body"], MAX_API_BODY, Mutation, Ordinary, Viewer;
    ViewerReviewWait, "viewer.reviewWait", "POST", "/api/viewer/review/wait", Control, ["body"], MAX_API_BODY, Registration, Review, Viewer;
    JobList, "job.list", "GET", "/api/jobs", Backend, [], MAX_JOB_BODY, Read, Ordinary, Job;
    JobLaunch, "job.launch", "POST", "/api/jobs", Backend, ["body"], MAX_JOB_BODY, Mutation, JobLaunch, Job;
    JobInspect, "job.inspect", "GET", "/api/jobs/{id}", Backend, ["id"], MAX_JOB_BODY, Read, Ordinary, Job;
    JobLogs, "job.logs", "GET", "/api/jobs/{id}/logs", Backend, ["id", "cursor?", "tail?" as Count, "limit?" as Count, "waitMs?" as Count, "format?"], MAX_JOB_BODY, Read, JobLogs, Job;
    JobInput, "job.input", "POST", "/api/jobs/{id}/input", Backend, ["id", "body"], MAX_JOB_BODY, Mutation, Ordinary, Job;
    JobStop, "job.stop", "POST", "/api/jobs/{id}/stop", Backend, ["id"], MAX_JOB_BODY, Mutation, Ordinary, Job;
    JobForget, "job.forget", "POST", "/api/jobs/{id}/forget", Backend, ["id"], MAX_JOB_BODY, Mutation, Ordinary, Job;
    HostDirectory, "remote.directory.get", "GET", "/api/remote/directory", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    SshPrompts, "ssh.prompts.get", "GET", "/api/remote/ssh-prompts", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    SshAnswer, "ssh.prompts.answer", "POST", "/api/remote/ssh-prompts", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    HostList, "host.list", "GET", "/api/remote/hosts", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    HostForget, "host.forget", "DELETE", "/api/remote/hosts", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    HostPreferencesGet, "hostPreferences.get", "GET", "/api/remote/host-preferences", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    HostPreferencesUpdate, "hostPreferences.update", "PUT", "/api/remote/host-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    HostOrderGet, "hostOrder.get", "GET", "/api/remote/host-order", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    HostOrderUpdate, "hostOrder.update", "PUT", "/api/remote/host-order", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    RemoteScreensGet, "remoteScreens.get", "GET", "/api/remote/screens", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    RemoteScreensUpdate, "remoteScreens.update", "PUT", "/api/remote/screens", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    RemoteScreensUpdateHost, "remoteScreens.updateHost", "PUT", "/api/remote/screens/host", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    CatalogGet, "catalog.get", "GET", "/api/remote/catalog", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    CatalogUpdate, "catalog.update", "PUT", "/api/remote/catalog", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    ForwardList, "forward.list", "GET", "/api/remote/forwards", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    ForwardOpen, "forward.open", "POST", "/api/remote/forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward, Plain;
    ForwardStop, "forward.stop", "DELETE", "/api/remote/forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward, Plain;
    ForwardPreferencesGet, "forwardPreferences.get", "GET", "/api/remote/forward-preferences", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    ForwardPreferencesSave, "forwardPreferences.save", "PUT", "/api/remote/forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    ForwardPreferencesForget, "forwardPreferences.forget", "DELETE", "/api/remote/forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    ReverseList, "reverse.list", "GET", "/api/remote/reverse-forwards", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    ReverseOpen, "reverse.open", "POST", "/api/remote/reverse-forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward, Plain;
    ReverseStop, "reverse.stop", "DELETE", "/api/remote/reverse-forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward, Plain;
    ReversePreferencesGet, "reversePreferences.get", "GET", "/api/remote/reverse-forward-preferences", LocalHub, [], MAX_API_BODY, Read, Ordinary, Plain;
    ReversePreferencesSave, "reversePreferences.save", "PUT", "/api/remote/reverse-forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
    ReversePreferencesForget, "reversePreferences.forget", "DELETE", "/api/remote/reverse-forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary, Plain;
}

/// These are classified explicitly and can never become native UI operations.
pub const ADAPTER_ROUTES: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("POST", "/api/session"),
    ("POST", "/api/ws-ticket"),
    ("POST", "/api/bootstrap"),
    ("GET", "/api/client-assignment"),
    ("GET", "/ws"),
    ("GET", "/ws/remote"),
    ("GET", "/api/agents"),
    ("POST", "/api/agents"),
    ("GET", "/api/agents/{id}"),
    ("POST", "/api/agents/{id}/revoke"),
    ("POST", "/api/jobs/{id}/owner"),
    ("POST", "/api/daemon/stop"),
];

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub operation: Operation,
    #[serde(default)]
    pub args: Map<String, Value>,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum Rejection {
    #[error("unsupported or malformed native request: {0}")]
    Invalid(String),
    #[error("operation is not available to this connection")]
    Forbidden,
    #[error("request exceeds the operation's body limit")]
    TooLarge,
}

impl Request {
    pub fn validate(&self, role: Role) -> Result<(), Rejection> {
        let description = self.operation.description();
        if role == Role::Remote && description.scope == Scope::LocalHub {
            return Err(Rejection::Forbidden);
        }
        // Typed internal callers pass through the same size check as decoded
        // requests. Count serialization without allocating another body copy.
        struct Limit(usize);
        impl Write for Limit {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0 = self
                    .0
                    .checked_sub(bytes.len())
                    .ok_or_else(|| io::Error::other("request body limit"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(Limit(description.body_limit), self)
            .map_err(|_| Rejection::TooLarge)?;
        for key in self.args.keys() {
            if description.argument(key).is_none() {
                return Err(Rejection::Invalid(format!("unexpected argument: {key}")));
            }
        }
        for argument in description.arguments {
            let name = argument.name;
            match self.args.get(name) {
                None if !argument.optional => {
                    return Err(Rejection::Invalid(format!("missing argument: {name}")))
                }
                // Count arguments are decimal strings too; the operation's own
                // decoding reports one that is no number in its error style.
                Some(value) if argument.kind != ArgumentType::Body && !value.is_string() => {
                    return Err(Rejection::Invalid(format!("{name} must be a string")));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Existing session JSON types. They retain their dispatcher/domain validation
/// and ordering; they cannot mint credentials or perform host administration.
pub const SESSION_OPERATIONS: &[&str] = &[
    "create",
    "attach",
    "detach",
    "input",
    "input_bytes",
    "resize",
    "close",
    "save_image",
    "upload",
    "upload_cancel",
    "activity",
    "idle",
    "hide",
    "mux.op",
    "mux.resync",
    "mux.workspace_resync",
    "mux.extract_template",
    "mux.assess_close",
    "editor.register",
    "editor.result",
    "file.register",
    "file.accept",
    "file.decline",
    "file.read",
    "viewer.read",
    "viewer.anchor",
    "viewer.review",
    "viewer.review_answer",
    "hub.catalog.observe",
];

/// All session operations act on the assigned backend. Disconnect cancels its
/// attachments, uploads and registrations; input and mutations are never replayed.
pub fn validate_session(
    bytes: &[u8],
    budget: &splitty_transport::budget::ConnectionBudget,
) -> Result<splitty_transport::budget::Charged<Value>, Rejection> {
    if bytes.len() > MAX_SESSION_FRAME {
        return Err(Rejection::TooLarge);
    }
    let lease = splitty_transport::codec::reserve_json(bytes, budget, false)
        .map_err(|_| Rejection::TooLarge)?;
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| Rejection::Invalid(error.to_string()))?;
    if !value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| SESSION_OPERATIONS.contains(&kind))
    {
        return Err(Rejection::Invalid("unknown session operation".into()));
    }
    for field in [
        "url",
        "socket",
        "headers",
        "authorization",
        "role",
        "backend",
        "connection",
        "window",
        "target",
        "method",
        "token",
        "origin",
    ] {
        // This one local-hub message names cached metadata, never a backend
        // assignment. The carrier separately requires the fixed Main role.
        if field == "target" && value["type"] == "hub.catalog.observe" {
            continue;
        }
        if value.get(field).is_some() {
            return Err(Rejection::Invalid(format!(
                "unexpected authority field: {field}"
            )));
        }
    }
    Ok(lease.charge(value))
}
