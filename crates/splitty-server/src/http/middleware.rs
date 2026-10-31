//! Axum middleware for authentication, origin enforcement and the localhost guard.
//!
//! - `require_token` — accepts an `HttpOnly` session cookie or a `Bearer` token.
//! - `require_origin` — rejects cross-origin browser requests, including ones
//!   from other loopback ports (`Origin` plus Fetch Metadata).
//! - `require_localhost` — rejects requests whose Host header is not loopback.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::session::{cookie_value, SESSION_COOKIE};
use super::AppState;

/// Authentication middleware.
///
/// Two credentials are accepted, and the set is deliberately small
/// (security review SR-04):
///
/// 1. The `splitty-session` cookie, established by the bootstrap exchange. It is
///    `HttpOnly`, so a page on another `127.0.0.1:*` port cannot read it out of
///    `document.cookie` — cookies are not port-scoped, which is what made the
///    previous script-readable cookie a full compromise.
/// 2. `Authorization: Bearer <token>` with the process token, for the CLI and the
///    native launchers. Never sent by the browser UI.
///
/// Two former sources were removed and must not come back:
///
/// - **`?token=` in the query string.** It put the credential into the address
///   bar, browser history, `Referer` headers and every server log line.
/// - **The `splitty-token` cookie.** It was set from JavaScript and therefore
///   readable by JavaScript, including by any other page on the same host.
///
/// `/ws` and `/ws/remote` are exempt here because they authenticate with a
/// single-use ticket instead; see `session.rs`.
pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();

    // Health check needs no auth.
    if path == "/health" {
        return next.run(request).await;
    }

    // The bootstrap exchange is the endpoint that *establishes* a credential,
    // so it cannot require one. It is protected instead by the bootstrap code
    // being single-use and unguessable, plus the origin and Host guards.
    if path == "/api/session" {
        return next.run(request).await;
    }

    // The WebSockets carry their own single-use ticket, validated in the handler.
    if is_websocket_path(path) {
        return next.run(request).await;
    }

    let needs_auth = path.starts_with("/api") || path.starts_with("/proxy/");

    if needs_auth && !authenticated(&state, &request) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    next.run(request).await
}

fn authenticated(state: &AppState, request: &Request) -> bool {
    if let Some(session) = cookie_value(request, SESSION_COOKIE) {
        if state.sessions.session_valid(&session) {
            return true;
        }
    }

    if let Some(bearer) = bearer_token(request) {
        return validate_token(&bearer, &state.token);
    }

    false
}

/// Extract a `Authorization: Bearer` token, if present.
fn bearer_token(request: &Request) -> Option<String> {
    let value = request.headers().get("authorization")?.to_str().ok()?;
    value.strip_prefix("Bearer ").map(str::to_string)
}

// The local `validate_token` helper is gone (security review
// SR-17/SR-22). It was a second, subtly different copy of the comparison that
// returned `true` whenever the expected token was empty — and `ws_upgrade` used
// the *other* copy, so fixing only this one would have left the WebSocket path
// unchanged. There is now a single implementation in `splitty_core::auth`.
use splitty_core::auth::validate as validate_token;

/// Is this request's `Origin` acceptable?
///
/// Returns `None` when the request carries no `Origin` header at all, which
/// callers must interpret per endpoint — see [`require_origin`].
pub fn origin_allowed(request: &Request, allowed: &[String]) -> Option<bool> {
    let origin = request.headers().get("origin")?.to_str().ok()?;
    Some(allowed.iter().any(|candidate| candidate == origin))
}

