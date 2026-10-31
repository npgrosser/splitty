/**
 * RemoteScreenSync — what a host's welcome, snapshots and nudges do to the
 * window's screens of that host (`remote-screens`).
 *
 * The host daemon is the source of truth for its screens, and a screen of the
 * window is only a reference to one of them. Unlike the local server's
 * `ScreenStartup`, this policy never creates a screen of the window from a
 * welcome or a snapshot, and it has no placeholders: the window shows a host's
 * screen only when the user created or opened it. Wire ids are resolved only
 * among this connection's own screens.
 *
 * - Every welcome (the first, and the one after each reconnect) resyncs the
 *   open screens the host lists; an open screen it no longer lists is flagged
 *   as missing on the host, as is a screen bound later that it did not list
 *   (a remembered screen).
 * - A screen the user creates gets its `create_screen` exactly once: after the
 *   welcome, once its view is ready and the reopen offer was answered (or
 *   there was nothing to offer). A reconnect never sends it again.
 * - The host's screens the window does not show stay available, for the
 *   reopen offer: the ones the last `remote.ready` and welcome listed, and the
 *   ones detached since.
 */

import type { ScreenInfo, ScreenState } from "../protocol/types.ts";
import type { HostScreen } from "../remote/remote_socket.ts";
import type { ScreenSyncPolicy } from "../connection/screen_sync.ts";
import { resolveScreenKey, type ScreenRef } from "../connection/routing.ts";
import type { ServerConnection } from "../connection/server_connection.ts";

/** A screen of the window as the sync sees it. */
export interface RemoteScreenRef extends ScreenRef {
  readonly name: string;
}

export interface RemoteSyncEffects {
  /** The window's screens (of any connection; only this connection's are used). */
  screens(): readonly RemoteScreenRef[];
  /** Apply a snapshot to the screen with the window key `key`, or queue it until its view is ready. */
  snapshot(key: string, state: ScreenState): void;
  /** Add the first pane of the new screen `key` and send its `create_screen`. */
  startFresh(key: string): void;
  /** What the UI shows changed: missing flags, pending choices, available screens. */
  changed(): void;
}

interface PendingScreen {
  viewReady: boolean;
  /** The user answered the reopen offer with "create a new screen". */
  decided: boolean;
}

export class RemoteScreenSync implements ScreenSyncPolicy {
  /** A welcome arrived on the current connection. */
  private welcomed = false;
  /** Distinguish a verified empty directory from one we have never fetched. */
  private observed = false;
  /** The host's screens as far as this window knows them, by wire id. */
  private readonly known = new Map<string, HostScreen>();
  /** New screens of the window whose `create_screen` was not sent yet, by key. */
  private readonly pending = new Map<string, PendingScreen>();
  /** Keys of open screens the host no longer lists. */
  private readonly missing = new Set<string>();
  /** Wire ids resynced since the last welcome; a view that becomes ready later needs no second one. */
  private readonly resynced = new Set<string>();
  private readonly unsubscribe: () => void;

  constructor(
    private readonly conn: ServerConnection,
    private readonly fx: RemoteSyncEffects,
  ) {
    this.unsubscribe = conn.socket.onDisconnect(() => { this.welcomed = false; });
  }

  // ── Host Messages ──

  /** `remote.ready`: the host's screens with their pane counts. */
  ready(screens: readonly HostScreen[]): void {
    this.observed = true;
    this.known.clear();
    for (const s of screens) this.known.set(s.id, { ...s });
    this.fx.changed();
  }

  welcome(screens: ScreenInfo[]): void {
    this.observed = true;
    this.welcomed = true;
    const listed = new Set(screens.map((s) => s.id));
    // The welcome's list is the host's current one; keep the pane counts the
    // ready message gave.
    for (const id of [...this.known.keys()]) if (!listed.has(id)) this.known.delete(id);
    for (const s of screens) {
      const name = s.name || this.known.get(s.id)?.name || s.id;
      this.known.set(s.id, { id: s.id, name, panes: this.known.get(s.id)?.panes ?? 0 });
    }

    this.resynced.clear();
    for (const s of this.own()) {
      if (this.pending.has(s.id)) continue;
      if (listed.has(s.wireId)) {
        this.missing.delete(s.id);
        this.resync(s.wireId);
      } else {
        this.missing.add(s.id);
      }
    }
    for (const key of [...this.pending.keys()]) this.maybeCreate(key);
    this.fx.changed();
  }

  snapshot(state: ScreenState): void {
    const key = resolveScreenKey(this.conn.key, state.id, this.fx.screens());
    if (!key) return;
    this.missing.delete(key);
    this.known.set(state.id, { id: state.id, name: state.name || this.known.get(state.id)?.name || state.id,
      panes: Object.keys(state.panes).length });
    this.fx.snapshot(key, state);
    this.fx.changed();
  }

  nudge(wireId?: string): void {
    if (wireId && resolveScreenKey(this.conn.key, wireId, this.fx.screens())) this.resync(wireId);
  }

