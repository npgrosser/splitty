// Local ports shared with a host: reverse forwards (`remote-screens`, Reverse
// Forwards Through The Hub, Saved Reverse Forwards, Sharing Local Ports In The
// Window).
//
// A reverse forward makes `localhost:<local>` of this machine reachable as
// `localhost:<remote>` for the user's programs on a host. The window starts one
// only on the user's own action for a chosen host: Share in the form, and
// Start, Resume or Retry of a listed rule, which was consented to when it was
// shared (they do not warn again). Nothing here reacts to a host's messages,
// detected ports or links in panes, and there is no automatic mode; the hub
// restores enabled saved rules itself when their host connects.
//
// As for forwards (`port_forwarding.ts`), active and saved state are read
// independently and passively, mutations are ordered per mapping, and
// generations drop answers that a later action superseded.

import { validateTarget } from "./api.ts";
import { portNumber } from "./manual_port.ts";
import { ForwardError, REFRESH_INTERVAL, failure, jsonRequest } from "./port_forwarding.ts";
import { byTargetAndPort, isPort, parseFailure, portKey, withForward, type ForwardFailure } from "./ports.ts";

export const REVERSE_FORWARDS_PATH = "/api/remote/reverse-forwards";
export const REVERSE_PREFERENCES_PATH = "/api/remote/reverse-forward-preferences";
export const MAX_SAVED_REVERSE_FORWARDS = 256;
const MAX_SAVED_PER_HOST = 32;

/** A local port shared with a host. */
export interface ReverseForward {
  target: string;
  /** The host's port. */
  remote: number;
  /** The port here, always on `localhost`. */
  local: number;
  /** `down` while the local server rebuilds it. */
  state: "up" | "down";
}

/** A remembered share; restored when its host connects while enabled. */
export interface SavedReverseForward {
  target: string;
  remote: number;
  local: number;
  enabled: boolean;
  lastError?: ForwardFailure;
}

/** What the client is doing for a share. */
export type ShareAction = "share" | "stop" | "save" | "forget";

/** A share of one host as the Hosts panel lists it: active, saved or both. */
export interface ShareRow {
  /** The host's port. */
  remote: number;
  /** The port here: the active share's, else the saved rule's. */
  local: number;
  forward?: ReverseForward;
  saved?: SavedReverseForward;
  error?: ForwardFailure;
  pending?: ShareAction;
}

export interface HostShares {
  target: string;
  rows: ShareRow[];
}

// ── Reads ──

export function parseReverseForward(value: unknown): ReverseForward | null {
  if (typeof value !== "object" || value === null) return null;
  const { target, remote, local, state } = value as Record<string, unknown>;
  if (typeof target !== "string" || validateTarget(target) !== target || !isPort(remote) || !isPort(local)) return null;
  return { target, remote, local, state: state === "down" ? "down" : "up" };
}

/** The active shares of a `GET`, ordered by target and port; null for an invalid read. */
export function parseReverseForwards(value: unknown): ReverseForward[] | null {
  if (!Array.isArray(value)) return null;
  const forwards: ReverseForward[] = [];
  for (const entry of value) {
    const forward = parseReverseForward(entry);
    if (!forward) return null;
    forwards.push(forward);
  }
  return forwards.sort(byTargetAndPort);
}

export function parseSavedReverseForward(value: unknown): SavedReverseForward | null {
  if (typeof value !== "object" || value === null) return null;
  const { target, remote, local, enabled, lastError } = value as Record<string, unknown>;
  if (typeof target !== "string" || validateTarget(target) !== target || !isPort(remote) || !isPort(local)
    || typeof enabled !== "boolean") return null;
  const saved: SavedReverseForward = { target, remote, local, enabled };
  const failure = parseFailure(lastError);
  if (failure) saved.lastError = failure;
  return saved;
}

/** Null means an invalid read, not an empty rule list. */
export function parseSavedReverseForwards(value: unknown): SavedReverseForward[] | null {
  if (typeof value !== "object" || value === null) return null;
  const { version, forwards } = value as Record<string, unknown>;
  if (version !== 1 || !Array.isArray(forwards) || forwards.length > MAX_SAVED_REVERSE_FORWARDS) return null;
  const entries = new Map<string, SavedReverseForward>();
  const counts = new Map<string, number>();
  for (const entry of forwards) {
    const saved = parseSavedReverseForward(entry);
    if (!saved || entries.has(portKey(saved.target, saved.remote))) return null;
    const count = (counts.get(saved.target) ?? 0) + 1;
    if (count > MAX_SAVED_PER_HOST) return null;
    counts.set(saved.target, count);
    entries.set(portKey(saved.target, saved.remote), saved);
  }
  return [...entries.values()].sort(byTargetAndPort);
}

