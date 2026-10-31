import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MAX_REMEMBERED, PATH, RememberedScreens, WRITE_DELAY, hostToConnect, parseRemembered, planRestore,
  rememberedList, type RememberedList, type RememberedScreen,
} from "./remembered.ts";
import { detachRemoteScreen, removeRemoteScreen, terminateRemoteScreen, type CloseEffects } from "./close_screen.ts";
import { connectionFor, ensureRemote, removeConnection } from "../connection/registry.ts";
import { hostKey } from "../connection/server_connection.ts";
import { fakeServer } from "../testing/fake_server.ts";
import { RemoteSocket, type HubWebSocket } from "./remote_socket.ts";
import type { Screen } from "../../stores/screens.svelte.ts";

const local = (id: string): Screen => ({ id, name: id, conn: "local", wireId: id });
const remote = (id: string, target: string, wireId: string, name = wireId): Screen =>
  ({ id, name, conn: hostKey(target), wireId });

/** The window's order as `name@host` (remote) or `name` (local). */
const order = (screens: readonly Screen[]) =>
  screens.map((s) => (s.conn === "local" ? s.name : `${s.wireId}@${s.conn.slice(5)}`));

/** Apply a restore plan the way the screen store does. */
function applyPlan(screens: Screen[], plan: ReturnType<typeof planRestore>): void {
  let n = 0;
  for (const { screen, index } of plan) {
    screens.splice(index, 0, remote(`rs-${++n}`, screen.target, screen.screenId, screen.name ?? screen.screenId));
  }
}

describe("parseRemembered", () => {
  it("keeps the entries of the shape, each screen once", () => {
    expect(parseRemembered({
      version: 1,
      screens: [
        { target: "me@vm1", screenId: "s1", name: "api", position: 2 },
        { target: "vm2", screenId: "s9" },
        { target: "vm1 ", screenId: "s1" },
        { target: "-oProxyCommand=sh", screenId: "s1" },
        { target: "vm2", screenId: "" },
        { target: "vm2", screenId: "x".repeat(257) },
        { target: "me@vm1", screenId: "s1", name: "again" },
        { target: "vm2", screenId: "s3", name: "a‮b", position: -1 },
        { target: "vm2", screenId: "s4", position: 1.5 },
        "junk",
      ],
    })).toEqual([
      { target: "me@vm1", screenId: "s1", name: "api", position: 2 },
      { target: "vm2", screenId: "s9" },
      { target: "vm2", screenId: "s3", name: "ab" },
      { target: "vm2", screenId: "s4" },
    ]);
  });

  it("is empty for another version or no list", () => {
    expect(parseRemembered({ version: 2, screens: [{ target: "vm1", screenId: "s1" }] })).toEqual([]);
    expect(parseRemembered(null)).toEqual([]);
    expect(parseRemembered({ version: 1 })).toEqual([]);
  });
});

describe("rememberedList", () => {
  it("lists the remote screens with their places among the local screens", () => {
    const screens = [local("L1"), remote("rs-a", "vm1", "s1", "api"), local("L2"), remote("rs-b", "me@vm2:2222", "s7", "web")];
    expect(rememberedList(screens, () => false)).toEqual({
      version: 1,
      screens: [
        { target: "vm1", screenId: "s1", name: "api", position: 1 },
        { target: "me@vm2:2222", screenId: "s7", name: "web", position: 3 },
      ],
    });
  });

  it("leaves out new screens the host does not have yet and screens the server would refuse", () => {
    const screens = [
      local("L1"),
      remote("rs-new", "vm1", "screen-new"),
      remote("rs-long", "vm1", "x".repeat(257)),
      remote("rs-a", "vm1", "s1", `${"n".repeat(199)}\u{1F600}`),
    ];
    const list = rememberedList(screens, (key) => key === "rs-new");
    // The name is cut to 200 UTF-16 units, which split the emoji: its lone half goes.
    expect(list.screens).toEqual([{ target: "vm1", screenId: "s1", name: "n".repeat(199), position: 3 }]);
  });

  it("keeps at most as many screens as the server takes", () => {
    const screens = Array.from({ length: MAX_REMEMBERED + 5 }, (_, i) => remote(`rs-${i}`, "vm1", `s${i}`));
    expect(rememberedList(screens, () => false).screens).toHaveLength(MAX_REMEMBERED);
  });
});

