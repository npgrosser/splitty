// Last observed host directories belong to the local hub, independently of
// open screens. This controller only reads/writes metadata: it never connects
// a host. The window calls flush(true) on pagehide and dispose() on teardown.

import { MAX_NAME, plainText } from "../protocol/untrusted.ts";
import { validateTarget } from "./api.ts";
import { parseKnownHosts, type KnownHost } from "./known_hosts.ts";
import type { HostScreen } from "./remote_socket.ts";

const PATH = "/api/remote/catalog";
const MAX_HOSTS = 20;
const MAX_SCREENS = 200;
const MAX_ID = 256;
const WRITE_DELAY = 400;

export interface HostCatalog {
  target: string;
  /** Unix seconds; successful writes replace the provisional observation time. */
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

interface WriteState {
  body: string;
  revision: number;
  observedAt: number;
  /** Last accepted or rejected (400) body; unchanged rejections are not retried. */
  settled?: string;
  rejected?: string;
  inFlight: boolean;
  queued: boolean;
  keepalive: boolean;
  timer?: ReturnType<typeof setTimeout>;
}

export class HostDirectory {
  private serverOwned = false;
  private merged: DirectoryHost[] = [];
  private revision = 0;
  private readonly reported = new Map<string, string>();
  private readonly catalog = new Map<string, HostCatalog>();
  private readonly writes = new Map<string, WriteState>();
  private hosts: KnownHost[] = [];
  private readonly hidden = new Set<string>();
  /** Includes evicted observations, so a late initial read cannot resurrect them. */
  private readonly observed = new Set<string>();
  private readDone = false;
  private loading?: Promise<void>;
  private disposed = false;

  constructor(private readonly fx: Effects) {}

  /** A supporting hub owns the merge and all persistence. */
  useServerDirectory(enabled: boolean): void {
    if (this.serverOwned !== enabled) {
      // Cached entries belong to the previous authority. An older hub's
      // separate reads only merge catalog entries, so retaining them here
      // would show screens the newly connected hub never returned.
      this.merged = [];
      this.catalog.clear();
      this.hosts = [];
      this.observed.clear();
      this.readDone = false;
    }
    this.serverOwned = enabled;
    this.revision++;
    this.reported.clear();
    this.loading = undefined;
    if (enabled) {
      for (const state of this.writes.values()) clearTimeout(state.timer);
      this.writes.clear();
    }
    this.fx.changed();
  }

  mergedHosts(): readonly DirectoryHost[] | undefined { return this.serverOwned ? this.merged : undefined; }

  async refresh(): Promise<void> {
    if (!this.serverOwned) { this.loading = undefined; await this.load(); return; }
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
    const state = this.writes.get(target);
    if (state) clearTimeout(state.timer);
    this.writes.delete(target);
    this.fx.changed();
  }

