/**
 * Applies a mux ScreenState snapshot to the dockview layout of its screen.
 *
 * A snapshot that the window can apply to the panels it shows is applied in
 * place: the viewer and job panes it adds or removes, a viewer's newer record,
 * the server's sizes. Every panel that stays keeps its renderer, and with it
 * its view, its terminal and the keyboard focus (`mux-protocol`, Job Snapshots
 * And Background Rendering). Any other snapshot rebuilds the layout from the
 * server's layout tree, and so does the first one on each connection: it
 * attaches the terminals again, as after reconnecting to a persistent server.
 */

import type { AddPanelOptions, DockviewApi, IDockviewPanel } from "dockview-core";
import type { Arrangement, LayoutNode, PaneState, ScreenState } from "../lib/protocol/types.ts";
import type { ScreenBinding } from "../lib/connection/screen_binding.ts";
import {
  VIEWER_COMPONENT, isViewerPanel, sameViewerSource, viewerPaneTitle, viewerPanelParams, viewerParams,
  type ViewerPanelParams,
} from "../lib/viewer/panel.ts";
import { dropViewerStates } from "../panes/viewer_view_state.ts";
import { jobParams, jobCanAttach } from "./job_snapshot.ts";
import { gridArrangement, layoutArrangement, sameShape, withoutPanes } from "./arrangement.ts";

/** The last snapshot applied to a layout, and the connection it came on (its client ID). */
const previousSnapshots = new WeakMap<DockviewApi, { readonly screen: ScreenState; readonly client: string | undefined }>();

type Direction = "left" | "right" | "above" | "below";

/**
 * The component, title and params of a snapshot's pane, by its role.
 *
 * A pane of the role `viewer` is a viewer pane, also from a server without
 * viewer panes or with a missing record: it then says that it cannot be shown,
 * and no terminal session is ever started for it (`viewer-panes`, Viewer
 * Capability). Every other pane is a terminal, whatever role it names: a host
 * of an earlier release may still send the removed role "webview", and
 * honouring it would rebuild an iframe pane (security review SR-01). A
 * terminal is the fail-safe direction; dockview has no "webview" component.
 */
function panelFor(binding: ScreenBinding, screen: ScreenState, paneId: string): { component: string; title: string; params: Record<string, unknown> } {
  const pane = screen.panes[paneId];
  if (pane?.role === "viewer") {
    return {
      component: VIEWER_COMPONENT,
      title: pane.title || viewerPaneTitle(pane.viewer),
      params: viewerPanelParams(pane, binding.conn.mux.viewers),
    };
  }
  const session = pane?.sessionId ? screen.sessions[pane.sessionId] : undefined;
  return {
    component: "terminal",
    title: pane?.title ?? "Terminal",
    params: {
      ...jobParams(screen, paneId),
      existingSessionId: pane?.sessionId,
      muxManaged: !pane?.sessionId,
      serverSpawnOnly: true,
      dir: pane?.cwd ?? session?.cwd,
      command: session?.command,
      shellProfileId: session?.shellProfileId,
    },
  };
}

/**
 * Viewer panes that the server brought to the front of their slot since the
 * last snapshot, and that this window shows in front of their group too: the
 * ones whose revision rose, as `open_viewer` showed their file again
 * (`viewer-panes`, Reopening A File), and the ones new since then that the
 * server made their slot's active pane (Opening A Viewer). `added` are the
 * panes the window has no panel of.
 */
function frontViewers(previous: ScreenState | undefined, screen: ScreenState, added: readonly string[]): string[] {
  if (!previous) return [];
  const reopened = Object.values(screen.panes).filter((pane) => {
    const before = previous.panes[pane.id]?.viewer?.revision;
    return pane.viewer !== undefined && before !== undefined && pane.viewer.revision > before;
  }).map((pane) => pane.id);
  const opened = added.filter((id) => !previous.panes[id] && screen.panes[id]?.role === "viewer" && activeInSlot(screen, id));
  return [...reopened, ...opened];
}