/**
 * Per host (ordered by target) its active shares and saved rules, one row per
 * host port, ordered by port. An active share shows its own local port.
 */
export function shareLists(
  forwards: readonly ReverseForward[],
  saved: readonly SavedReverseForward[],
  pending: (target: string, port: number) => ShareAction | undefined = () => undefined,
  error: (target: string, port: number) => ForwardFailure | undefined = () => undefined,
): HostShares[] {
  const hosts = new Map<string, Map<number, ShareRow>>();
  const rowOf = (target: string, remote: number, local: number): ShareRow => {
    const rows = hosts.get(target) ?? new Map<number, ShareRow>();
    hosts.set(target, rows);
    const row = rows.get(remote) ?? { remote, local };
    rows.set(remote, row);
    return row;
  };
  for (const forward of forwards) rowOf(forward.target, forward.remote, forward.local).forward = forward;
  for (const rule of saved) rowOf(rule.target, rule.remote, rule.local).saved = rule;
  return [...hosts.keys()].sort().map((target) => ({
    target,
    rows: [...hosts.get(target)!.values()].sort((a, b) => a.remote - b.remote).map((row) => {
      const action = pending(target, row.remote);
      const failure = error(target, row.remote);
      return { ...row, ...(action ? { pending: action } : {}), ...(failure ? { error: failure } : {}) };
    }),
  }));
}

// ── Consent ──

/**
 * Local ports of administrative services, with their usual service: sharing
 * one gives programs on the host control of this machine.
 */
export const ADMIN_PORTS: ReadonlyMap<number, string> = new Map([
  [22, "SSH"],
  [2375, "the Docker API"],
  [2376, "the Docker API (TLS)"],
  [3389, "Remote Desktop"],
  [5900, "VNC"],
  [6443, "the Kubernetes API"],
  [10250, "the kubelet API"],
]);

export interface ShareWarning {
  /** Shown wherever the user starts sharing, whatever the port. */
  statement: string;
  /** Only for a port of {@link ADMIN_PORTS}. */
  strong?: string;
}

/** What the share form says before Share, for the local port entered so far (null: none valid yet). */
export function shareWarning(target: string, local: number | null): ShareWarning {
  const what = local === null ? "the local port you share" : `localhost:${local} of this machine`;
  const statement = `Programs running as you on ${target} can use ${what}. Most local services ask for no password.`;
  const service = local === null ? undefined : ADMIN_PORTS.get(local);
  if (!service) return { statement };
  return { statement, strong: `Port ${local} is usually ${service}. Sharing it gives programs on ${target} control of this machine.` };
}

export interface ShareInput {
  local: string;
  /** The host's port; empty means the same number as `local`. */
  remote: string;
  remember: boolean;
}

export interface Share {
  port: number;
  local: number;
  remember: boolean;
}

export function shareInput(input: ShareInput): { value: Share; error?: never } | { error: string; value?: never } {
  const local = portNumber(input.local);
  if (local === null) return { error: "Local port must be a whole number from 1 to 65535." };
  const port = input.remote.trim() === "" ? local : portNumber(input.remote);
  if (port === null) return { error: "Host port must be a whole number from 1 to 65535, or empty for the same number." };
  return { value: { port, local, remember: input.remember } };
}

// ── The Window's Shares ──

export interface ReverseForwardingDeps {
  fetch(path: string, init?: RequestInit): Promise<Response>;
  /** The shares, the pending actions or the errors changed. */
  changed(): void;
}

export class ReverseForwarding {
  private forwardList: ReverseForward[] = [];
  private savedList: SavedReverseForward[] = [];
  private readonly errors = new Map<string, { failure: ForwardFailure; baseline: string }>();
  private readonly pending = new Map<string, ShareAction>();
  private readonly lanes = new Map<string, Promise<unknown>>();
  private readonly versions = new Map<string, number>();
  private reads = 0;
  private epoch = 0;
  private viewOpen = false;
  private timer: ReturnType<typeof setInterval> | undefined;
  private disposed = false;

  constructor(private readonly deps: ReverseForwardingDeps) {}

