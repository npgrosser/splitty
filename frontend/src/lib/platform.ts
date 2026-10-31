export type ShellKind = "electron" | "tauri" | "web";

const params = new URLSearchParams(window.location.search);

/**
 * A session a desktop app opened in a remote window: its URL names the host
 * (`host`, see `desktop-remote`). The page is the host's UI in a window without
 * native rights, so it behaves as in a browser — no custom title bar, no native
 * calls. Tauri injects its IPC bridge into every page, remote windows included,
 * so the global alone does not decide the shell.
 */
export const isRemoteSession = params.has("host");

export const shellKind: ShellKind =
  isRemoteSession ? "web" :
  (window as any).__TAURI_INTERNALS__ ? "tauri" :
  (window as any).__SPLITTY_ELECTRON__ ? "electron" :
  "web";

export const isDesktop = shellKind !== "web";

export const isDebug = params.has("debug");

export const isPWA = !isDesktop && typeof window !== "undefined"
  && window.matchMedia("(display-mode: window-controls-overlay)").matches;

export function getDesktopPlatform(): "mac" | "linux" | "windows" | "other" {
  if (shellKind === "electron") {
    const p = (window as any).__SPLITTY_ELECTRON__?.platform;
    if (p === "darwin") return "mac";
    if (p === "linux") return "linux";
    if (p === "win32") return "windows";
    return "other";
  }
  const ua = navigator.userAgent.toLowerCase();
  const platform = navigator.platform.toLowerCase();

  if (platform.includes("mac") || ua.includes("mac os")) return "mac";
  if (platform.includes("win") || ua.includes("windows")) return "windows";
  if (platform.includes("linux") || ua.includes("linux")) return "linux";
  return "other";
}