  load(): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.serverOwned) return this.refresh();
    // Independent reads: a failed catalog must not hide the recent hosts.
    return this.loading ??= Promise.all([this.readCatalog(), this.readHosts()]).then(() => {});
  }

  /** Replace one host's last observation, including a successfully empty list. */
  observe(target: string, screens: readonly HostScreen[]): void {
    if (this.disposed || this.hidden.has(target) || !validTarget(target) || !Array.isArray(screens)) return;
    const clean = parseScreens(screens);
    const body = JSON.stringify({ target, screens: clean });
    if (this.serverOwned) {
      if (this.reported.get(target) !== body) {
        this.reported.set(target, body);
        this.revision++;
        this.fx.observe?.(target, clean);
      }
      return;
    }
    const now = Math.floor(Date.now() / 1000);
    if (!this.readDone) this.observed.add(target);
    let state = this.writes.get(target);
    if (state?.body === body && state.observedAt === now && this.catalog.has(target)) {
      // A fresh observation may retry a transient failure, but not an unchanged
      // successful write or a body the hub has already refused.
      if (!state.inFlight && state.settled !== body) this.schedule(target, state);
      return;
    }
    if (state) {
      if (state.body !== body) state.rejected = undefined;
      state.body = body;
      state.revision++;
      state.observedAt = now;
      state.settled = undefined;
    } else {
      state = { body, revision: 1, observedAt: now, inFlight: false, queued: false, keepalive: false };
      this.writes.set(target, state);
    }
    this.catalog.delete(target);
    this.catalog.set(target, { target, lastSeen: now, screens: clean });
    this.trim();
    this.schedule(target, state);
    this.fx.changed();
  }

  /**
   * Send pending observations now. A pagehide flush marks queued follow-ups as
   * keepalive too, but never overtakes a write to the same host: HTTP arrival
   * order must not let an older observation become the stored directory.
   */
  flush(keepalive = false): void {
    if (this.disposed) return;
    for (const [target, state] of this.writes) {
      if (!this.catalog.has(target)) continue;
      clearTimeout(state.timer);
      state.timer = undefined;
      this.send(target, state, keepalive);
    }
  }

  dispose(): void {
    this.disposed = true;
    for (const [target, state] of this.writes) {
      clearTimeout(state.timer);
      state.timer = undefined;
      // Teardown calls flush(true) first. Let that explicit final observation
      // drain behind its older write, without publishing any more UI changes.
      if (!(state.inFlight && state.queued && state.keepalive)) this.writes.delete(target);
    }
    this.observed.clear();
  }

  private ordered(): HostCatalog[] {
    // Reverse insertion order breaks ties in favor of the latest observation.
    return [...this.catalog.values()].reverse().sort((a, b) => b.lastSeen - a.lastSeen);
  }

  private trim(): void {
    for (const entry of this.ordered().slice(MAX_HOSTS)) {
      this.catalog.delete(entry.target);
      const state = this.writes.get(entry.target);
      if (!state) continue;
      clearTimeout(state.timer);
      state.timer = undefined;
      state.queued = false;
      // Keep an in-flight writer until it ends, even if its entry was evicted.
      // Observing that host again must still serialize behind this request.
      if (!state.inFlight) this.writes.delete(entry.target);
    }
  }

  private async readCatalog(): Promise<void> {
    const revision = this.revision;
    try {
      const res = await this.fx.fetch(PATH);
      if (!res.ok) return;
      const value: unknown = await res.json();
      if (this.disposed || this.serverOwned || revision !== this.revision || !record(value) || value.version !== 1 || !Array.isArray(value.hosts)) return;
      // The hub returns newest first, including writes within the same second.
      // Insert oldest first so ties retain that order in ordered() and trim().
      for (const item of [...value.hosts].reverse()) {
        const entry = parseEntry(item);
        if (!entry || this.hidden.has(entry.target) || this.observed.has(entry.target)) continue;
        const current = this.catalog.get(entry.target);
        if (!current || entry.lastSeen >= current.lastSeen) {
          this.catalog.delete(entry.target);
          this.catalog.set(entry.target, entry);
        }
        this.trim();
      }
      this.fx.changed();
    } catch {
      // A failed read is not an empty observation and never writes or deletes.
    } finally {
      if (revision === this.revision) {
        this.readDone = true;
        this.observed.clear();
      }
    }
  }

  private async readHosts(): Promise<void> {
    const revision = this.revision;
    try {
      const res = await this.fx.fetch("/api/remote/hosts");
      if (!res.ok) return;
      const value: unknown = await res.json();
      if (this.disposed || this.serverOwned || revision !== this.revision || !Array.isArray(value)) return;
      const hosts = new Map<string, KnownHost>();
      for (const host of parseKnownHosts(value)) {
        if (this.hidden.has(host.target) || !validTarget(host.target)) continue;
        if (!nonnegativeInteger(host.lastConnected)) delete host.lastConnected;
        const prior = hosts.get(host.target);
        if (!prior || (prior.source !== "recent" && host.source === "recent")) hosts.set(host.target, host);
      }
      this.hosts = [...hosts.values()];
      this.fx.changed();
    } catch {
      // Known hosts are independent of the catalog; keep either successful read.
    }
  }

  private schedule(target: string, state: WriteState): void {
    if (!this.catalog.has(target)) return;
    clearTimeout(state.timer);
    state.timer = setTimeout(() => {
      state.timer = undefined;
      this.send(target, state, false);
    }, WRITE_DELAY);
  }

  private send(target: string, state: WriteState, keepalive: boolean): void {
    if ((this.disposed && !keepalive) || this.writes.get(target) !== state || !this.catalog.has(target)) return;
    if (state.inFlight) {
      state.queued = true;
      state.keepalive ||= keepalive;
      return;
    }
    if (state.body === state.settled || state.body === state.rejected) return;
    // Once a different request starts, a previous success is no longer proof
    // of what is stored (even a lost response may have committed the write).
    state.settled = undefined;
    state.inFlight = true;
    state.queued = false;
    const alive = state.keepalive || keepalive;
    state.keepalive = false;
    void this.write(target, state, alive);
  }

  private async write(target: string, state: WriteState, keepalive: boolean): Promise<void> {
    const { body, revision } = state;
    try {
      const res = await this.fx.fetch(PATH, {
        method: "PUT", headers: { "Content-Type": "application/json" }, body, keepalive,
      });
      if (res.status === 400 || res.status === 409) {
        state.settled = body;
        state.rejected = body;
      } else if (res.ok) {
        const entry = parseEntry(await res.json());
        // A malformed or mismatched reply must not modify another host or
        // acknowledge an observation it did not store.
        if (!entry || entry.target !== target || JSON.stringify({ target, screens: entry.screens }) !== body) return;
        if (state.revision === revision) state.settled = body;
        if (!this.disposed && this.writes.get(target) === state && this.catalog.has(target) && state.revision === revision) {
          this.catalog.set(target, entry);
          this.trim();
          this.fx.changed();
        }
      }
    } catch {
      // No timer retries: another observation or explicit flush can retry a
      // transient failure. The local snapshot stays useful while offline.
    } finally {
      state.inFlight = false;
      if (this.writes.get(target) === state) {
        if (!this.catalog.has(target) || (this.disposed && !state.keepalive)) this.writes.delete(target);
        else if (state.queued && state.revision !== revision) this.send(target, state, state.keepalive);
        else if (this.disposed) this.writes.delete(target);
      }
    }
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

function parseEntry(value: unknown): HostCatalog | undefined {
  if (!record(value) || !validTarget(value.target) || !nonnegativeInteger(value.lastSeen)
    || !Array.isArray(value.screens)) return;
  return { target: value.target, lastSeen: value.lastSeen, screens: parseScreens(value.screens) };
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
