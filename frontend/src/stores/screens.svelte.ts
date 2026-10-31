import type { ConnKey } from "../lib/connection/server_connection.ts";
import { newRemoteScreenKey, newScreenId } from "../lib/ids.ts";
import { reorderScreensInGroup } from "../lib/remote/screen_groups.ts";

export interface Screen {
  /** The screen's key in this window; for a local screen, its id on the local server. */
  id: string;
  name: string;
  /** The connection the screen belongs to. */
  conn: ConnKey;
  /** The screen's id on its server; the same as `id` for a local screen. */
  wireId: string;
}

function localScreen(id: string, name: string): Screen {
  return { id, name, conn: "local", wireId: id };
}

const DEFAULT_NAME = /^Screen \d+$/;

/** Whether `name` is one {@link nextScreenName} gives, not one the user chose. */
export function isDefaultScreenName(name: string): boolean {
  return DEFAULT_NAME.test(name);
}

/**
 * The name of a new screen: "Screen N" with the lowest positive N that no
 * screen of this connection uses, nor a name in `alsoTaken` (the host's
 * screens the window does not show). Each host and local start at Screen 1.
 */
export function nextScreenName(conn: ConnKey = "local", alsoTaken: Iterable<string> = []): string {
  const taken = new Set([...screens.filter((v) => v.conn === conn).map((v) => v.name), ...alsoTaken]);
  let n = 1;
  while (taken.has(`Screen ${n}`)) n++;
  return `Screen ${n}`;
}

// ── Module State ──

// Ids are random (lib/ids.ts); names are not unique keys.
const defaultScreen = localScreen(newScreenId(), "Screen 1");
let screens = $state<Screen[]>([defaultScreen]);
let activeId = $state(defaultScreen.id);
/** The order of the host groups the user arranged, by SSH target (remote-hosts, Host Group Order). */
let hostOrder = $state<string[]>([]);

const listeners = new Set<() => void>();

function notify(): void {
  for (const cb of listeners) cb();
}

// ── Hooks for mux notifications (set by the workspace to avoid a circular dep) ──
// Each sends to the screen's own connection (lib/connection/screen_sync.ts).

export interface ScreenMuxHooks {
  onRename?: (screen: Screen, name: string) => void;
  onSetActive?: (screen: Screen) => void;
  onReorder?: (screens: Screen[]) => void;
}

let hooks: ScreenMuxHooks = {};

export function setScreenMuxHooks(next: ScreenMuxHooks): void {
  hooks = next;
}

// ── Public API ──

export function getScreens(): Screen[] {
  return screens;
}

export function getActiveScreenId(): string {
  return activeId;
}

export function getHostOrder(): string[] {
  return hostOrder;
}

/** The arranged order of the host groups, as loaded or after a move; the display follows it. */
export function setHostOrder(order: string[]): void {
  if (order.length === hostOrder.length && order.every((target, i) => target === hostOrder[i])) return;
  hostOrder = order;
  notify();
}

/**
 * Create a screen of the local server and make it active. `id` is the
 * server's id of a screen it already has; a key that is taken is not added
 * twice. It is placed after the screen `after`, by default at the end.
 */
export function createScreen(name?: string, id?: string, options?: { activate?: boolean; after?: string }): Screen {
  const existing = id === undefined ? undefined : screens.find((v) => v.id === id);
  if (existing) return existing;
  const v = localScreen(id ?? newScreenId(), name ?? nextScreenName());
  const after = options?.after === undefined ? -1 : screens.findIndex((s) => s.id === options.after);
  screens = after < 0 ? [...screens, v] : [...screens.slice(0, after + 1), v, ...screens.slice(after + 1)];
  if (options?.activate !== false || !screens.some(s => s.id === activeId)) activeId = v.id;
  notify();
  return v;
}

/**
 * Add a screen of a host's connection (`remote-screens`) under a random key of
 * its own. `wireId` is the screen's id on the host; without a `name` it gets
 * the next free "Screen N". It is placed after the screen `after`, or at `index`
 * (a remembered screen's position), by default at the end, and becomes active
 * unless `activate` is false.
 */
export function addRemoteScreen(
  conn: Exclude<ConnKey, "local">,
  wireId: string,
  options?: { name?: string; after?: string; index?: number; activate?: boolean },
): Screen {
  const screen: Screen = { id: newRemoteScreenKey(), name: options?.name ?? nextScreenName(conn), conn, wireId };
  let at = screens.length;
  if (options?.after !== undefined) {
    const after = screens.findIndex((v) => v.id === options.after);
    if (after >= 0) at = after + 1;
  } else if (options?.index !== undefined) {
    at = Math.max(0, Math.min(options.index, screens.length));
  }
  screens = [...screens.slice(0, at), screen, ...screens.slice(at)];
  if (options?.activate !== false) activeId = screen.id;
  notify();
  return screen;
}

/** Take a name the screen's server reports; nothing is sent back. */
export function setScreenName(id: string, name: string): void {
  const screen = screens.find((v) => v.id === id);
  if (!screen || !name || screen.name === name) return;
  screens = screens.map((v) => (v.id === id ? { ...v, name } : v));
  notify();
}

export function removeScreen(id: string): void {
  if (screens.length <= 1) return;
  const idx = screens.findIndex((v) => v.id === id);
  if (idx < 0) return;
  screens = screens.filter((v) => v.id !== id);
  if (activeId === id) {
    activeId = screens[Math.min(idx, screens.length - 1)].id;
  }
  notify();
}

export function renameScreen(id: string, name: string): void {
  const screen = screens.find((v) => v.id === id);
  if (!screen) return;
  const renamed = { ...screen, name };
  screens = screens.map((v) => (v.id === id ? renamed : v));
  notify();
  hooks.onRename?.(renamed, name);
}

export function moveScreen(fromIndex: number, toIndex: number): void {
  if (fromIndex === toIndex) return;
  if (fromIndex < 0 || fromIndex >= screens.length) return;
  if (toIndex < 0 || toIndex >= screens.length) return;
  const next = [...screens];
  const [moved] = next.splice(fromIndex, 1);
  next.splice(toIndex, 0, moved);
  screens = next;
  notify();
  hooks.onReorder?.(screens);
}

/** Move within a host's (or the local) group, preserving every other group's raw slots. */
export function moveScreenInGroup(conn: ConnKey, fromIndex: number, toIndex: number): void {
  const next = reorderScreensInGroup(screens, conn, fromIndex, toIndex);
  if (!next) return;
  screens = next;
  notify();
  hooks.onReorder?.(screens);
}

export function setActiveScreen(id: string): void {
  if (activeId === id) return;
  const screen = screens.find((v) => v.id === id);
  if (!screen) return;
  activeId = id;
  notify();
  hooks.onSetActive?.(screen);
}

export function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
