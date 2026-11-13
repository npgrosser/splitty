// The remote screens of the window, remembered across restarts by the local
// server (`GET /api/remote/screens` and `PUT /api/remote/screens/host`;
// `remote-screens`, Remembered Remote Screens).
//
// The list is the server's, shared with the other windows: this window
// writes only the hosts it shows or showed, one host per request, and the
// server replaces only that host's entries. A host whose last screen leaves
// the window is forgotten. Entries marked `closed` (a host another window
// closed) stay as they are and are not brought back.
//
// A screen's position is its index in the window's screen list, local screens
// included, when the list was written. On start the remembered screens go
// back only once the local server's first welcome has settled the local
// screens, so `ScreenStartup` and its placeholder see local screens only: in
// the order of their positions, each is inserted at its position, or at the
// end when the list is shorter. With the same local screens that is the old
// order again.
//
// A restored screen does not connect: its host's connection opens when the
// screen becomes the active one, or when the window connects the host anyway
// (`hostToConnect`), so an unreachable host does not slow the start. Only the
// hosts the user listed in `remote.reconnectHosts` are connected at once
// (`hostsToReconnect`), as the native window does.
//
// The list is written after every change, debounced, and never before the
// remembered one was read and restored: a write before that would replace it.
// After a failed read the page leaves the list alone, and so it does with a
// local server that does not announce `hub-host-state-v1`.

import type { ScreenRef } from "../connection/routing.ts";
import { hostKey, hostOf, type ConnKey } from "../connection/server_connection.ts";
import { MAX_NAME, plainText } from "../protocol/untrusted.ts";
import { validateTarget } from "./api.ts";

/** The format version of the list. */
export const FORMAT = 1;
/** How many screens the server keeps; more are left out. */
export const MAX_REMEMBERED = 200;
/** The longest screen id the server takes, in characters. */
export const MAX_SCREEN_ID = 256;
/** How long changes gather before the list is written. */
export const WRITE_DELAY = 400;

export const PATH = "/api/remote/screens";
/** Where one host's screens are written. */
export const HOST_PATH = "/api/remote/screens/host";

export interface RememberedScreen {
  target: string;
  /** The screen's id on the host. */
  screenId: string;
  /** Its name when last seen, shown until the host is connected. */
  name?: string;
  /** Its index in the window's screen list, local screens included. */
  position?: number;
  /** Its host was closed in a window: offered again, not brought back at start. */
  closed?: true;
}

/** One host's screens as the window writes them. */
export interface HostEntry {
  screenId: string;
  name?: string;
  position?: number;
}

/** The body of `PUT /api/remote/screens/host`; an empty list forgets the host. */
export interface HostWrite {
  target: string;
  screens: HostEntry[];
}

/** A screen of the window as the list sees it. */
export interface WindowScreen extends ScreenRef {
  readonly name: string;
}

/** Where a remembered screen goes back: inserted at `index`, in this order. */
export interface RestoreStep {
  screen: RememberedScreen;
  index: number;
}

// ── The List ──

/**
 * The entries of a list the server sent that have the shape, in their order:
 * a target the client accepts as it is, a screen id of 1 to 256 characters,
 * each (target, screen id) once, at most 200.
 */
export function parseRemembered(value: unknown): RememberedScreen[] {
  if (typeof value !== "object" || value === null) return [];
  const { version, screens } = value as { version?: unknown; screens?: unknown };
  if (version !== FORMAT || !Array.isArray(screens)) return [];
  const result: RememberedScreen[] = [];
  const seen = new Set<string>();
  for (const entry of screens) {
    if (result.length >= MAX_REMEMBERED) break;
    if (typeof entry !== "object" || entry === null) continue;
    const { target, screenId, name, position, closed } = entry as Record<string, unknown>;
    if (typeof target !== "string" || validateTarget(target) !== target) continue;
    if (!validScreenId(screenId)) continue;
    const id = identity(target, screenId);
    if (seen.has(id)) continue;
    seen.add(id);
    const screen: RememberedScreen = { target, screenId };
    const shown = plainText(name, MAX_NAME);
    if (shown) screen.name = shown;
    if (typeof position === "number" && Number.isInteger(position) && position >= 0) screen.position = position;
    if (closed === true) screen.closed = true;
    result.push(screen);
  }
  return result;
}

/**
 * The screens to write per host for the window's `screens`: every remote
 * screen but the ones `skip` names, with its index in the window as its
 * position, hosts in the order of their first screen. A screen the server
 * would refuse (an id it does not take) is left out, so one odd screen cannot
 * cost its host's write.
 */
