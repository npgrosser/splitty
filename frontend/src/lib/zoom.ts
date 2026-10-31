// Zoom with Cmd/Ctrl and +, − and 0 (display-zoom): the whole window through
// the desktop shell's own page zoom, or only the terminals' font size.

import { isMac } from "./shortcuts.ts";
import { shellKind, type ShellKind } from "./platform.ts";
import type { ZoomScope } from "./settings.ts";

/** The levels, as Chromium's. */
export const ZOOM_STEPS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3] as const;

export type ZoomAction = "in" | "out" | "reset";

/** The next level in `action`'s direction, staying within the steps. */
export function nextZoom(current: number, action: ZoomAction): number {
  if (action === "reset") return 1;
  if (action === "in") return ZOOM_STEPS.find((step) => step > current + 0.001) ?? ZOOM_STEPS[ZOOM_STEPS.length - 1];
  return [...ZOOM_STEPS].reverse().find((step) => step < current - 0.001) ?? ZOOM_STEPS[0];
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

/** A terminal's font size at `zoom`: zoomed only with the terminals scope. */
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
  } else if (shell === "electron") {
    await (window as { __SPLITTY_ELECTRON__?: { invoke(channel: string, ...args: unknown[]): Promise<unknown> } })
      .__SPLITTY_ELECTRON__?.invoke("window:set-zoom", factor);
  }
}

/** "125 %" */
export function zoomLabel(zoom: number): string {
  return `${Math.round(zoom * 100)} %`;
}
