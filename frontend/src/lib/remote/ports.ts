// The ports of a host and their forwards (`remote-screens`, Detected Ports In
// The Window): what a host's `ports.changed` lists, which of those ports are
// new, the local server's forwards (`/api/remote/forwards`) joined with them,
// and the links of a host's pane that point at its loopback.
//
// A host's list is untrusted data: ports are checked, process names are plain
// text with the caps of `untrusted.ts`, and the list is bounded.

import { hostOf, type ConnKey } from "../connection/server_connection.ts";
import { MAX_NAME, MAX_TEXT, MAX_TITLE, plainLines, plainText } from "../protocol/untrusted.ts";
import { validateTarget } from "./api.ts";

/** How many ports of one host the client keeps; a longer list is cut (provisional, tbd). */
export const MAX_PORTS = 256;

/** A port a process of a host's session listens on. */
export interface DetectedPort {
  port: number;
  sessionId: string;
  /** The process's name, as plain text; may be empty. */
  process: string;
}

/** A forward of the local server, from a local port to a port of a host. */
export interface RemoteForward {
  target: string;
  /** The host's port. */
  remote: number;
  /** The port here. */
  local: number;
  /** Always `http://localhost:<local>`, never `127.0.0.1`, whose cookies are Splitty's. */
  url: string;
  /** `down` while the local server rebuilds it. */
  state: "up" | "down";
}

/** What the client is doing for a port: opening its forward or stopping it. */
export type PortAction = "forward" | "stop" | "save" | "forget";

export const MAX_SAVED_FORWARDS = 256;
export const MAX_FORWARD_LABEL = 200;

export interface ForwardFailure {
  code: string;
  message: string;
  hint?: string;
}

export interface SavedForward {
  target: string;
  remote: number;
  local: number;
  label: string;
  enabled: boolean;
  lastError?: ForwardFailure;
}

/** Strip hidden formatting and cap Unicode characters without splitting a surrogate pair. */
export function forwardLabel(value: string): string {
  return [...plainText(value, value.length)].slice(0, MAX_FORWARD_LABEL).join("");
}

export function parseSavedForward(value: unknown): SavedForward | null {
  if (typeof value !== "object" || value === null) return null;
  const { target, remote, local, label, enabled, lastError } = value as Record<string, unknown>;
  if (typeof target !== "string" || validateTarget(target) !== target || !isPort(remote) || !isPort(local)
    || typeof label !== "string" || typeof enabled !== "boolean") return null;
  const saved: SavedForward = { target, remote, local, label: forwardLabel(label), enabled };
  if (typeof lastError === "object" && lastError !== null) {
    const { code, message, hint } = lastError as Record<string, unknown>;
    if (typeof code === "string" && typeof message === "string") {
      saved.lastError = { code: plainText(code, MAX_NAME), message: plainLines(message, MAX_TEXT) };
      if (typeof hint === "string") saved.lastError.hint = plainText(hint, MAX_TEXT);
    }
  }
  return saved;
}

/** Null means an invalid read, not an empty preference list. */
export function parseSavedForwards(value: unknown): SavedForward[] | null {
  if (typeof value !== "object" || value === null) return null;
  const { version, forwards } = value as Record<string, unknown>;
  if (version !== 1 || !Array.isArray(forwards) || forwards.length > MAX_SAVED_FORWARDS) return null;
  const entries = new Map<string, SavedForward>();
  const counts = new Map<string, number>();
  for (const entry of forwards) {
    const saved = parseSavedForward(entry);
    if (!saved || entries.has(portKey(saved.target, saved.remote))) return null;
    const count = (counts.get(saved.target) ?? 0) + 1;
    if (count > 32) return null;
    counts.set(saved.target, count);
    entries.set(portKey(saved.target, saved.remote), saved);
  }
  return [...entries.values()].sort(byTargetAndPort);
}

/** A port of a host as the Ports list shows it. */
export interface PortRow {
  port: number;
  /** From the host's last list; undefined when the host does not list the port (any more). */
  process?: string;
  forward?: RemoteForward;
  saved?: SavedForward;
  error?: ForwardFailure;
  pending?: PortAction;
}

/** The Ports list of one host. */
export interface HostPorts {
  target: string;
  rows: PortRow[];
}

export function isPort(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= 65535;
}

/** The address a forward is opened at. */
export function forwardUrl(local: number): string {
  return `http://localhost:${local}`;
}

// ── The Host's List ──

/**
 * The ports of a `ports.changed`, ordered by port, each port once and at most
 * {@link MAX_PORTS}; null when it carries no list. Entries without a valid port
 * are dropped.
 */
export function parsePorts(value: unknown): DetectedPort[] | null {
  if (!Array.isArray(value)) return null;
  const byPort = new Map<number, DetectedPort>();
  for (const entry of value) {
    if (byPort.size >= MAX_PORTS) break;
    if (typeof entry !== "object" || entry === null) continue;
    const { port, sessionId, process } = entry as Record<string, unknown>;
    if (!isPort(port) || byPort.has(port)) continue;
    byPort.set(port, {
      port,
      sessionId: plainText(sessionId, MAX_NAME),
      process: plainText(process, MAX_TITLE),
    });
  }
  return [...byPort.values()].sort((a, b) => a.port - b.port);
}

