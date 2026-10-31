import { shellKind } from "./platform.ts";
import type { ActionReturn } from "svelte/action";

export interface WindowAPI {
  minimize(): void;
  maximize(): void;
  unmaximize(): void;
  toggleMaximize(): void;
  close(): void;
  isMaximized(): Promise<boolean>;
  /** Open `url` in the system browser; false when nothing opened (a blocked window). */
  openExternal(url: string): boolean;
  /** Unmaximize and reposition for drag from maximized state (Linux WM edge workaround). */
  startDragFromMaximized(screenX: number, screenY: number): void;
  /** Start an interactive resize from a window edge/corner. */
  startResizeDragging(direction: string): void;
  /** Update the macOS title bar rendering mode. */
  setTitleBarStyle(style: "visible" | "transparent" | "overlay"): void;
  /** Update the native macOS title bar background color. */
  setBackgroundColor(r: number, g: number, b: number): void;
}

/**
 * The desktop opener refused or failed: say so, with its reason, instead of a
 * click that seems to do nothing.
 */
function openFailed(error: unknown): void {
  console.warn("could not open link in the system browser", error);
  void import("svelte-sonner").then(({ toast }) => {
    toast.error("Could not open the link in the browser", { description: String(error).slice(0, 300) });
  });
}

// ---------------------------------------------------------------------------
// Tauri
// ---------------------------------------------------------------------------

let tauriWindow: typeof import("@tauri-apps/api/window") | undefined;

const tauriReady: Promise<void> = shellKind === "tauri"
  ? import("@tauri-apps/api/window").then((m) => { tauriWindow = m; })
  : Promise.resolve();

function tauriWin() {
  return tauriWindow?.getCurrentWindow();
}

const tauriAPI: WindowAPI = {
  minimize() { tauriWin()?.minimize(); },
  maximize() { tauriWin()?.maximize(); },
  unmaximize() { tauriWin()?.unmaximize(); },
  toggleMaximize() { tauriWin()?.toggleMaximize(); },
  close() { tauriWin()?.close(); },
  isMaximized() { return tauriWin()?.isMaximized() ?? Promise.resolve(false); },
  openExternal(url) {
    // tauri-plugin-opener is registered in the Tauri shell; invoke it directly.
    // No `window.open` fallback: on Windows that used to open the target in a
    // WebView2 popup inside Splitty (security review SR-26, see
    // docs/security/findings.md). The capability scopes the opener to http(s).
    (window as any).__TAURI_INTERNALS__?.invoke("plugin:opener|open_url", { url }).catch(openFailed);
    return true;
  },
  startDragFromMaximized() {
    // Tauri's data-tauri-drag-region handles drag-from-maximized natively.
  },
  startResizeDragging(direction) {
    tauriWin()?.startResizeDragging(direction as any);
  },
  setTitleBarStyle(style) {
    tauriReady.then(() => {
      tauriWin()?.setTitleBarStyle(style as any);
    });
  },
  setBackgroundColor(r, g, b) {
    tauriReady.then(() => {
      (window as any).__TAURI_INTERNALS__?.invoke("set_title_bar_color", { r, g, b });
    });
  },
};

// ---------------------------------------------------------------------------
// Electron
// ---------------------------------------------------------------------------

function electronInvoke(channel: string, ...args: unknown[]): Promise<unknown> {
  return (window as any).__SPLITTY_ELECTRON__?.invoke(channel, ...args) ?? Promise.resolve();
}

const electronAPI: WindowAPI = {
  minimize() { electronInvoke("window:minimize"); },
  maximize() { electronInvoke("window:maximize"); },
  unmaximize() { electronInvoke("window:unmaximize"); },
  toggleMaximize() { electronInvoke("window:toggle-maximize"); },
  close() { electronInvoke("window:close"); },
  isMaximized() { return electronInvoke("window:is-maximized") as Promise<boolean>; },
  openExternal(url) { electronInvoke("shell:open-external", url).catch(openFailed); return true; },
  startDragFromMaximized(screenX, screenY) { electronInvoke("window:start-drag", screenX, screenY); },
  startResizeDragging() {},
  setTitleBarStyle() {},
  setBackgroundColor() {},
};

// ---------------------------------------------------------------------------
// Web (no-op)
// ---------------------------------------------------------------------------

const webAPI: WindowAPI = {
  minimize() {},
  maximize() {},
  unmaximize() {},
  toggleMaximize() {},
  close() { window.close(); },
  isMaximized() { return Promise.resolve(false); },
  openExternal(url) {
    // Opened blank first and cut from this page, so the page it shows cannot
    // navigate Splitty's (xterm.js opens OSC 8 links the same way). Null when
    // the browser blocked the window, e.g. long after the click.
    const opened = window.open("", "_blank");
    if (!opened) return false;
    try {
      opened.opener = null;
    } catch {
      // Not ours to change any more; it still opens.
    }
    opened.location.href = url;
    return true;
  },
  startDragFromMaximized() {},
  startResizeDragging() {},
  setTitleBarStyle() {},
  setBackgroundColor() {},
};

