//! Bootstrap exchange, browser sessions and WebSocket tickets.
//!
//! # Why this exists
//!
//! The server used to authenticate every caller with one long-lived token that
//! was simultaneously present in the launch URL, `localStorage`, a JavaScript
//! readable cookie, every API query string, the process stdout and the
//! environment of every spawned shell (security review SR-04).
//! Any single one of those disclosures granted full terminal control, for as
//! long as the server ran.
//!
//! This module splits that one credential into three, each with the narrowest
//! shape its job allows:
//!
//! | Credential | Who holds it | Lifetime |
//! |---|---|---|
//! | Bootstrap code | the native launcher, in the URL it navigates to | one exchange, 120 s |
//! | Session | the browser, as an `HttpOnly` cookie | the server process |
//! | WebSocket ticket | the page, for the duration of one handshake | one upgrade, 10 s |
//!
//! # The shape of the guarantees
//!
//! *Bootstrap codes are single-use by construction, not by check.* The exchange
//! takes the code out of the map with `remove`, so there is no window in which
//! two concurrent requests can both observe it as valid — a property an
//! `if valid { invalidate() }` sequence would not have.
//!
//! *The session cookie is `HttpOnly`, so script cannot read it.* That matters
//! because cookies are not port-scoped: a page served from any other
//! `127.0.0.1:*` port is same-site with Splitty and its script could otherwise
//! read `document.cookie` and take the credential.
//!
//! *WebSocket upgrades use a ticket, not the cookie.* WebSockets are exempt from
//! CORS, so a cookie alone would let any origin open the socket. A ticket must be
//! fetched over HTTP first, and that fetch is subject to both the origin check
//! and the same-origin response-reading rule — a foreign page can issue the
//! request but cannot read the ticket out of the response.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use super::AppState;

/// How long a WebSocket ticket stays valid.
///
/// Long enough for the page to open the socket right after fetching it, short
/// enough that a ticket captured from a log or a screenshot is already dead.
const TICKET_TTL: Duration = Duration::from_secs(10);

/// How long an unredeemed bootstrap code stays valid.
///
/// Launchers redeem their code immediately. The window only has to cover a
/// browser that is slow to start under `serve --open` and the SSH round trip of
/// `splitty connect`. A code that was printed and never used, and is later found
/// in scrollback or a log, is dead by then (security review SR-32, see
/// docs/security/findings.md).
const BOOTSTRAP_TTL: Duration = Duration::from_secs(120);

/// Name of the session cookie. `__Host-` is deliberately not used: that prefix
/// requires `Secure`, which browsers refuse to honour over plaintext HTTP, and
/// the loopback listener has no TLS.
pub const SESSION_COOKIE: &str = "splitty-session";

/// Credential store for one server process.
///
/// Nothing here is persisted. A restart invalidates every session and ticket,
/// which is the desired behaviour: these credentials describe a live UI
/// connection, not a durable identity.
#[derive(Default)]
pub struct SessionStore {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Unredeemed bootstrap codes and their issue time. Removed on first use.
    bootstrap: HashMap<String, Instant>,
    /// Active browser sessions.
    sessions: HashMap<String, ()>,
    /// Unredeemed WebSocket tickets and their issue time.
    tickets: HashMap<String, Instant>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a bootstrap code for a launcher to embed in the URL it opens.
    pub fn issue_bootstrap(&self) -> String {
        let code =
            splitty_core::auth::generate_oneshot().expect("failed to generate bootstrap code");
        let mut inner = self.lock();
        inner.bootstrap.insert(code.clone(), Instant::now());
        prune_bootstrap(&mut inner);
        code
    }

    /// Redeem a bootstrap code for a session id.
    ///
    /// `remove` is what makes this single-use: the code is gone before this
    /// function returns, so a replayed request finds nothing. An empty code is
    /// rejected outright so a missing field can never match a stray empty key.
    /// An expired code is consumed and rejected.
    pub fn redeem_bootstrap(&self, code: &str) -> Option<String> {
        if code.is_empty() {
            return None;
        }
        let mut inner = self.lock();
        let issued = inner.bootstrap.remove(code)?;
        if issued.elapsed() > BOOTSTRAP_TTL {
            return None;
        }
        Some(insert_session(&mut inner))
    }

