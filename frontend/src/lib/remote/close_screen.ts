// Closing a remote screen (`remote-screens`, Collapse And Terminate).
//
// Detaching is the default: the screen leaves the window, its binding goes
// first so no layout op is sent for it, and its panes only detach (the
// binding's `detach` teardown). Its sessions keep running on the host, where
// the reopen offer finds the screen again. Terminating additionally sends the
// host one `close_screen`, which ends the screen's panes and their processes
// there. Either way the window keeps a screen: closing the last one creates a
// local screen first.

import type { ConnKey, ServerConnection } from "../connection/server_connection.ts";
import type { RemoteScreenSync } from "../mux/remote_screen_sync.ts";
import type { Screen } from "../../stores/screens.svelte.ts";

export interface CloseEffects {
  screens(): readonly Screen[];
  /** Create a local screen and make it the active one. */
  createLocalScreen(): void;
  /**
   * The screen leaves the window: its binding first, then its layout, whose
   * panes let go of their sessions as the binding's teardown says.
   */
  removeScreen(key: string): void;
  connection(key: ConnKey): ServerConnection | undefined;
  sync(key: ConnKey): RemoteScreenSync | undefined;
  /** How many panes the screen shows. */
  panes(key: string): number;
  /** No screen of the host is left in the window: its connection can go. */
  releaseHost(key: ConnKey): void;
}

/** Detach a remote screen: it leaves the window and keeps running on its host. */
export function detachRemoteScreen(key: string, fx: CloseEffects): void {
  const screen = remoteScreen(key, fx);
  if (!screen) return;
  fx.sync(screen.conn)?.detached(key, fx.panes(key));
  leave(screen, fx);
}

/**
 * Terminate a remote screen: its host ends its panes and their processes.
 * Returns false, and changes nothing, when the screen's connection is gone.
 */
export function terminateRemoteScreen(key: string, fx: CloseEffects): boolean {
  const screen = remoteScreen(key, fx);
  if (!screen) return false;
  const conn = fx.connection(screen.conn);
  if (!conn) return false;
  conn.mux.sendOp(screen.wireId, { type: "close_screen", screenId: screen.wireId });
  fx.sync(screen.conn)?.terminated(screen.wireId);
  leave(screen, fx);
  return true;
}

/**
 * Remove a remote screen the host never had or no longer has (a cancelled new
 * screen, a screen missing on the host); nothing is recorded for the reopen
 * offer.
 */
export function removeRemoteScreen(key: string, fx: CloseEffects): void {
  const screen = remoteScreen(key, fx);
  if (screen) leave(screen, fx);
}

function remoteScreen(key: string, fx: CloseEffects): Screen | undefined {
  const screen = fx.screens().find((s) => s.id === key);
  return screen && screen.conn !== "local" ? screen : undefined;
}

function leave(screen: Screen, fx: CloseEffects): void {
  if (fx.screens().length <= 1) fx.createLocalScreen();
  fx.sync(screen.conn)?.forget(screen.id);
  fx.removeScreen(screen.id);
  if (!fx.screens().some((s) => s.conn === screen.conn)) fx.releaseHost(screen.conn);
}

// ── First Terminate ──

const CONFIRMED_KEY = "splitty-remote-terminate-confirmed";

/**
 * Whether the user confirmed a terminate before (provisional, tbd: once per
 * browser profile). Without storage every terminate asks.
 */
export function terminateConfirmedBefore(storage: Pick<Storage, "getItem"> | undefined = safeStorage()): boolean {
  try {
    return storage?.getItem(CONFIRMED_KEY) === "true";
  } catch {
    return false;
  }
}

export function rememberTerminateConfirmed(storage: Pick<Storage, "setItem"> | undefined = safeStorage()): void {
  try {
    storage?.setItem(CONFIRMED_KEY, "true");
  } catch {
    // Not remembered: the next terminate asks again.
  }
}

function safeStorage(): Storage | undefined {
  try {
    return typeof localStorage === "undefined" ? undefined : localStorage;
  } catch {
    return undefined;
  }
}
