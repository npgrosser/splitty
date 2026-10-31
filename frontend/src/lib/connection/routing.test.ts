/**
 * The routing rule (connection-scoped-frontend): a message is resolved only
 * against the objects of the connection it arrived on.
 *
 * The window shows a local screen `screen-1` with the pane `terminal-1`, whose
 * session is `sess-1`. A hostile second connection reuses exactly those ids: it
 * has a screen `screen-1` of its own (bound under a random key), with a pane
 * `terminal-1` showing its own `sess-1`. Every message it sends must leave the
 * local objects untouched and send nothing on the local socket.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DockviewApi } from "dockview-core";
import { fakeEmulator, fakeServer, type FakeEmulator, type FakeSocket } from "../testing/fake_server.ts";
import { TerminalSession } from "../terminal/terminal_session.ts";
import { ScreenStartup } from "../mux/screen_startup.ts";
import { attachNotifications, getNotifications, getUnreadCountForScreen, getUnreadForPane, markPaneRead } from "../notifications.ts";
import { MAX_NAME, MAX_TITLE } from "../protocol/untrusted.ts";
import { newRemoteScreenKey } from "../ids.ts";
import type { ClientCommand, ScreenState } from "../protocol/types.ts";
import type { Screen } from "../../stores/screens.svelte.ts";
import { LocalScreenSync, ScreenViews, connectScreenSync, screenMuxHooks } from "./screen_sync.ts";
import { RemoteScreenSync } from "../mux/remote_screen_sync.ts";
import { parseCommand, resolveCommandTarget, resolveScreenKey, type CommandTarget } from "./routing.ts";
import type { ServerConnection } from "./server_connection.ts";

/** The window key of the host's `screen-1`. */
const HOST_KEY = newRemoteScreenKey();

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Settles a promise into a flag, so a test can see that it did not settle. */
function track<T>(promise: Promise<T>) {
  const state: { settled: boolean; value?: T; error?: unknown } = { settled: false };
  promise.then(
    (value) => { state.settled = true; state.value = value; },
    (error) => { state.settled = true; state.error = error; },
  );
  return state;
}

let layoutCounter = 0;

/** A layout API with the given panes, as far as routing looks at it. */
function fakeLayout(paneIds: string[], active: string): DockviewApi {
  return {
    id: `layout-${++layoutCounter}`,
    getPanel: (id: string) => (paneIds.includes(id) ? { id } : undefined),
    activePanel: { id: active },
  } as unknown as DockviewApi;
}

interface World {
  local: { socket: FakeSocket; conn: ServerConnection };
  host: { socket: FakeSocket; conn: ServerConnection };
  screens: Screen[];
  activeKey: string;
  views: ScreenViews;
  layouts: Map<string, DockviewApi>;
  /** Snapshots applied, as `<screen key>:<state id>`. */
  applied: string[];
  startupLog: string[];
  localEmulator: FakeEmulator;
  hostEmulator: FakeEmulator;
  localSession: TerminalSession;
  localWaiter: ReturnType<typeof track<string>>;
  localAttach: ReturnType<typeof track<unknown>>;
  localCreate: ReturnType<typeof track<unknown>>;
  /** Targets of the commands each connection sent. */
  targets: { origin: string; target: CommandTarget | null }[];
  allowClipboardWrite: boolean;
}