    /// Start a session without a bootstrap code, for a caller that has already
    /// proven possession of the process token.
    pub fn create_session(&self) -> String {
        let mut inner = self.lock();
        insert_session(&mut inner)
    }

    pub fn session_valid(&self, session: &str) -> bool {
        !session.is_empty() && self.lock().sessions.contains_key(session)
    }

    pub fn issue_ticket(&self) -> String {
        let ticket = splitty_core::auth::generate_oneshot().expect("failed to generate ticket");
        let mut inner = self.lock();
        inner.tickets.insert(ticket.clone(), Instant::now());
        prune_tickets(&mut inner);
        ticket
    }

    /// Redeem a ticket. Single-use and time-bounded.
    pub fn redeem_ticket(&self, ticket: &str) -> bool {
        let mut inner = self.lock();
        match inner.tickets.remove(ticket) {
            Some(issued) => issued.elapsed() <= TICKET_TTL,
            None => false,
        }
    }

    /// Number of bootstrap codes minted but not yet redeemed.
    ///
    /// Test support. Public because the binary's tests live in a different
    /// compilation unit than this library, so `#[cfg(test)]` would not reach
    /// them. It exposes a count and never a code, so it is not a disclosure.
    #[doc(hidden)]
    pub fn pending_bootstrap_count(&self) -> usize {
        self.lock().bootstrap.len()
    }

    /// A poisoned mutex must not take the server down, and an auth store that
    /// panics on lock would be a denial-of-service primitive.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn insert_session(inner: &mut Inner) -> String {
    let session = splitty_core::auth::generate_oneshot().expect("failed to generate session id");
    inner.sessions.insert(session.clone(), ());
    session
}

fn prune_tickets(inner: &mut Inner) {
    inner
        .tickets
        .retain(|_, issued| issued.elapsed() <= TICKET_TTL);
}

fn prune_bootstrap(inner: &mut Inner) {
    inner
        .bootstrap
        .retain(|_, issued| issued.elapsed() <= BOOTSTRAP_TTL);
}

/// Read a cookie value out of a request's `Cookie` header.
pub fn cookie_value(request: &Request, name: &str) -> Option<String> {
    let raw = request.headers().get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

#[derive(Deserialize)]
pub struct BootstrapRequest {
    /// A single-use code minted by the launcher's `serve` invocation.
    #[serde(default)]
    bootstrap: String,
    /// The process token, for a user opening the UI in a plain browser where no
    /// launcher was involved and there is no bootstrap code to redeem. It is
    /// exchanged for a session immediately so the token itself never has to be
    /// held by the page.
    #[serde(default)]
    token: String,
}

/// `POST /api/session` — exchange a bootstrap code for a session cookie.
///
/// The response sets `HttpOnly` so the value never becomes readable by script,
/// and `SameSite=Strict` so it is not attached to navigations from other sites.
/// Neither replaces the origin check — `SameSite` treats every `127.0.0.1:*`
/// port as the same site — they are layers, not alternatives.
pub async fn exchange_bootstrap(
    State(state): State<AppState>,
    Json(body): Json<BootstrapRequest>,
) -> Response {
    let session = if !body.bootstrap.is_empty() {
        state.sessions.redeem_bootstrap(&body.bootstrap)
    } else if !body.token.is_empty() && splitty_core::auth::validate(&body.token, &state.token) {
        Some(state.sessions.create_session())
    } else {
        None
    };

    let Some(session) = session else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid credential" })),
        )
            .into_response();
    };

    let cookie = format!("{SESSION_COOKIE}={session}; Path=/; HttpOnly; SameSite=Strict");
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

/// `POST /api/bootstrap` — mint a bootstrap code for a browser elsewhere.
///
/// Used by `splitty connect`, which runs this over SSH on the remote host so a
/// local browser can establish a session through the tunnel.
///
/// Reachable **only** over HTTP with the process token, never over the local
/// control socket (security review SR-05/SR-24, see docs/security/findings.md):
/// a bootstrap code buys a full browser session, including configuration and
/// shell-profile writes, and the control socket is offered to every pane.
///
/// This is not a boundary against processes of the same user. The token lives
/// in a 0600 file that any process running as the user can read, including one
/// started in a pane, just as such a process can edit `~/.bashrc` or Splitty's
/// configuration directly. What the separation guarantees is that panes are
/// never *handed* a credential, so none leaks through `env` output, crash
/// reports or CI logs.
pub async fn issue_bootstrap(State(state): State<AppState>) -> Response {
    Json(json!({ "bootstrap": state.sessions.issue_bootstrap() })).into_response()
}