/**
 * Apply a ScreenState snapshot to the dockview instance of the screen bound by
 * `binding`: in place where it can be (`canPatch`), else by rebuilding the
 * layout from the server's layout tree. Layout ops of this screen are held
 * back meanwhile, so neither is echoed to the server; other screens are not
 * affected.
 */
export function applyScreenSnapshot(api: DockviewApi, binding: ScreenBinding, screen: ScreenState): void {
  const last = previousSnapshots.get(api);
  const previous = last?.screen;
  const client = binding.conn.mux.clientId;
  const oldPanels = [...api.panels];
  const active = api.activePanel?.id;
  const activeTabs = api.groups.map(group => group.activePanel?.id).filter((id): id is string => !!id);
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const oldIds = new Set(oldPanels.map(panel => panel.id));
  const added = Object.keys(screen.panes).filter(id => !oldIds.has(id));
  const removed = oldPanels.filter(p => !screen.panes[p.id]).map(p => p.id);
  const front = frontViewers(previous, screen, added);
  // A snapshot on another connection than the last one rebuilds: the terminals attach on this one.
  const inPlace = !!previous && client !== undefined && last?.client === client
    && canPatch(api, binding, previous, screen, oldPanels, added, removed);
  previousSnapshots.set(api, { screen, client });
  // What a later drag is compared with (mux-protocol, Window Layout Changes Reach the Server).
  binding.arrangement = layoutArrangement(screen.root);
  // What a later tab selection is compared with (mux-protocol, Selected Tabs Reach the Server).
  binding.activeTabs.clear();
  for (const node of [screen.root, ...(screen.floating ?? []).map(entry => entry.root)]) {
    forEachSlot(node, slot => binding.noteActiveTab(slot.paneIds ?? [], slot.activePaneId));
  }
  binding.suppressOps = true;
  binding.suppressFocus = true;
  try {
    if (inPlace) patchLayout(api, binding, previous, screen, front);
    else rebuildLayout(api, binding, previous, screen, { active, activeTabs, front });
  } finally {
    binding.suppressFocus = false;
    // A viewer pane the screen no longer holds keeps no view state.
    dropViewerStates(binding.key, id => !!screen.panes[id]);
    // Where the focused element stayed, it keeps the keyboard. A terminal that
    // was rebuilt takes it back once its session is attached, as the window's
    // active pane (`TerminalPaneRenderer`); keys typed until then are lost.
    if (focused?.isConnected && binding.isActive()) focused.focus({ preventScroll: true });
    requestAnimationFrame(() => { binding.suppressOps = false; });
  }
}

// ── In Place ──

/**
 * Whether `screen` can be applied to the window's panels in place: every pane
 * it adds or removes is a viewer pane or a job's pane; every pane that stays
 * is still the same kind of pane, a viewer of the same document and a
 * terminal of the same session; and the panes that stay are arranged as the
 * window has them, in the same rows, columns, tab groups, tab order and
 * floating groups. A viewer's origin and revision, the active tabs and the
 * sizes may differ.
 */