describe("planRestore", () => {
  it("puts the screens back at their places among the local screens", () => {
    const screens = [local("L1"), local("L2")];
    applyPlan(screens, planRestore(screens, [
      { target: "vm1", screenId: "c", position: 4 },
      { target: "vm1", screenId: "d" },
      { target: "vm1", screenId: "a", position: 0 },
      { target: "vm2", screenId: "b", position: 2 },
    ]));
    expect(order(screens)).toEqual(["a@vm1", "L1", "b@vm2", "L2", "c@vm1", "d@vm1"]);
  });

  it("gives back the order it was written in when the local screens are the same", () => {
    const before = [
      remote("rs-1", "vm1", "s1"), local("L1"), remote("rs-2", "vm2", "s2"), remote("rs-3", "vm1", "s3"), local("L2"),
      remote("rs-4", "vm1", "s4"),
    ];
    const written = rememberedList(before, () => false).screens;
    const after = [local("L1"), local("L2")];
    applyPlan(after, planRestore(after, written));
    expect(order(after)).toEqual(order(before));
    expect(rememberedList(after, () => false).screens).toEqual(written);
  });

  it("puts what no longer fits at the end, and skips screens the window shows already", () => {
    const screens = [local("L1"), remote("rs-x", "vm1", "s1")];
    applyPlan(screens, planRestore(screens, [
      { target: "vm1", screenId: "s1", position: 0 },
      { target: "vm1", screenId: "s2", position: 9 },
    ]));
    expect(order(screens)).toEqual(["L1", "s1@vm1", "s2@vm1"]);
  });
});

// ── Lazy Connect ──

/** A hub socket that never answers: what matters is whether one is opened. */
function silentHub(): HubWebSocket {
  return {
    binaryType: "blob", readyState: 0, onopen: null, onmessage: null, onclose: null, onerror: null,
    send() {}, close() {},
  };
}

describe("lazy connect", () => {
  afterEach(() => {
    removeConnection("host:vm1");
    removeConnection("host:vm2");
  });

  it("opens no socket at start; a remembered screen connects its host when it is shown", async () => {
    const opened: string[] = [];
    const open = (target: string) => new RemoteSocket(target, {
      fetchTicket: async () => "t",
      open: (to) => { opened.push(to); return silentHub(); },
    });
    const connect = (screens: Screen[], active: string) => {
      const key = hostToConnect(screens, active, (k) => !!connectionFor(k));
      if (key) ensureRemote(key.slice(5), open);
    };

    const screens = [local("L1")];
    applyPlan(screens, planRestore(screens, [
      { target: "vm1", screenId: "s1", position: 0 },
      { target: "vm1", screenId: "s2", position: 2 },
      { target: "vm2", screenId: "s3", position: 3 },
    ]));
    const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
    connect(screens, "L1");
    await tick();
    expect(opened).toEqual([]);
    expect(connectionFor("host:vm1")).toBeUndefined();

    // Shown: its host connects, once.
    connect(screens, screens[0].id);
    connect(screens, screens[0].id);
    await tick();
    expect(opened).toEqual(["vm1"]);

    // vm1's other screen needs no connection of its own: its host is connected anyway.
    expect(hostToConnect(screens, screens[2].id, (k) => !!connectionFor(k))).toBeUndefined();
    // vm2 stays unconnected until its screen is shown.
    expect(connectionFor("host:vm2")).toBeUndefined();
    expect(hostToConnect(screens, screens[3].id, (k) => !!connectionFor(k))).toBe("host:vm2");
  });
});

// ── Reading and Writing ──

interface Request {
  method: string;
  body?: RememberedList;
  keepalive?: boolean;
}

interface Win {
  screens: Screen[];
  pending: Set<string>;
  requests: Request[];
  /** The list the server has. */
  stored: unknown;
  /** Statuses of the next writes; 200 when empty. */
  statuses: number[];
  /** The read fails. */
  readFails: boolean;
  /** Writes wait until this is resolved, when set. */
  gate?: Promise<void>;
  warnings: string[];
  remembered: RememberedScreens;
  /** Change the window's screens the way the store does: mutate, then notify. */
  change(fn: (screens: Screen[]) => void): void;
}

function win(screens: Screen[], stored: RememberedScreen[] = []): Win {
  const w = {
    screens, pending: new Set<string>(), requests: [] as Request[], stored: { version: 1, screens: stored },
    statuses: [] as number[], readFails: false, warnings: [] as string[],
  } as Win;
  w.remembered = new RememberedScreens({
    fetch: async (path, init) => {
      expect(path).toBe(PATH);
      const method = init?.method ?? "GET";
      if (method === "GET") {
        w.requests.push({ method });
        if (w.readFails) return new Response("", { status: 500 });
        return new Response(JSON.stringify(w.stored));
      }
      const body = JSON.parse(String(init?.body)) as RememberedList;
      w.requests.push({ method, body, keepalive: init?.keepalive });
      if (w.gate) await w.gate;
      const status = w.statuses.shift() ?? 200;
      if (status !== 200) return new Response(JSON.stringify({ error: "nope" }), { status });
      w.stored = body;
      return new Response(JSON.stringify(body));
    },
    screens: () => w.screens,
    skip: (key) => w.pending.has(key),
    restore: (steps) => {
      w.change((s) => applyPlan(s, steps as ReturnType<typeof planRestore>));
    },
    warn: (message) => w.warnings.push(message),
  });
  w.change = (fn) => {
    fn(w.screens);
    w.remembered.changed();
  };
  return w;
}

