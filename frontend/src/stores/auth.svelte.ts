// Authentication is the local server's: the page's session cookie and the
// socket of the local connection. Another connection failing never changes
// this state.
import { socket } from "../lib/protocol/websocket.ts";
import { exchangeBootstrap, exchangeToken } from "../lib/protocol/http.ts";

export type AuthState = "checking" | "needsToken" | "connected" | "reconnecting";

let authState = $state<AuthState>("checking");
let authError = $state(false);

export function getAuthState(): AuthState {
  return authState;
}

export function getAuthError(): boolean {
  return authError;
}

export function setAuthState(state: AuthState): void {
  authState = state;
}

/**
 * Remove the credential from the address bar.
 *
 * Run immediately after the exchange, before anything can read `location`, take
 * a screenshot or follow a link that would carry it in a `Referer`. The code is
 * already spent by this point, so this is defence in depth rather than the
 * primary control — but a spent code in a support bundle still invites the wrong
 * conclusion about what leaked.
 */
function clearCredentialFromUrl(): void {
  const url = new URL(window.location.href);
  if (!url.searchParams.has("bootstrap")) return;
  url.searchParams.delete("bootstrap");
  history.replaceState(null, "", url.pathname + url.search + url.hash);
}

/** Enter a token manually — the plain-browser path, where no launcher ran. */
export async function handleTokenSubmit(token: string): Promise<void> {
  authState = "checking";
  authError = false;

  if (await exchangeToken(token)) {
    void socket.connect();
  } else {
    authState = "needsToken";
    authError = true;
  }
}

/**
 * Redeem the launcher's bootstrap code, if the URL carries one.
 *
 * Must run before the first API call — settings and shell profiles are fetched
 * during startup and need the session cookie to already exist.
 *
 * Nothing is persisted on the client (security review SR-04).
 * The token used to be written to `localStorage` and mirrored into a
 * script-readable cookie; authentication now lives entirely in an `HttpOnly`
 * cookie the page cannot read, which also means a reload re-uses the session
 * without the page ever having held a credential.
 *
 * A failed exchange is not an error here: a reload that still carries the
 * original URL presents an already-spent code while its session cookie is
 * perfectly good. Whether the app is actually authenticated is decided by the
 * connect attempt in [`initAuth`].
 */
export async function establishSession(): Promise<void> {
  const bootstrap = new URLSearchParams(window.location.search).get("bootstrap");
  if (!bootstrap) return;

  await exchangeBootstrap(bootstrap);
  clearCredentialFromUrl();
}

/** Wire socket callbacks and connect. */
export function initAuth(): void {
  socket.onConnected = () => {
    authState = "connected";
    authError = false;
  };
  socket.onAuthError = () => {
    authState = "needsToken";
    authError = true;
  };
  socket.onDisconnected = () => {
    authState = "reconnecting";
  };

  void socket.connect();
}
