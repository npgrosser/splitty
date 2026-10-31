/**
 * The routing rule: a message is resolved only against the objects of the
 * connection it arrived on.
 *
 * Ids a server sends are its own. Two servers may use the same screen, pane or
 * session ids, and a hostile one may pick a local id on purpose. Every lookup of
 * an incoming id therefore goes through its connection: sessions, attaches and
 * spawn waiters live per connection (`ServerConnection`), screens are found here
 * by connection and wire id, and server commands are confined to the screens of
 * the connection that sent them.
 */

import type { ClientCommand } from "../protocol/types.ts";
import type { ConnKey, ConnKind } from "./server_connection.ts";

/** A screen of the window: its key here, its connection, its id on that server. */
export interface ScreenRef {
  readonly id: string;
  readonly conn: ConnKey;
  readonly wireId: string;
}

/** The window key of `conn`'s screen with the server id `wireId`, searching only `conn`'s screens. */
export function resolveScreenKey(conn: ConnKey, wireId: string, screens: readonly ScreenRef[]): string | undefined {
  return screens.find((s) => s.conn === conn && s.wireId === wireId)?.id;
}

// ── Server Commands ──

/** A server command as it arrived, or null when it is not one this client knows. */
export function parseCommand(raw: unknown): ClientCommand | null {
  if (typeof raw !== "object" || raw === null) return null;
  const { type, paneId, direction } = raw as Record<string, unknown>;
  if (paneId !== undefined && typeof paneId !== "string") return null;
  switch (type) {
    case "pane.activate":
    case "pane.copy":
    case "pane.paste":
    case "pane.clear":
    case "pane.close":
      return { type, paneId };
    case "pane.split":
      if (direction !== "right" && direction !== "below") return null;
      return { type, paneId, direction };
    default:
      return null;
  }
}

export interface CommandPolicy {
  /**
   * Whether terminals may write the clipboard (`terminal.allowOsc52Write`);
   * `pane.copy` from a server other than the local one follows it.
   */
  allowClipboardWrite: boolean;
}

/**
 * Whether a command from a connection of `kind` may run at all. `pane.paste`
 * from a server other than the local one would paste the local clipboard into
 * that server's terminal, so it is refused; `pane.copy` writes the local
 * clipboard, so it follows the setting for terminal clipboard writes
 * (provisional, tbd: `connection-scoped-frontend`).
 */
export function commandAllowed(kind: ConnKind, command: ClientCommand, policy: CommandPolicy): boolean {
  if (kind === "local") return true;
  if (command.type === "pane.paste") return false;
  if (command.type === "pane.copy") return policy.allowClipboardWrite;
  return true;
}

/** What a command may see of the window. */
export interface CommandScope {
  screens: readonly ScreenRef[];
  activeKey: string;
  hasPane(screenKey: string, paneId: string): boolean;
  /** The active pane of a screen, if it has one. */
  activePane(screenKey: string): string | undefined;
}

export interface CommandTarget {
  screenKey: string;
  paneId: string;
}

/**
 * The pane a server command acts on, or null when it must be ignored
 * (mux-protocol, Server Commands): only panes of the sender's screens; without
 * a pane id, the active pane of the active screen if that screen is the
 * sender's. Resolving never changes the active screen; the caller acts on the
 * target where it is.
 */
export function resolveCommandTarget(
  origin: { key: ConnKey; kind: ConnKind },
  command: ClientCommand,
  scope: CommandScope,
  policy: CommandPolicy,
): CommandTarget | null {
  if (!commandAllowed(origin.kind, command, policy)) return null;
  const own = scope.screens.filter((s) => s.conn === origin.key);

  if (command.paneId !== undefined) {
    const paneId = command.paneId;
    const screen = own.find((s) => scope.hasPane(s.id, paneId));
    return screen ? { screenKey: screen.id, paneId } : null;
  }

  const active = own.find((s) => s.id === scope.activeKey);
  if (!active) return null;
  const paneId = scope.activePane(active.id);
  return paneId ? { screenKey: active.id, paneId } : null;
}
