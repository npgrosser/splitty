// Detected ports and their forwards in the window (`remote-screens`, Detected
// Ports In The Window, Automatic Port Forwarding).
//
// Per host connection, the last `ports.changed` the host sent on it; the
// local connection is not watched, since its ports need no forward. A port
// that appears brings no notice: the host's port list offers it (provisional,
// tbd). With `remote.autoForwardPorts`, a port that appears after the first
// list, or the first after a reconnect, is forwarded at once, once per
// connection and process, so a dev server that restarts is not forwarded
// again after a Stop, and a port forwarded already not at all.
//
// Saved and active state are read independently. Reads never restore a rule:
// restoration belongs to the hub's successful host-readiness path. Client
// mutations are ordered per mapping; generations suppress superseded answers
// and browser-open continuations, including when Stop follows a slow opening.

import type { ReservedExternalOpen } from "../window.ts";
import type { ConnSocket } from "../protocol/socket.ts";
import { hostOf, type ConnKey } from "../connection/server_connection.ts";
import { MAX_NAME, MAX_TEXT, plainLines, plainText } from "../protocol/untrusted.ts";
import {
  appearedPorts, forwardedLink, loopbackLink, parseForward, parseForwards, parsePorts, portKey, portLists,
  withForward, parseSavedForward, parseSavedForwards, forwardLabel, isPort, MAX_FORWARD_LABEL,
  type DetectedPort, type HostPorts, type PortAction, type RemoteForward, type SavedForward, type ForwardFailure,
} from "./ports.ts";

import { validateTarget } from "./api.ts";

export const PREFERENCES_PATH = "/api/remote/forward-preferences";
export const FORWARDS_PATH = "/api/remote/forwards";
/** Refresh while a host, saved/live mapping or the Ports view is in use (D6). */
export const REFRESH_INTERVAL = 5000;
/** How long a notice with an action stays (provisional, tbd). */
export const NOTICE_DURATION = 15_000;

/** A notice for the user; a later one with the same id replaces it. */
export interface PortNotice {
  id: string;
  kind: "info" | "success" | "error";
  title: string;
  description?: string;
  action?: { label: string; run: () => void };
}

export interface PortForwardingDeps {
  fetch(path: string, init?: RequestInit): Promise<Response>;
  /** Open a URL in the browser; false when nothing opened (a refused scheme, a blocked window). */
  open(url: string): boolean;
  /** Reserve a blank browser tab synchronously during an explicit click. */
  reserveOpen?(): ReservedExternalOpen | undefined;
  /** Whether new ports are forwarded without a click (`remote.autoForwardPorts`). */
  autoForward(): boolean;
  notify(notice: PortNotice): void;
  /** The ports, the forwards or the pending actions changed. */
  changed(): void;
}

/** What a host connection gives the ports: its key and its socket. */
export interface WatchedConnection {
  readonly key: ConnKey;
  readonly socket: ConnSocket;
}

/** Why the local server did not forward or stop. */
export class ForwardError extends Error {
  constructor(message: string, readonly hint?: string, readonly code = "internal") {
    super(message);
  }
}

interface Host {
  target: string;
  /** The last list; undefined until the first after a (re)connect. */
  ports: DetectedPort[] | undefined;
  /** The ports forwarded by the setting on this connection, with their process. */
  autoForwarded: Map<number, string>;
}

export interface ForwardOptions {
  open?: boolean;
  local?: number;
  automatic?: boolean;
  remember?: boolean;
  label?: string;
}

export class PortForwarding {
  private readonly hosts = new Map<ConnKey, Host>();
  private forwardList: RemoteForward[] = [];
  private savedList: SavedForward[] = [];
  private readonly errors = new Map<string, { failure: ForwardFailure; action: PortAction; baseline: string }>();
  private readonly pending = new Map<string, PortAction>();
  private readonly lanes = new Map<string, Promise<unknown>>();
  private readonly versions = new Map<string, number>();
  private readonly opening = new Map<string, { signature: string; version: number; request: Promise<RemoteForward | null> }>();
  private readonly reservations = new Map<string, Set<ReservedExternalOpen>>();
  private reads = 0;
  private epoch = 0;
  private viewOpen = false;
  private timer: ReturnType<typeof setInterval> | undefined;
  private disposed = false;

  constructor(private readonly deps: PortForwardingDeps) {}