function canPatch(
  api: DockviewApi,
  binding: ScreenBinding,
  previous: ScreenState,
  screen: ScreenState,
  panels: readonly IDockviewPanel[],
  added: readonly string[],
  removed: readonly string[],
): boolean {
  if (!added.every(id => screen.panes[id].role === "viewer" || !!screen.panes[id].jobId)) return false;
  if (!removed.every(id => {
    const panel = panels.find(p => p.id === id);
    return isViewerPanel(panel) || !!previous.panes[id]?.jobId || !!(panel?.params as { jobId?: unknown } | undefined)?.jobId;
  })) return false;
  if (!panels.every(panel => !screen.panes[panel.id] || samePane(binding, previous, screen.panes[panel.id], panel))) return false;

  const layout = windowLayout(api) ?? snapshotLayout(previous);
  const omit = new Set([...added, ...removed, ...layout.popout]);
  if (!sameShape(withoutPanes(layout.grid, omit), withoutPanes(layoutArrangement(screen.root), omit))) return false;
  const groups = (lists: readonly (readonly string[])[]) => lists
    .map(ids => ids.filter(id => !omit.has(id)).join("\n"))
    .filter(Boolean)
    .sort()
    .join("\n\n");
  if (groups(layout.floating) !== groups((screen.floating ?? []).map(entry => slotPanes(entry.root)))) return false;
  // A floating group moved by another client is placed anew.
  return (screen.floating ?? []).every(entry => {
    const before = previous.floating?.find(other => other.id === entry.id)?.bounds;
    const after = entry.bounds;
    return !before || (before.x === after.x && before.y === after.y && before.w === after.w && before.h === after.h);
  });
}

/** Whether the panel can stay for `pane`: the same kind of pane, of the same document or session. */
function samePane(binding: ScreenBinding, previous: ScreenState, pane: PaneState, panel: IDockviewPanel): boolean {
  if (pane.role === "viewer") {
    if (!isViewerPanel(panel)) return false;
    const shown = viewerParams(panel.params);
    const next = viewerPanelParams(pane, binding.conn.mux.viewers).viewer;
    if (!shown || shown.supported !== next.supported || !shown.record !== !next.record) return false;
    return !next.record || sameViewerSource(shown.record?.source, next.record.source);
  }
  if (isViewerPanel(panel)) return false;
  const before = previous.panes[pane.id];
  if (before && (before.sessionId !== pane.sessionId || before.jobId !== pane.jobId)) return false;
  const linked = binding.sessionForPane(pane.id)?.id;
  return !linked || !pane.sessionId || linked === pane.sessionId;
}

/** The window's layout in the server's terms (`canPatch`). */
interface WindowLayout {
  readonly grid: Arrangement;
  /** The panes of each floating group, in tab order. */
  readonly floating: readonly (readonly string[])[];
  /** Panes in a popout window: the server keeps them where they were. */
  readonly popout: ReadonlySet<string>;
}

/** The layout the window shows; undefined while a group is maximized, which reading the grid would leave for a moment. */
function windowLayout(api: DockviewApi): WindowLayout | undefined {
  if (api.hasMaximizedGroup()) return undefined;
  const located = (type: string) => api.groups.filter(group => group.api.location.type === type);
  return {
    grid: gridArrangement(api.toJSON().grid),
    floating: located("floating").map(group => group.panels.map(panel => panel.id)),
    popout: new Set(located("popout").flatMap(group => group.panels.map(panel => panel.id))),
  };
}

/** The layout of the last snapshot, which the window showed when it was applied. */
function snapshotLayout(screen: ScreenState): WindowLayout {
  return {
    grid: layoutArrangement(screen.root),
    floating: (screen.floating ?? []).map(entry => slotPanes(entry.root)),
    popout: new Set(),
  };
}

/**
 * Apply `screen` to the panels the window shows (`canPatch`): close the panes
 * it no longer holds, give the viewers that stay their newer record, and add
 * its new panes where it has them. Nothing here makes a group the window's
 * active one, leaves a maximized group or moves the keyboard focus; a new
 * group shows its slot's active pane, and a viewer the server brought to the
 * front becomes its group's shown tab.
 */