/// `POST /api/ws-ticket` — mint a single-use WebSocket ticket.
///
/// Reachable only through the authenticated, origin-checked middleware stack, so
/// possession of a valid session and an allowed origin is already proven by the
/// time this runs.
pub async fn issue_ws_ticket(State(state): State<AppState>) -> Response {
    Json(json!({ "ticket": state.sessions.issue_ticket() })).into_response()
}

/// Test-only accessor so integration tests can assert single-use semantics
/// without reaching through HTTP.
#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SessionStore {
        SessionStore::new()
    }

    #[test]
    fn bootstrap_code_works_exactly_once() {
        let store = store();
        let code = store.issue_bootstrap();

        assert!(
            store.redeem_bootstrap(&code).is_some(),
            "first use must succeed"
        );
        assert!(store.redeem_bootstrap(&code).is_none(), "replay must fail");
    }

    #[test]
    fn unknown_bootstrap_code_is_rejected() {
        let store = store();
        store.issue_bootstrap();
        assert!(store.redeem_bootstrap("not-a-real-code").is_none());
        assert!(store.redeem_bootstrap("").is_none());
    }

    #[test]
    fn redeeming_a_bootstrap_yields_a_valid_session() {
        let store = store();
        let code = store.issue_bootstrap();
        let session = store.redeem_bootstrap(&code).expect("session");

        assert!(store.session_valid(&session));
        assert!(!store.session_valid("some-other-value"));
    }

    /// An empty session id must never validate — otherwise a request with no
    /// cookie at all would authenticate.
    #[test]
    fn empty_session_is_never_valid() {
        let store = store();
        assert!(!store.session_valid(""));
    }

    #[test]
    fn expired_bootstrap_code_is_rejected() {
        let store = store();
        let code = store.issue_bootstrap();

        // Backdate the issue time past the TTL rather than sleeping. A machine
        // that has been up for less than the TTL cannot represent that instant.
        let Some(stale) = Instant::now().checked_sub(BOOTSTRAP_TTL + Duration::from_secs(1)) else {
            return;
        };
        store.lock().bootstrap.insert(code.clone(), stale);

        assert!(store.redeem_bootstrap(&code).is_none());
        assert_eq!(
            store.pending_bootstrap_count(),
            0,
            "an expired code is consumed"
        );
    }

    #[test]
    fn issuing_prunes_expired_bootstrap_codes() {
        let store = store();
        let Some(stale) = Instant::now().checked_sub(BOOTSTRAP_TTL + Duration::from_secs(1)) else {
            return;
        };
        store.lock().bootstrap.insert("stale".to_string(), stale);

        store.issue_bootstrap();

        assert_eq!(
            store.pending_bootstrap_count(),
            1,
            "only the fresh code remains"
        );
    }

    #[test]
    fn ticket_works_exactly_once() {
        let store = store();
        let ticket = store.issue_ticket();

        assert!(store.redeem_ticket(&ticket), "first use must succeed");
        assert!(!store.redeem_ticket(&ticket), "replay must fail");
    }

    #[test]
    fn unknown_ticket_is_rejected() {
        let store = store();
        assert!(!store.redeem_ticket("not-a-real-ticket"));
        assert!(!store.redeem_ticket(""));
    }

    #[test]
    fn expired_ticket_is_rejected() {
        let store = store();
        let ticket = store.issue_ticket();

        // Backdate the issue time past the TTL rather than sleeping.
        store.lock().tickets.insert(
            ticket.clone(),
            Instant::now() - TICKET_TTL - Duration::from_secs(1),
        );

        assert!(!store.redeem_ticket(&ticket));
    }

    #[test]
    fn cookie_parsing_picks_the_right_value() {
        let request = Request::builder()
            .header(
                header::COOKIE,
                "other=1; splitty-session=abc123; trailing=2",
            )
            .body(axum::body::Body::empty())
            .expect("request");

        assert_eq!(
            cookie_value(&request, SESSION_COOKIE).as_deref(),
            Some("abc123")
        );
        assert_eq!(cookie_value(&request, "missing"), None);
    }
}