/// Reject cross-origin browser requests.
///
/// This is the control that makes a *leaked* credential far less useful
/// (security review SR-04/SR-07). Without it, any page the user
/// visits can open `ws://127.0.0.1:<port>/ws` with a stolen token and drive every
/// terminal — WebSockets are not subject to CORS, so nothing else stops it.
///
/// The two endpoint classes are treated differently, and the asymmetry is
/// deliberate:
///
/// - **`/ws` and `/ws/remote` require an `Origin`.** Browsers always send one
///   on a WebSocket handshake, so an absent header means the caller is not a
///   browser. Nothing in Splitty needs a non-browser WebSocket client — the
///   CLI speaks HTTP — so absent is rejected. This is the strict direction and
///   costs nothing.
///
/// - **`/api` rejects only a *wrong* `Origin`.** Browsers omit `Origin` on
///   same-origin GETs, and the CLI sends none at all, so requiring it here would
///   break both. That is still sound: a browser attaches `Origin` to every
///   cross-origin request, so "absent" cannot be a cross-origin page. What we
///   must never do is accept a *present but foreign* origin, and that is exactly
///   what is rejected.
///
/// After the `Origin` decision, API and WebSocket paths also have to pass
/// [`fetch_site_allowed`], which catches the requests that carry no `Origin`
/// but do carry the cookie.
pub async fn require_origin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();

    if path == "/health" {
        return next.run(request).await;
    }

    match origin_allowed(&request, &state.allowed_origins) {
        Some(true) => {}
        Some(false) => return (StatusCode::FORBIDDEN, "Forbidden origin").into_response(),
        None => {
            if is_websocket_path(path) {
                return (StatusCode::FORBIDDEN, "Origin required").into_response();
            }
        }
    }

    if is_api_path(path) && !fetch_site_allowed(&request) {
        return (StatusCode::FORBIDDEN, "Cross-origin request").into_response();
    }

    next.run(request).await
}

/// Paths that carry the API or the terminal stream, as opposed to the app shell
/// and its static assets.
fn is_api_path(path: &str) -> bool {
    path.starts_with("/api") || is_websocket_path(path)
}

/// The WebSocket endpoints: the server's own, and the remote screen hub's.
fn is_websocket_path(path: &str) -> bool {
    path == "/ws" || path == "/ws/remote"
}

/// Is this request's `Sec-Fetch-Site` acceptable for an API or WebSocket path?
///
/// Closes the gap the `Origin` check leaves (security review SR-31, see
/// docs/security/findings.md). A page on any other `127.0.0.1` port is
/// *same-site* with Splitty, so the browser attaches the session cookie to its
/// requests, and a no-cors GET (`<img>`, `<script>`, a top-level navigation)
/// carries no `Origin` at all. Without this check the only thing standing
/// between such a page and the API is the convention that GET handlers never
/// change state.
///
/// Fetch Metadata tells *same-origin* apart from *same-site*, which is exactly
/// the distinction needed. Only `same-origin` is accepted: the UI never reaches
/// the API through a navigation, so `none` (a URL typed into the address bar) is
/// rejected as well.
///
/// An absent header is accepted. The CLI, the Electron main process and older
/// engines do not send one, and for them the `Origin` rules above still apply.
pub fn fetch_site_allowed(request: &Request) -> bool {
    match request.headers().get("sec-fetch-site") {
        None => true,
        Some(value) => value.as_bytes() == b"same-origin",
    }
}

/// Localhost-only middleware. Rejects requests whose `Host` header is not a
/// loopback name.
///
/// This is a **DNS-rebinding guard, not a binding control** (security review
/// SR-07). It only inspects a header the client chose to send, so a
/// raw network client can trivially set `Host: localhost`. What actually keeps
/// the service off the network is the loopback bind invariant enforced at
/// startup; do not document or rely on this function as if it replaced that.
pub async fn require_localhost(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    // Strip port if present
    let host = if let Some(idx) = host.rfind(':') {
        // Check if this is an IPv6 address like [::1]:3000
        if host.starts_with('[') {
            // IPv6 with port: [::1]:3000
            if let Some(bracket_end) = host.find(']') {
                if idx > bracket_end {
                    &host[..idx]
                } else {
                    host
                }
            } else {
                host
            }
        } else {
            &host[..idx]
        }
    } else {
        host
    };

    // Strip brackets from IPv6
    let host = host.trim_start_matches('[').trim_end_matches(']');

    // An empty Host used to be accepted. Nothing legitimate produces one here —
    // HTTP/1.1 requires the header and browsers never omit it — so it is now
    // rejected rather than treated as loopback.
    match host {
        "localhost" | "127.0.0.1" | "::1" => next.run(request).await,
        _ => (StatusCode::FORBIDDEN, "Forbidden").into_response(),
    }
}

// REMOVED: cors_layer (security review SR-07).
//
// It emitted `Access-Control-Allow-Origin: *` on every response — including
// preflights — whenever remote mode was enabled. A wildcard is not itself an
// authentication bypass, but combined with a leaked token it removes the last
// browser-side obstacle for a foreign origin.
//
// No replacement is needed: the UI is served from the same origin as the API, so
// it never triggers CORS. Emitting no CORS headers at all is both correct and
// the strictest option. If a remote deployment is ever added, the allowance must
// name concrete origins and must not be reflected from the request.
