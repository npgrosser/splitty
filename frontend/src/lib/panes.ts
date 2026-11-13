import type { DockviewApi, DockviewGroupPanel } from "dockview-core";
import { bindingForApi } from "./connection/screen_binding.ts";
import { createScreenOp, newTabOp, openViewerOp, splitOp } from "./mux/mux_client.ts";
import type { OpenViewerOp, RestoreMode, ViewerPlacement, ViewerRecord } from "./protocol/types.ts";
import { getDefaultShellProfileId } from "./shell_profiles.ts";
import { newPaneId, newViewerPaneId } from "./ids.ts";
import { MAX_TEXT_BYTES, isViewerCwd, isViewerPath, isViewerTitle, utf8Length, wellFormed } from "./viewer/limits.ts";
import {
  VIEWER_COMPONENT, findViewerPanel, firstViewerOf, isViewerPanel, viewerDirectoryOf, viewerPaneTitle, viewerRecordOf,
  type ViewerPanelParams,
} from "./viewer/panel.ts";
import { arrangementSlots } from "../layout/arrangement.ts";
import { nameToSend } from "../stores/screens.svelte.ts";

/**
 * Add a terminal pane to the screen whose layout is `api` and register it with
 * that screen's server. Returns the pane's id, or undefined when the layout
 * belongs to no screen binding: such a pane would not know its server, so it is
 * not created.
 */
export function addTerminalPane(
  api: DockviewApi,
  options?: {
    direction?: "right" | "below";
    group?: DockviewGroupPanel;
    referencePanel?: string;
    command?: string;
    dir?: string;
    root?: string;
    restoreMode?: RestoreMode;
    shellProfileId?: string;
    existingSessionId?: string;
    floating?: { x: number; y: number; width: number; height: number };
    id?: string;
    inactive?: boolean;
    /** Don't send a mux op for this pane — the caller registers it itself
     *  (e.g. the first pane of a screen, which rides along on create_screen). */
    skipMuxNotify?: boolean;
    /** Spawn the PTY from the client instead of letting the server do it.
     *  For panes the mux model can't express (templates with commands). */
    directSpawn?: boolean;
    /** Only the server starts its shell: no direct spawn when the op that
     *  creates it is refused (a first screen another window made first). */
    serverSpawnOnly?: boolean;
  },
): string | undefined {
  const binding = bindingForApi(api);
  if (!binding) return undefined;
  const id = options?.id ?? newPaneId();
  const { direction, group, referencePanel, command, root, restoreMode, existingSessionId } = options ?? {};

  // The pane the new one is created next to — resolved before addPanel, which
  // moves the active panel. Also the reference for the mux op: any existing pane
  // will do, so fall back to the first one rather than treating "nothing active"
  // as "screen is empty" (which would re-create the screen and drop the rest).
  const refPaneId = referencePanel
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ?? (group ? (group as any).activePanel?.id as string | undefined : undefined)
    ?? api.activePanel?.id
    ?? api.panels[0]?.id;

  // Inherit CWD from the pane we split off when not explicitly provided; a
  // viewer pane has no session, and a terminal next to it starts in its
  // directory (workspace-model, Split Session Inheritance).
  const inheritOnServer = binding.conn.sessions.serverDerivedPaneState && !options?.directSpawn && !command && !existingSessionId;
  const dir = options?.dir ?? (!inheritOnServer && refPaneId
    ? binding.sessionForPane(refPaneId)?.lastCwd ?? viewerDirectoryOf(api.getPanel(refPaneId))
    : undefined);
  // Capable hosts resolve unspecified profiles themselves; an explicit choice
  // names a profile on that host. Keep the local defaults for older servers.
  const shellProfileId = inheritOnServer
    ? options?.shellProfileId
    : binding.conn.kind !== "local" ? undefined
      : options?.shellProfileId ?? (direction && refPaneId
        ? binding.sessionForPane(refPaneId)?.shellProfileId
        : getDefaultShellProfileId());

  // Mux-managed panes let the server spawn the PTY (it owns session lifecycle
  // and can hand the same session to other windows). Panes carrying a command
  // or restoring a known session keep spawning/attaching directly — the mux op
  // has no way to express them.
  const muxManaged = !options?.directSpawn
    && !command
    && !existingSessionId;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let position: any;

  if (group) {
    position = { referenceGroup: group, direction: direction ?? "within" };
  } else if (direction && referencePanel) {
    position = { referencePanel, direction };
  } else if (direction && api.activePanel) {
    position = { referencePanel: api.activePanel.id, direction };
  }

  const panelOpts = {
    id,
    component: "terminal",
    title: "",
    position,
    params: { command, dir, root, restoreMode, shellProfileId, existingSessionId, muxManaged,
      ...(options?.serverSpawnOnly ? { serverSpawnOnly: true } : {}) },
    renderer: "always" as const,
    inactive: options?.inactive,
    ...(options?.floating ? { floating: options.floating } : {}),
  };
  api.addPanel(panelOpts);

  if (muxManaged && !options?.skipMuxNotify) {
    if (!refPaneId) {
      // First pane of an otherwise empty screen (watermark) — there is no pane
      // to split off, so re-create the screen around this one. A server that
      // removed the screen with its last pane creates it anew, under the name
      // it has in this window, or names it when the window has none yet.
      const name = nameToSend(binding.key, binding.conn.mux.screenNames);
      binding.sendOp(createScreenOp(binding.wireId, name, id, shellProfileId));
      if (name === undefined) binding.conn.mux.requestWorkspace();
    } else if (direction) {
      binding.sendOp(splitOp(refPaneId, id, direction, dir, shellProfileId));
    } else {
      binding.sendOp(newTabOp(refPaneId, id, dir, shellProfileId));
      // The server makes the new tab its slot's active one: no select_tab for it.
      binding.noteActiveTab(api.getPanel(id)?.group?.panels.map((panel) => panel.id) ?? [id], id);
    }
  }

  return id;
}

