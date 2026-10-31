import type { HostGroup } from "../../lib/remote/on_hosts.ts";
import type { PortRow } from "../../lib/remote/ports.ts";
import type { ShareRow } from "../../lib/remote/reverse_forwards.ts";

/** Presentation only: never infer tunnel health from a screen connection. */
export function hostSummary(group: HostGroup): string {
  const phase = group.state?.phase;
  const status = phase === "ready" ? "Connected"
    : phase === "connecting" ? "Connecting…"
    : phase === "reconnecting" ? "Reconnecting…"
    : phase === "failed" ? "Connection failed"
    : phase === "offline" ? "Offline" : "Not connected";
  const counts = [];
  if (group.open.length) counts.push(`${group.open.length} open`);
  if (group.screens.length) counts.push(`${group.screens.length} ${group.stale ? "last seen" : "available"}`);
  return [status, ...counts].join(" · ");
}

/** The heading note of a host's forwarded ports, or of the local ports shared with it. */
export function portsSummary(rows: readonly (PortRow | ShareRow)[]): string {
  if (rows.some((row) => row.pending)) return "Updating…";
  const errors = rows.filter((row) => row.error).length;
  if (errors) return `${errors} ${errors === 1 ? "needs" : "need"} attention`;
  if (rows.some((row) => row.forward?.state === "down")) return "Reconnecting…";
  const active = rows.filter((row) => row.forward?.state === "up").length;
  if (active) return `${active} active`;
  const paused = rows.filter((row) => row.saved?.enabled === false).length;
  if (paused) return `${paused} paused`;
  const saved = rows.filter((row) => row.saved).length;
  return saved ? `${saved} saved` : "";
}

export function portStatus(row: PortRow): string {
  if (row.pending) return { forward: "Forwarding…", stop: "Stopping…", save: "Saving…", forget: "Forgetting…" }[row.pending];
  if (row.error) return row.error.code === "local_port_conflict" ? "Local port in use" : row.saved?.lastError ? "Forward failed" : "Action failed";
  if (row.forward) return row.forward.state === "up" ? "Forwarded" : "Reconnecting…";
  if (row.saved?.enabled === false) return "Paused";
  if (row.saved) return "Starts when host connects";
  return "Detected";
}

/** The one action a port row shows next to its number. */
export function portPrimary(row: PortRow): "Open" | "Resume" | "Retry" | "Forward" {
  if (row.forward) return "Open";
  if (row.saved?.enabled === false) return "Resume";
  return row.error ? "Retry" : "Forward";
}

export type PortMenuAction = "stop" | "label" | "local" | "forget";

/** What a port row's "⋯" menu offers, with what each does where it is not obvious. */
export function portMenu(row: PortRow): { action: PortMenuAction; label: string; detail?: string }[] {
  const items: { action: PortMenuAction; label: string; detail?: string }[] = [];
  if (row.forward || row.saved?.enabled) {
    items.push({ action: "stop", label: "Stop", detail: row.saved ? "Pauses the saved rule until you resume it" : undefined });
  }
  items.push(row.saved
    ? { action: "label", label: "Edit label…" }
    : { action: "label", label: "Remember…", detail: "Forward it again on later connections" });
  if (!row.forward && row.saved && row.error?.code !== "local_port_conflict") {
    items.push({ action: "local", label: "Use another local port…" });
  }
  if (row.saved) {
    items.push({ action: "forget", label: "Forget", detail: row.forward ? "Keeps this forward running; Stop pauses it" : "Removes the saved mapping" });
  }
  return items;
}

// ── Shared Local Ports ──

export function shareStatus(row: ShareRow): string {
  if (row.pending) return { share: "Sharing…", stop: "Stopping…", save: "Saving…", forget: "Forgetting…" }[row.pending];
  if (row.forward) return row.forward.state === "up" ? "Shared" : "Reconnecting…";
  if (row.error) {
    return row.error.code === "host_port_conflict" ? "Port in use on the host"
      : row.error.code === "unsupported_host" ? "Not supported on this host"
      : row.saved?.lastError ? "Share failed" : "Action failed";
  }
  if (row.saved?.enabled === false) return "Paused";
  return "Starts when host connects";
}

/**
 * The one action a share row shows. Start, Resume and Retry share a listed
 * rule again; it was consented to when it was shared, so they do not warn.
 */
export function sharePrimary(row: ShareRow): "Stop" | "Resume" | "Retry" | "Start" {
  if (row.forward) return "Stop";
  if (row.error) return "Retry";
  return row.saved?.enabled === false ? "Resume" : "Start";
}

export type ShareMenuAction = "stop" | "remember" | "forget";

/** What a share row's "⋯" menu offers, with what each does where it is not obvious. */
export function shareMenu(row: ShareRow): { action: ShareMenuAction; label: string; detail?: string }[] {
  const items: { action: ShareMenuAction; label: string; detail?: string }[] = [];
  if (!row.forward && row.saved?.enabled) {
    items.push({ action: "stop", label: "Stop", detail: "Pauses the saved share until you resume it" });
  }
  if (row.saved) {
    items.push({ action: "forget", label: "Forget", detail: row.forward ? "Keeps sharing now; Stop ends it" : "Removes the saved share" });
  } else {
    items.push({ action: "remember", label: "Remember", detail: "Share it again when this host connects" });
  }
  return items;
}