export function rememberedHosts(screens: readonly WindowScreen[], skip: (key: string) => boolean): Map<string, HostEntry[]> {
  const hosts = new Map<string, HostEntry[]>();
  const seen = new Set<string>();
  let count = 0;
  screens.forEach((screen, position) => {
    const target = hostOf(screen.conn);
    if (!target || count >= MAX_REMEMBERED || skip(screen.id)) return;
    if (!validScreenId(screen.wireId)) return;
    const id = identity(target, screen.wireId);
    if (seen.has(id)) return;
    seen.add(id);
    const entry: HostEntry = { screenId: screen.wireId };
    const name = wellFormed(plainText(screen.name, MAX_NAME));
    if (name) entry.name = name;
    entry.position = position;
    let list = hosts.get(target);
    if (!list) hosts.set(target, list = []);
    list.push(entry);
    count++;
  });
  return hosts;
}

/**
 * The open (not `closed`) entries of `remembered` per host, in the order of
 * their positions, as `rememberedHosts` lists them once they are back.
 */
function storedHosts(remembered: readonly RememberedScreen[]): Map<string, HostEntry[]> {
  const hosts = new Map<string, HostEntry[]>();
  const ordered = remembered
    .filter((screen) => !screen.closed)
    .map((screen, order) => ({ screen, order }))
    .sort((a, b) => (a.screen.position ?? Number.MAX_SAFE_INTEGER) - (b.screen.position ?? Number.MAX_SAFE_INTEGER)
      || a.order - b.order);
  for (const { screen } of ordered) {
    // A screen without a name goes back under its id (`Workspace.svelte`).
    const entry: HostEntry = { screenId: screen.screenId, name: screen.name ?? screen.screenId };
    if (screen.position !== undefined) entry.position = screen.position;
    let list = hosts.get(screen.target);
    if (!list) hosts.set(screen.target, list = []);
    list.push(entry);
  }
  return hosts;
}

/**
 * Where the remembered screens go back into the window's `shown` screens:
 * by position (screens without one last, in their order), each at its
 * position or at the end of the list as it is then. Screens the window shows
 * already, and the screens of a closed host, are skipped.
 */
export function planRestore(shown: readonly ScreenRef[], remembered: readonly RememberedScreen[]): RestoreStep[] {
  const present = new Set(shown.map((s) => `${s.conn}\n${s.wireId}`));
  const ordered = remembered
    .filter((screen) => !screen.closed)
    .map((screen, order) => ({ screen, order }))
    .sort((a, b) => (a.screen.position ?? Number.MAX_SAFE_INTEGER) - (b.screen.position ?? Number.MAX_SAFE_INTEGER)
      || a.order - b.order);
  let length = shown.length;
  const steps: RestoreStep[] = [];
  for (const { screen } of ordered) {
    const key = `${hostKey(screen.target)}\n${screen.screenId}`;
    if (present.has(key)) continue;
    present.add(key);
    steps.push({ screen, index: Math.min(screen.position ?? length, length) });
    length++;
  }
  return steps;
}

/**
 * The host whose connection the window should open now: the active screen's,
 * when it is a remote screen whose host is not connected yet (lazy connect).
 * Other remembered screens wait until they are shown, or until their host is
 * connected for another screen.
 */
export function hostToConnect(
  screens: readonly ScreenRef[],
  activeKey: string,
  connected: (key: ConnKey) => boolean,
): ConnKey | undefined {
  const active = screens.find((s) => s.id === activeKey);
  if (!active || active.conn === "local" || connected(active.conn)) return undefined;
  return active.conn;
}

/**
 * The hosts the window connects at its start: those of the restored screens
 * that `remote.reconnectHosts` lists, each once, in the order of the steps
 * (provisional, tbd).
 */
export function hostsToReconnect(steps: readonly RestoreStep[], chosen: readonly string[] | undefined): string[] {
  const listed = new Set(chosen ?? []);
  const hosts: string[] = [];
  for (const { screen } of steps) {
    if (listed.has(screen.target) && !hosts.includes(screen.target)) hosts.push(screen.target);
  }
  return hosts;
}

// ── Reading and Writing ──

export interface RememberedDeps {
  fetch(path: string, init?: RequestInit): Promise<Response>;
  /** The window's screens, in order. */
  screens(): readonly WindowScreen[];
  /** Whether the screen `key` is left out: a new remote screen its host does not have yet. */
  skip(key: string): boolean;
  /** Put remembered screens back into the window, each at its index, in this order. */
  restore(steps: readonly RestoreStep[]): void;
  warn?(message: string): void;
}

type Phase = "idle" | "loading" | "waiting" | "ready" | "failed";

export class RememberedScreens {
  /** `waiting`: read, waiting for the local screens; `ready`: restored, changes are written. */
  private phase: Phase = "idle";
  private read: RememberedScreen[] = [];
  private localReady = false;
  /** The local server writes one host at a time (`hub-host-state-v1`). */
  private hostWrites = false;
  private timer: ReturnType<typeof setTimeout> | undefined;
  /** Per host this window shows or showed, its entries as the server holds them, as JSON. */
  private readonly stored = new Map<string, string>();
  /** Per host, the entries the server refused (400), not sent again unchanged. */
  private readonly refused = new Map<string, string>();
  private inFlight = false;
  private again = false;
  private disposed = false;