/**
 * The ports of `next` that `previous` did not list. The first list of a
 * connection, and the first after a reconnect (`previous` undefined), lists
 * what was listening already: it is shown, but nothing in it counts as new
 * (provisional, tbd).
 */
export function appearedPorts(previous: readonly DetectedPort[] | undefined, next: readonly DetectedPort[]): DetectedPort[] {
  if (!previous) return [];
  const known = new Set(previous.map((p) => p.port));
  return next.filter((p) => !known.has(p.port));
}

// ── Forwards ──

/** A forward as the local server answers with it, or null when it has not the shape. */
export function parseForward(value: unknown): RemoteForward | null {
  if (typeof value !== "object" || value === null) return null;
  const { target, remote, local, state } = value as Record<string, unknown>;
  if (typeof target !== "string" || validateTarget(target) !== target) return null;
  if (!isPort(remote) || !isPort(local)) return null;
  return { target, remote, local, url: forwardUrl(local), state: state === "down" ? "down" : "up" };
}

/** The forwards of a `GET /api/remote/forwards`, ordered by target and port. */
export function parseForwards(value: unknown): RemoteForward[] {
  if (!Array.isArray(value)) return [];
  const forwards: RemoteForward[] = [];
  for (const entry of value) {
    const forward = parseForward(entry);
    if (forward) forwards.push(forward);
  }
  return forwards.sort(byTargetAndPort);
}

function byTargetAndPort(a: { target: string; remote: number }, b: { target: string; remote: number }): number {
  if (a.target !== b.target) return a.target < b.target ? -1 : 1;
  return a.remote - b.remote;
}

/** `forwards` with `forward` in place of the one of the same target and port. */
export function withForward<T extends { target: string; remote: number }>(forwards: readonly T[], forward: T): T[] {
  return [...forwards.filter((f) => f.target !== forward.target || f.remote !== forward.remote), forward].sort(byTargetAndPort);
}

/** The key of a host's port among the client's pending actions. */
export function portKey(target: string, port: number): string {
  return `${target}\n${port}`;
}

/**
 * The Ports lists: per watched host (in the given order) its listed ports,
 * forwarded or not, and its forwards whose port it does not list; then the
 * saved and active mappings of disconnected hosts. Missing discovery says
 * nothing about whether a manually entered service listens. Empty hosts are omitted.
 */
export function portLists(
  hosts: readonly { key: ConnKey; ports: readonly DetectedPort[] | undefined }[],
  forwards: readonly RemoteForward[],
  pending: (target: string, port: number) => PortAction | undefined = () => undefined,
  saved: readonly SavedForward[] = [],
): HostPorts[] {
  const targets: string[] = [];
  const listed = new Map<string, readonly DetectedPort[]>();
  for (const host of hosts) {
    const target = hostOf(host.key);
    if (target === undefined || listed.has(target)) continue;
    targets.push(target);
    listed.set(target, host.ports ?? []);
  }
  for (const forward of [...forwards, ...saved]) {
    if (!listed.has(forward.target)) {
      targets.push(forward.target);
      listed.set(forward.target, []);
    }
  }

  const lists: HostPorts[] = [];
  for (const target of targets) {
    const rows = new Map<number, PortRow>();
    for (const port of listed.get(target) ?? []) rows.set(port.port, { port: port.port, process: port.process });
    for (const forward of forwards) {
      if (forward.target !== target) continue;
      const row = rows.get(forward.remote) ?? { port: forward.remote };
      rows.set(forward.remote, { ...row, forward });
    }
    for (const preference of saved) {
      if (preference.target !== target) continue;
      const row = rows.get(preference.remote) ?? { port: preference.remote };
      rows.set(preference.remote, { ...row, saved: preference });
    }
    for (const [port, row] of rows) {
      const action = pending(target, port);
      if (action) row.pending = action;
    }
    if (rows.size > 0) lists.push({ target, rows: [...rows.values()].sort((a, b) => a.port - b.port) });
  }
  return lists;
}

// ── Links ──

/** The hosts of a link that mean the host itself. */
export const LOOPBACK_HOSTS: readonly string[] = ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"];

/** A link to a port on the loopback of the host whose pane printed it. */
export interface LoopbackLink {
  url: URL;
  port: number;
}

/**
 * The loopback link `raw` is, or null: an `http` or `https` URL whose host is
 * one of {@link LOOPBACK_HOSTS}. Without a port it is the scheme's, 80 or 443
 * (provisional, tbd).
 */
export function loopbackLink(raw: string): LoopbackLink | null {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (!LOOPBACK_HOSTS.includes(url.hostname)) return null;
  const port = url.port === "" ? (url.protocol === "https:" ? 443 : 80) : Number(url.port);
  return isPort(port) ? { url, port } : null;
}

/** The link at the forward's local port: host `localhost`, scheme, path, query and fragment kept. */
export function forwardedLink(link: LoopbackLink, local: number): string {
  const url = new URL(link.url.href);
  url.hostname = "localhost";
  url.port = String(local);
  return url.href;
}