function patchLayout(api: DockviewApi, binding: ScreenBinding, previous: ScreenState, screen: ScreenState, front: readonly string[]): void {
  for (const panel of [...api.panels]) {
    const before = jobParams(previous, panel.id);
    const after = jobParams(screen, panel.id);
    // A job that could attach and no longer can, or the other way round, gets a new panel.
    const remount = after.jobId && before.jobState !== after.jobState
      && (!jobCanAttach(before.jobState) || !jobCanAttach(after.jobState));
    if (screen.panes[panel.id] && !remount) continue;
    // The server ended the sessions of the panes it removed; one moved to
    // another screen runs on there. Either way the panel only lets go.
    letGo(binding, previous, screen, panel);
    panel.api.close();
  }
  for (const panel of api.panels) {
    if (!isViewerPanel(panel)) continue;
    const params = panelFor(binding, screen, panel.id).params as ViewerPanelParams;
    if (!sameJson(viewerParams(panel.params)?.record, params.viewer.record)) panel.api.updateParameters(params);
  }
  const nodeToPane = new Map<string, string>();
  buildFromNode(api, binding, screen, screen.root, undefined, undefined, nodeToPane, true);
  buildFloating(api, binding, screen, true);
  requestAnimationFrame(() => {
    if (previousSnapshots.get(api)?.screen !== screen) return;
    applySizes(api, screen.root, nodeToPane, api.width, api.height);
  });
  for (const id of front) showTab(api.getPanel(id));
}

/**
 * Make `panel` its group's shown tab without making the group the window's
 * active one, as dockview's `setActive` would, leaving a maximized group for
 * it (`DockviewComponent.setActivePanel`). In the active group it becomes the
 * window's active pane.
 */
function showTab(panel: IDockviewPanel | undefined): void {
  panel?.group.model.openPanel(panel, { skipSetGroupActive: true });
}

