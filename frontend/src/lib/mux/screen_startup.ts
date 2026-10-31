/**
 * ScreenStartup — decides how each local screen gets its content.
 *
 * The server's workspace is authoritative (mux-protocol, Client
 * Resynchronization). A screen the server knows is built from its
 * `mux.screen_snapshot`; only a screen the server does not know gets a first
 * pane and a `create_screen`. Sending `create_screen` for a screen the server
 * already has replaces it there, which used to throw away every running
 * session on a page reload.
 *
 * Until the first `mux.welcome` arrives nothing is known, so a screen whose
 * view is ready earlier only waits. The store's initial screens are
 * placeholders: when the welcome lists screens, placeholders it does not list
 * are dropped; when it lists none, they become the client's first screen.
 *
 * The welcome is tracked per page here; `mux_client` additionally holds back any
 * `create_screen` until the current connection's welcome. Screens the server
 * listed or sent a snapshot for stay known across reconnects, so none of them
 * is ever re-created.
 */

import type { ScreenInfo } from "../protocol/types.ts";

export interface StartupEffects {
  /** IDs of the local screens, in order. */
  localScreenIds(): string[];
  createLocalScreen(id: string, name: string): void;
  /** Drop a local screen the server never had; sends nothing. */
  removeLocalScreen(id: string): void;
  resync(screenId: string): void;
  /** Add the first pane to a screen the server does not know and send `create_screen`. */
  startFresh(screenId: string): void;
}

export class ScreenStartup {
  private welcomed = false;
  /** Screens the server has, from any welcome or snapshot. */
  private readonly known = new Set<string>();
  /** Views that became ready before the first welcome. */
  private readonly waiting = new Set<string>();
  private readonly placeholders: Set<string>;

  constructor(placeholders: string[], private readonly fx: StartupEffects) {
    this.placeholders = new Set(placeholders);
  }

  /** A screen's view is ready and no snapshot is queued for it. */
  viewReady(screenId: string): void {
    if (!this.welcomed) {
      this.waiting.add(screenId);
      return;
    }
    // A known screen is built by its snapshot, which may still be on its way.
    if (this.known.has(screenId)) return;
    this.fx.startFresh(screenId);
  }

  welcome(screens: ScreenInfo[]): void {
    const first = !this.welcomed;
    this.welcomed = true;
    for (const s of screens) this.known.add(s.id);

    for (const s of screens) {
      if (!this.fx.localScreenIds().includes(s.id)) {
        this.fx.createLocalScreen(s.id, s.name ?? s.id);
      }
      this.fx.resync(s.id);
    }

    if (!first) return;

    // Listed screens are in place now, so removing a placeholder never
    // leaves the store without a screen.
    if (screens.length > 0) {
      for (const id of this.placeholders) {
        if (!this.known.has(id) && this.fx.localScreenIds().includes(id)) {
          this.waiting.delete(id);
          this.fx.removeLocalScreen(id);
        }
      }
    }
    this.placeholders.clear();

    const waiting = [...this.waiting];
    this.waiting.clear();
    for (const id of waiting) {
      if (!this.known.has(id)) this.fx.startFresh(id);
    }
  }

  /**
   * The server is about to have this screen (a pane moves into it): its view
   * waits for its snapshot instead of starting fresh.
   */
  expect(screenId: string): void {
    this.known.add(screenId);
  }

  /** A snapshot arrived: the server has this screen. */
  snapshot(screenId: string, name?: string): void {
    this.known.add(screenId);
    if (!this.fx.localScreenIds().includes(screenId)) {
      this.fx.createLocalScreen(screenId, name ?? screenId);
    }
  }

  nudge(screenId?: string): void {
    if (screenId) this.fx.resync(screenId);
  }
}