const writes = (w: Win) => w.requests.filter((r) => r.method === "PUT");
const settle = () => vi.advanceTimersByTimeAsync(0);
const afterDelay = () => vi.advanceTimersByTimeAsync(WRITE_DELAY);

/** Minimal close effects over the window: what the workspace does to the store. */
function closeEffects(w: Win, host = fakeServer("host:vm1", "vm1")): CloseEffects {
  return {
    screens: () => w.screens,
    createLocalScreen: () => w.change((s) => s.push(local("L-new"))),
    removeScreen: (key) => w.change((s) => s.splice(s.findIndex((v) => v.id === key), 1)),
    connection: (key) => (key === "host:vm1" ? host.conn : undefined),
    sync: () => undefined,
    panes: () => 1,
    releaseHost: () => {},
  };
}

describe("RememberedScreens", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("puts the screens back once the local screens are settled, whichever comes first", async () => {
    const stored = [{ target: "vm1", screenId: "s1", name: "api", position: 0 }];

    const readFirst = win([local("L1")], stored);
    await readFirst.remembered.load();
    expect(order(readFirst.screens)).toEqual(["L1"]);
    readFirst.remembered.localScreensReady();
    expect(order(readFirst.screens)).toEqual(["s1@vm1", "L1"]);
    expect(readFirst.screens[0].name).toBe("api");

    const localFirst = win([local("L1")], stored);
    localFirst.remembered.localScreensReady();
    await localFirst.remembered.load();
    expect(order(localFirst.screens)).toEqual(["s1@vm1", "L1"]);

    // A second welcome (a reconnect of the local server) restores nothing again.
    localFirst.remembered.localScreensReady();
    expect(localFirst.screens).toHaveLength(2);
  });

  it("writes nothing before the remembered list was read and restored", async () => {
    const w = win([local("L1")], [{ target: "vm1", screenId: "s1", position: 0 }]);
    const loading = w.remembered.load();
    w.change((s) => s.push(remote("rs-new", "vm2", "s9")));
    w.remembered.flush();
    await afterDelay();
    await loading;
    await afterDelay();
    expect(writes(w)).toEqual([]);

    // Restored: the screen opened meanwhile is written with the remembered one.
    w.remembered.localScreensReady();
    await afterDelay();
    expect(order(w.screens)).toEqual(["s1@vm1", "L1", "s9@vm2"]);
    expect(writes(w).map((r) => r.body?.screens.map((s) => `${s.screenId}:${s.position}`))).toEqual([["s1:0", "s9:2"]]);
  });

  it("writes nothing when the restore left the list as it was", async () => {
    const w = win([local("L1"), local("L2")], [
      { target: "vm1", screenId: "s1", name: "api", position: 1 },
      { target: "vm2", screenId: "s2", name: "web", position: 3 },
    ]);
    w.remembered.localScreensReady();
    await w.remembered.load();
    await afterDelay();
    expect(order(w.screens)).toEqual(["L1", "s1@vm1", "L2", "s2@vm2"]);
    expect(writes(w)).toEqual([]);

    // Switching screens changes nothing in the list.
    w.change(() => {});
    await afterDelay();
    expect(writes(w)).toEqual([]);
  });

  async function restored(): Promise<Win> {
    const w = win([local("L1")], [
      { target: "vm1", screenId: "s1", name: "api", position: 1 },
      { target: "vm1", screenId: "s2", name: "infra", position: 2 },
    ]);
    w.remembered.localScreensReady();
    await w.remembered.load();
    await afterDelay();
    return w;
  }

  const lastWrite = (w: Win) => writes(w).at(-1)?.body?.screens.map((s) => `${s.screenId}@${s.target}:${s.position}`);

  it("writes the list after a collapse, once the changes settled", async () => {
    const w = await restored();
    detachRemoteScreen(w.screens[1].id, closeEffects(w));
    await vi.advanceTimersByTimeAsync(WRITE_DELAY - 1);
    expect(writes(w)).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    expect(lastWrite(w)).toEqual(["s2@vm1:1"]);
    expect(w.stored).toEqual({ version: 1, screens: [{ target: "vm1", screenId: "s2", name: "infra", position: 1 }] });
  });

  it("writes the list after a terminate, an open and a missing screen's removal", async () => {
    const w = await restored();
    const fx = closeEffects(w);

    expect(terminateRemoteScreen(w.screens[1].id, fx)).toBe(true);
    await afterDelay();
    expect(lastWrite(w)).toEqual(["s2@vm1:1"]);

    // Opened from "On hosts" or the reopen offer: at the end.
    w.change((s) => s.push(remote("rs-o", "vm1", "s5", "logs")));
    await afterDelay();
    expect(lastWrite(w)).toEqual(["s2@vm1:1", "s5@vm1:2"]);

    removeRemoteScreen(w.screens[1].id, fx);
    await afterDelay();
    expect(lastWrite(w)).toEqual(["s5@vm1:1"]);

    // Renamed by the host's snapshot, and moved.
    w.change((s) => { s[1] = { ...s[1], name: "logs (prod)" }; s.reverse(); });
    await afterDelay();
    expect(writes(w).at(-1)?.body?.screens).toEqual([{ target: "vm1", screenId: "s5", name: "logs (prod)", position: 0 }]);
    expect(writes(w)).toHaveLength(4);
  });

  it("gathers changes into one write", async () => {
    const w = await restored();
    w.change((s) => s.push(remote("rs-o", "vm2", "s7")));
    await vi.advanceTimersByTimeAsync(WRITE_DELAY / 2);
    w.change((s) => s.push(local("L2")));
    await vi.advanceTimersByTimeAsync(WRITE_DELAY / 2);
    w.change((s) => s.push(remote("rs-p", "vm2", "s8")));
    await afterDelay();
    expect(writes(w)).toHaveLength(1);
    expect(lastWrite(w)).toEqual(["s1@vm1:1", "s2@vm1:2", "s7@vm2:3", "s8@vm2:5"]);
  });

  it("leaves out a new screen until its host has it", async () => {
    const w = await restored();
    w.pending.add("rs-new");
    w.change((s) => s.push(remote("rs-new", "vm1", "screen-new")));
    await afterDelay();
    expect(writes(w)).toEqual([]);

    // `create_screen` went out: the screen sync's change notice.
    w.pending.delete("rs-new");
    w.remembered.changed();
    await afterDelay();
    expect(lastWrite(w)).toEqual(["s1@vm1:1", "s2@vm1:2", "screen-new@vm1:3"]);
  });

  it("never writes after a failed read", async () => {
    const w = win([local("L1")]);
    w.readFails = true;
    w.remembered.localScreensReady();
    await w.remembered.load();
    w.change((s) => s.push(remote("rs-o", "vm1", "s1")));
    w.remembered.flush(true);
    await afterDelay();
    expect(writes(w)).toEqual([]);
    expect(w.warnings).toHaveLength(1);
    expect(w.warnings[0]).toContain("HTTP 500");
  });

  it("logs a failed write; resends after a server failure, but not a refused list unchanged", async () => {
    const w = await restored();
    w.statuses = [500];
    w.change((s) => s.splice(1, 1));
    await afterDelay();
    expect(writes(w)).toHaveLength(1);
    expect(w.warnings[0]).toContain("HTTP 500");

    // The next notice sends the same list again.
    w.change(() => {});
    await afterDelay();
    expect(writes(w)).toHaveLength(2);

    w.statuses = [400];
    w.change((s) => s.splice(1, 1));
    await afterDelay();
    expect(writes(w)).toHaveLength(3);
    expect(w.warnings.at(-1)).toContain("HTTP 400");
    w.change(() => {});
    await afterDelay();
    expect(writes(w)).toHaveLength(3);
  });

  it("sends one write at a time and the change made during it afterwards", async () => {
    const w = await restored();
    let release!: () => void;
    w.gate = new Promise<void>((resolve) => { release = resolve; });
    w.change((s) => s.splice(1, 1));
    await afterDelay();
    w.change((s) => s.splice(1, 1));
    await afterDelay();
    expect(writes(w)).toHaveLength(1);

    w.gate = undefined;
    release();
    await settle();
    expect(writes(w)).toHaveLength(2);
    expect(lastWrite(w)).toEqual([]);
  });

  it("writes at once, outliving the page, when the page goes", async () => {
    const w = await restored();
    w.change((s) => s.splice(1, 1));
    w.remembered.flush(true);
    await settle();
    expect(writes(w)).toHaveLength(1);
    expect(writes(w)[0].keepalive).toBe(true);
    await afterDelay();
    expect(writes(w)).toHaveLength(1);
  });
});