  constructor(private readonly deps: RememberedDeps) {}

  /** Read the remembered list; its screens go back once the local screens are settled too. */
  async load(): Promise<void> {
    if (this.phase !== "idle") return;
    this.phase = "loading";
    try {
      const res = await this.deps.fetch(PATH);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      this.read = parseRemembered(await res.json());
    } catch (err) {
      if (this.disposed) return;
      this.phase = "failed";
      this.warn(`remembered remote screens: reading failed (${String(err)}); this window does not write them`);
      return;
    }
    if (this.disposed) return;
    this.phase = "waiting";
    this.restore();
  }

  /** The local server's first welcome settled the local screens. */
  localScreensReady(): void {
    this.localReady = true;
    this.restore();
  }

  /** Whether the local server takes writes per host; without them nothing is written. */
  useHostWrites(enabled: boolean): void {
    this.hostWrites = enabled;
    if (enabled) this.changed();
  }

  /** The window's screens changed: write the hosts that changed, soon. */
  changed(): void {
    if (this.phase !== "ready" || this.disposed || !this.hostWrites) return;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = undefined;
      this.flush();
    }, WRITE_DELAY);
  }

  /**
   * Write the hosts that changed, now. With `keepalive` the requests outlive
   * the page (`pagehide`) and do not wait for one in flight.
   */
  flush(keepalive = false): void {
    clearTimeout(this.timer);
    this.timer = undefined;
    if (this.phase !== "ready" || this.disposed || !this.hostWrites) return;
    const writes = this.pending();
    if (writes.length === 0) return;
    if (keepalive) {
      for (const [target, body] of writes) void this.write(target, body, true);
      return;
    }
    if (this.inFlight) {
      this.again = true;
      return;
    }
    this.inFlight = true;
    void (async () => {
      for (const [target, body] of writes) await this.write(target, body, false);
    })().finally(() => {
      this.inFlight = false;
      if (this.again) {
        this.again = false;
        this.flush();
      }
    });
  }

  dispose(): void {
    this.disposed = true;
    clearTimeout(this.timer);
    this.timer = undefined;
  }

  /** The hosts whose entries differ from what the server holds, with their new entries as JSON. */
  private pending(): [string, string][] {
    const shown = rememberedHosts(this.deps.screens(), (key) => this.deps.skip(key));
    const writes: [string, string][] = [];
    for (const target of new Set([...shown.keys(), ...this.stored.keys()])) {
      const body = JSON.stringify(shown.get(target) ?? []);
      if (body !== (this.stored.get(target) ?? "[]") && body !== this.refused.get(target)) writes.push([target, body]);
    }
    return writes;
  }

  private restore(): void {
    if (this.phase !== "waiting" || !this.localReady) return;
    const steps = planRestore(this.deps.screens(), this.read);
    // What the server holds counts as written; a restore that changes nothing
    // writes nothing. Closed hosts are not this window's to write.
    for (const [target, entries] of storedHosts(this.read)) this.stored.set(target, JSON.stringify(entries));
    if (steps.length > 0) this.deps.restore(steps);
    this.phase = "ready";
    this.changed();
  }

  private async write(target: string, body: string, keepalive: boolean): Promise<void> {
    const write: HostWrite = { target, screens: JSON.parse(body) as HostEntry[] };
    try {
      const res = await this.deps.fetch(HOST_PATH, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(write),
        keepalive,
      });
      if (res.ok) {
        this.refused.delete(target);
        if (body === "[]") this.stored.delete(target);
        else this.stored.set(target, body);
        return;
      }
      // A host's entries the server refuses are not sent again unchanged; any
      // other failure is retried with the next change.
      if (res.status === 400) this.refused.set(target, body);
      this.warn(`remembered remote screens: writing ${target} failed (HTTP ${res.status}) ${await errorOf(res)}`);
    } catch (err) {
      this.warn(`remembered remote screens: writing ${target} failed (${String(err)})`);
    }
  }

  private warn(message: string): void {
    (this.deps.warn ?? console.warn)(message);
  }
}

// ── Helpers ──

function identity(target: string, screenId: string): string {
  return `${target}\n${screenId}`;
}

function validScreenId(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const length = [...value].length;
  return length >= 1 && length <= MAX_SCREEN_ID;
}

/** `text` without lone surrogates, which a cut name may end with and JSON parsers refuse. */
function wellFormed(text: string): string {
  let out = "";
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        out += text[i] + text[i + 1];
        i++;
      }
    } else if (code < 0xdc00 || code > 0xdfff) {
      out += text[i];
    }
  }
  return out;
}

async function errorOf(res: Response): Promise<string> {
  try {
    const body = await res.json() as { error?: unknown };
    return typeof body?.error === "string" ? body.error : "";
  } catch {
    return "";
  }
}