  /** Follow the ports a host connection reports, until the returned function is called. */
  watch(conn: WatchedConnection): () => void {
    const target = hostOf(conn.key);
    if (target === undefined) return () => {};
    const host: Host = { target, ports: undefined, autoForwarded: new Map() };
    this.hosts.set(conn.key, host);
    this.schedule();
    const unsubMessage = conn.socket.onMessage((msg) => {
      if (msg.type === "ports.changed") this.portsChanged(host, msg.ports);
    });
    // While the host is away its ports are unknown; its next list is a first one again.
    const unsubDisconnect = conn.socket.onDisconnect(() => {
      host.ports = undefined;
      this.deps.changed();
    });
    return () => {
      unsubMessage();
      unsubDisconnect();
      if (this.hosts.get(conn.key) === host) this.hosts.delete(conn.key);
      this.schedule();
      this.deps.changed();
    };
  }

  /** The last list of a host connection; undefined before its first or while it is away. */
  ports(key: ConnKey): readonly DetectedPort[] | undefined {
    return this.hosts.get(key)?.ports;
  }

  forwards(): readonly RemoteForward[] {
    return this.forwardList;
  }

  forwardOf(target: string, port: number): RemoteForward | undefined {
    return this.forwardList.find((f) => f.target === target && f.remote === port);
  }

  saved(): readonly SavedForward[] { return this.savedList; }

  savedOf(target: string, port: number): SavedForward | undefined {
    return this.savedList.find((f) => f.target === target && f.remote === port);
  }

  errorOf(target: string, port: number): ForwardFailure | undefined {
    return this.errors.get(portKey(target, port))?.failure ?? this.savedOf(target, port)?.lastError;
  }

  /** The Ports lists as the UI shows them. */
  lists(): HostPorts[] {
    const hosts = [...this.hosts].map(([key, host]) => ({ key, ports: host.ports }));
    return portLists(hosts, this.forwardList, (target, port) => this.pending.get(portKey(target, port)), this.savedList)
      .map((host) => ({ ...host, rows: host.rows.map((row) => {
        const error = this.errorOf(host.target, row.port);
        return error ? { ...row, error } : row;
      }) }));
  }

  /** Opening the drawer must discover changes even when this window was empty. */
  setViewOpen(open: boolean): void {
    if (this.viewOpen === open) return;
    this.viewOpen = open;
    this.schedule();
    if (open) void this.refresh();
  }

  /** Both reads are passive. Each failure keeps that resource's last good state. */
  async refresh(): Promise<void> {
    if (this.disposed || this.lanes.size > 0) return;
    const read = ++this.reads;
    const epoch = this.epoch;
    const current = () => !this.disposed && read === this.reads && epoch === this.epoch;
    const [activeRead, savedRead] = await Promise.all([this.read(FORWARDS_PATH, (body) => {
      if (!Array.isArray(body) || body.some((entry) => !parseForward(entry))) throw new Error("Invalid active forwards");
      if (current()) this.forwardList = parseForwards(body);
    }), this.read(PREFERENCES_PATH, (body) => {
      const saved = parseSavedForwards(body);
      if (!saved) throw new Error("Invalid forward preferences");
      if (current()) this.savedList = saved;
    })]);
    if (current()) {
      for (const [key, error] of this.errors) {
        const [target, rawPort] = key.split("\n");
        const port = Number(rawPort);
        const active = this.forwardOf(target, port);
        const saved = this.savedOf(target, port);
        const changed = error.baseline !== this.mappingState(key, error.action);
        const resolved = error.action === "save" ? savedRead && changed
          : error.action === "forget" ? savedRead && !saved
          : error.action === "stop" ? activeRead && savedRead && !active && (!saved || !saved.enabled)
          : (savedRead && !!saved?.lastError) || (activeRead && !!active
            && (error.failure.code !== "active_mapping_conflict" || changed))
            || (activeRead && savedRead && !active && changed);
        if (resolved) this.errors.delete(key);
      }
      this.schedule(); this.deps.changed();
    }
  }

  private async read(path: string, apply: (body: unknown) => void): Promise<boolean> {
    try {
      const res = await this.deps.fetch(path);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      apply(await res.json());
      return true;
    } catch (err) {
      console.warn(`port forwards: reading ${path} failed (${String(err)})`);
      return false;
    }
  }