/**
 * Add a viewer pane for `source` to the screen whose layout is `api`, next to
 * `anchor`, and send `open_viewer` with its id (`viewer-panes`, Opening A
 * Viewer). The panel is there at once; the server adds the same pane, or
 * refuses the op and the screen's next snapshot removes the panel again.
 *
 * Returns the pane's id, or undefined when nothing was added: the layout has no
 * binding, its server has no viewer panes, or the source is outside the
 * server's limits.
 *
 * A directional placement beside a terminal that already has viewers becomes a
 * tab of the first of them in layout order, as the server groups the CLI's
 * viewers per terminal; the op names that viewer, which passes the terminal on
 * as origin. A directional placement next to a pane of a floating group or a
 * popout window becomes a tab: the server places directionally only in the
 * root layout.
 */
export function addViewerPane(
  api: DockviewApi,
  options: {
    source: OpenViewerOp["source"];
    /** The pane the viewer goes next to. */
    anchor?: string;
    /** `right` by default. */
    placement?: ViewerPlacement;
    /** A text's directory. */
    cwd?: string;
  },
): string | undefined {
  const binding = bindingForApi(api);
  if (!binding || !binding.conn.mux.viewers) return undefined;
  // The server's rules (`viewer-panes`, Opening A Viewer): an op it would
  // refuse is never sent.
  const source: OpenViewerOp["source"] = options.source.kind === "text"
    ? { ...options.source, text: wellFormed(options.source.text) }
    : options.source;
  if (source.kind === "file" ? !isViewerPath(source.path) : !isViewerTitle(source.title) || utf8Length(source.text) > MAX_TEXT_BYTES) {
    return undefined;
  }
  const cwd = options.cwd && isViewerCwd(options.cwd) ? options.cwd : undefined;
  let anchor = options.anchor ? api.getPanel(options.anchor) : undefined;
  if (options.anchor && !anchor) return undefined;
  let placement = options.placement ?? "right";
  if (anchor && placement !== "tab" && !isViewerPanel(anchor)) {
    // The terminal's viewers in the layout order the window knows: its grid as
    // last arranged, then every other panel.
    const order = binding.arrangement ? arrangementSlots(binding.arrangement).flatMap((slot) => slot.paneIds) : [];
    const sibling = firstViewerOf(anchor.id, api.panels, order);
    if (sibling) {
      anchor = sibling;
      placement = "tab";
    }
  }
  if (anchor && placement !== "tab" && anchor.group.api.location.type !== "grid") placement = "tab";

  // As the server sets it: a terminal anchor, or a viewer anchor's own origin.
  const originPaneId = anchor ? (isViewerPanel(anchor) ? viewerRecordOf(anchor)?.originPaneId : anchor.id) : undefined;
  const record: ViewerRecord = {
    source: source.kind === "file" ? { kind: "file", path: source.path } : { kind: "text", title: source.title },
    revision: 1,
    ...(originPaneId ? { originPaneId } : {}),
  };
  const params: ViewerPanelParams = { viewer: { record, supported: true, ...(cwd ? { cwd } : {}) } };

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let position: any;
  if (anchor && placement === "tab") position = { referenceGroup: anchor.group, direction: "within" };
  else if (anchor) position = { referencePanel: anchor.id, direction: placement };
  else if (api.panels.length > 0) position = { direction: "right" };

  const id = newViewerPaneId();
  api.addPanel({
    id,
    component: VIEWER_COMPONENT,
    title: viewerPaneTitle(record),
    params,
    renderer: "always",
    position,
  });
  binding.sendOp(openViewerOp(id, source, {
    ...(anchor ? { paneId: anchor.id, placement } : {}),
    ...(cwd ? { cwd } : {}),
  }));
  // The server makes a new tab its slot's active one: no select_tab for it.
  if (anchor && placement === "tab") binding.noteActiveTab(anchor.group.panels.map((panel) => panel.id), id);
  return id;
}