  forwards(): readonly ReverseForward[] { return this.forwardList; }

  saved(): readonly SavedReverseForward[] { return this.savedList; }

  forwardOf(target: string, port: number): ReverseForward | undefined {
    return this.forwardList.find((f) => f.target === target && f.remote === port);
  }

  savedOf(target: string, port: number): SavedReverseForward | undefined {
    return this.savedList.find((f) => f.target === target && f.remote === port);
  }

  /** The last failure of an action here, else a saved rule's failed restore while it is not active. */
  errorOf(target: string, port: number): ForwardFailure | undefined {
    return this.errors.get(portKey(target, port))?.failure
      ?? (this.forwardOf(target, port) ? undefined : this.savedOf(target, port)?.lastError);
  }

  lists(): HostShares[] {
    return shareLists(this.forwardList, this.savedList,
      (target, port) => this.pending.get(portKey(target, port)), (target, port) => this.errorOf(target, port));
  }

  /** Opening the drawer reads at once and keeps reading while it is open. */
  setViewOpen(open: boolean): void {
    if (this.viewOpen === open) return;
    this.viewOpen = open;
    this.schedule();
    if (open) void this.refresh();
  }

  /** Both reads are passive and never start a share. Each failure keeps that resource's last good state. */
  async refresh(): Promise<void> {
    if (this.disposed || this.lanes.size > 0) return;
    const read = ++this.reads;
    const epoch = this.epoch;
    const [active, saved] = await Promise.all([
      this.read(REVERSE_FORWARDS_PATH, parseReverseForwards),
      this.read(REVERSE_PREFERENCES_PATH, parseSavedReverseForwards),
    ]);
    if (this.disposed || read !== this.reads || epoch !== this.epoch) return;
    if (active) this.forwardList = active;
    if (saved) this.savedList = saved;
    // A failure stays until its mapping changes, here or in another window.
    if (active && saved) {
      for (const [key, error] of this.errors) if (error.baseline !== this.mappingState(key)) this.errors.delete(key);
    }
    this.schedule();
    this.deps.changed();
  }

  private async read<T>(path: string, parse: (body: unknown) => T | null): Promise<T | null> {
    try {
      const res = await this.deps.fetch(path);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const value = parse(await res.json());
      if (value === null) throw new Error("invalid answer");
      return value;
    } catch (err) {
      console.warn(`shared ports: reading ${path} failed (${String(err)})`);
      return null;
    }
  }

  /**
   * Share `localhost:<local>` with `target` as its port `port`: the user's
   * explicit Share, Start, Resume or Retry. With `remember`, the rule is saved
   * once the share is up; a failed save leaves the share running and says so.
   */
  share(target: string, port: number, local: number, options: { remember?: boolean } = {}): Promise<ReverseForward | null> {
    const key = portKey(target, port);
    return this.mutate(key, "share", async (current) => {
      try {
        validateMapping(target, port, local);
        const res = await this.deps.fetch(REVERSE_FORWARDS_PATH, jsonRequest("POST", { target, port, local }));
        if (!res.ok) throw await failure(res);
        const forward = parseReverseForward(await res.json());
        if (!forward || forward.target !== target || forward.remote !== port || forward.local !== local) {
          throw new ForwardError("The local Splitty server answered with a different mapping.");
        }
        if (!current()) return null;
        this.forwardList = withForward(this.forwardList, forward);
        // An explicit share enables a saved rule and keeps its local port.
        const saved = this.savedOf(target, port);
        if (saved) this.savedList = withForward(this.savedList, { target, remote: port, local, enabled: true });
        if (options.remember) {
          try {
            const rule = await this.put(target, port, local);
            if (current()) this.savedList = withForward(this.savedList, rule);
          } catch (err) {
            if (current()) this.recordError(key, err, "Shared, but not remembered: ");
          }
        }
        return current() ? forward : null;
      } catch (err) {
        if (current()) this.recordError(key, err);
        return null;
      }
    });
  }

  /** Stop also durably pauses a saved rule, including an inactive one. */
  stop(target: string, port: number): Promise<boolean> {
    const key = portKey(target, port);
    return this.mutate(key, "stop", async (current) => {
      try {
        const res = await this.deps.fetch(REVERSE_FORWARDS_PATH, jsonRequest("DELETE", { target, port }));
        // 404: nothing is shared or saved for it any more (stopped elsewhere).
        if (!res.ok && res.status !== 404) throw await failure(res);
        if (current()) {
          this.forwardList = this.forwardList.filter((f) => portKey(f.target, f.remote) !== key);
          const saved = this.savedOf(target, port);
          if (saved) this.savedList = withForward(this.savedList, { target, remote: port, local: saved.local, enabled: false });
        }
        return current();
      } catch (err) {
        if (current()) this.recordError(key, err);
        return false;
      }
    });
  }