  /** Explicit by default; automation never opts into Remember or resumes a pause. */
  async forward(target: string, port: number, options: ForwardOptions = {}): Promise<RemoteForward | null> {
    const key = portKey(target, port);
    if (this.disposed || (options.automatic && this.savedOf(target, port)?.enabled === false)) return null;
    const reservation = options.open ? this.deps.reserveOpen?.() : undefined;
    const { open, ...intent } = options;
    const signature = JSON.stringify(intent);
    let opening = this.opening.get(key);
    if (!opening || opening.signature !== signature || opening.version !== this.versions.get(key)) {
      const request = this.mutate(key, "forward", async (current) => {
        try {
          validateMapping(target, port, options.local, options.label);
          if (options.automatic && (options.local !== undefined || options.remember)) {
            throw new ForwardError("Automatic forwarding cannot change or remember a mapping.");
          }
          const saved = this.savedOf(target, port);
          // A cached active entry may have been stopped in another window.
          const forward = await this.request(target, port, options);
          if (!current()) return null;
          let preference: SavedForward | undefined;
          let rememberFailure: { error: unknown } | undefined;
          if (options.remember) {
            // Two independent requests: a failed save must leave the live tunnel visible.
            try {
              preference = await this.put(target, port, { local: forward.local, label: options.label });
            } catch (err) {
              rememberFailure = { error: err };
            }
          }
          if (!current()) return null;
          // POST can have completed before another window's Stop, with only
          // its response delayed. A passive read must confirm it before we
          // publish or open it; retrying POST here would undo that Stop.
          const active = await this.readActiveForward(target, port);
          if (!current()) return null;
          // This snapshot may predate mutations of other keys. Apply only the
          // key protected by this lane and its generation, never the full list.
          this.forwardList = this.forwardList.filter((f) => portKey(f.target, f.remote) !== key);
          if (active) this.forwardList = withForward(this.forwardList, active);
          if (!active || active.state !== "up" || active.local !== forward.local) {
            throw new ForwardError("The forward is no longer available at the verified local port. Try again.");
          }
          if (preference) this.savedList = withForward(this.savedList, preference);
          else if (saved) this.savedList = withForward(this.savedList, { ...saved, local: active.local, enabled: true, lastError: undefined });
          if (rememberFailure) this.recordError(key, `Port ${port} is forwarded but was not remembered`, rememberFailure.error);
          this.deps.changed();
          return active;
        } catch (err) {
          if (current() && !(options.automatic && err instanceof ForwardError && err.code === "forward_paused")) {
            this.recordError(key, `Port ${port} of ${target} was not forwarded`, err, "forward");
          }
          return null;
        }
      });
      opening = { signature, version: this.versions.get(key)!, request };
      this.opening.set(key, opening);
      const started = opening;
      void request.finally(() => { if (this.opening.get(key) === started) this.opening.delete(key); });
    }
    this.trackReservation(key, reservation);
    const forward = await opening.request;
    this.releaseReservation(key, reservation);
    if (!forward || this.disposed || opening.version !== this.versions.get(key)) {
      reservation?.close();
      return null;
    }
    if (open) this.openUrl(forward.url, forward, reservation);
    return forward;
  }

  /** Save metadata only; no host connection or forwarding request. */
  save(target: string, port: number, options: { local?: number; label?: string } = {}): Promise<boolean> {
    const key = portKey(target, port);
    return this.mutate(key, "save", async (current) => {
      try {
        validateMapping(target, port, options.local, options.label);
        const saved = await this.put(target, port, options);
        if (current()) this.savedList = withForward(this.savedList, saved);
        return current();
      } catch (err) {
        if (current()) this.recordError(key, `Port ${port} of ${target} was not remembered`, err, "save");
        return false;
      }
    });
  }

  /** Stop also durably pauses a saved rule, including an inactive one. */
  stop(target: string, port: number): Promise<boolean> {
    const key = portKey(target, port);
    return this.mutate(key, "stop", async (current) => {
      try {
        const res = await this.deps.fetch(FORWARDS_PATH, jsonRequest("DELETE", { target, port }));
        if (!res.ok && res.status !== 404) throw await failure(res);
        if (current()) {
          this.forwardList = this.forwardList.filter((f) => portKey(f.target, f.remote) !== key);
          const saved = this.savedOf(target, port);
          if (saved) this.savedList = withForward(this.savedList, { ...saved, enabled: false, lastError: undefined });
        }
        return current();
      } catch (err) {
        if (current()) this.recordError(key, `The forward of port ${port} of ${target} was not stopped`, err, "stop");
        return false;
      }
    });
  }

