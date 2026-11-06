//! The agent channel of viewer panes (`control-api`, Viewer Comment Requests
//! and Review Requests; `viewer-annotations`, Agents Read And Reply To
//! Handed-Over Threads and Waiting For A Review): what the user handed over
//! on the viewer panes of the caller's screen, an agent's reply to it, and
//! the held request that waits for the user's verdict on a document.
//!
//! Served on both control transports and never on a grant socket. The screen
//! is the scope: a caller names its screen and pane, and reads and replies
//! only among the viewer panes of the screen that holds that pane. Agents'
//! replies and waits are trusted operations the server derives here; no
//! client submits them.
use std::collections::HashMap;
use std::sync::Arc;

use http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};
use splitty_core::ids::{hex_id, PaneId, ScreenId};
use splitty_core::mux::{
    AnnotationReply, AnnotationStatus, MuxError, Op, PaneRole, ReplyAuthor, ReplyFrom, ReviewWait,
    ScreenState, Store, ViewerAnnotation, ViewerSource, ViewerState, REPLY_TITLE_LIMIT,
};
use splitty_core::protocol::ServerEvent;

use super::reply::{IntoReply, Payload, Reply};
use super::State;
use crate::client_session::reviews::{Outcome, Refusal, Registration};
use crate::client_session::ClientState;

/// The title of a reply or a wait when the caller's pane has none.
const DEFAULT_TITLE: &str = "agent";

/// An answer other than 200, as `{code, error}`.
struct Failure {
    status: StatusCode,
    code: &'static str,
    error: String,
}

impl Failure {
    fn invalid(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid",
            error: error.into(),
        }
    }

    fn not_found(error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            error: error.into(),
        }
    }

    fn conflict(code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            error: error.into(),
        }
    }
}

impl IntoReply for Failure {
    fn into_reply(self) -> Reply {
        (
            self.status,
            Payload(json!({"code": self.code, "error": self.error})),
        )
            .into_reply()
    }
}

fn answer(result: Result<Value, Failure>) -> Reply {
    match result {
        Ok(body) => Payload(body).into_reply(),
        Err(failure) => failure.into_reply(),
    }
}

/// A body that is no JSON object of the endpoint's shape.
pub(crate) fn invalid_body(error: impl std::fmt::Display) -> Reply {
    Failure::invalid(format!("invalid request: {error}")).into_reply()
}

// ── Scope ───────────────────────────────────────────────────────────────────

/// The screen of the caller's pane: the named screen when it holds the pane,
/// else the screen the pane moved to, as `POST /api/mux/op` follows it.
fn caller_screen(store: &Store, screen_id: &str, pane_id: &str) -> Result<ScreenState, Failure> {
    if screen_id.is_empty() || pane_id.is_empty() {
        return Err(Failure::invalid(
            "screenId and paneId name the caller's screen and pane",
        ));
    }
    let pane = PaneId::from(pane_id);
    store
        .get_screen(&ScreenId::from(screen_id))
        .filter(|screen| screen.panes.contains_key(&pane))
        .or_else(|| {
            let moved = store.screen_of_movable_pane(&pane)?;
            store.get_screen(&moved)
        })
        .ok_or_else(|| Failure::not_found(format!("no screen holds pane {pane_id}")))
}

/// The record of `pane_id` when it is a viewer pane of `screen`.
fn viewer_of<'a>(screen: &'a ScreenState, pane_id: &PaneId) -> Option<&'a ViewerState> {
    let pane = screen.panes.get(pane_id)?;
    if pane.role != Some(PaneRole::Viewer) {
        return None;
    }
    pane.viewer.as_ref()
}

/// The viewer panes of `screen` in layout order.
fn viewer_panes(screen: &ScreenState) -> Vec<PaneId> {
    screen
        .panes_in_layout_order()
        .filter(|id| viewer_of(screen, id).is_some())
        .collect()
}

