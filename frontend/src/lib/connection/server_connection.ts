/**
 * A connection to one Splitty server.
 *
 * It owns everything that speaks to that server: the socket, the mux client,
 * the attach registry, the session client and the server-command handlers.
 * Nothing in it is shared with another connection, so ids a server sends
 * (screens, panes, sessions) are only ever resolved against that server's
 * objects. The local server, the one that served the page, is the connection
 * `local`.
 */

import type { ConnSocket } from "../protocol/socket.ts";
import type { ClientCommand } from "../protocol/types.ts";
import { MuxClient } from "../mux/mux_client.ts";
import { AttachRegistry } from "../terminal/attach_registry.ts";
import { SessionClient } from "../terminal/session_client.ts";
import { Scratchpads } from "../scratchpad.ts";
import { parseCommand } from "./routing.ts";

/** `local`, or `host:<target>` for a connection to another host. */
export type ConnKey = "local" | `host:${string}`;
export type ConnKind = "local" | "remote";

/** The key of the connection to `target`. */
export function hostKey(target: string): Exclude<ConnKey, "local"> {
  return `host:${target}`;
}

/** The target of a host's connection, or undefined for the local one. */
export function hostOf(key: ConnKey): string | undefined {
  return key === "local" ? undefined : key.slice("host:".length);
}

export class ServerConnection {
  readonly kind: ConnKind;
  readonly mux: MuxClient;
  readonly attachments: AttachRegistry;
  readonly sessions: SessionClient;
  /** Its sessions' scratchpads, fed by its snapshots (`session-scratchpad`). */
  readonly scratchpads: Scratchpads;
  private readonly commandHandlers = new Set<(command: ClientCommand) => void>();
  private unsubscribe: () => void;

  constructor(
    readonly key: ConnKey,
    /** How the UI names the server, e.g. a host. */
    readonly label: string,
    readonly socket: ConnSocket,
  ) {
    this.kind = key === "local" ? "local" : "remote";
    this.mux = new MuxClient(socket);
    this.attachments = new AttachRegistry(socket);
    this.sessions = new SessionClient(socket, this.attachments);
    this.scratchpads = new Scratchpads((screenId, sessionId, text) =>
      this.mux.sendOp(screenId, { type: "set_scratchpad", sessionId, text }));
    this.unsubscribe = socket.onMessage((msg) => {
      if (msg.type !== "command") return;
      const command = parseCommand(msg.command);
      if (!command) return;
      for (const handler of [...this.commandHandlers]) handler(command);
    });
  }

  /**
   * Server commands (mux-protocol, Server Commands) from this server. Where
   * they may act is decided by `resolveCommandTarget`.
   */
  onCommand(handler: (command: ClientCommand) => void): () => void {
    this.commandHandlers.add(handler);
    return () => this.commandHandlers.delete(handler);
  }

  /** Stop listening to the socket. The socket itself belongs to whoever opened it. */
  dispose(): void {
    this.unsubscribe();
    this.commandHandlers.clear();
    this.mux.dispose();
    this.sessions.dispose();
    this.attachments.dispose();
  }
}
