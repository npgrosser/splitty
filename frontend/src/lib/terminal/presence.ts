/**
 * Tells each server when this window's user acts and when the window is not in
 * use, for the server's PTY size policy (terminal-sessions, Client Activity).
 *
 * Only the user's own actions count: a key, a pointer button, focusing or
 * resizing the window. Scrolling and pointer movement only read. An action goes
 * to the connection of the active screen, at most every
 * {@link ACTIVITY_INTERVAL_MS}; a hidden window is idle on every connection.
 * Each welcome of a server that decides sizes gets the window's state before
 * anything is attached, so a window nobody touched (opened, reconnected) never
 * takes the size from a client in use.
 */

import type { ServerConnection } from "../connection/server_connection.ts";

/** At most one `activity` per connection in this time; servers wait minutes before a client is idle. */
export const ACTIVITY_INTERVAL_MS = 5000;

/** What the tracker asks the window. */
export interface PresenceWindow {
  visible(): boolean;
  focused(): boolean;
}

export class Presence {
  /** When each connection last got an `activity`; none since its last `idle`. */
  private readonly lastSent = new Map<ServerConnection, number>();

  constructor(
    private readonly activeConnection: () => ServerConnection | undefined,
    private readonly window: PresenceWindow,
    private readonly now: () => number = () => performance.now(),
  ) {}

  /** Greet `conn` now if its server decides sizes, and after each of its welcomes. Returns unsubscribe. */
  watch(conn: ServerConnection): () => void {
    if (conn.sessions.sizing) this.greet(conn);
    const unsubscribe = conn.socket.onMessage((msg) => {
      // The connection's session client has read the welcome's capabilities already.
      if (msg.type === "mux.welcome" && conn.sessions.sizing) this.greet(conn);
    });
    return () => {
      unsubscribe();
      this.lastSent.delete(conn);
    };
  }

  /** The user acted in the window. */
  acted(): void {
    const conn = this.activeConnection();
    if (!conn?.sessions.sizing) return;
    const last = this.lastSent.get(conn);
    if (last !== undefined && this.now() - last < ACTIVITY_INTERVAL_MS) return;
    this.sendActivity(conn);
  }

  /** The window was hidden. */
  hidden(connections: Iterable<ServerConnection>): void {
    for (const conn of connections) {
      if (conn.sessions.sizing) this.sendIdle(conn);
    }
  }

  private greet(conn: ServerConnection): void {
    if (this.window.visible() && this.window.focused() && this.activeConnection() === conn) {
      this.sendActivity(conn);
    } else {
      this.sendIdle(conn);
    }
  }

  private sendActivity(conn: ServerConnection): void {
    conn.socket.send({ type: "activity" });
    this.lastSent.set(conn, this.now());
  }

  private sendIdle(conn: ServerConnection): void {
    conn.socket.send({ type: "idle" });
    this.lastSent.delete(conn);
  }
}

/** Feed `presence` from this window's events. Returns cleanup. */
export function watchWindowPresence(presence: Presence, connections: () => Iterable<ServerConnection>): () => void {
  const acted = () => presence.acted();
  const onVisibility = () => {
    if (document.visibilityState === "hidden") presence.hidden(connections());
  };
  const options = { capture: true, passive: true };
  window.addEventListener("keydown", acted, options);
  window.addEventListener("pointerdown", acted, options);
  window.addEventListener("focus", acted);
  window.addEventListener("resize", acted);
  document.addEventListener("visibilitychange", onVisibility);
  return () => {
    window.removeEventListener("keydown", acted, options);
    window.removeEventListener("pointerdown", acted, options);
    window.removeEventListener("focus", acted);
    window.removeEventListener("resize", acted);
    document.removeEventListener("visibilitychange", onVisibility);
  };
}
