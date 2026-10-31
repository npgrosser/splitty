/**
 * MuxClient — one server connection's side of the mux protocol.
 *
 * Shadow Mode: The frontend works as before (dockview is source of truth).
 * Ops go out fire-and-forget to keep the server's model in sync.
 *
 * On connect, the server sends a `mux.welcome` with the screen list and client ID.
 * No explicit attach/detach — the client requests screen state via `mux.resync`
 * when it needs the full layout.
 *
 * Every connection has its own client, and a client only ever sees its own
 * socket's messages: a `mux.pane_spawned` resolves only this connection's
 * waiters, a welcome or snapshot reaches only this connection's handlers. Screen
 * ids here are the server's (wire ids); which screen of the window they mean is
 * decided by the screen sync (`lib/connection/screen_sync.ts`).
 */

import type { ConnSocket } from "../protocol/socket.ts";
import type { MuxOperation, ScreenInfo, ScreenState, OutgoingMessage } from "../protocol/types.ts";
import { screenList, screenState } from "./incoming.ts";

export interface MuxHandlers {
  onWelcome?: (clientId: string, screens: ScreenInfo[]) => void;
  onSnapshot?: (screen: ScreenState) => void;
  onNudge?: (reason: string, screenId?: string) => void;
}

interface PaneSpawnedWaiter {
  resolve: (sessionId: string) => void;
  reject: (err: Error) => void;
}

/** Rejection reason when no `mux.pane_spawned` arrives within the timeout. */
export class PaneSpawnedTimeoutError extends Error {
  constructor(paneId: string, timeoutMs: number) {
    super(`Timeout waiting for pane_spawned (pane=${paneId}, after ${timeoutMs}ms)`);
    this.name = "PaneSpawnedTimeoutError";
  }
}

/** Rejection reason when the server rejected the op that would spawn the pane. */
export class PaneSpawnRejectedError extends Error {
  constructor(paneId: string, op: string, error: string) {
    super(`Server rejected ${op} for pane ${paneId}: ${error}`);
    this.name = "PaneSpawnRejectedError";
  }
}

export class MuxClient {
  /** From the last `mux.welcome`. */
  clientId: string | undefined;
  screens: ScreenInfo[] = [];
  serverMode: "embedded" | "daemon" = "embedded";
  /** Whether the server keeps scratchpads (`session-scratchpad`, Scratchpad Capability). */
  scratchpads = false;
  /** Whether the server takes a window's grid as `arrange_panes` (mux-protocol, Window Layout Changes Reach the Server). */
  arrangements = false;
  /** Whether the server takes a selected tab as `select_tab` (mux-protocol, Selected Tabs Reach the Server). */
  tabSelections = false;

  // mux-protocol, Client Resynchronization: no `create_screen` before the
  // current connection's `mux.welcome`. Sent earlier, it reaches the server ahead
  // of everything the client could know about the server's screens, and used to
  // replace a screen the server already had. Held ops go out after the welcome.
  private welcomed = false;
  private readonly heldCreateScreens: { screenId: string; op: MuxOperation }[] = [];
  private handlers: MuxHandlers = {};
  private readonly waiters = new Map<string, PaneSpawnedWaiter>();
  private unsubscribes: (() => void)[];

  constructor(private readonly socket: ConnSocket) {
    this.unsubscribes = [
      socket.onMessage((msg) => this.handle(msg)),
      socket.onDisconnect(() => { this.welcomed = false; }),
    ];
  }

  /** Register the handlers for mux events, replacing earlier ones. Returns a reset. */
  setHandlers(handlers: MuxHandlers): () => void {
    this.handlers = handlers;
    return () => {
      if (this.handlers === handlers) this.handlers = {};
    };
  }

  /** Send a layout operation for a screen (fire-and-forget). */
  sendOp(screenId: string, op: MuxOperation): void {
    if (op.type === "create_screen" && !this.welcomed) {
      this.heldCreateScreens.push({ screenId, op });
      return;
    }
    this.socket.send({ type: "mux.op", screenId, op });
  }

