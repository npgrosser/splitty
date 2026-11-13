// Last observed host directories belong to the local hub, independently of
// open screens (`remote-screens`, Host Screen Directory Cache and Merged Host
// Directory). This controller reads the hub's merged directory
// (`/api/remote/directory`) and reports the window's observations of a
// host's screens (`hub.catalog.observe`); it never connects a host and keeps
// no directory of its own. A hub without `hub-host-decisions-v1` gives the
// window no directory: nothing is read or written then.

import { MAX_NAME, plainText } from "../protocol/untrusted.ts";
import { validateTarget } from "./api.ts";
import type { KnownHost } from "./known_hosts.ts";
import type { HostScreen } from "./remote_socket.ts";

const MAX_SCREENS = 200;
const MAX_ID = 256;

export interface HostCatalog {
  target: string;
  /** Unix seconds of the hub's last observation. */
  lastSeen: number;
  screens: HostScreen[];
}

export interface DirectoryHost {
  target: string; alias?: string; source?: "recent" | "ssh-config";
  lastConnected?: number; lastSeen?: number; screens: HostScreen[];
}

interface Effects {
  observe?(target: string, screens: readonly HostScreen[]): void;
  order?(targets: string[]): void;
  fetch(path: string, init?: RequestInit): Promise<Response>;
  changed(): void;
}

export class HostDirectory {
  private serverOwned = false;
  private merged: DirectoryHost[] = [];
  private revision = 0;
  private readonly reported = new Map<string, string>();
  private readonly catalog = new Map<string, HostCatalog>();
  private hosts: KnownHost[] = [];
  private readonly hidden = new Set<string>();
  private disposed = false;

  constructor(private readonly fx: Effects) {}

  /** A supporting hub owns the merge and all persistence; without one there is no directory. */
  useServerDirectory(enabled: boolean): void {
    if (this.serverOwned !== enabled) {
      // Cached entries belong to the previous hub.
      this.merged = [];
      this.catalog.clear();
      this.hosts = [];
    }
    this.serverOwned = enabled;
    this.revision++;
    this.reported.clear();
    this.fx.changed();
  }

  mergedHosts(): readonly DirectoryHost[] | undefined { return this.serverOwned ? this.merged : undefined; }

  async refresh(): Promise<void> {
    if (!this.serverOwned || this.disposed) return;
    const revision = ++this.revision;
    try {
      const response = await this.fx.fetch("/api/remote/directory");
      if (!response.ok) return;
      const value: unknown = await response.json();
      if (this.disposed || revision !== this.revision || !record(value) || value.version !== 1 || !Array.isArray(value.hosts)) return;
      const hosts: DirectoryHost[] = [];
      const seen = new Set<string>();
      for (const entry of value.hosts) {
        if (!record(entry) || typeof entry.target !== "string" || !validTarget(entry.target) || seen.has(entry.target) || this.hidden.has(entry.target)) continue;
        seen.add(entry.target);
        hosts.push({ target: entry.target, screens: parseScreens(Array.isArray(entry.screens) ? entry.screens : []),
          ...(typeof entry.alias === "string" ? {alias: plainText(entry.alias, MAX_NAME)} : {}),
          ...(entry.source === "recent" || entry.source === "ssh-config" ? {source: entry.source} : {}),
          ...(nonnegativeInteger(entry.lastConnected) ? {lastConnected: entry.lastConnected} : {}),
          ...(nonnegativeInteger(entry.lastSeen) ? {lastSeen: entry.lastSeen} : {}),
        });
      }
      this.merged = hosts;
      this.hosts = hosts.map((host) => ({target: host.target, source: host.source ?? "recent", alias: host.alias, lastConnected: host.lastConnected}));
      this.catalog.clear();
      for (const host of hosts) if (host.lastSeen !== undefined) this.catalog.set(host.target, {target:host.target, lastSeen:host.lastSeen, screens:host.screens});
      if (Array.isArray(value.order)) this.fx.order?.(value.order.filter((target): target is string => typeof target === "string" && validTarget(target)));
      this.fx.changed();
    } catch { /* Keep the last successful passive read. */ }
  }