  // ── Window Screens ──

  /**
   * The host is about to have the screen `wireId` (a pane moves into it): a
   * view of it is built from its snapshot and never flagged missing.
   */
  adopt(wireId: string, name: string): void {
    this.known.set(wireId, { id: wireId, name, panes: 1 });
    this.fx.changed();
  }

  /** The user created the screen `key` on this host; its `create_screen` follows. */
  addNew(key: string): void {
    this.pending.set(key, { viewReady: false, decided: false });
    this.fx.changed();
  }

  /** The view of the screen `key` is mounted and has no snapshot yet. */
  viewReady(key: string): void {
    const entry = this.pending.get(key);
    if (entry) {
      entry.viewReady = true;
      this.maybeCreate(key);
      return;
    }
    // An opened screen: built from its snapshot. Before the welcome, the
    // welcome resyncs it. A screen bound after the welcome that the host did
    // not list (a remembered screen restored late) is flagged as missing
    // until a snapshot shows it.
    const screen = this.own().find((s) => s.id === key);
    if (!screen || !this.welcomed) return;
    if (!this.known.has(screen.wireId) && !this.missing.has(key)) {
      this.missing.add(key);
      this.fx.changed();
    }
    if (!this.resynced.has(screen.wireId)) this.resync(screen.wireId);
  }

  /** The user answered the reopen offer of the new screen `key` with "create a new screen". */
  decide(key: string): void {
    const entry = this.pending.get(key);
    if (!entry) return;
    entry.decided = true;
    this.maybeCreate(key);
    this.fx.changed();
  }

  /**
   * The window's screens changed. A new screen whose offer lost its last
   * entry (opened from another new screen meanwhile) is created now.
   */
  refresh(): void {
    for (const key of [...this.pending.keys()]) this.maybeCreate(key);
  }

  /** The screen `key` detached: it keeps running on the host, with `panes` panes. */
  detached(key: string, panes: number): void {
    const screen = this.own().find((s) => s.id === key);
    if (screen && !this.pending.has(key) && !this.missing.has(key)) {
      this.known.set(screen.wireId, { id: screen.wireId, name: screen.name || screen.wireId, panes });
    }
    this.forget(key);
  }

  /** The screen with the wire id `wireId` was terminated on the host. */
  terminated(wireId: string): void {
    this.known.delete(wireId);
    this.fx.changed();
  }

  /** The screen `key` left the window. */
  forget(key: string): void {
    const screen = this.own().find((s) => s.id === key);
    if (screen) this.resynced.delete(screen.wireId);
    this.pending.delete(key);
    this.missing.delete(key);
    this.fx.changed();
  }

  // ── What the UI Shows ──

  isMissing(key: string): boolean {
    return this.missing.has(key);
  }

  /** Whether the new screen `key` waits for the user's answer to the reopen offer. */
  needsChoice(key: string): boolean {
    const entry = this.pending.get(key);
    return !!entry && !entry.decided && this.welcomed && this.available().length > 0;
  }

  /** Whether the screen `key` was created here and its `create_screen` is not sent yet. */
  isPending(key: string): boolean {
    return this.pending.has(key);
  }

  /** The host's screens the window does not show, for the reopen offer. */
  available(): HostScreen[] {
    const shown = new Set(this.own().map((s) => s.wireId));
    return [...this.known.values()].filter((s) => !shown.has(s.id));
  }

  /** All observed screens, including the ones open here; never synthesize a directory before ready. */
  catalog(): HostScreen[] | undefined {
    return this.observed ? [...this.known.values()].map((s) => ({ ...s })) : undefined;
  }

  /** Local rename/layout changes do not echo as snapshots to their sender. */
  updateOpen(key: string, panes?: number): void {
    const screen = this.own().find((s) => s.id === key);
    if (!screen || this.missing.has(key) || this.pending.has(key)) return;
    const old = this.known.get(screen.wireId);
    if (!old) return;
    const name = screen.name || old.name;
    const count = panes ?? old.panes;
    if (old.name === name && old.panes === count) return;
    this.known.set(screen.wireId, { id: screen.wireId, name, panes: count });
    this.fx.changed();
  }

  dispose(): void {
    this.unsubscribe();
  }

  // ── Internals ──

  private own(): RemoteScreenRef[] {
    return this.fx.screens().filter((s) => s.conn === this.conn.key);
  }

  private resync(wireId: string): void {
    this.resynced.add(wireId);
    this.conn.mux.resync(wireId);
  }

  private maybeCreate(key: string): void {
    const entry = this.pending.get(key);
    if (!entry || !entry.viewReady || !this.welcomed) return;
    if (!entry.decided && this.available().length > 0) return;
    const screen = this.own().find((s) => s.id === key);
    this.pending.delete(key);
    if (!screen) return;
    this.known.set(screen.wireId, { id: screen.wireId, name: screen.name || screen.wireId, panes: 1 });
    this.fx.startFresh(key);
    this.fx.changed();
  }
}
