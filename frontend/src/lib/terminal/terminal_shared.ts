import { isMac, modKey } from "../shortcuts.ts";
import { getSettings } from "../settings.ts";
import { handlesZoomKeys, zoomAction } from "../zoom.ts";

// ── Font Loading ──

let fontLoaded = false;

export async function ensureFont(cfg: { fontSize: number; fontFamily: string }): Promise<void> {
  if (fontLoaded) return;
  try {
    const face = new FontFace(
      "SplittyMono",
      "url(/JetBrainsMonoNerdFontMono-Regular.ttf)",
    );
    await face.load();
    document.fonts.add(face);
    cfg.fontFamily = `"SplittyMono", "Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji", ${cfg.fontFamily}`;
    fontLoaded = true;
  } catch { /* proceed with fallback */ }
}

// ── Key Filtering ──

export function isSplittyShortcut(e: KeyboardEvent): boolean {
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;

  if (modKey(e) && !e.ctrlKey && !e.shiftKey && (key === "d" || key === "t" || key === "w" || key === "p" || key === "j")) {
    return true;
  }
  if (modKey(e) && e.shiftKey && !e.ctrlKey && (key === "d" || key === "p")) return true;
  if (e.ctrlKey && e.shiftKey && key === " ") return true;
  if (modKey(e) && e.shiftKey && e.code >= "Digit1" && e.code <= "Digit9") return true;
  if (modKey(e) && e.shiftKey && (key === "ArrowLeft" || key === "ArrowRight" || key === "ArrowUp" || key === "ArrowDown")) {
    return true;
  }
  if (modKey(e) && key === "x" && ((e.shiftKey && !e.ctrlKey) || (e.ctrlKey && !e.shiftKey))) {
    return true;
  }
  if (e.ctrlKey && !e.shiftKey && !e.altKey && key === ",") return true;
  if (e.ctrlKey && e.shiftKey && key.startsWith("Arrow")) return true;
  // Cmd+F (Mac) / Ctrl+F (other) — terminal search
  if ((isMac ? e.metaKey : e.ctrlKey) && !e.shiftKey && !e.altKey && key === "f") return true;
  // Cmd/Ctrl and +, − or 0 — zoom, unless a browser keeps them (display-zoom)
  if (zoomAction(e) && handlesZoomKeys(getSettings().appearance.zoomScope)) return true;
  return false;
}

export function shouldUseNativeClipboardShortcut(e: KeyboardEvent): boolean {
  if (isMac) {
    return e.metaKey && !e.ctrlKey && !e.altKey && (e.code === "KeyC" || e.code === "KeyV");
  }
  return e.ctrlKey && e.shiftKey && !e.altKey && (e.code === "KeyC" || e.code === "KeyV");
}

export function isCopyShortcut(e: KeyboardEvent): boolean {
  return shouldUseNativeClipboardShortcut(e) && e.code === "KeyC";
}

// ── Clipboard Fallback ──

export async function writeClipboardText(text: string): Promise<boolean> {
  if (!text) return true;
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch { /* fall through */ }

  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.top = "-1000px";
  ta.style.left = "-1000px";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  ta.setSelectionRange(0, text.length);
  try {
    return document.execCommand("copy");
  } finally {
    document.body.removeChild(ta);
  }
}

// ── Wheel Helpers ──

/** Mouse tracking mode as reported by xterm.js (`Terminal.modes.mouseTrackingMode`). */
export type MouseTrackingMode = "none" | "x10" | "vt200" | "drag" | "any";

/**
 * True when the running app receives wheel events itself over the mouse
 * reporting protocol (DECSET 1000/1002/1003 + SGR 1006).
 *
 * TUIs like opencode or fullscreen Claude Code enable these and scroll their own
 * viewport from the reports. Synthesizing cursor keys for them instead lands as
 * literal Up/Down in their keymap — history navigation, list selection, etc.
 * `x10` (DECSET 9) only reports button presses, so the terminal still owns the
 * wheel there.
 */
export function appReportsWheel(mode: MouseTrackingMode): boolean {
  return mode === "vt200" || mode === "drag" || mode === "any";
}

export function wheelDeltaToLines(delta: number, deltaMode: number, viewportRows: number): number {
  switch (deltaMode) {
    case WheelEvent.DOM_DELTA_LINE:
      return delta;
    case WheelEvent.DOM_DELTA_PAGE:
      return delta * Math.max(viewportRows, 1);
    default:
      return delta / 33;
  }
}

// ── Wait for Layout ──

export function waitForLayout(el: HTMLElement): Promise<void> {
  return new Promise(resolve => {
    if (el.offsetWidth > 0 && el.offsetHeight > 0) {
      resolve();
      return;
    }
    const ro = new ResizeObserver(() => {
      if (el.offsetWidth > 0 && el.offsetHeight > 0) {
        ro.disconnect();
        clearTimeout(fallback);
        resolve();
      }
    });
    const fallback = setTimeout(() => {
      ro.disconnect();
      resolve();
    }, 500);
    ro.observe(el);
  });
}

// ── DEC 2026 Synchronized Update Constants ──

export const BSU = new Uint8Array([0x1b, 0x5b, 0x3f, 0x32, 0x30, 0x32, 0x36, 0x68]); // ESC[?2026h
export const ESU = new Uint8Array([0x1b, 0x5b, 0x3f, 0x32, 0x30, 0x32, 0x36, 0x6c]); // ESC[?2026l
export const MAX_WRITE_BUF = 4 * 1024 * 1024; // 4 MB cap