  /** Request a full state resync for a screen. */
  resync(screenId: string): void {
    this.socket.send({ type: "mux.resync", screenId });
  }

  /**
   * Wait for a mux.pane_spawned event for a specific pane.
   * Resolves with the sessionId when the server spawns the PTY.
   * Rejects with a `PaneSpawnRejectedError` as soon as the server reports the op
   * failed, or with a `PaneSpawnedTimeoutError` after timeoutMs (default 8000ms).
   * Both are recoverable — the caller spawns the session itself instead.
   */
  waitForPaneSpawned(paneId: string, timeoutMs = 8000): Promise<string> {
    return new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters.delete(paneId);
        reject(new PaneSpawnedTimeoutError(paneId, timeoutMs));
      }, timeoutMs);

      const settle = (fn: () => void) => {
        clearTimeout(timer);
        this.waiters.delete(paneId);
        fn();
      };

      this.waiters.set(paneId, {
        resolve: (sessionId) => settle(() => resolve(sessionId)),
        reject: (err) => settle(() => reject(err)),
      });
    });
  }

  /** Stop listening; waiting spawns time out as they would without an answer. */
  dispose(): void {
    for (const unsubscribe of this.unsubscribes) unsubscribe();
    this.unsubscribes = [];
    this.handlers = {};
    this.heldCreateScreens.length = 0;
  }

  private handle(msg: OutgoingMessage): void {
    switch (msg.type) {
      case "mux.welcome": {
        const screens = screenList(msg.screens);
        this.clientId = typeof msg.clientId === "string" ? msg.clientId : undefined;
        this.screens = screens;
        this.serverMode = msg.serverMode === "daemon" ? "daemon" : "embedded";
        this.scratchpads = Array.isArray(msg.capabilities) && msg.capabilities.includes("scratchpad-v1");
        this.arrangements = Array.isArray(msg.capabilities) && msg.capabilities.includes("layout-arrange-v1");
        this.tabSelections = Array.isArray(msg.capabilities) && msg.capabilities.includes("tab-select-v1");
        this.welcomed = true;
        this.handlers.onWelcome?.(this.clientId ?? "", screens);
        for (const { screenId, op } of this.heldCreateScreens.splice(0)) {
          this.socket.send({ type: "mux.op", screenId, op });
        }
        break;
      }

      case "mux.screen_snapshot": {
        const screen = screenState(msg.screen);
        if (screen) this.handlers.onSnapshot?.(screen);
        break;
      }

      case "mux.nudge":
        this.handlers.onNudge?.(
          typeof msg.reason === "string" ? msg.reason : "",
          typeof msg.screenId === "string" ? msg.screenId : undefined,
        );
        break;

      case "mux.pane_spawned":
        if (typeof msg.paneId === "string" && typeof msg.sessionId === "string") {
          this.waiters.get(msg.paneId)?.resolve(msg.sessionId);
        }
        break;

      case "mux.op_failed":
        if (typeof msg.paneId === "string") {
          this.waiters
            .get(msg.paneId)
            ?.reject(new PaneSpawnRejectedError(msg.paneId, String(msg.op), String(msg.error)));
        }
        // The server kept its own layout: the screen shows it again.
        if (msg.op === "arrange_panes" && typeof msg.screenId === "string") this.resync(msg.screenId);
        break;
    }
  }
}

// ── Op builders ──

export function splitOp(
  paneId: string,
  newPaneId: string,
  direction: "right" | "below",
  cwd?: string,
  shellProfileId?: string,
): MuxOperation {
  const axis = direction === "right" ? "horizontal" : "vertical";
  return { type: "split_pane", paneId, axis, newPaneId, cwd, shellProfileId };
}

/** A new tab in the same slot as `paneId`. */
export function newTabOp(paneId: string, newPaneId: string, cwd?: string, shellProfileId?: string): MuxOperation {
  return { type: "new_tab", paneId, newPaneId, cwd, shellProfileId };
}

export function createScreenOp(screenId: string, name?: string, initialPaneId?: string, shellProfileId?: string): MuxOperation {
  return { type: "create_screen", screenId, name, newPaneId: initialPaneId, shellProfileId };
}