  /** Save the rule; metadata only, starts nothing. */
  remember(target: string, port: number, local?: number): Promise<boolean> {
    const key = portKey(target, port);
    return this.mutate(key, "save", async (current) => {
      try {
        validateMapping(target, port, local);
        const rule = await this.put(target, port, local);
        if (current()) this.savedList = withForward(this.savedList, rule);
        return current();
      } catch (err) {
        if (current()) this.recordError(key, err);
        return false;
      }
    });
  }

  /** Forget never stops an active share. */
  forget(target: string, port: number): Promise<boolean> {
    const key = portKey(target, port);
    return this.mutate(key, "forget", async (current) => {
      try {
        const res = await this.deps.fetch(REVERSE_PREFERENCES_PATH, jsonRequest("DELETE", { target, port }));
        if (!res.ok) throw await failure(res);
        if (current()) this.savedList = this.savedList.filter((f) => portKey(f.target, f.remote) !== key);
        return current();
      } catch (err) {
        if (current()) this.recordError(key, err);
        return false;
      }
    });
  }

  dispose(): void {
    this.disposed = true;
    clearInterval(this.timer);
    this.timer = undefined;
  }

  // ── Internals ──

  private mutate<T>(key: string, action: ShareAction, run: (current: () => boolean) => Promise<T>): Promise<T> {
    const version = (this.versions.get(key) ?? 0) + 1;
    this.versions.set(key, version);
    this.epoch++;
    this.pending.set(key, action);
    this.errors.delete(key);
    this.deps.changed();
    const previous = this.lanes.get(key);
    const current = () => !this.disposed && this.versions.get(key) === version;
    const execute = async () => {
      try { return await run(current); }
      finally {
        this.epoch++;
        if (this.versions.get(key) === version) {
          this.lanes.delete(key);
          this.pending.delete(key);
        }
        if (!this.disposed) { this.schedule(); this.deps.changed(); void this.refresh(); }
      }
    };
    const task = previous ? previous.then(execute, execute) : execute();
    this.lanes.set(key, task);
    return task;
  }

  private async put(target: string, port: number, local?: number): Promise<SavedReverseForward> {
    const res = await this.deps.fetch(REVERSE_PREFERENCES_PATH, jsonRequest("PUT", {
      target, port, ...(local === undefined ? {} : { local }),
    }));
    if (!res.ok) throw await failure(res);
    const saved = parseSavedReverseForward(await res.json());
    if (!saved || saved.target !== target || saved.remote !== port) throw new ForwardError("Invalid saved share response.");
    return saved;
  }

  /** What a recorded failure is about; when it changes, the failure is resolved. */
  private mappingState(key: string): string {
    const [target, rawPort] = key.split("\n");
    const port = Number(rawPort);
    const saved = this.savedOf(target, port);
    return JSON.stringify({ active: this.forwardOf(target, port)?.local ?? null,
      saved: saved ? { local: saved.local, enabled: saved.enabled } : null });
  }

  private recordError(key: string, err: unknown, prefix = ""): void {
    const cause = err instanceof ForwardError ? err : new ForwardError(`The local Splitty server did not answer (${String(err)}).`);
    this.errors.set(key, {
      failure: { code: cause.code, message: prefix + cause.message, ...(cause.hint ? { hint: cause.hint } : {}) },
      baseline: this.mappingState(key),
    });
  }

  /** Poll passive snapshots; a read never starts a share. */
  private schedule(): void {
    if (this.disposed) return;
    const active = this.viewOpen || this.forwardList.length > 0 || this.savedList.length > 0;
    if (active && this.timer === undefined) {
      this.timer = setInterval(() => void this.refresh(), REFRESH_INTERVAL);
    } else if (!active && this.timer !== undefined) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }
}

function validateMapping(target: string, port: number, local?: number): void {
  if (validateTarget(target) !== target || !isPort(port) || (local !== undefined && !isPort(local))) {
    throw new ForwardError("Enter a valid SSH target and ports from 1 to 65535.");
  }
}
