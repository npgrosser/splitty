//! Explicit mapping between the browser API and native UI requests.
//!
//! The assigned role is trusted adapter state, never a request field. Arguments
//! carry operation data, not URLs, HTTP methods, headers or socket addresses.
//! Domain validation is performed by the shared operation after admission.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io::{self, Write};
use std::time::Duration;

pub const MAX_API_BODY: usize = 1024 * 1024;
pub const MAX_JOB_BODY: usize = 128 * 1024;
pub const MAX_SESSION_FRAME: usize = 16 * 1024 * 1024;

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
    /// Explicit local host-management authority, unavailable to remote windows.
    LocalHub,
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
    /// Fixed path/query/body argument names. A '?' suffix means optional.
    pub arguments: &'static [&'static str],
    pub body_limit: usize,
    pub cancellation: Cancellation,
    pub deadline: Deadline,
}

macro_rules! operations {
    ($( $variant:ident, $name:literal, $method:literal, $path:literal,
        $scope:ident, [$($arg:literal),*], $limit:ident, $cancel:ident, $deadline:ident; )*) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
        pub enum Operation {
            $(#[serde(rename = $name)] $variant,)*
        }
        pub const OPERATIONS: &[Description] = &[
            $(Description {
                operation: Operation::$variant, name: $name, method: $method,
                path: $path, scope: Scope::$scope, arguments: &[$($arg),*],
                body_limit: $limit, cancellation: Cancellation::$cancel,
                deadline: Deadline::$deadline,
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
    HomeGet, "home.get", "GET", "/api/home/settings", Backend, [], MAX_API_BODY, Read, Ordinary;
    HomeMetadata, "home.metadata", "GET", "/api/home/settings/metadata", Backend, [], MAX_API_BODY, Read, Ordinary;
    HomeUpdate, "home.update", "PATCH", "/api/home/settings", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary;
    ThemeGet, "theme.get", "GET", "/api/home/theme", Backend, ["name?"], MAX_API_BODY, Read, Ordinary;
    ThemeList, "theme.list", "GET", "/api/home/themes", Backend, [], MAX_API_BODY, Read, Ordinary;
    ShellProfilesGet, "shellProfiles.get", "GET", "/api/shell-profiles", Backend, [], MAX_API_BODY, Read, Ordinary;
    ShellProfilesUpdate, "shellProfiles.update", "PUT", "/api/shell-profiles", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary;
    ServerSettingsGet, "serverSettings.get", "GET", "/api/server/settings", Backend, [], MAX_API_BODY, Read, Ordinary;
    ServerSettingsUpdate, "serverSettings.update", "PATCH", "/api/server/settings", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary;
    TemplateExtract, "template.extract", "POST", "/api/templates/extract/screen", Backend, ["body"], MAX_API_BODY, Read, Ordinary;
    TemplateList, "template.list", "GET", "/api/templates", Backend, [], MAX_API_BODY, Read, Ordinary;
    TemplateGet, "template.get", "GET", "/api/templates/{slug}", Backend, ["slug"], MAX_API_BODY, Read, Ordinary;
    TemplateSave, "template.save", "POST", "/api/templates/{slug}", Backend, ["slug", "body"], MAX_API_BODY, Mutation, Ordinary;
    TemplateDelete, "template.delete", "DELETE", "/api/templates/{slug}", Backend, ["slug"], MAX_API_BODY, Mutation, Ordinary;
    BuildGet, "build.get", "GET", "/api/build", Backend, [], MAX_API_BODY, Read, Ordinary;
    StatsGet, "stats.get", "GET", "/api/stats", Backend, [], MAX_API_BODY, Read, Ordinary;
    CommandSend, "command.send", "POST", "/api/command", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary;
    MuxState, "mux.state", "GET", "/api/mux/state", Backend, [], MAX_API_BODY, Read, Ordinary;
    MuxScreens, "mux.screens", "GET", "/api/mux/screens", Backend, [], MAX_API_BODY, Read, Ordinary;
    MuxScreen, "mux.screen", "GET", "/api/mux/screen/{id}", Backend, ["id"], MAX_API_BODY, Read, Ordinary;
    MuxOp, "mux.op", "POST", "/api/mux/op", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary;
    MuxNeighbor, "mux.neighbor", "GET", "/api/mux/neighbor", Backend, ["screen?", "pane?", "dir?"], MAX_API_BODY, Read, Ordinary;
    MuxStatus, "mux.status", "GET", "/api/mux/status", Backend, [], MAX_API_BODY, Read, Ordinary;
    SessionSend, "session.send", "POST", "/api/session/{id}/send", Backend, ["id", "body"], MAX_API_BODY, Mutation, Ordinary;
    EditorOpen, "editor.open", "POST", "/api/editor/open", Backend, ["body"], MAX_API_BODY, Registration, Editor;
    FileOffer, "file.offer", "POST", "/api/file/offer", Backend, ["body"], MAX_API_BODY, Registration, FileTransfer;
    ViewerComments, "viewer.comments", "GET", "/api/viewer/comments", Backend, ["screenId?", "paneId?", "document?", "all?"], MAX_API_BODY, Read, Ordinary;
    ViewerCommentReply, "viewer.commentReply", "POST", "/api/viewer/comments/reply", Backend, ["body"], MAX_API_BODY, Mutation, Ordinary;
    ViewerReviewWait, "viewer.reviewWait", "POST", "/api/viewer/review/wait", Backend, ["body"], MAX_API_BODY, Registration, Review;
    JobList, "job.list", "GET", "/api/jobs", Backend, [], MAX_JOB_BODY, Read, Ordinary;
    JobLaunch, "job.launch", "POST", "/api/jobs", Backend, ["body"], MAX_JOB_BODY, Mutation, JobLaunch;
    JobInspect, "job.inspect", "GET", "/api/jobs/{id}", Backend, ["id"], MAX_JOB_BODY, Read, Ordinary;
    JobLogs, "job.logs", "GET", "/api/jobs/{id}/logs", Backend, ["id", "cursor?", "tail?", "limit?", "waitMs?", "format?"], MAX_JOB_BODY, Read, JobLogs;
    JobInput, "job.input", "POST", "/api/jobs/{id}/input", Backend, ["id", "body"], MAX_JOB_BODY, Mutation, Ordinary;
    JobStop, "job.stop", "POST", "/api/jobs/{id}/stop", Backend, ["id"], MAX_JOB_BODY, Mutation, Ordinary;
    JobForget, "job.forget", "POST", "/api/jobs/{id}/forget", Backend, ["id"], MAX_JOB_BODY, Mutation, Ordinary;
    HostDirectory, "remote.directory.get", "GET", "/api/remote/directory", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    HostList, "host.list", "GET", "/api/remote/hosts", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    HostForget, "host.forget", "DELETE", "/api/remote/hosts", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    HostPreferencesGet, "hostPreferences.get", "GET", "/api/remote/host-preferences", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    HostPreferencesUpdate, "hostPreferences.update", "PUT", "/api/remote/host-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    HostOrderGet, "hostOrder.get", "GET", "/api/remote/host-order", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    HostOrderUpdate, "hostOrder.update", "PUT", "/api/remote/host-order", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    RemoteScreensGet, "remoteScreens.get", "GET", "/api/remote/screens", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    RemoteScreensUpdate, "remoteScreens.update", "PUT", "/api/remote/screens", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    CatalogGet, "catalog.get", "GET", "/api/remote/catalog", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    CatalogUpdate, "catalog.update", "PUT", "/api/remote/catalog", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    ForwardList, "forward.list", "GET", "/api/remote/forwards", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    ForwardOpen, "forward.open", "POST", "/api/remote/forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward;
    ForwardStop, "forward.stop", "DELETE", "/api/remote/forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward;
    ForwardPreferencesGet, "forwardPreferences.get", "GET", "/api/remote/forward-preferences", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    ForwardPreferencesSave, "forwardPreferences.save", "PUT", "/api/remote/forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    ForwardPreferencesForget, "forwardPreferences.forget", "DELETE", "/api/remote/forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    ReverseList, "reverse.list", "GET", "/api/remote/reverse-forwards", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    ReverseOpen, "reverse.open", "POST", "/api/remote/reverse-forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward;
    ReverseStop, "reverse.stop", "DELETE", "/api/remote/reverse-forwards", LocalHub, ["body"], MAX_API_BODY, Mutation, Forward;
    ReversePreferencesGet, "reversePreferences.get", "GET", "/api/remote/reverse-forward-preferences", LocalHub, [], MAX_API_BODY, Read, Ordinary;
    ReversePreferencesSave, "reversePreferences.save", "PUT", "/api/remote/reverse-forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
    ReversePreferencesForget, "reversePreferences.forget", "DELETE", "/api/remote/reverse-forward-preferences", LocalHub, ["body"], MAX_API_BODY, Mutation, Ordinary;
}

/// These are classified explicitly and can never become native UI operations.
pub const ADAPTER_ROUTES: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("POST", "/api/session"),
    ("POST", "/api/ws-ticket"),
    ("POST", "/api/bootstrap"),
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
    #[cfg(test)]
    pub fn parse(bytes: &[u8], role: Role) -> Result<Self, Rejection> {
        if bytes.len() > MAX_API_BODY {
            return Err(Rejection::TooLarge);
        }
        let request: Self =
            serde_json::from_slice(bytes).map_err(|error| Rejection::Invalid(error.to_string()))?;
        if bytes.len() > request.operation.description().body_limit {
            return Err(Rejection::TooLarge);
        }
        request.validate(role)?;
        Ok(request)
    }

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
            if !description
                .arguments
                .iter()
                .any(|arg| arg.trim_end_matches('?') == key)
            {
                return Err(Rejection::Invalid(format!("unexpected argument: {key}")));
            }
        }
        for arg in description.arguments {
            let optional = arg.ends_with('?');
            let name = arg.trim_end_matches('?');
            match self.args.get(name) {
                None if !optional => {
                    return Err(Rejection::Invalid(format!("missing argument: {name}")))
                }
                Some(value) if name != "body" && !value.is_string() => {
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
    "editor.register",
    "editor.result",
    "file.register",
    "file.accept",
    "file.decline",
    "file.read",
    "viewer.read",
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

#[cfg(test)]
mod tests;