  /** Forget never stops the active forward or its supervisor. */
  forget(target: string, port: number): Promise<boolean> {
    const key = portKey(target, port);
    return this.mutate(key, "forget", async (current) => {
      try {
        const res = await this.deps.fetch(PREFERENCES_PATH, jsonRequest("DELETE", { target, port }));
        if (!res.ok) throw await failure(res);
        if (current()) this.savedList = this.savedList.filter((f) => portKey(f.target, f.remote) !== key);
        return current();
      } catch (err) {
        if (current()) this.recordError(key, `Port ${port} of ${target} was not forgotten`, err, "forget");
        return false;
      }
    });
  }

  private mutate<T>(key: string, action: PortAction, run: (current: () => boolean) => Promise<T>): Promise<T> {
    this.cancelReservations(key);
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

  private async put(target: string, port: number, options: { local?: number; label?: string }): Promise<SavedForward> {
    const res = await this.deps.fetch(PREFERENCES_PATH, jsonRequest("PUT", {
      target, port, ...options, ...(options.label === undefined ? {} : { label: forwardLabel(options.label) }),
    }));
    if (!res.ok) throw await failure(res);
    const saved = parseSavedForward(await res.json());
    if (!saved || saved.target !== target || saved.remote !== port) throw new ForwardError("Invalid saved mapping response.");
    return saved;
  }

  /** Verify even a visible/notice mapping: another window may have stopped it. */
  open(forward: RemoteForward, url = forward.url): void {
    const link = url === forward.url ? null : loopbackLink(url);
    this.forwardAndOpen(forward.target, forward.remote, link);
  }

  private forwardAndOpen(target: string, port: number, link: ReturnType<typeof loopbackLink>): void {
    const key = portKey(target, port);
    const reservation = this.deps.reserveOpen?.();
    const request = this.forward(target, port);
    this.trackReservation(key, reservation);
    const version = this.versions.get(portKey(target, port));
    void request.then((forward) => {
      this.releaseReservation(key, reservation);
      if (forward && !this.disposed && version === this.versions.get(key)) {
        this.openUrl(link ? forwardedLink(link, forward.local) : forward.url, forward, reservation);
      } else reservation?.close();
    });
  }

  /**
   * A link opened in a pane of `conn`. A host's pane opens a link to its own
   * loopback (`localhost`, `127.0.0.1`, `0.0.0.0`, `[::1]`) at the forward of
   * that port, which the click opens first when there is none: the click is
   * the consent (D4). Any other link, and every link of a local pane, opens as
   * it is.
   */
  openLink(conn: ConnKey, raw: string): void {
    const target = hostOf(conn);
    const link = target === undefined ? null : loopbackLink(raw);
    if (target === undefined || !link) {
      this.deps.open(raw);
      return;
    }
    this.forwardAndOpen(target, link.port, link);
  }

  dispose(): void {
    this.disposed = true;
    clearInterval(this.timer);
    this.timer = undefined;
    this.hosts.clear();
    for (const key of this.reservations.keys()) this.cancelReservations(key);
  }

  // ── Internals ──

  private portsChanged(host: Host, raw: unknown): void {
    const next = parsePorts(raw);
    if (!next) return;
    const appeared = appearedPorts(host.ports, next);
    host.ports = next;
    this.deps.changed();
    this.autoForwardAppeared(host, appeared);
  }

  /** With the setting, forward the ports that appeared on a host, without a notice. */
  private autoForwardAppeared(host: Host, appeared: readonly DetectedPort[]): void {
    if (this.deps.autoForward() !== true) return;
    const fresh = appeared.filter((p) => !this.forwardOf(host.target, p.port) && host.autoForwarded.get(p.port) !== p.process
      && this.savedOf(host.target, p.port)?.enabled !== false);
    for (const p of fresh) {
      host.autoForwarded.set(p.port, p.process);
      void this.forward(host.target, p.port, { automatic: true });
    }
  }

  private async request(target: string, port: number, options: ForwardOptions): Promise<RemoteForward> {
    const res = await this.deps.fetch(FORWARDS_PATH, jsonRequest("POST", {
      target, port, ...(options.local === undefined ? {} : { local: options.local }),
      ...(options.automatic ? { automatic: true } : {}),
    }));
    if (!res.ok) throw await failure(res);
    const forward = parseForward(await res.json());
    if (!forward || forward.target !== target || forward.remote !== port
      || (options.local !== undefined && forward.local !== options.local)) {
      throw new ForwardError("The local Splitty server answered with a different mapping.");
    }
    return forward;
  }

  private async readActiveForward(target: string, port: number): Promise<RemoteForward | undefined> {
    const res = await this.deps.fetch(FORWARDS_PATH, { cache: "no-store" });
    if (!res.ok) throw await failure(res);
    const body: unknown = await res.json();
    if (!Array.isArray(body) || body.some((entry) => !parseForward(entry))) {
      throw new ForwardError("Invalid active forwards response.");
    }
    return parseForwards(body).find((f) => f.target === target && f.remote === port);
  }

  private mappingState(key: string, action: PortAction): string {
    const [target, rawPort] = key.split("\n");
    const port = Number(rawPort);
    const saved = this.savedOf(target, port);
    // A supervisor reconnect or a metadata-only edit does not resolve a failed
    // save/Stop or an explicit request for a different active local port.
    return JSON.stringify(action === "save" || action === "forget"
      ? saved && { local: saved.local, label: saved.label }
      : { active: this.forwardOf(target, port)?.local,
        saved: saved && { local: saved.local, enabled: saved.enabled },
      }) ?? "absent";
  }

  private recordError(key: string, title: string, err: unknown, action: PortAction = "save"): void {
    const failure = err instanceof ForwardError ? err : new ForwardError(`The local Splitty server did not answer (${String(err)}).`);
    this.errors.set(key, { failure: { code: failure.code, message: failure.message, hint: failure.hint }, action, baseline: this.mappingState(key, action) });
    this.notifyError(`${action}:${key}`, title, failure);
  }

  /** Open `url`; when the browser opened nothing (a blocked window), offer it in a notice, whose click may. */
  private openUrl(url: string, forward: RemoteForward, reservation?: ReservedExternalOpen): void {
    if (reservation ? reservation.open(url) : this.deps.open(url)) return;
    this.deps.notify({
      id: `port:${portKey(forward.target, forward.remote)}`,
      kind: "success",
      title: `${plainText(forward.target, MAX_NAME)}: port ${forward.remote} is forwarded`,
      description: url,
      action: { label: "Open", run: () => this.open(forward, url) },
    });
  }

  private trackReservation(key: string, reservation?: ReservedExternalOpen): void {
    if (!reservation) return;
    const pending = this.reservations.get(key) ?? new Set<ReservedExternalOpen>();
    pending.add(reservation);
    this.reservations.set(key, pending);
  }

  private releaseReservation(key: string, reservation?: ReservedExternalOpen): void {
    const pending = this.reservations.get(key);
    if (reservation) pending?.delete(reservation);
    if (pending?.size === 0) this.reservations.delete(key);
  }

  private cancelReservations(key: string): void {
    for (const reservation of this.reservations.get(key) ?? []) reservation.close();
    this.reservations.delete(key);
  }

  private notifyError(id: string, title: string, err: unknown): void {
    const message = err instanceof ForwardError
      ? [err.message, err.hint].filter(Boolean).join("\n")
      : `The local Splitty server did not answer (${String(err)}).`;
    this.deps.notify({ id, kind: "error", title, description: message });
  }

  /** Poll passive snapshots; failures here never authorize another opening. */
  private schedule(): void {
    if (this.disposed) return;
    const active = this.viewOpen || this.hosts.size > 0 || this.forwardList.length > 0 || this.savedList.length > 0;
    if (active && this.timer === undefined) {
      this.timer = setInterval(() => void this.refresh(), REFRESH_INTERVAL);
    } else if (!active && this.timer !== undefined) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }
}

export function jsonRequest(method: string, body: unknown): RequestInit {
  return { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
}

/** The local server's error answer (`{ error, code?, hint? }`) as plain text. */
export async function failure(res: Response): Promise<ForwardError> {
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    body = undefined;
  }
  const { error, hint, code } = (typeof body === "object" && body !== null ? body : {}) as Record<string, unknown>;
  const message = plainLines(error, MAX_TEXT) || `The local Splitty server answered HTTP ${res.status}.`;
  return new ForwardError(message, plainText(hint, MAX_TEXT) || undefined, plainText(code, MAX_NAME) || "internal");
}

function validateMapping(target: string, port: number, local?: number, label?: string): void {
  if (validateTarget(target) !== target || !isPort(port) || (local !== undefined && !isPort(local))) {
    throw new ForwardError("Enter a valid SSH target and ports from 1 to 65535.");
  }
  if (label !== undefined && [...label].length > MAX_FORWARD_LABEL) {
    throw new ForwardError(`Label must be at most ${MAX_FORWARD_LABEL} characters.`);
  }
}
