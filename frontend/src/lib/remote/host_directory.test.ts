import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostDirectory, type HostCatalog } from "./host_directory.ts";
import type { HostScreen } from "./remote_socket.ts";

const PATH = "/api/remote/catalog";
const HOSTS = "/api/remote/hosts";
const DELAY = 400;
const NOW = 1_800_000_000;
const screen = (id = "s1", name = "API", panes = 1): HostScreen => ({ id, name, panes });
const entry = (target = "vm1", lastSeen = 100, screens = [screen()]): HostCatalog => ({ target, lastSeen, screens });
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { "Content-Type": "application/json" },
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

interface Update { target: string; screens: HostScreen[] }

class Hub {
  catalog: unknown = { version: 1, hosts: [] };
  known: unknown = [];
  readCatalog = async () => json(this.catalog);
  readHosts = async () => json(this.known);
  write = async (body: Update, _init: RequestInit): Promise<Response> => json({ ...body, lastSeen: NOW + 10 });
  updates: { body: Update; init: RequestInit }[] = [];
  fetch = vi.fn(async (path: string, init?: RequestInit): Promise<Response> => {
    if (path === HOSTS && !init?.method) return this.readHosts();
    expect(path).toBe(PATH);
    if (!init?.method) return this.readCatalog();
    expect(init.method).toBe("PUT");
    expect(init.headers).toEqual({ "Content-Type": "application/json" });
    const body: Update = JSON.parse(String(init.body));
    expect(Object.keys(body).sort()).toEqual(["screens", "target"]);
    this.updates.push({ body, init });
    return this.write(body, init);
  });
}

