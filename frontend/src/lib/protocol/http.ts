/**
 * HTTP access to the Splitty backend.
 *
 * The browser holds no credential of its own (security review
 * SR-04). Authentication rides on an `HttpOnly` session cookie that the server
 * sets during the bootstrap exchange, so there is nothing here for script — ours
 * or anyone else's — to read back out.
 *
 * What used to be in this file, and must not return:
 *
 * - `syncTokenCookie()`, which mirrored the token into a script-readable cookie
 *   so iframes could authenticate. Cookies are not port-scoped, so any page on
 *   any other `127.0.0.1:*` port could read it and take over every terminal.
 * - A `host` query parameter that redirected all API traffic to an arbitrary
 *   origin, which made a crafted app URL enough to send the local credential to
 *   a remote server.
 * - Appending `?token=` to every request, which put the credential into logs,
 *   `Referer` headers and browser history.
 */

/** Exchange a one-time bootstrap code for a session cookie. */
export async function exchangeBootstrap(bootstrap: string): Promise<boolean> {
  const res = await fetch("/api/session", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ bootstrap }),
  });
  return res.ok;
}

/** Exchange a manually entered server token for a session cookie. */
export async function exchangeToken(token: string): Promise<boolean> {
  const res = await fetch("/api/session", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ token }),
  });
  return res.ok;
}

/**
 * Fetch a single-use WebSocket ticket.
 *
 * The socket cannot authenticate with the session cookie: WebSocket handshakes
 * are exempt from CORS, so a cookie would let any origin that can reach the port
 * open one. Fetching a ticket over HTTP first puts the origin check and the
 * same-origin response rule in the way.
 *
 * Tickets are single-use, so every connect — including every reconnect — needs a
 * fresh one.
 */
export async function fetchWsTicket(): Promise<string | null> {
  try {
    const res = await fetch("/api/ws-ticket", {
      method: "POST",
      credentials: "same-origin",
    });
    if (!res.ok) return null;
    const body = await res.json();
    return typeof body?.ticket === "string" ? body.ticket : null;
  } catch {
    return null;
  }
}

/** Thin wrapper around fetch for API calls. */
export function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  return fetch(path, { ...init, credentials: "same-origin" });
}
