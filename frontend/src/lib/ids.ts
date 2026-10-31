/**
 * Client-generated identifiers.
 *
 * Pane and screen ids are chosen by the client and used verbatim by the server
 * (workspace-model, Identifier Assignment). They used to be counters that
 * restarted at 1 on every page load, so a reloaded client handed a long-lived
 * workspace ids it already held. Random ids make that practically impossible;
 * the server rejects a pane id that is still in use.
 *
 * `crypto.getRandomValues` rather than `crypto.randomUUID`: the latter only
 * exists in secure contexts, and the UI is also served over plain HTTP to
 * another machine (`--remote`).
 */

function randomHex(bytes: number): string {
  const buf = new Uint8Array(bytes);
  crypto.getRandomValues(buf);
  return Array.from(buf, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** A new terminal pane id. The `terminal-` prefix marks terminal panes (`context_menus.ts`). */
export function newPaneId(): string {
  return `terminal-${randomHex(8)}`;
}

/** A new id for a screen of the local server, which is also its key in the window. */
export function newScreenId(): string {
  return `screen-${randomHex(8)}`;
}

/**
 * A key for a screen of another server. Its id on that server is kept apart, so
 * nothing that server sends becomes a local identifier.
 */
export function newRemoteScreenKey(): string {
  return `rs-${randomHex(8)}`;
}