/// The viewer pane of `screen` that `document` names: a viewer pane's ID,
/// or the absolute path of a file that exactly one viewer pane shows.
fn document_pane(screen: &ScreenState, document: &str) -> Result<PaneId, Failure> {
    let named = PaneId::from(document);
    if viewer_of(screen, &named).is_some() {
        return Ok(named);
    }
    if document.starts_with("viewer-") {
        return Err(Failure::not_found(format!(
            "this screen holds no viewer pane {document}"
        )));
    }
    if !document.starts_with('/') {
        return Err(Failure::invalid(
            "document is an absolute path or the ID of a viewer pane",
        ));
    }
    let showing: Vec<PaneId> = viewer_panes(screen)
        .into_iter()
        .filter(|id| {
            viewer_of(screen, id).is_some_and(
                |viewer| matches!(&viewer.source, ViewerSource::File { path } if path == document),
            )
        })
        .collect();
    match showing.as_slice() {
        [] => Err(Failure::not_found(format!(
            "no viewer pane of this screen shows {document}"
        ))),
        [pane] => Ok(pane.clone()),
        several => Err(Failure::conflict(
            "ambiguous",
            format!(
                "{document} is shown in the viewer panes {}; name one by its ID",
                several
                    .iter()
                    .map(PaneId::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// The title the server knows for the caller's pane (design D6 of
/// `review-threads`): the pane's own title, else its session's process
/// title, else its OSC title, else `agent`; without control and invisible
/// formatting characters, at most 256 bytes.
fn caller_title(client_state: &ClientState, screen: &ScreenState, pane_id: &PaneId) -> String {
    let pane = screen.panes.get(pane_id);
    if let Some(title) = pane.and_then(|pane| shown_title(pane.title.as_deref()?)) {
        return title;
    }
    let session = pane
        .and_then(|pane| pane.session_id.as_ref())
        .and_then(|id| client_state.dispatcher.backend().get(id.as_str()));
    session
        .and_then(|session| {
            shown_title(&session.title().proc)
                .or_else(|| shown_title(&session.osc_title_watch().borrow().title))
        })
        .unwrap_or_else(|| DEFAULT_TITLE.to_owned())
}

/// `title` as a reply shows it; none when nothing of it is left.
fn shown_title(title: &str) -> Option<String> {
    let visible: String = title
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{206f}'
                        | '\u{feff}'
                )
        })
        .collect();
    let mut title = visible.trim();
    if title.len() > REPLY_TITLE_LIMIT {
        let mut end = REPLY_TITLE_LIMIT;
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        title = title[..end].trim_end();
    }
    (!title.is_empty()).then(|| title.to_owned())
}

/// Every connected window shows the change of a viewer pane on `screen_id`
/// (`mux-protocol`, Change Propagation to Other Clients).
fn nudge(client_state: &ClientState, screen_id: &ScreenId) {
    client_state.broadcast_events(
        &[ServerEvent::mux_nudge(
            "mux_op".into(),
            Some(screen_id.to_string()),
        )],
        None,
    );
}

/// A failure of the store for a pane the request resolved a moment ago.
fn gone(error: MuxError) -> Failure {
    match error {
        MuxError::ScreenNotFound(_) | MuxError::PaneNotFound(_) | MuxError::NotAViewerPane(_) => {
            Failure::not_found(error.to_string())
        }
        other => {
            tracing::warn!("viewer review request: {other}");
            Failure {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal",
                error: other.to_string(),
            }
        }
    }
}

// ── GET /api/viewer/comments ────────────────────────────────────────────────

/// What an agent may read of `annotation`: none for an annotation the user
/// has not handed over, or a resolved one unless `all`; the agents' replies
/// and the user's handed-over ones.
fn readable(annotation: &ViewerAnnotation, all: bool) -> Option<Value> {
    if !annotation.sent || (!all && annotation.status != AnnotationStatus::Open) {
        return None;
    }
    let replies: Vec<Value> = annotation
        .replies
        .iter()
        .filter(|reply| reply.author == ReplyAuthor::Agent || reply.sent)
        .map(|reply| {
            let mut entry = json!({"author": reply.author, "body": reply.body});
            if let Some(from) = &reply.from {
                entry["from"] = json!({"paneId": from.pane_id, "title": from.title});
            }
            entry
        })
        .collect();
    let mut entry = json!({
        "number": annotation.number,
        "kind": annotation.kind,
        "status": annotation.status,
        "replies": replies,
    });
    if let Some(body) = &annotation.body {
        entry["body"] = json!(body);
    }
    if let Some(replacement) = &annotation.replacement {
        entry["replacement"] = json!(replacement);
    }
    if let Some(target) = &annotation.target {
        entry["target"] = json!({
            "view": target.view, "quote": target.quote,
            "startLine": target.start_line, "endLine": target.end_line,
        });
    }
    Some(entry)
}

/// One viewer pane in the listing: its source without a text's text, and
/// what an agent may read of its annotations, by number.
fn document(pane_id: &PaneId, viewer: &ViewerState, all: bool) -> Value {
    let source = match &viewer.source {
        ViewerSource::File { path } => json!({"kind": "file", "path": path}),
        ViewerSource::Text { title, .. } => json!({"kind": "text", "title": title}),
    };
    let mut annotations: Vec<&ViewerAnnotation> = viewer.annotations.iter().collect();
    annotations.sort_by_key(|annotation| annotation.number);
    let annotations: Vec<Value> = annotations
        .into_iter()
        .filter_map(|annotation| readable(annotation, all))
        .collect();
    json!({"paneId": pane_id, "source": source, "annotations": annotations})
}

/// `GET /api/viewer/comments?screenId=&paneId=[&document=][&all=true]`.
pub(crate) async fn get_viewer_comments(state: &State, params: HashMap<String, String>) -> Reply {
    answer(comments(state, &params))
}

fn comments(state: &State, params: &HashMap<String, String>) -> Result<Value, Failure> {
    let param = |name: &str| params.get(name).map(String::as_str).unwrap_or_default();
    let all = match params.get("all").map(String::as_str) {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(Failure::invalid("all is true or false")),
    };
    let screen = caller_screen(&state.mux_store, param("screenId"), param("paneId"))?;
    let panes = match params.get("document") {
        Some(named) => vec![document_pane(&screen, named)?],
        None => viewer_panes(&screen),
    };
    let documents: Vec<Value> = panes
        .iter()
        .filter_map(|id| Some(document(id, viewer_of(&screen, id)?, all)))
        .collect();
    Ok(json!({"documents": documents}))
}

// ── POST /api/viewer/comments/reply ─────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplyRequest {
    screen_id: String,
    pane_id: String,
    document: String,
    number: u32,
    body: String,
}

/// `POST /api/viewer/comments/reply {screenId, paneId, document, number, body}`.
pub(crate) async fn post_comment_reply(state: &State, body: Value) -> Reply {
    answer(match serde_json::from_value::<ReplyRequest>(body) {
        Ok(request) => reply(state, request),
        Err(error) => Err(Failure::invalid(format!("invalid request: {error}"))),
    })
}

fn reply(state: &State, request: ReplyRequest) -> Result<Value, Failure> {
    let screen = caller_screen(&state.mux_store, &request.screen_id, &request.pane_id)?;
    let viewer = document_pane(&screen, &request.document)?;
    let caller = PaneId::from(request.pane_id.as_str());
    let from = ReplyFrom {
        title: caller_title(&state.client_state, &screen, &caller),
        pane_id: caller,
    };
    // A reply ID the pane already holds is chosen again, twice at most.
    let mut attempts = 0;
    loop {
        let op = Op::AgentReply {
            screen_id: screen.id.clone(),
            pane_id: viewer.clone(),
            number: request.number,
            reply: AnnotationReply {
                id: hex_id("rep"),
                author: ReplyAuthor::Agent,
                body: request.body.clone(),
                sent: false,
                from: Some(from.clone()),
            },
        };
        let error = match state.mux_store.dispatch(op) {
            Ok(_) => {
                nudge(&state.client_state, &screen.id);
                return Ok(json!({"ok": true, "paneId": viewer, "number": request.number}));
            }
            Err(MuxError::ReplyRepeated(_)) if attempts < 2 => {
                attempts += 1;
                continue;
            }
            Err(error) => error,
        };
        return Err(match error {
            // One answer for both, so that a caller learns nothing of the
            // user's drafts.
            MuxError::AnnotationNumberNotFound { .. } | MuxError::AnnotationNotSent { .. } => {
                Failure::not_found(format!(
                    "viewer pane {viewer} holds no handed-over annotation #{}",
                    request.number
                ))
            }
            MuxError::ReplyInvalid { .. } | MuxError::ReplyTooLong { .. } => {
                Failure::invalid(error.to_string())
            }
            MuxError::RepliesTooMany { .. } | MuxError::AnnotationsTooLong { .. } => {
                Failure::conflict("limit", error.to_string())
            }
            other => gone(other),
        });
    }
}

// ── POST /api/viewer/review/wait ────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaitRequest {
    screen_id: String,
    pane_id: String,
    viewer_pane_id: String,
    /// The viewer's revision the caller showed (`viewerRevision` of the
    /// `open_viewer` answer); the wait is refused when the pane shows
    /// another one by then.
    #[serde(default)]
    viewer_revision: Option<u64>,
}

/// `POST /api/viewer/review/wait {screenId, paneId, viewerPaneId,
/// viewerRevision?}`, held until the user answers or the viewer pane closes.
/// Dropping the request, as a cancelled native call or a closed connection
/// does, ends the wait without an outcome.
pub(crate) async fn post_review_wait(state: &State, body: Value) -> Reply {
    let request = match serde_json::from_value::<WaitRequest>(body) {
        Ok(request) => request,
        Err(error) => return invalid_body(error),
    };
    answer(wait(state, request).await)
}

/// Ends the wait when the waiting request returns or is dropped: takes it
/// out of the registry, so that no later answer is accepted, then out of the
/// viewer record, and nudges the windows of the screen the pane is on then.
struct EndWait {
    registration: Option<Registration>,
    store: Arc<Store>,
    client_state: Arc<ClientState>,
    pane_id: PaneId,
    wait_id: String,
}

impl Drop for EndWait {
    fn drop(&mut self) {
        drop(self.registration.take());
        let pane = self.pane_id.clone();
        let ended = self.store.dispatch_inspect(
            Op::EndReviewWait {
                pane_id: self.pane_id.clone(),
                wait_id: self.wait_id.clone(),
            },
            |_, after| {
                after
                    .screens
                    .values()
                    .find(|screen| screen.panes.contains_key(&pane))
                    .map(|screen| screen.id.clone())
            },
        );
        // A closed pane took its wait with it.
        if let Ok((_, Some(screen))) = ended {
            nudge(&self.client_state, &screen);
        }
    }
}

async fn wait(state: &State, request: WaitRequest) -> Result<Value, Failure> {
    let store = &state.mux_store;
    let screen = caller_screen(store, &request.screen_id, &request.pane_id)?;
    let viewer = PaneId::from(request.viewer_pane_id.as_str());
    if viewer_of(&screen, &viewer).is_none() {
        return Err(Failure::not_found(format!(
            "this screen holds no viewer pane {}",
            request.viewer_pane_id
        )));
    }
    let mut registration = match state.client_state.reviews.register(viewer.clone()) {
        Ok(registration) => registration,
        Err(Refusal::PaneBusy) => {
            return Err(Failure::conflict(
                "busy",
                format!("a process already waits for a review of {viewer}"),
            ))
        }
        Err(Refusal::Full) => {
            return Err(Failure {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "busy",
                error: format!(
                    "{} processes already wait for a review",
                    crate::client_session::reviews::MAX_WAITS
                ),
            })
        }
    };
    let wait_id = registration.wait_id().to_owned();
    let caller = PaneId::from(request.pane_id.as_str());
    let title = caller_title(&state.client_state, &screen, &caller);
    // Taken before the wait starts, so that no change after it is missed.
    let mut changes = store.changes();
    store
        .dispatch(Op::StartReviewWait {
            screen_id: screen.id.clone(),
            pane_id: viewer.clone(),
            wait: ReviewWait {
                id: wait_id.clone(),
                pane_id: caller,
                title,
            },
            revision: request.viewer_revision,
        })
        .map_err(|error| match error {
            MuxError::ReviewWaitBusy(_) => Failure::conflict("busy", error.to_string()),
            MuxError::ReviewRevisionChanged(_) => Failure::conflict("changed", error.to_string()),
            other => gone(other),
        })?;
    let answered = registration.answer();
    let _end = EndWait {
        registration: Some(registration),
        store: store.clone(),
        client_state: state.client_state.clone(),
        pane_id: viewer.clone(),
        wait_id: wait_id.clone(),
    };
    nudge(&state.client_state, &screen.id);

    let closed = || Failure {
        status: StatusCode::GONE,
        code: "closed",
        error: format!("viewer pane {viewer} was closed before an answer"),
    };
    tokio::pin!(answered);
    loop {
        tokio::select! {
            outcome = &mut answered => return match outcome {
                Some(Outcome::Approved) => Ok(json!({"outcome": "approved"})),
                Some(Outcome::Feedback(review)) => Ok(json!({"outcome": "feedback", "review": review})),
                None => Err(closed()),
            },
            changed = changes.changed() => {
                // The pane, and with it its wait, is gone (on any screen).
                if changed.is_err()
                    || store.review_wait(&viewer).is_none_or(|held| held.id != wait_id)
                {
                    return Err(closed());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// design D6 of `review-threads`: what a title keeps.
    #[test]
    fn a_title_keeps_its_visible_text_within_its_limit() {
        assert_eq!(shown_title("  claude \n").as_deref(), Some("claude"));
        assert_eq!(
            shown_title("cl\u{1b}[31maude\u{202e}").as_deref(),
            Some("cl[31maude")
        );
        assert_eq!(shown_title(" \u{7}\t "), None);
        let long = format!("{}ä", "x".repeat(REPLY_TITLE_LIMIT - 1));
        let shown = shown_title(&long).expect("title");
        assert_eq!(shown, "x".repeat(REPLY_TITLE_LIMIT - 1));
        assert!(shown_title(&"ä".repeat(200)).is_some_and(|title| title.len() <= REPLY_TITLE_LIMIT));
    }
}
