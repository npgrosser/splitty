/**
 * Rebuilds a dockview layout from a mux ScreenState snapshot.
 *
 * Used when a client receives a screen_snapshot (e.g. after reconnecting
 * to a persistent server) and must reconstruct the dockview layout.
 */

import type { DockviewApi } from "dockview-core";
import type { ScreenState, LayoutNode } from "../lib/protocol/types.ts";
import type { ScreenBinding } from "../lib/connection/screen_binding.ts";
import { jobParams, jobCanAttach, topology } from "./job_snapshot.ts";
import { layoutArrangement } from "./arrangement.ts";

const previousSnapshots = new WeakMap<DockviewApi, ScreenState>();

/**
 * Apply a ScreenState snapshot to the dockview instance of the screen bound by
 * `binding`. Clears the existing layout and rebuilds from the server's layout
 * tree. Layout ops of this screen are held back meanwhile, so the rebuild is
 * not echoed to the server; other screens are not affected.
 */
export function applyScreenSnapshot(api: DockviewApi, binding: ScreenBinding, screen: ScreenState): void {
  const previous = previousSnapshots.get(api);
  const oldPanels = [...api.panels];
  const active = api.activePanel?.id;
  const activeTabs = api.groups.map(group => group.activePanel?.id).filter((id): id is string => !!id);
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const oldIds = new Set(oldPanels.map(panel => panel.id));
  const added = Object.keys(screen.panes).filter(id => !oldIds.has(id));
  const removed = oldPanels.filter(p => !screen.panes[p.id]).map(p => p.id);
  const omit = new Set([...added, ...removed]);
  const patchJobs = added.every(id => !!screen.panes[id].jobId)
    && removed.every(id => !!previous?.panes[id]?.jobId)
    && (!previous || JSON.stringify(topology(previous.root, omit)) === JSON.stringify(topology(screen.root, omit)))
    && (added.length > 0 || removed.length > 0 || !!Object.keys(screen.jobs ?? {}).length);
  previousSnapshots.set(api, screen);
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
    // Collect session IDs the snapshot will re-use so we can detach
    // (not kill) them before closing panels — prevents a race where
    // dispose sends "close" and the new panel immediately sends "attach"
    // for the same session. The sessions of the previous snapshot are let go
    // of too: the server ended those of panes it removed, and a pane moved to
    // another screen keeps its session there, also when another client moved
    // it. Only a pane the server never listed ends its session with its panel.
    const keepSessions = new Set<string>();
    for (const pane of [...Object.values(screen.panes), ...Object.values(previous?.panes ?? {})]) {
      if (pane.sessionId) keepSessions.add(pane.sessionId);
    }
    for (const panel of patchJobs ? [] : [...api.panels]) {
      const session = binding.sessionForPane(panel.id);
      if (session && keepSessions.has(session.id)) {
        session.detach();
      }
    }

    // Close all existing panels
    for (const panel of [...api.panels]) {
      const before = previous ? jobParams(previous, panel.id) : undefined;
      const after = jobParams(screen, panel.id);
      const remount = after.jobId && before?.jobState !== after.jobState
        && (!jobCanAttach(before?.jobState) || !jobCanAttach(after.jobState));
      if (!patchJobs || !screen.panes[panel.id] || remount) {
        if (patchJobs && screen.panes[panel.id]) binding.sessionForPane(panel.id)?.detach();
        panel.api.close();
      }
    }
    if (!patchJobs) binding.clearFloatingIds();

    // Walk the layout tree and create panels
    const nodeToPane = new Map<string, string>();
    buildFromNode(api, screen, screen.root, undefined, undefined, true, nodeToPane);

    // Restore floating entries
    for (const entry of screen.floating ?? []) {
      const paneIds = entry.root.paneIds ?? [];
      let group = paneIds.map(id => api.getPanel(id)?.group).find(Boolean);
      for (const paneId of paneIds) {
        if (api.getPanel(paneId)) continue;
        const pane = screen.panes[paneId];
        const session = pane?.sessionId ? screen.sessions[pane.sessionId] : undefined;

        api.addPanel({
          id: paneId,
          // Always "terminal": a persisted or legacy mux state may still carry
          // role "webview", and honouring it would rebuild an iframe pane
          // (security review SR-01). Restoring it as a terminal is
          // the fail-safe direction — dockview has no "webview" component left.
          component: "terminal",
          title: pane?.title ?? "Terminal",
          // A floating entry is one tab group, not one window per pane.
          ...(group ? { position: { referenceGroup: group, direction: "within" as const } } : { floating: {
            x: entry.bounds.x * api.width,
            y: entry.bounds.y * api.height,
            width: entry.bounds.w * api.width,
            height: entry.bounds.h * api.height,
          } }),
          params: {
            ...jobParams(screen, paneId),
            existingSessionId: pane?.sessionId,
            dir: pane?.cwd ?? session?.cwd,
            command: session?.command,
            shellProfileId: session?.shellProfileId,
          },
        });

        // Register the dockview group → mux floating entry mapping
        const panel = api.getPanel(paneId);
        if (panel?.group) {
          group = panel.group;
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          binding.registerFloatingId(String((panel.group as any).id), entry.id);
        }
      }
      if (paneIds.length > 1) api.getPanel(shownPane(paneIds, entry.root.activePaneId))?.api.setActive();
    }

    // Apply sizes after browser layout
    requestAnimationFrame(() => {
      if (previousSnapshots.get(api) !== screen) return;
      applySizes(api, screen.root, nodeToPane, api.width, api.height);
    });
    for (const id of activeTabs) api.getPanel(id)?.api.setActive();
    if (active) api.getPanel(active)?.api.setActive();
  } finally {
    binding.suppressFocus = false;
    if (focused?.isConnected && binding.isActive()) focused.focus({ preventScroll: true });
    requestAnimationFrame(() => { binding.suppressOps = false; });
  }
}

