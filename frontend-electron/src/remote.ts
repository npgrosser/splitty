// The pure parts of the remote windows (`desktop-remote`): the lines of
// `splitty connect --json`, the known hosts, the window title and the checks
// on what reaches a process or a window.

/** One line of `splitty connect --json` (`cli`, Remote Connect Machine-Readable Output). */
export type ConnectEvent =
  | { event: "progress"; step: string; message: string }
  | { event: "notice"; message: string }
  | { event: "ready"; url: string; port: number }
  | { event: "lost"; message: string }
  | { event: "retry"; message: string; retryIn: number }
  | { event: "restored" }
  | { event: "url"; url: string }
  | { event: "failed"; message: string; hint?: string }
  | { event: "closed" };

/** The event on a line, or null for anything that is not one. */
export function parseConnectEvent(line: string): ConnectEvent | null {
  let value: any;
  try {
    value = JSON.parse(line);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object") return null;
  const text = (field: unknown): field is string => typeof field === "string";

  switch (value.event) {
    case "progress":
      return text(value.step) && text(value.message)
        ? { event: "progress", step: value.step, message: value.message }
        : null;
    case "notice":
      return text(value.message) ? { event: "notice", message: value.message } : null;
    case "ready":
      return text(value.url) && typeof value.port === "number"
        ? { event: "ready", url: value.url, port: value.port }
        : null;
    case "lost":
      return text(value.message) ? { event: "lost", message: value.message } : null;
    case "retry":
      return text(value.message)
        ? { event: "retry", message: value.message, retryIn: typeof value.retryIn === "number" ? value.retryIn : 0 }
        : null;
    case "restored":
      return { event: "restored" };
    case "url":
      return text(value.url) ? { event: "url", url: value.url } : null;
    case "failed":
      if (!text(value.message)) return null;
      return text(value.hint) && value.hint
        ? { event: "failed", message: value.message, hint: value.hint }
        : { event: "failed", message: value.message };
    case "closed":
      return { event: "closed" };
    default:
      return null;
  }
}

export interface KnownHost {
  target: string;
  alias?: string;
  source: "recent" | "ssh-config";
  lastConnected?: number;
}

/** The output of `splitty remote-hosts --json`; nothing for anything unexpected. */
export function parseKnownHosts(output: string): KnownHost[] {
  let value: unknown;
  try {
    value = JSON.parse(output);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  return value.flatMap((item): KnownHost[] => {
    if (!item || typeof item.target !== "string") return [];
    if (item.source !== "recent" && item.source !== "ssh-config") return [];
    const host: KnownHost = { target: item.target, source: item.source };
    if (typeof item.alias === "string" && item.alias.trim()) host.alias = item.alias.trim();
    if (typeof item.lastConnected === "number") host.lastConnected = item.lastConnected;
    return [host];
  });
}

export type ConnectionState = "connected" | "reconnecting" | "disconnected";

/** The title of a remote window, which names the host and its connection state (D4). */
export function remoteTitle(target: string, state: ConnectionState): string {
  const status = state === "reconnecting" ? " (reconnecting…)" : state === "disconnected" ? " (disconnected)" : "";
  return `${target}${status} — Splitty`;
}

/**
 * A first check of a target before a process sees it.
 *
 * `splitty connect` validates it fully as `[user@]host[:port]` and exits with
 * status 2 otherwise (`access-control`). This only keeps out what must never
 * reach its command line: an empty target, one led by a dash (Unicode
 * lookalikes included) that could pass for an option, and whitespace or
 * control characters.
 */
export function isAcceptableTarget(target: unknown): target is string {
  if (typeof target !== "string" || !target) return false;
  if (/^[-‐-―−﹘﹣－]/.test(target)) return false;
  return !/[\s\u0000-\u001F\u007F-\u009F]/.test(target);
}

/**
 * The origin a remote window is pinned to, from a session URL of `connect`:
 * plain `http` on `127.0.0.1`, the local end of the tunnel. Anything else is
 * never loaded into a remote window.
 */
export function tunnelOrigin(url: string): string | null {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" && parsed.hostname === "127.0.0.1" ? parsed.origin : null;
  } catch {
    return null;
  }
}

/** Why `connect` ended before it reported anything: the last line it printed on stderr. */
export function exitFailure(code: number | null, stderr: string): string {
  const lines = stderr.split("\n").map((line) => line.trim()).filter(Boolean);
  return lines[lines.length - 1] ?? `splitty connect ended with status ${code ?? "unknown"}.`;
}
