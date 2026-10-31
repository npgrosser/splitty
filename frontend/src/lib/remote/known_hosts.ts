// The hosts a user knows (`remote-hosts`, Known Hosts), as the desktop shells
// and the hub (`GET /api/remote/hosts`) list them for a host picker.

export interface KnownHost {
  target: string;
  alias?: string;
  source: "recent" | "ssh-config";
  /** Seconds since the Unix epoch; recent hosts only. */
  lastConnected?: number;
}

/** Keep the entries that have the documented shape, in their order. */
export function parseKnownHosts(value: unknown): KnownHost[] {
  if (!Array.isArray(value)) return [];
  const hosts: KnownHost[] = [];
  for (const item of value) {
    if (!item || typeof item.target !== "string") continue;
    if (item.source !== "recent" && item.source !== "ssh-config") continue;
    const host: KnownHost = { target: item.target, source: item.source };
    if (typeof item.alias === "string" && item.alias.trim()) host.alias = item.alias.trim();
    if (typeof item.lastConnected === "number") host.lastConnected = item.lastConnected;
    hosts.push(host);
  }
  return hosts;
}
