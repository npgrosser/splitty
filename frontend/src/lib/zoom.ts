// Zoom with Cmd/Ctrl and +, − and 0 (display-zoom): the whole window through
// the desktop shell's own page zoom, or only the terminals' font size.

import { isMac } from "./shortcuts.ts";
import { shellKind, type ShellKind } from "./platform.ts";
import { allowedValues, type ZoomScope } from "./settings.ts";

/**
 * The levels the server allows, ascending: the `values` of `appearance.zoom`
 * in its settings metadata. Empty without metadata.
 */
export function zoomSteps(): number[] {
  const levels = (allowedValues("appearance.zoom") ?? [])
    .filter((level): level is number => typeof level === "number" && Number.isFinite(level) && level > 0);
  return [...new Set(levels)].sort((a, b) => a - b);
}

export type ZoomAction = "in" | "out" | "reset";

/** The next level in `action`'s direction, staying within `steps`; without steps only a reset changes it. */
export function nextZoom(current: number, action: ZoomAction, steps: readonly number[] = zoomSteps()): number {
  if (action === "reset") return 1;
  if (steps.length === 0) return current;
  if (action === "in") return steps.find((step) => step > current + 0.001) ?? steps[steps.length - 1];
  return [...steps].reverse().find((step) => step < current - 0.001) ?? steps[0];
}

/** The zoom a key press asks for: Cmd (macOS) or Ctrl (elsewhere) with +, =, - or 0. */
export function zoomAction(
  e: Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey">,
  mac = isMac,
): ZoomAction | null {
  const modifier = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!modifier || e.altKey) return null;
  if (e.key === "+" || e.key === "=" || e.code === "NumpadAdd") return "in";
  if (e.key === "-" || e.code === "NumpadSubtract") return "out";
  if (e.key === "0" || e.code === "Numpad0") return "reset";
  return null;
}

/**
 * Whether Splitty takes the zoom keys. A browser page cannot set the
 * browser's zoom, so with the window scope a browser keeps its own keys.
 */
export function handlesZoomKeys(scope: ZoomScope, shell: ShellKind = shellKind): boolean {
  return scope === "terminals" || shell !== "web";
}

/** A terminal's font size at `zoom`: zoomed only with the terminals scope, to whole pixels as in every client. */
export function zoomedFontSize(fontSize: number, zoom: number, scope: ZoomScope): number {
  return scope === "terminals" ? Math.round(fontSize * zoom) : fontSize;
}

/** The page zoom the desktop shell sets: the level with the window scope, 100 % otherwise. */
export function pageZoom(zoom: number, scope: ZoomScope): number {
  return scope === "window" ? zoom : 1;
}

/** Set the desktop shell's page zoom; a browser's zoom is the browser's. */
export async function applyPageZoom(factor: number, shell: ShellKind = shellKind): Promise<void> {
  if (shell === "tauri") {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    await getCurrentWebview().setZoom(factor);
  }
}

/** "125 %" */
export function zoomLabel(zoom: number): string {
  return `${Math.round(zoom * 100)} %`;
}
