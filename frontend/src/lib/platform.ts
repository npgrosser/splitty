export type ShellKind = "tauri" | "web";

const params = new URLSearchParams(typeof window === "undefined" ? "" : window.location.search);

/** UI capabilities follow a native or authenticated gateway assignment. The
 * backend independently enforces authority; these flags choose presentation. */
export let isRemoteSession = false;
export let shellKind: ShellKind = "web";
/** Assigned carrier shell, including restricted remote documents. This does
 * not grant desktop commands; it only selects native navigation handoff. */
export let nativeShell: Exclude<ShellKind, "web"> | undefined;
export let isDesktop = false;
let assigned = false;
export function assignPlatform(shell: ShellKind, role: "main" | "remote"): void {
  if (assigned) throw Error("Platform already assigned for this document");
  assigned = true;
  nativeShell = shell === "web" ? undefined : shell;
  isRemoteSession = role === "remote";
  shellKind = isRemoteSession ? "web" : shell;
  isDesktop = shellKind !== "web";
  isPWA = !isDesktop && window.matchMedia("(display-mode: window-controls-overlay)").matches;
}

export const isDebug = params.has("debug");

export let isPWA = !isDesktop && typeof window !== "undefined"
  && window.matchMedia("(display-mode: window-controls-overlay)").matches;

export function getDesktopPlatform(): "mac" | "linux" | "windows" | "other" {
  const ua = navigator.userAgent.toLowerCase();
  const platform = navigator.platform.toLowerCase();

  if (platform.includes("mac") || ua.includes("mac os")) return "mac";
  if (platform.includes("win") || ua.includes("windows")) return "windows";
  if (platform.includes("linux") || ua.includes("linux")) return "linux";
  return "other";
}