// ---------------------------------------------------------------------------
// Exported singleton
// ---------------------------------------------------------------------------

export { tauriReady };

export const windowAPI: WindowAPI =
  shellKind === "tauri" ? tauriAPI :
  shellKind === "electron" ? electronAPI :
  webAPI;

// ---------------------------------------------------------------------------
// External targets
// ---------------------------------------------------------------------------

/**
 * Open a user-supplied target in the system browser.
 *
 * Splitty no longer renders foreign web content in-process (security review
 * SR-01/SR-02): serving it under the application's own origin let a
 * web context with no local privileges reach the parent DOM, the auth cookie and
 * the terminal API. Every "open this URL/port" path therefore hands off to the
 * OS browser instead of creating an iframe pane.
 *
 * This is where the UI checks a target's scheme. Only `http:` and `https:` are
 * forwarded — `javascript:`, `data:`, `file:` and unknown schemes are dropped
 * rather than passed to a shell opener that may or may not reject them itself.
 *
 * It is not the only check, and must not be relied on as one: libraries can
 * bypass it (ghostty-web opens OSC 8 hyperlinks with a bare `window.open`). The
 * desktop shells enforce the same rule natively for every new window — Electron
 * in its window-open handler, Tauri in its new-window guard (security review
 * SR-27, see docs/security/findings.md).
 *
 * Returns false when nothing opened: a refused scheme, or a window the
 * browser blocked.
 */
export function openExternalTarget(
  target: { port: number; query?: string } | { url: string },
): boolean {
  const raw = "port" in target
    ? `http://localhost:${target.port}/${target.query ?? ""}`
    : target.url;

  let parsed: URL;
  try {
    parsed = new URL(raw, window.location.origin);
  } catch {
    return false;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return false;

  return windowAPI.openExternal(parsed.toString());
}

/** A blank browser tab reserved during a click, before asynchronous verification. */
export interface ReservedExternalOpen {
  open(url: string): boolean;
  close(): void;
}

/** Native shells have no browser activation deadline and keep their usual opener. */
export function reserveExternalOpen(): ReservedExternalOpen | undefined {
  if (shellKind !== "web") return undefined;
  let opened: Window | null;
  try {
    opened = window.open("", "_blank");
  } catch {
    return undefined;
  }
  if (!opened) return undefined;
  let completed = false;
  const close = () => {
    if (completed) return;
    completed = true;
    try { opened.close(); } catch { /* A browser may have revoked access to the tab. */ }
  };
  try {
    // Do this while still blank; the service must never receive an opener.
    opened.opener = null;
    const policy = opened.document.createElement("meta");
    policy.name = "referrer";
    policy.content = "no-referrer";
    opened.document.head.append(policy);
    opened.document.title = "Opening forwarded port…";
    opened.document.body.textContent = "Waiting for Splitty to verify the port forward…";
  } catch {
    close();
    return undefined;
  }
  return {
    open(url) {
      if (completed || opened.closed) return false;
      try {
        const parsed = new URL(url);
        if (parsed.protocol !== "http:" && parsed.protocol !== "https:") { close(); return false; }
        opened.location.replace(parsed.href);
        completed = true;
        return true;
      } catch {
        close();
        return false;
      }
    },
    close,
  };
}

// ---------------------------------------------------------------------------
// Svelte actions for drag regions
// ---------------------------------------------------------------------------

/** Make an element a window drag region. */
export function dragRegion(node: HTMLElement): ActionReturn {
  if (shellKind === "tauri") {
    node.setAttribute("data-tauri-drag-region", "");
  } else if (shellKind === "electron") {
    node.style.setProperty("-webkit-app-region", "drag");
    node.style.setProperty("app-region", "drag");
  }
  return {
    destroy() {
      if (shellKind === "tauri") {
        node.removeAttribute("data-tauri-drag-region");
      } else if (shellKind === "electron") {
        node.style.removeProperty("-webkit-app-region");
        node.style.removeProperty("app-region");
      }
    },
  };
}

/** Exclude an element from the drag region (for interactive children). */
export function noDragRegion(node: HTMLElement): ActionReturn {
  if (shellKind === "electron") {
    node.style.setProperty("-webkit-app-region", "no-drag");
    node.style.setProperty("app-region", "no-drag");
  }
  // Tauri: only elements with data-tauri-drag-region are draggable, so children are excluded by default.
  return {
    destroy() {
      if (shellKind === "electron") {
        node.style.removeProperty("-webkit-app-region");
        node.style.removeProperty("app-region");
      }
    },
  };
}
