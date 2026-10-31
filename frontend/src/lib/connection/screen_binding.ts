/**
 * ScreenBinding — a screen of the window bound to the connection it belongs to.
 *
 * Every pane, tab, context menu and lookup finds its connection through the
 * binding of its screen, which it reaches from dockview's layout API
 * (`bindingForApi`), never through a global table. A pane whose layout has no
 * binding is not mounted.
 *
 * The binding also holds what used to be global and is per screen: the flag
 * that stops layout ops while a snapshot of the screen is applied, and the
 * mapping of dockview's floating groups to the server's floating entries.
 */

import type { DockviewApi } from "dockview-core";
import type { Arrangement, MuxOperation } from "../protocol/types.ts";
import { arrangementSlots, gridArrangement, sameArrangement, withActivePane } from "../../layout/arrangement.ts";
import type { TerminalSession } from "../terminal/terminal_session.ts";
import type { ServerConnection } from "./server_connection.ts";

/**
 * What removing a screen's layout does to its sessions: `close` ends them (the
 * local server's screens), `detach` only lets go of them, so they keep running
 * on their server (a collapsed remote screen, `remote-screens`).
 */
export type Teardown = "close" | "detach";

export class ScreenBinding {
  /** No layout ops are sent while a snapshot of this screen is being applied. */
  suppressOps = false;
  /** Snapshot construction must not redirect keyboard focus to a background pane. */
  suppressFocus = false;
  /**
   * The grid as the server last had it: from the last applied snapshot, or
   * the last `arrange_panes` sent. A gesture that leaves it as it is sends nothing.
   */
  arrangement: Arrangement | undefined;
  /**
   * The panes the server has as the active tab of their slot, as far as this
   * window knows from the last applied snapshot and the ops it sent since.
   */
  readonly activeTabs = new Set<string>();
  readonly teardown: Teardown;
  private active: boolean;
  private readonly activateListeners = new Set<() => void>();
  private readonly deactivateListeners = new Set<() => void>();
  /** Dockview group id → the server's floating entry id. */
  private readonly floatingIds = new Map<string, string>();
  /** Panes and sessions moving to another screen: removing them here only lets go. */
  private readonly handedOverPanes = new Set<string>();
  private readonly handedOverSessions = new Set<string>();
  private disposed = false;

  constructor(
    /** The screen's key in the window. */
    readonly key: string,
    readonly conn: ServerConnection,
    /** The screen's id on its server. */
    readonly wireId: string,
    options?: { active?: boolean; teardown?: Teardown },
  ) {
    this.active = options?.active ?? false;
    this.teardown = options?.teardown ?? (conn.kind === "local" ? "close" : "detach");
  }

  // ── Mux ──

  /** Send a layout op for this screen; dropped during a snapshot rebuild and after dispose. */
  sendOp(op: MuxOperation): void {
    if (this.disposed || this.suppressOps) return;
    this.conn.mux.sendOp(this.wireId, op);
  }

  resync(): void {
    if (this.disposed) return;
    this.conn.mux.resync(this.wireId);
  }

  /** The server has `active` as the active tab of the slot that holds `paneIds`. */
  noteActiveTab(paneIds: readonly string[], active: string | undefined): void {
    for (const id of paneIds) this.activeTabs.delete(id);
    if (active) this.activeTabs.add(active);
  }

  // ── Sessions ──

  sessionForPane(paneId: string): TerminalSession | undefined {
    return this.conn.sessions.forPane(paneId);
  }

  linkPane(paneId: string, sessionId: string): void {
    this.conn.sessions.linkPane(paneId, sessionId, this.key);
  }

  unlinkPane(paneId: string): void {
    this.conn.sessions.unlinkPane(paneId);
  }

  /**
   * A pane of this screen goes away: end its session or only let go of it,
   * per {@link teardown}; a session handed over to another screen is only let go of.
   */
  release(session: Pick<TerminalSession, "id" | "dispose" | "detach">): void {
    if (this.handedOverSessions.delete(session.id) || this.teardown === "detach") session.detach();
    else session.dispose();
  }

  /**
   * The pane `paneId` and its session move to another screen
   * (`move_pane_to_screen`): removing its panel here sends no `close_pane`
   * and lets go of the session instead of ending it.
   */
  handOver(paneId: string, sessionId: string | undefined): void {
    this.handedOverPanes.add(paneId);
    if (sessionId) this.handedOverSessions.add(sessionId);
  }

  /** Whether the removal of `paneId` is its hand-over; true once. */
  takeHandedOver(paneId: string): boolean {
    return this.handedOverPanes.delete(paneId);
  }

  // ── Active Screen ──

  /** Whether this is the window's active screen; only its terminals take focus. */
  isActive(): boolean {
    return this.active;
  }

  setActive(active: boolean): void {
    if (active === this.active) return;
    this.active = active;
    for (const cb of [...(active ? this.activateListeners : this.deactivateListeners)]) cb();
  }

  /** Called when the screen becomes the active one. Returns unsubscribe. */
  onActivate(cb: () => void): () => void {
    this.activateListeners.add(cb);
    return () => this.activateListeners.delete(cb);
  }

  /** Called when another screen becomes the active one. Returns unsubscribe. */
  onDeactivate(cb: () => void): () => void {
    this.deactivateListeners.add(cb);
    return () => this.deactivateListeners.delete(cb);
  }

  // ── Floating Panes ──

  registerFloatingId(groupId: string, floatingId: string): void {
    this.floatingIds.set(groupId, floatingId);
  }

  unregisterFloatingId(groupId: string): void {
    this.floatingIds.delete(groupId);
  }

  floatingId(groupId: string): string | undefined {
    return this.floatingIds.get(groupId);
  }