async function world(): Promise<World> {
  const local = fakeServer("local", "Local");
  const host = fakeServer("host:evil", "evil");
  const screens: Screen[] = [
    { id: "screen-1", name: "Default", conn: "local", wireId: "screen-1" },
    { id: HOST_KEY, name: "api", conn: "host:evil", wireId: "screen-1" },
  ];
  const layouts = new Map<string, DockviewApi>([
    ["screen-1", fakeLayout(["terminal-1", "terminal-local"], "terminal-1")],
    [HOST_KEY, fakeLayout(["terminal-1", "terminal-h"], "terminal-h")],
  ]);
  const applied: string[] = [];
  const views = new ScreenViews((_api, binding, state) => applied.push(`${binding.key}:${state.id}`));
  const localBinding = views.bind(screens[0], local.conn, true);
  const hostBinding = views.bind(screens[1], host.conn, false);
  if (!localBinding || !hostBinding) throw new Error("screens not bound");
  for (const [key, api] of layouts) views.ready(key, api);

  const startupLog: string[] = [];
  const localIds = () => screens.filter((s) => s.conn === "local").map((s) => s.id);
  const startup = new ScreenStartup(localIds(), {
    localScreenIds: localIds,
    createLocalScreen: (id) => startupLog.push(`create ${id}`),
    removeLocalScreen: (id) => startupLog.push(`remove ${id}`),
    resync: (id) => { startupLog.push(`resync ${id}`); local.conn.mux.resync(id); },
    startFresh: (id) => startupLog.push(`fresh ${id}`),
  });
  connectScreenSync(local.conn, new LocalScreenSync(local.conn, startup, views));
  connectScreenSync(host.conn, new RemoteScreenSync(host.conn, {
    screens: () => screens,
    snapshot: (key, state) => views.snapshot(host.conn.key, key, state),
    startFresh: (key) => { throw new Error(`a host message created ${key}`); },
    changed: () => {},
  }));
  attachNotifications(local.conn);
  attachNotifications(host.conn);

  // The local server welcomes and the local screen is built.
  local.socket.message({ type: "mux.welcome", clientId: "c-local", screens: [{ id: "screen-1", name: "Default" }] });

  // The same session id on both servers, each shown in its own `terminal-1`.
  const localEmulator = fakeEmulator();
  const localSession = new TerminalSession(local.conn.sessions, "sess-1", localEmulator);
  localSession.wire();
  localBinding.linkPane("terminal-1", "sess-1");
  const hostEmulator = fakeEmulator();
  new TerminalSession(host.conn.sessions, "sess-1", hostEmulator).wire();
  hostBinding.linkPane("terminal-1", "sess-1");

  // Local requests in flight that a host message must not settle.
  const localWaiter = track(local.conn.mux.waitForPaneSpawned("terminal-9", 60_000));
  const localAttach = track(local.conn.sessions.attach("sess-2", 80, 24));
  const localCreate = track(local.conn.sessions.requestSession({ cols: 80, rows: 24, paneId: "terminal-8" }));
  await flush();

  local.socket.sent = [];
  host.socket.sent = [];
  startupLog.length = 0;
  applied.length = 0;
  const w: World = {
    local, host, screens, activeKey: "screen-1", views, layouts, applied, startupLog,
    localEmulator, hostEmulator, localSession, localWaiter, localAttach, localCreate,
    targets: [], allowClipboardWrite: false,
  };
  for (const conn of [local.conn, host.conn]) {
    conn.onCommand((command) => w.targets.push({ origin: conn.key, target: resolve(w, conn, command) }));
  }
  return w;
}

function resolve(w: World, origin: ServerConnection, command: ClientCommand): CommandTarget | null {
  return resolveCommandTarget(origin, command, {
    screens: w.screens,
    activeKey: w.activeKey,
    hasPane: (key, paneId) => !!w.layouts.get(key)?.getPanel(paneId),
    activePane: (key) => w.layouts.get(key)?.activePanel?.id,
  }, { allowClipboardWrite: w.allowClipboardWrite });
}

function expectLocalUntouched(w: World) {
  expect(w.local.socket.sent).toEqual([]);
  expect(w.localEmulator.written).toEqual([]);
  expect(w.localSession.lastTitle).toBe("");
  expect(w.local.conn.sessions.homeDir).toBe("");
  expect(w.startupLog).toEqual([]);
  expect(w.applied.filter((a) => a.startsWith("screen-1:"))).toEqual([]);
  expect(w.localWaiter.settled).toBe(false);
  expect(w.localAttach.settled).toBe(false);
  expect(w.localCreate.settled).toBe(false);
  expect(getUnreadForPane("local", "terminal-1")).toBeUndefined();
  expect(getUnreadCountForScreen("local", "screen-1")).toBe(0);
  expect(w.targets.filter((t) => t.target?.screenKey === "screen-1")).toEqual([]);
}

const snapshot = (id: string, name = "taken over"): Partial<ScreenState> => ({
  id, name, revision: 1,
  root: { type: "slot", id: "root", paneIds: ["terminal-1"] },
  floating: [], panes: { "terminal-1": { id: "terminal-1", sessionId: "sess-1" } }, sessions: {},
});

