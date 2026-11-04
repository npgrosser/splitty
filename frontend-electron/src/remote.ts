// Pure target, host-list and window-title helpers for native remote windows.

export interface KnownHost {
  target: string;
  alias?: string;
  source: "recent" | "ssh-config";
  lastConnected?: number;
}

export interface MigrationOffer {
  available: true;
  fromVersion: string;
  toVersion: string;
  sessions: number;
}

/** Copy only the bounded, descriptive fields supplied by native discovery. */
export function migrationOffer(value: unknown): MigrationOffer | undefined {
  if (!value || typeof value !== "object") return undefined;
  const offer = value as Partial<MigrationOffer>;
  if (offer.available !== true ||
      typeof offer.fromVersion !== "string" || !offer.fromVersion || offer.fromVersion.length > 64 ||
      typeof offer.toVersion !== "string" || !offer.toVersion || offer.toVersion.length > 64 ||
      !Number.isInteger(offer.sessions) || offer.sessions! < 0 || offer.sessions! > 0xffffffff) return undefined;
  return { available: true, fromVersion: offer.fromVersion, toVersion: offer.toVersion, sessions: offer.sessions! };
}

/** The native host.list reply body; nothing for anything unexpected. */
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
 * The native helper validates it fully as `[user@]host[:port]` before SSH.
 * This preliminary check keeps out what must never reach its command line: an empty target, one led by a dash (Unicode
 * lookalikes included) that could pass for an option, and whitespace or
 * control characters.
 */
export function isAcceptableTarget(target: unknown): target is string {
  if (typeof target !== "string" || !target) return false;
  if (/^[-‐-―−﹘﹣－]/.test(target)) return false;
  return !/[\s\u0000-\u001F\u007F-\u009F]/.test(target);
}
