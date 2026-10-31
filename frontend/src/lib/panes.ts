import type { DockviewApi, DockviewGroupPanel } from "dockview-core";
import { bindingForApi } from "./connection/screen_binding.ts";
import { createScreenOp, newTabOp, splitOp } from "./mux/mux_client.ts";
import type { RestoreMode } from "./protocol/types.ts";
import { getDefaultShellProfileId } from "./shell_profiles.ts";
import { newPaneId } from "./ids.ts";

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

  // Inherit CWD from the pane we split off when not explicitly provided
  const dir = options?.dir ?? (refPaneId
    ? binding.sessionForPane(refPaneId)?.lastCwd
    : undefined);
  // Shell profiles are the local server's. Another server does not know their
  // ids, and a split there inherits its pane's profile on that server.
  const shellProfileId = binding.conn.kind !== "local"
    ? undefined
    : options?.shellProfileId
      ?? (direction && refPaneId
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
    params: { command, dir, root, restoreMode, shellProfileId, existingSessionId, muxManaged },
    renderer: "always" as const,
    inactive: options?.inactive,
    ...(options?.floating ? { floating: options.floating } : {}),
  };
  api.addPanel(panelOpts);

  if (muxManaged && !options?.skipMuxNotify) {
    if (!refPaneId) {
      // First pane of an otherwise empty screen (watermark) — there is no pane
      // to split off, so re-create the screen around this one.
      binding.sendOp(createScreenOp(binding.wireId, undefined, id, shellProfileId));
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
