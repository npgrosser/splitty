// The remote screens of the window, remembered across restarts by the local
// server (`GET` and `PUT /api/remote/screens`; `remote-screens`, Remembered
// Remote Screens).
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
// (`hostToConnect`), so an unreachable host does not slow the start.
//
// The list is written after every change, debounced, and never before the
// remembered one was read and restored: a write before that would replace it.
// After a failed read the page leaves the list alone.

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

export interface RememberedScreen {
  target: string;
  /** The screen's id on the host. */
  screenId: string;
  /** Its name when last seen, shown until the host is connected. */
  name?: string;
  /** Its index in the window's screen list, local screens included. */
  position?: number;
}

export interface RememberedList {
  version: typeof FORMAT;
  screens: RememberedScreen[];
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
    const { target, screenId, name, position } = entry as Record<string, unknown>;
    if (typeof target !== "string" || validateTarget(target) !== target) continue;
    if (!validScreenId(screenId)) continue;
    const id = identity(target, screenId);
    if (seen.has(id)) continue;
    seen.add(id);
    const screen: RememberedScreen = { target, screenId };
    const shown = plainText(name, MAX_NAME);
    if (shown) screen.name = shown;
    if (typeof position === "number" && Number.isInteger(position) && position >= 0) screen.position = position;
    result.push(screen);
  }
  return result;
}

/**
 * The list to write for the window's `screens`: every remote screen but the
 * ones `skip` names, with its index in the window as its position. A screen
 * the server would refuse (an id it does not take) is left out, so one odd
 * screen cannot cost the whole list.
 */
export function rememberedList(screens: readonly WindowScreen[], skip: (key: string) => boolean): RememberedList {
  const list: RememberedScreen[] = [];
  const seen = new Set<string>();
  screens.forEach((screen, position) => {
    const target = hostOf(screen.conn);
    if (!target || list.length >= MAX_REMEMBERED || skip(screen.id)) return;
    if (!validScreenId(screen.wireId)) return;
    const id = identity(target, screen.wireId);
    if (seen.has(id)) return;
    seen.add(id);
    const entry: RememberedScreen = { target, screenId: screen.wireId };
    const name = wellFormed(plainText(screen.name, MAX_NAME));
    if (name) entry.name = name;
    entry.position = position;
    list.push(entry);
  });
  return { version: FORMAT, screens: list };
}

/**
 * Where the remembered screens go back into the window's `shown` screens:
 * by position (screens without one last, in their order), each at its
 * position or at the end of the list as it is then. Screens the window shows
 * already are skipped.
 */
export function planRestore(shown: readonly ScreenRef[], remembered: readonly RememberedScreen[]): RestoreStep[] {
  const present = new Set(shown.map((s) => `${s.conn}\n${s.wireId}`));
  const ordered = remembered
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
  private timer: ReturnType<typeof setTimeout> | undefined;
  /** The list last sent, as JSON, so an unchanged list is not sent again. */
  private sent: string | undefined;
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

  /** The window's screens changed: write the list soon, if it changed. */
  changed(): void {
    if (this.phase !== "ready" || this.disposed) return;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = undefined;
      this.flush();
    }, WRITE_DELAY);
  }

  /**
   * Write the list now, if it changed. With `keepalive` the request outlives
   * the page (`pagehide`) and does not wait for one in flight.
   */
  flush(keepalive = false): void {
    clearTimeout(this.timer);
    this.timer = undefined;
    if (this.phase !== "ready" || this.disposed) return;
    const body = JSON.stringify(rememberedList(this.deps.screens(), (key) => this.deps.skip(key)));
    if (body === this.sent) return;
    if (this.inFlight && !keepalive) {
      this.again = true;
      return;
    }
    this.sent = body;
    this.inFlight = true;
    void this.write(body, keepalive).finally(() => {
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

  private restore(): void {
    if (this.phase !== "waiting" || !this.localReady) return;
    const steps = planRestore(this.deps.screens(), this.read);
    // What the server holds counts as written; a restore that changes nothing
    // writes nothing.
    this.sent = JSON.stringify({ version: FORMAT, screens: this.read });
    if (steps.length > 0) this.deps.restore(steps);
    this.phase = "ready";
    this.changed();
  }

  private async write(body: string, keepalive: boolean): Promise<void> {
    try {
      const res = await this.deps.fetch(PATH, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body,
        keepalive,
      });
      if (res.ok) return;
      // A list the server refuses is not sent again unchanged; any other
      // failure is retried with the next change.
      if (res.status !== 400 && this.sent === body) this.sent = undefined;
      this.warn(`remembered remote screens: writing failed (HTTP ${res.status}) ${await errorOf(res)}`);
    } catch (err) {
      if (this.sent === body) this.sent = undefined;
      this.warn(`remembered remote screens: writing failed (${String(err)})`);
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