  clearFloatingIds(): void {
    this.floatingIds.clear();
  }

  /** The screen is going away: nothing is sent for it from now on. */
  dispose(): void {
    this.disposed = true;
    this.active = false;
    this.activateListeners.clear();
    this.deactivateListeners.clear();
  }
}

// ── Layout API → Binding ──
// Every pane-creation path and renderer reaches its screen through the
// dockview API it was created in. A pane the server never learned about breaks
// every later op that references it ("pane X not found").
//
// Keyed by the layout's id, not by the API object: dockview hands group header
// actions and the watermark API wrappers of their own, so only the id is the
// same everywhere.

const layoutBindings = new Map<string, ScreenBinding>();

export function registerScreenApi(api: DockviewApi, binding: ScreenBinding): void {
  layoutBindings.set(api.id, binding);
}

export function unregisterScreenApi(api: DockviewApi): void {
  layoutBindings.delete(api.id);
}

export function bindingForApi(api: DockviewApi): ScreenBinding | undefined {
  return layoutBindings.get(api.id);
}

/** The session shown in `paneId` of the screen whose layout is `api`. */
export function sessionForPane(api: DockviewApi, paneId: string): TerminalSession | undefined {
  return bindingForApi(api)?.sessionForPane(paneId);
}

// ── Dockview Bridge ──

/** How long after a drop or a divider drag the grid is read and sent. */
const ARRANGE_DELAY_MS = 100;

/**
 * Bridge a screen's DockviewApi to the mux backend.
 * Hooks into dockview events that can be reliably inferred:
 * - onDidRemovePanel → close_pane
 * - onDidMovePanel and the end of a divider drag on `element` → arrange_panes
 * - onDidActivePanelChange and onDidRemovePanel → select_tab
 */
export function bridgeDockviewToMux(binding: ScreenBinding, api: DockviewApi, element?: HTMLElement): () => void {
  const disposables: { dispose(): void }[] = [];

  // One report per gesture: the several moves of a group arrive together, and
  // the ops the float and dock buttons send in the same task go out first.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let moved = false;
  const schedule = (arranged: boolean) => {
    // A change while a snapshot is applied is the snapshot's, not the user's.
    if (binding.suppressOps) return;
    moved ||= arranged;
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      // The grid first: a tab dragged into another group is in its new slot then.
      if (moved) reportArrangement(binding, api);
      moved = false;
      reportActiveTabs(binding, api);
    }, ARRANGE_DELAY_MS);
  };
  disposables.push({ dispose: () => clearTimeout(timer) });

  disposables.push(
    api.onDidRemovePanel((e) => {
      // The server makes the first remaining tab active, dockview a neighbour.
      schedule(false);
      // A pane moving to another screen is not closed.
      if (binding.takeHandedOver(e.id)) return;
      binding.sendOp({ type: "close_pane", paneId: e.id });
    }),
  );
  disposables.push(api.onDidMovePanel(() => schedule(true)));
  disposables.push(api.onDidActivePanelChange(() => schedule(false)));

  // Dockview tells no one when a divider drag ends; its sashes are `.dv-sash`.
  if (element) {
    const doc = element.ownerDocument;
    const ended = () => {
      doc.removeEventListener("pointerup", ended);
      doc.removeEventListener("pointercancel", ended);
      schedule(true);
    };
    const pressed = (event: Event) => {
      const target = event.target as { closest?: (selector: string) => unknown } | null;
      if (!target?.closest?.(".dv-sash")) return;
      doc.addEventListener("pointerup", ended);
      doc.addEventListener("pointercancel", ended);
    };
    element.addEventListener("pointerdown", pressed, true);
    disposables.push({
      dispose: () => {
        element.removeEventListener("pointerdown", pressed, true);
        doc.removeEventListener("pointerup", ended);
        doc.removeEventListener("pointercancel", ended);
      },
    });
  }

  return () => {
    for (const d of disposables) d.dispose();
  };
}

/**
 * Send the screen's grid as `arrange_panes` when it is not what the server
 * has (mux-protocol, Window Layout Changes Reach the Server).
 */
function reportArrangement(binding: ScreenBinding, api: DockviewApi): void {
  if (binding.suppressOps || !binding.conn.mux.arrangements) return;
  // Reading a maximized grid leaves maximization for a moment, and a group in
  // a popout window is not part of the server's layout at all.
  if (api.hasMaximizedGroup() || api.groups.some((group) => group.api.location.type === "popout")) return;
  const root = gridArrangement(api.toJSON().grid);
  if (binding.arrangement && sameArrangement(root, binding.arrangement)) return;
  binding.arrangement = root;
  for (const slot of arrangementSlots(root)) binding.noteActiveTab(slot.paneIds, slot.activePaneId);
  binding.sendOp({ type: "arrange_panes", root });
}

/**
 * Send `select_tab` for every group whose shown tab the server does not have
 * as its slot's active one (mux-protocol, Selected Tabs Reach the Server).
 */
function reportActiveTabs(binding: ScreenBinding, api: DockviewApi): void {
  if (binding.suppressOps || !binding.conn.mux.tabSelections) return;
  for (const group of api.groups) {
    // A single tab is shown whichever the server names; the panes of a popout
    // group are still where the server had them, in another slot.
    if (group.panels.length < 2 || group.api.location.type === "popout") continue;
    const shown = group.activePanel?.id;
    if (!shown || binding.activeTabs.has(shown)) continue;
    binding.noteActiveTab(group.panels.map((panel) => panel.id), shown);
    if (binding.arrangement) binding.arrangement = withActivePane(binding.arrangement, shown);
    binding.sendOp({ type: "select_tab", paneId: shown });
  }
}