/** Everything a host may send, each naming the local ids. */
const hostileMessages: [string, (s: FakeSocket) => void][] = [
  ["output for sess-1 and sess-2", (s) => { s.output("sess-1", "pwned"); s.output("sess-2", "pwned"); }],
  ["attached for the session the local client is attaching", (s) => s.message({ type: "attached", sessionId: "sess-2", homeDir: "/evil" })],
  ["error for the session the local client is attaching", (s) => s.message({ type: "error", sessionId: "sess-2", error: "nope" })],
  ["created while the local client waits for a create", (s) => s.message({ type: "created", sessionId: "sess-evil", homeDir: "/evil" })],
  ["closed for sess-1", (s) => s.message({ type: "closed", sessionId: "sess-1", exitCode: 1 })],
  ["title for sess-1", (s) => s.message({ type: "title", sessionId: "sess-1", title: "evil", cwd: "/evil", baseShell: false })],
  ["bell for sess-1", (s) => s.message({ type: "bell", sessionId: "sess-1" })],
  ["pane_spawned for the pane the local client waits for", (s) => s.message({ type: "mux.pane_spawned", paneId: "terminal-9", sessionId: "sess-evil" })],
  ["op_failed for the pane the local client waits for", (s) => s.message({ type: "mux.op_failed", op: "split_pane", paneId: "terminal-9", error: "x" })],
  ["welcome listing screen-1", (s) => s.message({ type: "mux.welcome", clientId: "c-evil", screens: [{ id: "screen-1" }, { id: "screen-2" }] })],
  ["snapshot of screen-1", (s) => s.message({ type: "mux.screen_snapshot", screen: snapshot("screen-1") })],
  ["snapshot of a screen the window does not show", (s) => s.message({ type: "mux.screen_snapshot", screen: snapshot("screen-2") })],
  ["nudge for screen-1", (s) => s.message({ type: "mux.nudge", reason: "x", screenId: "screen-1" })],
  ["process.finished for sess-1", (s) => s.message({ type: "process.finished", sessionId: "sess-1", process: "make" })],
  ["notification for sess-1", (s) => s.message({ type: "notification", sessionId: "sess-1", severity: "error", title: "evil" })],
  ["pane.close of terminal-1", (s) => s.message({ type: "command", command: { type: "pane.close", paneId: "terminal-1" } })],
  ["pane.close of a pane only the local screen has", (s) => s.message({ type: "command", command: { type: "pane.close", paneId: "terminal-local" } })],
  ["pane.split without a pane while the local screen is active", (s) => s.message({ type: "command", command: { type: "pane.split", direction: "right" } })],
  ["pane.paste without a pane", (s) => s.message({ type: "command", command: { type: "pane.paste" } })],
  ["pane.activate of terminal-1", (s) => s.message({ type: "command", command: { type: "pane.activate", paneId: "terminal-1" } })],
];

let w: World;