  /** Newest first, detached from the controller's internal state. */
  entries(): HostCatalog[] {
    return this.ordered().map((entry) => ({ ...entry, screens: entry.screens.map((s) => ({ ...s })) }));
  }

  /** Both recent hosts and SSH-config aliases; the view decides which to show. */
  known(): KnownHost[] {
    return this.hosts.map((host) => ({ ...host }));
  }

  setHidden(target: string, hidden: boolean): void {
    if (!hidden) { this.hidden.delete(target); return; }
    this.hidden.add(target);
    this.revision++;
    this.merged = this.merged.filter((host) => host.target !== target);
    this.catalog.delete(target);
    this.hosts = this.hosts.filter((host) => host.target !== target);
    this.reported.delete(target);
    this.fx.changed();
  }

  load(): Promise<void> {
    return this.refresh();
  }

  /** Report one host's screens as the window sees them now, including a successfully empty list. */
  observe(target: string, screens: readonly HostScreen[]): void {
    if (this.disposed || !this.serverOwned || this.hidden.has(target) || !validTarget(target) || !Array.isArray(screens)) return;
    const clean = parseScreens(screens);
    const body = JSON.stringify({ target, screens: clean });
    if (this.reported.get(target) === body) return;
    this.reported.set(target, body);
    this.revision++;
    this.fx.observe?.(target, clean);
  }

  dispose(): void {
    this.disposed = true;
  }

  private ordered(): HostCatalog[] {
    // Reverse insertion order breaks ties in favor of the latest observation.
    return [...this.catalog.values()].reverse().sort((a, b) => b.lastSeen - a.lastSeen);
  }
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nonnegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function validTarget(value: unknown): value is string {
  if (typeof value !== "string" || validateTarget(value) !== value) return false;
  // The shared picker check is deliberately only a first sieve. Persisted
  // records also have to match the hub's [user@]host[:port] target grammar.
  const at = value.lastIndexOf("@");
  if (at >= 0 && !/^[a-zA-Z0-9._-]+$/.test(value.slice(0, at))) return false;
  const host = value.slice(at + 1);
  if (host.startsWith("[")) {
    const end = host.indexOf("]");
    if (end < 0) return false;
    const tail = host.slice(end + 1);
    if (tail && (!tail.startsWith(":") || !validPort(tail.slice(1)))) return false;
    try {
      // URL parsing validates a bracketed IPv6 literal without any network I/O.
      new URL(`http://${host.slice(0, end + 1)}/`);
      return true;
    } catch {
      return false;
    }
  }
  const [name, port, extra] = host.split(":");
  return extra === undefined && (port === undefined || validPort(port)) && name.length <= 253
    && name.split(".").every((label) => /^[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?$/.test(label));
}

function validPort(value: string): boolean {
  return /^\+?\d+$/.test(value) && Number(value) >= 1 && Number(value) <= 65535;
}

function parseScreens(value: readonly unknown[]): HostScreen[] {
  const screens: HostScreen[] = [];
  const ids = new Set<string>();
  for (const item of value) {
    if (screens.length >= MAX_SCREENS) break;
    if (!record(item) || typeof item.id !== "string" || !item.id || item.id.length > MAX_ID * 2
      || [...item.id].length > MAX_ID || /[\uD800-\uDFFF]/u.test(item.id) || ids.has(item.id)
      || !nonnegativeInteger(item.panes) || (item.name !== undefined && typeof item.name !== "string")) continue;
    ids.add(item.id);
    // plainText bounds UTF-16 units; remove a lone surrogate if a cut split an emoji.
    const name = plainText(item.name, MAX_NAME).replace(/[\uD800-\uDFFF]/gu, "");
    screens.push({ id: item.id, name, panes: item.panes });
  }
  return screens;
}
