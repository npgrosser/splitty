/**
 * Screen sync — what a connection's welcome, snapshots and nudges do to the
 * window's screens.
 *
 * Each connection gets its own policy. The local server's screens follow
 * `ScreenStartup`, which may create and drop local screens. A host's
 * connection follows `RemoteScreenSync` (`lib/mux/remote_screen_sync.ts`),
 * which only touches screens already bound to it: its wire ids are resolved
 * among its own screens, and a message naming anything else is dropped.
 * `ScreenViews` keeps the layouts of all screens and makes sure a snapshot is
 * applied only to a screen of the connection it came from.
 */

import type { DockviewApi } from "dockview-core";
import type { ScreenInfo, ScreenState } from "../protocol/types.ts";
import type { ScreenStartup } from "../mux/screen_startup.ts";
import type { ScreenMuxHooks } from "../../stores/screens.svelte.ts";
import { registerScreenApi, ScreenBinding, unregisterScreenApi } from "./screen_binding.ts";
import type { ScreenRef } from "./routing.ts";
import type { ConnKey, ServerConnection } from "./server_connection.ts";

export interface ScreenSyncPolicy {
  welcome(screens: ScreenInfo[]): void;
  snapshot(state: ScreenState): void;
  nudge(wireId?: string): void;
}

/** Hand a connection's mux events to its policy. Returns a reset. */
export function connectScreenSync(conn: ServerConnection, policy: ScreenSyncPolicy): () => void {
  return conn.mux.setHandlers({
    onWelcome: (_clientId, screens) => policy.welcome(screens),
    onSnapshot: (state) => {
      conn.scratchpads.snapshot(state);
      policy.snapshot(state);
    },
    onNudge: (_reason, wireId) => policy.nudge(wireId),
  });
}

/** The local server: its screen keys are its screen ids, `ScreenStartup` decides what exists. */
export class LocalScreenSync implements ScreenSyncPolicy {
  constructor(
    private readonly conn: ServerConnection,
    private readonly startup: ScreenStartup,
    private readonly views: ScreenViews,
    /** After every welcome, once `ScreenStartup` has settled the local screens. */
    private readonly afterWelcome?: () => void,
  ) {}

  welcome(screens: ScreenInfo[]): void {
    // Resyncs every listed screen — on reconnect, existing screens need fresh
    // snapshots to re-establish terminal output streams via attach().
    this.startup.welcome(screens);
    this.afterWelcome?.();
  }

  snapshot(state: ScreenState): void {
    // Queued when the local screen does not exist yet, so its view finds it on mount.
    this.views.snapshot(this.conn.key, state.id, state);
    this.startup.snapshot(state.id, state.name);
  }

  nudge(wireId?: string): void {
    this.startup.nudge(wireId);
  }
}

/**
 * The layouts of the window's screens, their bindings, and snapshots waiting
 * for a layout that is not mounted yet.
 */
export class ScreenViews {
  private readonly apis = new Map<string, DockviewApi>();
  private readonly bindings = new Map<string, ScreenBinding>();
  private readonly pending = new Map<string, { conn: ConnKey; state: ScreenState }>();
  /** Per screen key, a pane moving in that its first snapshot shows in front. */
  private readonly reveals = new Map<string, string>();

  constructor(
    private readonly apply: (api: DockviewApi, binding: ScreenBinding, state: ScreenState) => void,
  ) {}

  /**
   * The binding of a screen, created on first use. Undefined when the
   * screen's connection is gone: such a screen is not mounted.
   */
  bind(screen: ScreenRef, conn: ServerConnection | undefined, active: boolean): ScreenBinding | undefined {
    const existing = this.bindings.get(screen.id);
    if (existing) return existing.conn.key === screen.conn ? existing : undefined;
    if (!conn || conn.key !== screen.conn) return undefined;
    const binding = new ScreenBinding(screen.id, conn, screen.wireId, { active });
    this.bindings.set(screen.id, binding);
    return binding;
  }

  binding(key: string): ScreenBinding | undefined {
    return this.bindings.get(key);
  }

  api(key: string): DockviewApi | undefined {
    return this.apis.get(key);
  }

  /** Mounted layouts by screen key. */
  get layouts(): ReadonlyMap<string, DockviewApi> {
    return this.apis;
  }

  /**
   * A bound screen's layout is mounted. Applies a snapshot queued for it and
   * returns whether there was one; without one the screen still needs content.
   */
  ready(key: string, api: DockviewApi): boolean {
    const binding = this.bindings.get(key);
    if (!binding) return false;
    this.apis.set(key, api);
    registerScreenApi(api, binding);
    const pending = this.pending.get(key);
    this.pending.delete(key);
    if (!pending || pending.conn !== binding.conn.key) return false;
    this.applyTo(key, api, binding, pending.state);
    return true;
  }

  /**
   * A snapshot from `conn` for the screen `key`: applied when its layout is
   * mounted, queued until then. Never applied to another connection's screen.
   */
  snapshot(conn: ConnKey, key: string, state: ScreenState): void {
    const binding = this.bindings.get(key);
    if (binding && binding.conn.key !== conn) return;
    const api = this.apis.get(key);
    if (binding && api) this.applyTo(key, api, binding, state);
    else this.pending.set(key, { conn, state });
  }

  /**
   * The pane `paneId` moves into the screen `key` at this window's request:
   * the first snapshot that holds it makes it its group's active tab, which a
   * snapshot otherwise leaves as the window had it.
   */
  reveal(key: string, paneId: string): void {
    this.reveals.set(key, paneId);
  }

  private applyTo(key: string, api: DockviewApi, binding: ScreenBinding, state: ScreenState): void {
    this.apply(api, binding, state);
    const paneId = this.reveals.get(key);
    if (!paneId || !state.panes[paneId]) return;
    this.reveals.delete(key);
    api.getPanel(paneId)?.api.setActive();
  }

  /** The screen is gone: its binding sends nothing any more. */
  remove(key: string): void {
    this.bindings.get(key)?.dispose();
    const api = this.apis.get(key);
    if (api) unregisterScreenApi(api);
    this.bindings.delete(key);
    this.apis.delete(key);
    this.pending.delete(key);
    this.reveals.delete(key);
  }

  clear(): void {
    for (const key of [...this.bindings.keys()]) this.remove(key);
    this.pending.clear();
    this.reveals.clear();
  }
}

/**
 * What the screen store's rename, switch and reorder send: each to the
 * screen's own connection. The local server orders only its own screens; where
 * another server's screens sit in this window is not that server's business.
 */
export function screenMuxHooks(connectionFor: (key: ConnKey) => ServerConnection | undefined): ScreenMuxHooks {
  return {
    onRename: (screen, name) => {
      connectionFor(screen.conn)?.mux.sendOp(screen.wireId, { type: "rename_screen", screenId: screen.wireId, name });
    },
    onSetActive: (screen) => {
      connectionFor(screen.conn)?.mux.sendOp(screen.wireId, { type: "set_active_screen", screenId: screen.wireId });
    },
    onReorder: (screens) => {
      const screenOrder = screens.filter((s) => s.conn === "local").map((s) => s.wireId);
      connectionFor("local")?.mux.sendOp("_", { type: "reorder_screens", screenOrder });
    },
  };
}