const instances: HostDirectory[] = [];
function setup() {
  const hub = new Hub();
  const changed = vi.fn();
  const directory = new HostDirectory({ fetch: hub.fetch, changed });
  instances.push(directory);
  return { hub, directory, changed };
}
const settle = () => vi.advanceTimersByTimeAsync(0);
const elapsed = () => vi.advanceTimersByTimeAsync(DELAY);

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW * 1000);
});
afterEach(() => {
  for (const directory of instances.splice(0)) directory.dispose();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("HostDirectory loading", () => {
  it("reads both local metadata APIs once, without writes or sockets", async () => {
    const socket = vi.fn(() => { throw new Error("a directory must never connect"); });
    vi.stubGlobal("WebSocket", socket);
    const { hub, directory } = setup();
    hub.catalog = { version: 1, hosts: [entry()] };
    hub.known = [
      { target: "vm2", source: "recent", lastConnected: 20 },
      { target: "vm1", source: "recent", lastConnected: 10 },
      { target: "alias", source: "ssh-config" },
    ];
    expect(hub.fetch).not.toHaveBeenCalled();
    await Promise.all([directory.load(), directory.load()]);
    await directory.load();
    directory.flush(true);
    await elapsed();
    expect(hub.fetch.mock.calls.map(([path]) => path)).toEqual([PATH, HOSTS]);
    expect(directory.entries()).toEqual([entry()]);
    expect(directory.known()).toEqual(hub.known);
    expect(hub.updates).toEqual([]);
    expect(socket).not.toHaveBeenCalled();
  });

  it("shows successful reads independently without waiting for the other API", async () => {
    const { hub, directory, changed } = setup();
    const read = deferred<Response>();
    hub.readCatalog = () => read.promise;
    hub.known = [{ target: "vm2", source: "recent", lastConnected: 10 }];
    const loading = directory.load();
    await settle();
    expect(directory.known()).toEqual(hub.known);
    expect(changed).toHaveBeenCalledOnce();
    read.resolve(json({ version: 1, hosts: [entry()] }));
    await loading;
    expect(directory.entries()).toEqual([entry()]);
  });

  it.each(["http", "network", "json"])("keeps recent hosts and observations after a %s catalog failure", async (failure) => {
    const { hub, directory } = setup();
    hub.known = [{ target: "vm2", source: "recent", lastConnected: 10 }];
    hub.readCatalog = async () => {
      if (failure === "network") throw new Error("offline");
      return failure === "http" ? json({}, 500) : new Response("bad json");
    };
    directory.observe("vm1", [screen()]);
    await directory.load();
    expect(directory.entries()).toEqual([entry("vm1", NOW)]);
    expect(directory.known()).toEqual(hub.known);
    expect(hub.updates).toHaveLength(0);
    // Unlike the whole open-screen list, a catalog write changes only its
    // observed host: it is safe even when the initial read was unavailable.
    await elapsed();
    expect(hub.updates.map((u) => u.body)).toEqual([{ target: "vm1", screens: [screen()] }]);
  });

  it("keeps the catalog when the known-host read fails", async () => {
    const { hub, directory } = setup();
    hub.catalog = { version: 1, hosts: [entry()] };
    hub.readHosts = async () => { throw new Error("offline"); };
    await directory.load();
    expect(directory.entries()).toEqual([entry()]);
    expect(directory.known()).toEqual([]);
    await elapsed();
    expect(hub.updates).toEqual([]);
  });

  it.each([null, [], { version: 2, hosts: [entry()] }, { version: 1, hosts: null }])(
    "ignores an invalid catalog document (%j) without writing", async (value) => {
      const { hub, directory } = setup();
      hub.catalog = value;
      await directory.load();
      directory.flush();
      expect(directory.entries()).toEqual([]);
      expect(hub.updates).toEqual([]);
    },
  );

  it("discards invalid records, deduplicates ids per host, and treats names as plain text", async () => {
    const { hub, directory } = setup();
    hub.catalog = { version: 1, hosts: [
      null, "bad", entry("-oProxyCommand=sh"), entry("vm1 "), entry("vm 1"), entry("vm\u202e1"),
      entry("vm1", -1), entry("vm1", 1.5), { ...entry(), screens: {} },
      { ...entry(), screens: [
        null, {}, screen(""), screen("x".repeat(257)), screen("\ud800"),
        screen("negative", "bad", -1), screen("fraction", "bad", 1.5), screen("large", "bad", 1e20),
        { id: "bad-name", name: {}, panes: 1 },
        screen("s1", "<b>API</b>\n\u0000\u202e!", 2), screen("s1", "duplicate", 99),
        { id: "s2", panes: 0 },
      ] },
      entry("vm2", 200, [screen("s1", "same id, different host")]),
    ] };
    await directory.load();
    expect(directory.entries()).toEqual([
      entry("vm2", 200, [screen("s1", "same id, different host")]),
      entry("vm1", 100, [screen("s1", "<b>API</b>!", 2), screen("s2", "", 0)]),
    ]);
    expect(hub.updates).toEqual([]);
  });

  it("keeps the newest 20 hosts and 200 screens, with bounded Unicode ids and names", async () => {
    const { hub, directory } = setup();
    const hosts = Array.from({ length: 25 }, (_, i) => entry(`vm${i}`, i));
    hosts.push(entry("vm24", 50, [
      screen("😀".repeat(256), `${"a".repeat(199)}😀`),
      screen("😀".repeat(257)),
      ...Array.from({ length: 205 }, (_, i) => screen(`s${i}`, "n".repeat(250))),
    ]));
    hub.catalog = { version: 1, hosts: hosts.reverse() };
    await directory.load();
    const entries = directory.entries();
    expect(entries).toHaveLength(20);
    expect(entries.at(-1)?.target).toBe("vm5");
    expect(entries[0].lastSeen).toBe(50);
    expect(entries[0].screens).toHaveLength(200);
    expect(entries[0].screens[0]).toEqual(screen("😀".repeat(256), "a".repeat(199)));
    expect(entries[0].screens[1].name).toBe("n".repeat(200));
  });

  it("preserves recent hosts outside the catalog and keeps recent over a duplicate config alias", async () => {
    const { hub, directory } = setup();
    hub.known = [
      null, { target: "vm1", source: "ssh-config" },
      { target: "vm1", source: "recent", lastConnected: 123 },
      { target: "vm1", source: "ssh-config" }, { target: "vm2", source: "recent", lastConnected: -1 },
      { target: "alias", source: "ssh-config" }, { target: " vm3", source: "recent" },
      { target: "-bad", source: "recent" }, { target: "vm4", source: "unknown" },
    ];
    await directory.load();
    expect(directory.entries()).toEqual([]);
    expect(directory.known()).toEqual([
      { target: "vm1", source: "recent", lastConnected: 123 },
      { target: "vm2", source: "recent" }, { target: "alias", source: "ssh-config" },
    ]);
  });

  it("preserves the hub's recency order when host timestamps share the same second", async () => {
    const { hub, directory } = setup();
    const hosts = Array.from({ length: 25 }, (_, i) => entry(`vm${i}`, 100));
    hub.catalog = { version: 1, hosts };
    await directory.load();
    expect(directory.entries()).toEqual(hosts.slice(0, 20));
  });

  it("does not expose mutable catalog or known-host records", async () => {
    const { hub, directory } = setup();
    hub.catalog = { version: 1, hosts: [entry()] };
    hub.known = [{ target: "vm1", source: "recent" }];
    await directory.load();
    directory.entries()[0].screens[0].name = "changed";
    directory.entries()[0].target = "other";
    directory.known()[0].target = "other";
    expect(directory.entries()).toEqual([entry()]);
    expect(directory.known()).toEqual(hub.known);
  });

  it("accepts canonical SSH targets and refuses malformed targets on both reads and observations", async () => {
    const { hub, directory } = setup();
    const valid = ["vm1", "dev@api.example:2222", "[::1]", "user@[2001:db8::1]:22", "[::ffff:127.0.0.1]"];
    const invalid = ["vm1;id", "a@@vm1", "@vm1", "vm1:", "vm1:0", "vm1:65536", "vm1:22:33", "::1",
      "[::1]trailing", "[::1]:0", "[bad]", "[127.0.0.1]", "host..name", "host-", "a".repeat(64), "vm\ud800"];
    hub.catalog = { version: 1, hosts: [...valid, ...invalid].map((target) => entry(target)) };
    await directory.load();
    expect(directory.entries().map((e) => e.target)).toEqual(valid);
    for (const target of invalid) directory.observe(target, []);
    directory.flush();
    await elapsed();
    expect(hub.updates).toEqual([]);
    for (const target of valid) directory.observe(target, []);
    await elapsed();
    expect(hub.updates.map((u) => u.body.target)).toEqual(valid);
  });
});

describe("HostDirectory observations and writes", () => {
  it("updates immediately, coalesces changes, and uses the server's epoch-second timestamp", async () => {
    const { hub, directory, changed } = setup();
    await directory.load();
    changed.mockClear();
    const screens = [screen()];
    directory.observe("vm1", screens);
    screens[0].name = "caller mutation";
    expect(directory.entries()).toEqual([entry("vm1", NOW)]);
    expect(changed).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(DELAY / 2);
    directory.observe("vm1", [screen("s1", "Renamed", 3)]);
    await vi.advanceTimersByTimeAsync(DELAY - 1);
    expect(hub.updates).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(hub.updates.map((u) => u.body)).toEqual([{ target: "vm1", screens: [screen("s1", "Renamed", 3)] }]);
    expect(directory.entries()).toEqual([entry("vm1", NOW + 10, [screen("s1", "Renamed", 3)])]);
    directory.observe("vm1", [screen("s1", "Renamed", 3)]);
    directory.flush();
    await elapsed();
    expect(hub.updates).toHaveLength(1);
  });

  it("persists an observation even if it matches the loaded directory", async () => {
    const { hub, directory } = setup();
    hub.catalog = { version: 1, hosts: [entry()] };
    await directory.load();
    directory.observe("vm1", [screen()]);
    await elapsed();
    expect(hub.updates).toHaveLength(1);
    expect(directory.entries()[0].lastSeen).toBe(NOW + 10);
  });

  it("refreshes last seen when revisiting a host whose screens have not changed", async () => {
    const { hub, directory } = setup();
    hub.write = async (body) => json({ ...body, lastSeen: Math.floor(Date.now() / 1000) });
    directory.observe("vm1", [screen()]);
    await elapsed();
    await vi.advanceTimersByTimeAsync(60_000);
    directory.observe("vm1", [screen()]);
    await elapsed();
    expect(hub.updates).toHaveLength(2);
    expect(directory.entries()).toEqual([entry("vm1", NOW + 60)]);
  });

  it("does not roll back a newer observation time through an old identical write response", async () => {
    const { hub, directory } = setup();
    const first = deferred<Response>();
    hub.write = async (body) => hub.updates.length === 1 ? first.promise : json({ ...body, lastSeen: NOW + 60 });
    directory.observe("vm1", [screen()]);
    await elapsed();
    await vi.advanceTimersByTimeAsync(60_000);
    directory.observe("vm1", [screen()]);
    await elapsed();
    first.resolve(json(entry("vm1", NOW)));
    await settle();
    expect(hub.updates).toHaveLength(2);
    expect(directory.entries()).toEqual([entry("vm1", NOW + 60)]);
  });

  it("replaces a nonempty directory with an empty one and leaves other hosts intact", async () => {
    const { hub, directory } = setup();
    hub.catalog = { version: 1, hosts: [entry(), entry("vm2")] };
    await directory.load();
    directory.observe("vm1", []);
    expect(directory.entries()).toEqual([entry("vm1", NOW, []), entry("vm2")]);
    await elapsed();
    expect(hub.updates.map((u) => u.body)).toEqual([{ target: "vm1", screens: [] }]);
    expect(directory.entries().find((e) => e.target === "vm2")).toEqual(entry("vm2"));
  });

  it("does not let a late read replace observations made before or during loading", async () => {
    const { hub, directory } = setup();
    const read = deferred<Response>();
    hub.readCatalog = () => read.promise;
    directory.observe("vm1", [screen("before")]);
    const loading = directory.load();
    directory.observe("vm2", []);
    await elapsed();
    read.resolve(json({ version: 1, hosts: [entry(), entry("vm2"), entry("vm3")] }));
    await loading;
    expect(directory.entries().find((e) => e.target === "vm1")).toEqual(entry("vm1", NOW + 10, [screen("before")]));
    expect(directory.entries().find((e) => e.target === "vm2")).toEqual(entry("vm2", NOW + 10, []));
    expect(directory.entries().find((e) => e.target === "vm3")).toEqual(entry("vm3"));
    expect(hub.updates).toHaveLength(2);
  });

  it("serializes each host, coalesces pending writes and never applies a stale write response", async () => {
    const { hub, directory } = setup();
    const first = deferred<Response>();
    const second = deferred<Response>();
    hub.write = () => hub.updates.length === 1 ? first.promise : second.promise;
    directory.observe("vm1", [screen("old")]);
    await elapsed();
    directory.observe("vm1", [screen("middle")]);
    await elapsed();
    directory.observe("vm1", []);
    await elapsed();
    expect(hub.updates).toHaveLength(1);
    first.resolve(json(entry("vm1", 900, [screen("old")])));
    await settle();
    expect(directory.entries()).toEqual([entry("vm1", NOW, [])]);
    expect(hub.updates.map((u) => u.body.screens)).toEqual([[screen("old")], []]);
    second.resolve(json(entry("vm1", 901, [])));
    await settle();
    expect(directory.entries()).toEqual([entry("vm1", 901, [])]);
  });

  it("does not block another host's write behind a slow one, even with the same screen id", async () => {
    const { hub, directory } = setup();
    const held = deferred<Response>();
    hub.write = async (body) => body.target === "vm1" ? held.promise : json({ ...body, lastSeen: 200 });
    directory.observe("vm1", [screen("s1", "First")]);
    directory.observe("vm2", [screen("s1", "Second")]);
    await elapsed();
    expect(hub.updates).toHaveLength(2);
    expect(directory.entries().find((e) => e.target === "vm2")).toEqual(entry("vm2", 200, [screen("s1", "Second")]));
    held.resolve(json(entry("vm1", 100, [screen("s1", "First")])));
    await settle();
    expect(directory.entries().find((e) => e.target === "vm1")).toEqual(entry("vm1", 100, [screen("s1", "First")]));
  });

  it("never retries an unchanged 400, but allows a newer observation", async () => {
    const { hub, directory } = setup();
    hub.write = async () => json({ error: "rejected" }, 400);
    directory.observe("vm1", [screen()]);
    await elapsed();
    for (let i = 0; i < 3; i++) {
      directory.observe("vm1", [screen()]);
      directory.flush(true);
      await elapsed();
    }
    expect(hub.updates).toHaveLength(1);
    directory.observe("vm1", []);
    await elapsed();
    expect(hub.updates).toHaveLength(2);
  });

  it.each(["http", "network", "json"])("retries a transient %s write failure only on a new trigger", async (failure) => {
    const { hub, directory } = setup();
    hub.write = async () => {
      if (failure === "network") throw new Error("offline");
      return failure === "http" ? json({}, 503) : new Response("bad json");
    };
    directory.observe("vm1", [screen()]);
    await elapsed();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(hub.updates).toHaveLength(1);
    expect(directory.entries()[0].screens).toEqual([screen()]);
    directory.observe("vm1", [screen()]);
    await elapsed();
    expect(hub.updates).toHaveLength(2);
  });

  it("still sends a changed observation after an in-flight failure", async () => {
    const { hub, directory } = setup();
    const first = deferred<Response>();
    hub.write = async (body) => hub.updates.length === 1 ? first.promise : json({ ...body, lastSeen: 300 });
    directory.observe("vm1", [screen()]);
    await elapsed();
    directory.observe("vm1", []);
    await elapsed();
    first.resolve(json({}, 400));
    await settle();
    expect(hub.updates).toHaveLength(2);
    expect(directory.entries()).toEqual([entry("vm1", 300, [])]);
  });

  it("rewrites the previous value after a different request loses its response", async () => {
    const { hub, directory } = setup();
    directory.observe("vm1", [screen()]);
    await elapsed();
    const held = deferred<Response>();
    hub.write = async (body) => hub.updates.length === 2 ? held.promise : json({ ...body, lastSeen: 300 });
    directory.observe("vm1", []);
    await elapsed();
    directory.observe("vm1", [screen()]);
    await elapsed();
    held.reject(new Error("response lost after commit"));
    await settle();
    expect(hub.updates.map((u) => u.body.screens)).toEqual([[screen()], [], [screen()]]);
    expect(directory.entries()).toEqual([entry("vm1", 300)]);
  });

  it.each([entry("vm2"), entry("vm1", 42, [screen("other")]), { ...entry(), lastSeen: "bad" }])(
    "does not accept mismatched or malformed write replies (%j)", async (reply) => {
      const { hub, directory } = setup();
      hub.write = async () => json(reply);
      directory.observe("vm1", [screen()]);
      await elapsed();
      expect(directory.entries()).toEqual([entry("vm1", NOW)]);
      directory.flush();
      await settle();
      expect(hub.updates).toHaveLength(2);
    },
  );

  it("flushes on pagehide with keepalive and cancels the debounce", async () => {
    const { hub, directory } = setup();
    directory.observe("vm1", [screen()]);
    directory.flush(true);
    expect(hub.updates).toHaveLength(1);
    expect(hub.updates[0].init.keepalive).toBe(true);
    await elapsed();
    expect(hub.updates).toHaveLength(1);
  });

  it("keeps pagehide follow-ups serialized and carries keepalive to the newest pending write", async () => {
    const { hub, directory } = setup();
    const first = deferred<Response>();
    hub.write = async (body) => hub.updates.length === 1 ? first.promise : json({ ...body, lastSeen: 300 });
    directory.observe("vm1", [screen()]);
    directory.flush();
    directory.observe("vm1", []);
    directory.flush(true);
    expect(hub.updates).toHaveLength(1);
    first.resolve(json(entry()));
    await settle();
    expect(hub.updates).toHaveLength(2);
    expect(hub.updates[1].body.screens).toEqual([]);
    expect(hub.updates[1].init.keepalive).toBe(true);
  });

  it("bounds observations and outgoing bodies to the catalog limits", async () => {
    const { hub, directory } = setup();
    const screens = [screen(""), screen("x".repeat(257)), screen("good", "\u202eName\n"), screen("good")];
    screens.push(...Array.from({ length: 210 }, (_, i) => screen(`s${i}`, "n".repeat(210))));
    for (let i = 0; i < 25; i++) directory.observe(`vm${i}`, screens);
    directory.observe(" vm1", [screen()]);
    directory.observe("-bad", [screen()]);
    directory.observe("x".repeat(256), [screen()]);
    expect(directory.entries()).toHaveLength(20);
    expect(directory.entries()[0].target).toBe("vm24");
    await elapsed();
    expect(hub.updates).toHaveLength(20);
    for (const { body } of hub.updates) {
      expect(body.screens).toHaveLength(200);
      expect(body.screens[0]).toEqual(screen("good", "Name"));
      expect(body.screens[1].name).toHaveLength(200);
    }
  });

  it("does not resurrect evicted observations through a late read", async () => {
    const { hub, directory } = setup();
    const read = deferred<Response>();
    hub.readCatalog = () => read.promise;
    const loading = directory.load();
    for (let i = 0; i < 21; i++) directory.observe(`vm${i}`, []);
    // Even an unusually new saved timestamp must not restore an old snapshot.
    read.resolve(json({ version: 1, hosts: [entry("vm0", NOW + 99)] }));
    await loading;
    expect(directory.entries()).toHaveLength(20);
    expect(directory.entries().some((e) => e.target === "vm0")).toBe(false);
  });

  it("serializes a reobserved host behind its evicted in-flight writer", async () => {
    const { hub, directory } = setup();
    const first = deferred<Response>();
    hub.write = async (body) => hub.updates.length === 1 ? first.promise : json({ ...body, lastSeen: NOW });
    directory.observe("vm0", [screen()]);
    directory.flush();
    for (let i = 1; i < 21; i++) directory.observe(`vm${i}`, []);
    directory.observe("vm0", []);
    directory.flush();
    expect(hub.updates.filter((u) => u.body.target === "vm0")).toHaveLength(1);
    first.resolve(json(entry("vm0", NOW - 1)));
    await settle();
    expect(hub.updates.filter((u) => u.body.target === "vm0").map((u) => u.body.screens)).toEqual([[screen()], []]);
    expect(directory.entries().find((e) => e.target === "vm0")?.screens).toEqual([]);
    expect(directory.entries()).toHaveLength(20);
  });
});

describe("HostDirectory disposal", () => {
  it("drains an explicitly requested keepalive follow-up even when the view is disposed", async () => {
    const { hub, directory, changed } = setup();
    const first = deferred<Response>();
    hub.write = async (body) => hub.updates.length === 1 ? first.promise : json({ ...body, lastSeen: 300 });
    directory.observe("vm1", [screen()]);
    directory.flush();
    directory.observe("vm1", []);
    directory.flush(true);
    directory.dispose();
    changed.mockClear();
    first.resolve(json(entry()));
    await settle();
    expect(hub.updates).toHaveLength(2);
    expect(hub.updates[1].body.screens).toEqual([]);
    expect(hub.updates[1].init.keepalive).toBe(true);
    expect(changed).not.toHaveBeenCalled();
  });

  it("cancels queued writes, ignores new work and leaves state unchanged after late reads", async () => {
    const { hub, directory, changed } = setup();
    const read = deferred<Response>();
    hub.readCatalog = () => read.promise;
    hub.readHosts = () => read.promise;
    const loading = directory.load();
    directory.observe("vm1", [screen()]);
    directory.dispose();
    changed.mockClear();
    read.resolve(json({ version: 1, hosts: [entry("vm2")] }));
    await loading;
    directory.observe("vm3", []);
    directory.flush(true);
    await directory.load();
    await elapsed();
    expect(directory.entries()).toEqual([entry("vm1", NOW)]);
    expect(hub.updates).toEqual([]);
    expect(changed).not.toHaveBeenCalled();
  });

  it("does not publish a late write or start its queued successor after disposal", async () => {
    const { hub, directory, changed } = setup();
    const held = deferred<Response>();
    hub.write = () => held.promise;
    directory.observe("vm1", [screen()]);
    directory.flush();
    directory.observe("vm1", []);
    directory.flush();
    directory.dispose();
    changed.mockClear();
    held.resolve(json(entry()));
    await elapsed();
    expect(directory.entries()[0].screens).toEqual([]);
    expect(hub.updates).toHaveLength(1);
    expect(changed).not.toHaveBeenCalled();
  });
});

describe("forgotten hosts", () => {
  it("cancels queued observations and prevents late reads and writes from resurrecting a host", async () => {
    const { hub, directory } = setup();
    const initial = deferred<Response>();
    hub.readCatalog = () => initial.promise;
    hub.known = [{ target: "vm1", source: "recent" }];
    const loading = directory.load();
    directory.observe("vm1", [screen()]);
    directory.setHidden("vm1", true);
    initial.resolve(json({ version: 1, hosts: [entry()] }));
    await loading;
    directory.observe("vm1", [screen()]);
    await elapsed();
    expect(directory.entries()).toEqual([]);
    expect(directory.known()).toEqual([]);
    expect(hub.updates).toEqual([]);
    directory.setHidden("vm1", false);
    directory.observe("vm1", [screen()]);
    await elapsed();
    expect(hub.updates).toHaveLength(1);
  });

  it("ignores a catalog write finishing after removal", async () => {
    const { hub, directory } = setup();
    const pending = deferred<Response>();
    hub.write = () => pending.promise;
    directory.observe("vm1", [screen()]);
    await elapsed();
    directory.setHidden("vm1", true);
    pending.resolve(json(entry("vm1", NOW + 10)));
    await settle();
    expect(directory.entries()).toEqual([]);
  });
});

it("does not let an old catalog response replace an explicitly re-added host", async () => {
  const { hub, directory } = setup();
  const pending = deferred<Response>();
  hub.write = () => pending.promise;
  directory.observe("vm1", [screen("old")]);
  await elapsed();
  directory.setHidden("vm1", true);
  directory.setHidden("vm1", false);
  directory.observe("vm1", [screen("new")]);
  pending.resolve(json(entry("vm1", NOW + 10, [screen("old")])));
  await settle();
  expect(directory.entries()[0].screens[0].id).toBe("new");
});