/**
 * Show the file `path` in a viewer pane of the screen whose layout is `api`,
 * as a relative link of a rendered document does: a viewer pane that shows it
 * already comes to the front, or a new one opens as a tab next to `anchor`,
 * which passes its origin on.
 */
export function openViewerFile(api: DockviewApi, path: string, anchor: string): string | undefined {
  const existing = findViewerPanel(api, path);
  if (existing) {
    existing.api.setActive();
    return existing.id;
  }
  return addViewerPane(api, { source: { kind: "file", path }, anchor, placement: "tab" });
}

// REMOVED: addWebviewPane (security review SR-01/SR-02).
//
// It created a dockview panel rendered by `panes/webview_pane.ts`, which built an
// iframe with `allow-same-origin allow-scripts`. For local ports it could route
// through `/proxy/{port}/`, which served the target under Splitty's own origin
// with `Content-Security-Policy` and `X-Frame-Options` stripped from the
// response. Foreign active content therefore became a same-origin document with
// access to the parent DOM, web storage, the auth cookie and the terminal API —
// i.e. command execution as the WSL user, reachable from a context that has no
// local privileges of its own.
//
// All former call sites now hand the target to `openExternalTarget`
// (`lib/window.ts`), which validates the scheme and opens the OS browser.
// Reactivation needs a separate, credential-free origin with no DOM relationship
// to the application — see SR-01 in docs/security/findings.md.

/** Find the nearest group in a direction using a 90° cone filter. */
export function findNeighborGroup(
  api: DockviewApi,
  direction: "left" | "right" | "up" | "down",
): DockviewGroupPanel | null {
  const active = api.activeGroup;
  if (!active) return null;
  const rect = active.element.getBoundingClientRect();
  const cx = rect.left + rect.width / 2;
  const cy = rect.top + rect.height / 2;
  let best: DockviewGroupPanel | null = null;
  let bestDist = Infinity;
  for (const group of api.groups) {
    if (group === active) continue;
    const r = group.element.getBoundingClientRect();
    const gx = r.left + r.width / 2;
    const gy = r.top + r.height / 2;
    const dx = gx - cx;
    const dy = gy - cy;
    const match =
      (direction === "right" && dx > 0 && Math.abs(dy) <= dx) ||
      (direction === "left" && dx < 0 && Math.abs(dy) <= -dx) ||
      (direction === "down" && dy > 0 && Math.abs(dx) <= dy) ||
      (direction === "up" && dy < 0 && Math.abs(dx) <= -dy);
    if (!match) continue;
    const dist = dx * dx + dy * dy;
    if (dist < bestDist) {
      bestDist = dist;
      best = group;
    }
  }
  return best;
}