/**
 * Recursively build dockview panels from the layout tree.
 */
function buildFromNode(
  api: DockviewApi,
  screen: ScreenState,
  node: LayoutNode,
  refPaneId: string | undefined,
  direction: "left" | "right" | "above" | "below" | undefined,
  isFirst: boolean,
  nodeToPane: Map<string, string>,
): string | undefined {
  if (node.type === "slot") {
    const paneIds = node.paneIds ?? [];
    if (paneIds.length === 0) return undefined;

    let firstCreatedId: string | undefined;

    for (let i = 0; i < paneIds.length; i++) {
      const paneId = paneIds[i];
      const pane = screen.panes[paneId];
      const session = pane?.sessionId ? screen.sessions[pane.sessionId] : undefined;

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      let position: any;

      if (i === 0) {
        if (refPaneId && direction) {
          position = { referencePanel: refPaneId, direction };
        }
        // else: very first pane, no reference needed
      } else {
        const refPanel = api.getPanel(firstCreatedId!);
        if (refPanel) {
          position = {
            referenceGroup: refPanel.group!,
            direction: "within",
          };
        }
      }

      if (!api.getPanel(paneId)) api.addPanel({
        id: paneId,
        // Always "terminal": a persisted or legacy mux state may still carry
        // role "webview", and honouring it would rebuild an iframe pane
        // (security review SR-01). Restoring it as a terminal is
        // the fail-safe direction — dockview has no "webview" component left.
        component: "terminal",
        title: pane?.title ?? "Terminal",
        params: {
          ...jobParams(screen, paneId),
          existingSessionId: pane?.sessionId,
          dir: pane?.cwd ?? session?.cwd,
          command: session?.command,
          shellProfileId: session?.shellProfileId,
        },
        renderer: "always" as const,
        position,
      });
      if (i === 0) {
        firstCreatedId = paneId;
        nodeToPane.set(node.id, paneId);
      }
    }

    // Set active pane in stack. This activates the panel only; its terminal
    // takes focus only when the screen is the active one (terminal_pane.ts).
    if (paneIds.length > 1) api.getPanel(shownPane(paneIds, node.activePaneId))?.api.setActive();

    return firstCreatedId;
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
          anchor = buildFromNode(api, screen, child, following, node.axis === "horizontal" ? "left" : "above", isFirst, nodeToPane);
        } else {
          anchor = buildFromNode(api, screen, child, refPaneId, direction, isFirst, nodeToPane);
        }
      } else {
        anchor = buildFromNode(api, screen, child, anchor, dir as "right" | "below", false, nodeToPane) ?? anchor;
      }
    }

    return anchor;
  }

  return undefined;
}

/**
 * The tab a stack shows: its active pane, or its first one when it names none,
 * as the server picks one (workspace-model, Active Pane of a Stack). Dockview
 * would show the last one added.
 */
function shownPane(paneIds: string[], active: string | undefined): string {
  return active && paneIds.includes(active) ? active : paneIds[0];
}

function forEachSlot(node: LayoutNode, visit: (slot: LayoutNode) => void): void {
  if (node.type === "slot") visit(node);
  else for (const child of node.children ?? []) forEachSlot(child, visit);
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