beforeEach(async () => {
  w = await world();
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
  vi.spyOn(console, "debug").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("a hostile second connection reusing the local ids", () => {
  it.each(hostileMessages)("changes nothing local: %s", async (_name, send) => {
    send(w.host.socket);
    await flush();
    expectLocalUntouched(w);
  });

  it("changes nothing local with all of it at once, and the local connection still works", async () => {
    for (const [, send] of hostileMessages) send(w.host.socket);
    await flush();
    expectLocalUntouched(w);

    w.local.socket.output("sess-1", "local output");
    w.local.socket.message({ type: "mux.pane_spawned", paneId: "terminal-9", sessionId: "sess-9" });
    w.local.socket.message({ type: "attached", sessionId: "sess-2" });
    w.local.socket.message({ type: "created", sessionId: "sess-8" });
    await flush();
    expect(w.localEmulator.written).toEqual(["local output"]);
    expect(w.localWaiter.value).toBe("sess-9");
    expect(w.localAttach.settled && !w.localAttach.error).toBe(true);
    expect(w.localCreate.value).toMatchObject({ sessionId: "sess-8" });
  });

  it("reaches its own objects under the same ids", async () => {
    w.host.socket.output("sess-1", "host output");
    w.host.socket.message({ type: "mux.screen_snapshot", screen: snapshot("screen-1") });
    w.host.socket.message({ type: "mux.nudge", reason: "x", screenId: "screen-1" });
    w.host.socket.message({ type: "mux.welcome", clientId: "c-evil", screens: [{ id: "screen-1" }] });
    await flush();

    expect(w.hostEmulator.written).toEqual(["host output"]);
    expect(w.applied).toEqual([`${HOST_KEY}:screen-1`]);
    expect(w.host.socket.sentOfType("mux.resync").map((m) => m.screenId)).toEqual(["screen-1", "screen-1"]);
    expect(w.local.socket.sent).toEqual([]);
  });

  it("tags its notifications with the host, on its own pane", async () => {
    w.host.socket.message({ id: "n-7", type: "process.finished", sessionId: "sess-1", process: "make" });
    w.local.socket.message({ id: "n-7", type: "process.finished", sessionId: "sess-1", process: "make" });
    await flush();

    const both = getNotifications().filter((n) => n.id.endsWith("/n-7"));
    expect(both.map((n) => [n.conn, n.title, n.screenId, n.paneId])).toEqual([
      ["host:evil", "evil: make finished", HOST_KEY, "terminal-1"],
      ["local", "make finished", "screen-1", "terminal-1"],
    ]);
    expect(getUnreadForPane("host:evil", "terminal-1")?.conn).toBe("host:evil");
    expect(getUnreadForPane("local", "terminal-1")?.id).toBe("local/n-7");
    // The notification store is module state; later tests start without unread local ones.
    markPaneRead("local", "terminal-1");
  });

  describe("server commands", () => {
    const cases: [string, "local" | "host", ClientCommand, string, boolean, string | null][] = [
      // name, origin, command, active screen, clipboard writes allowed, expected target screen
      ["host closes its own terminal-1", "host", { type: "pane.close", paneId: "terminal-1" }, "screen-1", false, HOST_KEY],
      ["host closes a pane only the local screen has", "host", { type: "pane.close", paneId: "terminal-local" }, "screen-1", false, null],
      ["host pastes into its own pane", "host", { type: "pane.paste", paneId: "terminal-h" }, HOST_KEY, false, null],
      ["host pastes without a pane on its active screen", "host", { type: "pane.paste" }, HOST_KEY, true, null],
      ["host copies, clipboard writes off", "host", { type: "pane.copy", paneId: "terminal-h" }, HOST_KEY, false, null],
      ["host copies, clipboard writes on", "host", { type: "pane.copy", paneId: "terminal-h" }, HOST_KEY, true, HOST_KEY],
      ["host splits without a pane, local screen active", "host", { type: "pane.split", direction: "right" }, "screen-1", false, null],
      ["host splits without a pane, its screen active", "host", { type: "pane.split", direction: "below" }, HOST_KEY, false, HOST_KEY],
      ["local closes a host-only pane", "local", { type: "pane.close", paneId: "terminal-h" }, "screen-1", false, null],
      ["local splits without a pane, host screen active", "local", { type: "pane.split", direction: "right" }, HOST_KEY, false, null],
      ["local pastes into its own pane", "local", { type: "pane.paste", paneId: "terminal-local" }, "screen-1", false, "screen-1"],
      // The target is on an inactive screen: it stays inactive, the command acts there.
      ["local activates a pane of an inactive local screen", "local", { type: "pane.activate", paneId: "terminal-1" }, HOST_KEY, false, "screen-1"],
    ];

    it.each(cases)("%s", (_name, origin, command, activeKey, allowClipboardWrite, expected) => {
      w.activeKey = activeKey;
      w.allowClipboardWrite = allowClipboardWrite;
      const conn = origin === "local" ? w.local.conn : w.host.conn;
      const target = resolve(w, conn, command);
      expect(target?.screenKey ?? null).toBe(expected);
    });

    it("drops commands it does not know", async () => {
      for (const command of [null, "pane.close", { type: "pane.explode" }, { type: "pane.close", paneId: 1 }, { type: "pane.split", direction: "left" }]) {
        w.host.socket.message({ type: "command", command });
      }
      await flush();
      expect(w.targets).toEqual([]);
      expect(parseCommand({ type: "pane.split", paneId: "t", direction: "below" })).toEqual({ type: "pane.split", paneId: "t", direction: "below" });
    });
  });

  it("finds screens only among the sender's", () => {
    expect(resolveScreenKey("host:evil", "screen-1", w.screens)).toBe(HOST_KEY);
    expect(resolveScreenKey("local", "screen-1", w.screens)).toBe("screen-1");
    expect(resolveScreenKey("host:other", "screen-1", w.screens)).toBeUndefined();
  });
});

describe("crafted strings", () => {
  it("stay plain data with length caps", async () => {
    const crafted = `<img src=x onerror=alert(1)>\u001b[31m‮${"y".repeat(5000)}`;
    w.host.socket.message({ id: "n-crafted", type: "notification", severity: "<script>", title: crafted, body: crafted, sessionId: "sess-1" });
    const snapshots: ScreenState[] = [];
    w.host.conn.mux.setHandlers({ onSnapshot: (s) => snapshots.push(s) });
    w.host.socket.message({ type: "mux.screen_snapshot", screen: { ...snapshot("screen-1"), name: crafted } });
    await flush();

    const n = getNotifications().find((x) => x.id === "host:evil/n-crafted");
    expect(n?.title.startsWith("evil: <img src=x onerror=alert(1)>[31m")).toBe(true);
    expect(n?.title).toHaveLength("evil: ".length + MAX_TITLE);
    expect(n?.body).not.toMatch(/[\u0000-\u001f‮]/);
    expect(n?.severity).toBe("info");
    expect(snapshots[0].name).toHaveLength(MAX_NAME);
    expect(snapshots[0].name).not.toMatch(/[\u0000-\u001f‮]/);
  });
});

describe("malformed messages", () => {
  it("throw nothing out of the socket, and later handlers still run", async () => {
    const later: string[] = [];
    w.host.socket.onMessage((msg) => later.push(msg.type));
    const malformed: Record<string, unknown>[] = [
      { type: "mux.welcome", clientId: 5, screens: null },
      { type: "mux.welcome", screens: [null, 5, { id: 7 }] },
      { type: "mux.screen_snapshot", screen: { id: "screen-1", root: { type: "slot", id: "r" } } },
      { type: "mux.screen_snapshot", screen: "screen-1" },
      { type: "mux.screen_snapshot", screen: { id: "screen-1", root: { type: "split", id: "s", children: "x" }, panes: [], floating: 3 } },
      { type: "mux.nudge", screenId: { id: "screen-1" } },
      { type: "mux.pane_spawned", paneId: null, sessionId: 1 },
      { type: "title", sessionId: "sess-1", title: null, cwd: 5 },
      { type: "closed", sessionId: ["sess-1"] },
      { type: "process.finished", sessionId: 1, process: { x: 1 } },
      { type: "command", command: { type: "pane.close", paneId: { toString: 1 } } },
    ];

    for (const msg of malformed) {
      expect(() => w.host.socket.message(msg)).not.toThrow();
    }
    expect(() => w.host.socket.binary(new Uint8Array([250, 1, 2]))).not.toThrow();
    expect(() => w.host.socket.text("{")).not.toThrow();
    await flush();

    expect(later).toHaveLength(malformed.length);
    expectLocalUntouched(w);
  });
});

describe("screen hooks", () => {
  it("send a reorder to the local server with local screens only, and a rename to the screen's server", () => {
    const hooks = screenMuxHooks((key) => (key === "local" ? w.local.conn : key === "host:evil" ? w.host.conn : undefined));
    hooks.onReorder?.([w.screens[1], w.screens[0]]);
    hooks.onRename?.(w.screens[1], "renamed");

    expect(w.local.socket.sentOfType("mux.op").map((m) => m.op)).toEqual([
      { type: "reorder_screens", screenOrder: ["screen-1"] },
    ]);
    expect(w.host.socket.sentOfType("mux.op").map((m) => m.op)).toEqual([
      { type: "rename_screen", screenId: "screen-1", name: "renamed" },
    ]);
  });
});

// workspace-model, Moving a Pane to Another Screen: the window follows the pane.
describe("screen views", () => {
  it("show a pane moving in as its group's active tab once a snapshot holds it", () => {
    const activated: string[] = [];
    const api = {
      id: "layout-reveal",
      getPanel: (id: string) => ({ id, api: { setActive: () => activated.push(id) } }),
    } as unknown as DockviewApi;
    const local = fakeServer("local", "Local");
    const views = new ScreenViews(() => {});
    views.bind({ id: "screen-2", conn: "local", wireId: "screen-2" }, local.conn, true);
    views.ready("screen-2", api);
    const state = (panes: string[]) => ({ id: "screen-2", panes: Object.fromEntries(panes.map((id) => [id, { id }])) }) as unknown as ScreenState;

    views.reveal("screen-2", "terminal-moved");
    views.snapshot("local", "screen-2", state(["terminal-2"]));
    views.snapshot("local", "screen-2", state(["terminal-2", "terminal-moved"]));
    views.snapshot("local", "screen-2", state(["terminal-2", "terminal-moved"]));

    expect(activated).toEqual(["terminal-moved"]);
    local.conn.dispose();
  });
});