/** Two records alike, field by field. */
function sameJson(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

// ── Rebuilt ──

/**
 * Close every panel and build the layout from the server's layout tree. Each
 * group shows its slot's active pane, unless the window showed another of its
 * panes before; a viewer the server brought to the front is shown in any case.
 * The window's active pane stays active, unless such a viewer took the front
 * of its group.
 */
function rebuildLayout(
  api: DockviewApi,
  binding: ScreenBinding,
  previous: ScreenState | undefined,
  screen: ScreenState,
  before: { readonly active?: string; readonly activeTabs: readonly string[]; readonly front: readonly string[] },
): void {
  // The panels of sessions the snapshot re-uses let go of them (not kill)
  // before they close: this prevents a race where dispose sends "close" and
  // the new panel immediately sends "attach" for the same session. The
  // sessions of the previous snapshot are let go of too (`letGo`). Only a
  // pane the server never listed ends its session with its panel.
  for (const panel of [...api.panels]) letGo(binding, previous, screen, panel);
  for (const panel of [...api.panels]) panel.api.close();
  binding.clearFloatingIds();

  const nodeToPane = new Map<string, string>();
  buildFromNode(api, binding, screen, screen.root, undefined, undefined, nodeToPane, false);
  buildFloating(api, binding, screen, false);

  // Apply sizes after browser layout
  requestAnimationFrame(() => {
    if (previousSnapshots.get(api)?.screen !== screen) return;
    applySizes(api, screen.root, nodeToPane, api.width, api.height);
  });
  for (const id of before.activeTabs) api.getPanel(id)?.api.setActive();
  for (const id of before.front) api.getPanel(id)?.api.setActive();
  // The active pane stays, unless a viewer brought to the front took the front of its stack.
  const activePanel = before.active ? api.getPanel(before.active) : undefined;
  if (activePanel && !before.front.some(id => id !== before.active && api.getPanel(id)?.group === activePanel.group)) {
    activePanel.api.setActive();
  }
}

/**
 * The panel's session, when the server listed it, is let go of instead of
 * ended with the panel: the server ended those of panes it removed, and a pane
 * moved to another screen keeps its session there, also when another client
 * moved it.
 */
function letGo(binding: ScreenBinding, previous: ScreenState | undefined, screen: ScreenState, panel: IDockviewPanel): void {
  const session = binding.sessionForPane(panel.id);
  if (!session) return;
  const listed = (state: ScreenState | undefined) => Object.values(state?.panes ?? {}).some(pane => pane.sessionId === session.id);
  if (screen.revision === 0 || listed(screen) || listed(previous)) session.detach();
}

// ── Building ──

/**
 * Add the panes of the layout tree that have no panel yet, each beside the
 * panes the tree puts it next to. `inPlace` adds them without making them
 * active (`patchLayout`).
 */
function buildFromNode(
  api: DockviewApi,
  binding: ScreenBinding,
  screen: ScreenState,
  node: LayoutNode,
  refPaneId: string | undefined,
  direction: Direction | undefined,
  nodeToPane: Map<string, string>,
  inPlace: boolean,
): string | undefined {
  if (node.type === "slot") {
    const paneIds = node.paneIds ?? [];
    if (paneIds.length === 0) return undefined;

    // A tab joins the group of its slot's panes that are there, at its place.
    let group = paneIds.map(id => api.getPanel(id)?.group).find(Boolean);
    paneIds.forEach((paneId, index) => {
      if (api.getPanel(paneId)) return;
      let position: AddPanelOptions["position"];
      if (group) position = { referenceGroup: group, direction: "within", index };
      else if (refPaneId && direction) position = { referencePanel: refPaneId, direction };
      // The first pane of a grid the window has no pane of; in place, the
      // window's active group would take it.
      else if (inPlace) position = { direction: "right" };
      const panel = addPane(api, binding, screen, paneId, position, inPlace);
      group ??= panel.group;
    });
    nodeToPane.set(node.id, paneIds[0]);
    showSlotTab(api, paneIds, node.activePaneId, inPlace);
    return paneIds[0];
  }

  if (node.type === "split") {
    const children = node.children ?? [];
    let anchor: string | undefined;

    for (let i = 0; i < children.length; i++) {
      const child = children[i];
      const dir = node.axis === "horizontal" ? "right" : "below";

      if (i === 0) {
        const following = children.slice(1).map(node => existingPane(api, node)).find(Boolean);
        if (!existingPane(api, child) && following) {
          anchor = buildFromNode(api, binding, screen, child, following, node.axis === "horizontal" ? "left" : "above", nodeToPane, inPlace);
        } else {
          anchor = buildFromNode(api, binding, screen, child, refPaneId, direction, nodeToPane, inPlace);
        }
      } else {
        anchor = buildFromNode(api, binding, screen, child, anchor, dir, nodeToPane, inPlace) ?? anchor;
      }
    }

    return anchor;
  }

  return undefined;
}

/** The floating entries: each one tab group, in the bounds the server has for it. */
function buildFloating(api: DockviewApi, binding: ScreenBinding, screen: ScreenState, inPlace: boolean): void {
  for (const entry of screen.floating ?? []) {
    const paneIds = slotPanes(entry.root);
    let group = paneIds.map(id => api.getPanel(id)?.group).find(Boolean);
    paneIds.forEach((paneId, index) => {
      if (api.getPanel(paneId)) return;
      const panel = api.addPanel({
        id: paneId,
        // A terminal or a viewer pane by its role, never anything else (SR-01).
        ...panelFor(binding, screen, paneId),
        ...(inPlace ? { inactive: true } : {}),
        // A floating entry is one tab group, not one window per pane.
        ...(group ? { position: { referenceGroup: group, direction: "within" as const, index } } : { floating: {
          x: entry.bounds.x * api.width,
          y: entry.bounds.y * api.height,
          width: entry.bounds.w * api.width,
          height: entry.bounds.h * api.height,
        } }),
      });
      group ??= panel.group;
    });
    // The dockview group → mux floating entry mapping, also for a group that stays.
    if (group) binding.registerFloatingId(String(group.id), entry.id);
    showSlotTab(api, paneIds, entry.root.activePaneId, inPlace);
  }
}

function addPane(
  api: DockviewApi,
  binding: ScreenBinding,
  screen: ScreenState,
  paneId: string,
  position: AddPanelOptions["position"],
  inPlace: boolean,
): IDockviewPanel {
  return api.addPanel({
    id: paneId,
    // A terminal or a viewer pane by its role, never anything else (SR-01).
    ...panelFor(binding, screen, paneId),
    renderer: "always" as const,
    ...(inPlace ? { inactive: true } : {}),
    position,
  });
}

/**
 * The tab a stack shows when it is built. Built anew, it is the slot's active
 * pane; it activates the panel only, and its terminal takes focus only when
 * the screen is the active one (terminal_pane.ts). In place, only a group
 * that has no shown tab yet, a new one, shows the slot's active pane.
 */
function showSlotTab(api: DockviewApi, paneIds: readonly string[], active: string | undefined, inPlace: boolean): void {
  const shown = api.getPanel(shownPane(paneIds, active));
  if (!inPlace) {
    if (paneIds.length > 1) shown?.api.setActive();
  } else if (shown && !shown.group.activePanel) {
    showTab(shown);
  }
}

/**
 * The tab a stack shows: its active pane, or its first one when it names none,
 * as the server picks one (workspace-model, Active Pane of a Stack). Dockview
 * would show the last one added.
 */
function shownPane(paneIds: readonly string[], active: string | undefined): string {
  return active && paneIds.includes(active) ? active : paneIds[0];
}

function forEachSlot(node: LayoutNode, visit: (slot: LayoutNode) => void): void {
  if (node.type === "slot") visit(node);
  else for (const child of node.children ?? []) forEachSlot(child, visit);
}

/** The panes of a layout tree's slots, depth first. */
function slotPanes(node: LayoutNode): string[] {
  const panes: string[] = [];
  forEachSlot(node, slot => panes.push(...(slot.paneIds ?? [])));
  return panes;
}

/** Whether the server has `paneId` as the active pane of its slot, in the grid or floating. */
function activeInSlot(screen: ScreenState, paneId: string): boolean {
  let active = false;
  for (const node of [screen.root, ...(screen.floating ?? []).map(entry => entry.root)]) {
    forEachSlot(node, slot => { active ||= slot.activePaneId === paneId && !!slot.paneIds?.includes(paneId); });
  }
  return active;
}

function existingPane(api: DockviewApi, node: LayoutNode): string | undefined {
  if (node.type === "slot") return node.paneIds?.find(id => !!api.getPanel(id));
  for (const child of node.children ?? []) {
    const id = existingPane(api, child);
    if (id) return id;
  }
}

/**
 * Apply sizes from the layout tree to the rendered dockview groups.
 */
function applySizes(
  api: DockviewApi,
  node: LayoutNode,
  nodeToPane: Map<string, string>,
  totalWidth: number,
  totalHeight: number,
): void {
  if (node.type !== "split") return;

  const children = node.children ?? [];
  const sizes = node.sizes ?? [];
  if (children.length < 2 || sizes.length !== children.length) return;

  for (let i = 0; i < children.length; i++) {
    const child = children[i];
    const paneId = findFirstPaneId(child, nodeToPane);
    if (!paneId) continue;

    const panel = api.getPanel(paneId);
    if (!panel?.group) continue;

    const size = sizes[i];
    if (node.axis === "horizontal") {
      panel.group.api.setSize({ width: Math.round(totalWidth * size) });
    } else {
      panel.group.api.setSize({ height: Math.round(totalHeight * size) });
    }
  }

  for (let i = 0; i < children.length; i++) {
    const child = children[i];
    const childSize = sizes[i] ?? (1 / children.length);
    if (node.axis === "horizontal") {
      applySizes(api, child, nodeToPane, totalWidth * childSize, totalHeight);
    } else {
      applySizes(api, child, nodeToPane, totalWidth, totalHeight * childSize);
    }
  }
}

function findFirstPaneId(node: LayoutNode, nodeToPane: Map<string, string>): string | undefined {
  if (node.type === "slot") {
    return nodeToPane.get(node.id);
  }
  for (const child of node.children ?? []) {
    const id = findFirstPaneId(child, nodeToPane);
    if (id) return id;
  }
  return undefined;
}
