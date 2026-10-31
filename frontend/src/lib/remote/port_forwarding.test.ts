import { afterEach, describe, expect, it, vi } from "vitest";
import {
  FORWARDS_PATH, PREFERENCES_PATH, PortForwarding, REFRESH_INTERVAL, type PortForwardingDeps, type PortNotice,
} from "./port_forwarding.ts";
import { fakeServer, type FakeSocket } from "../testing/fake_server.ts";
import { hostKey, type ServerConnection } from "../connection/server_connection.ts";

import type { SavedForward } from "./ports.ts";
import { manualPort } from "./manual_port.ts";

interface WireForward { target: string; remote: number; local: number; url: string; state: "up" | "down" }

type RequestBody = { target: string; port: number; local?: number; automatic?: boolean; label?: string };

/** Both hub APIs. Deferred fetch wrappers below delay responses, not real services. */
class FakeHub {
  forwards: WireForward[] = [];
  saved: SavedForward[] = [];
  requests: { path: string; method: string; body?: RequestBody }[] = [];
  fail: { status: number; body?: unknown; path?: string } | undefined;
  localFor = (port: number) => port;
  gate: Promise<void> | undefined;

  fetch = async (path: string, init?: RequestInit): Promise<Response> => {
    expect([FORWARDS_PATH, PREFERENCES_PATH]).toContain(path);
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    this.requests.push({ path, method, body });
    if (method === "GET") {
      const list = structuredClone(path === FORWARDS_PATH ? this.forwards : { version: 1, forwards: this.saved });
      if (this.gate) await this.gate;
      return json(200, list);
    }
    if (this.fail && (!this.fail.path || this.fail.path === path)) {
      const { status, body: failure } = this.fail;
      this.fail = undefined;
      return json(status, failure);
    }
    const index = this.forwards.findIndex((f) => f.target === body.target && f.remote === body.port);
    const savedIndex = this.saved.findIndex((f) => f.target === body.target && f.remote === body.port);
    const saved = this.saved[savedIndex];
    const active = this.forwards[index];
    if (path === PREFERENCES_PATH) {
      if (method === "DELETE") {
        this.saved = this.saved.filter((f) => f !== saved);
        return new Response(null, { status: 204 });
      }
      if (body.local !== undefined && active && body.local !== active.local) {
        return json(409, { code: "active_mapping_conflict", error: "Stop before changing the active mapping." });
      }
      const preference: SavedForward = { target: body.target, remote: body.port,
        local: body.local ?? saved?.local ?? active?.local ?? body.port,
        label: body.label ?? saved?.label ?? "", enabled: saved?.enabled ?? true,
        ...(saved?.lastError ? { lastError: saved.lastError } : {}),
      };
      if (savedIndex >= 0) this.saved[savedIndex] = preference; else this.saved.push(preference);
      return json(200, preference);
    }
    if (method === "POST") {
      if (body.automatic && saved?.enabled === false) return json(409, { code: "forward_paused", error: "Paused" });
      if (active && body.local !== undefined && body.local !== active.local) {
        return json(409, { code: "active_mapping_conflict", error: "Stop before changing the active mapping." });
      }
      const local = body.local ?? saved?.local ?? this.localFor(body.port);
      const created: WireForward = active ?? { target: body.target, remote: body.port, local, url: `http://localhost:${local}`, state: "up" };
      if (!active) this.forwards.push(created);
      if (saved) { saved.local = created.local; saved.enabled = true; delete saved.lastError; }
      return json(200, created);
    }
    if (saved) { saved.enabled = false; delete saved.lastError; }
    if (index < 0 && !saved) return json(404, { error: "There is no forward." });
    if (index >= 0) this.forwards.splice(index, 1);
    return new Response(null, { status: 204 });
  };

  of(method: string, path = FORWARDS_PATH) {
    return this.requests.filter((r) => r.method === method && r.path === path).map(({ method, body }) => ({ method, body }));
  }
}

function json(status: number, body: unknown): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

/** Let pending promises (fetches, their answers) run. */
async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await new Promise<void>((resolve) => queueMicrotask(resolve));
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

let instances: PortForwarding[] = [];

function setup(options: { auto?: boolean; openWorks?: boolean; hub?: FakeHub } = {}) {
  const hub = options.hub ?? new FakeHub();
  const opened: string[] = [];
  const notices: PortNotice[] = [];
  let auto = options.auto ?? false;
  const deps: PortForwardingDeps = {
    fetch: hub.fetch,
    open: (url) => {
      opened.push(url);
      return options.openWorks ?? true;
    },
    autoForward: () => auto,
    notify: (n) => notices.push(n),
    changed: vi.fn(),
  };
  const ports = new PortForwarding(deps);
  instances.push(ports);
  return { hub, opened, notices, ports, deps, setAuto: (on: boolean) => { auto = on; } };
}

function host(target: string): { socket: FakeSocket; conn: ServerConnection } {
  return fakeServer(hostKey(target), target);
}

const list = (socket: FakeSocket, ...entries: [number, string][]) =>
  socket.message({ type: "ports.changed", ports: entries.map(([port, process]) => ({ port, sessionId: "s1", process })) });

afterEach(() => {
  for (const instance of instances) instance.dispose();
  instances = [];
  vi.useRealTimers();
});

describe("PortForwarding: the hosts' lists", () => {
  it("takes a host's list only from that host's connection", () => {
    const { ports } = setup();
    const vm1 = host("vm1");
    const vm2 = host("vm2");
    const local = fakeServer("local");
    ports.watch(vm1.conn);
    ports.watch(vm2.conn);
    // The local server's ports need no forward; its connection is not watched.
    ports.watch(local.conn);

    list(vm1.socket, [8000, "python3"]);
    list(local.socket, [5173, "node"]);
    expect(ports.ports(vm1.conn.key)).toEqual([{ port: 8000, sessionId: "s1", process: "python3" }]);
    expect(ports.ports(vm2.conn.key)).toBeUndefined();
    expect(ports.ports("local")).toBeUndefined();
    expect(ports.lists()).toEqual([{ target: "vm1", rows: [{ port: 8000, process: "python3" }] }]);

    list(vm2.socket, [8000, "ruby"]);
    expect(ports.lists().map((h) => [h.target, h.rows[0].process])).toEqual([["vm1", "python3"], ["vm2", "ruby"]]);
  });

  it("ignores malformed lists and drops a host's list when it goes or drops", () => {
    const { ports } = setup();
    const vm1 = host("vm1");
    const unwatch = ports.watch(vm1.conn);
    list(vm1.socket, [3000, "node"]);
    vm1.socket.message({ type: "ports.changed", ports: "nonsense" });
    expect(ports.ports(vm1.conn.key)).toHaveLength(1);

    vm1.socket.drop();
    expect(ports.ports(vm1.conn.key)).toBeUndefined();
    list(vm1.socket, [3000, "node"]);
    unwatch();
    expect(ports.lists()).toEqual([]);
    list(vm1.socket, [4000, "node"]);
    expect(ports.lists()).toEqual([]);
  });

  it("leaves the connection's other handlers alone: they ignore the type", () => {
    const { ports } = setup();
    const vm1 = host("vm1");
    ports.watch(vm1.conn);
    const commands: unknown[] = [];
    vm1.conn.onCommand((c) => commands.push(c));
    list(vm1.socket, [3000, "node"]);
    vm1.socket.message({ type: "something.new", ports: [{ port: 1 }] });
    vm1.socket.message({ type: "command", command: { type: "pane.clear" } });
    expect(commands).toEqual([{ type: "pane.clear" }]);
    expect(ports.ports(vm1.conn.key)).toHaveLength(1);
  });

  it("joins the local server's forwards, read on start", async () => {
    const { hub, ports } = setup();
    hub.forwards = [
      { target: "vm1", remote: 8000, local: 8001, url: "http://localhost:8001", state: "down" },
      { target: "old", remote: 4000, local: 4000, url: "http://localhost:4000", state: "up" },
    ];
    const vm1 = host("vm1");
    ports.watch(vm1.conn);
    list(vm1.socket, [3000, "node"], [8000, "python3"]);
    await ports.refresh();
    expect(ports.lists()).toEqual([
      { target: "vm1", rows: [
        { port: 3000, process: "node" },
        { port: 8000, process: "python3", forward: { target: "vm1", remote: 8000, local: 8001, url: "http://localhost:8001", state: "down" } },
      ] },
      { target: "old", rows: [{ port: 4000, forward: { target: "old", remote: 4000, local: 4000, url: "http://localhost:4000", state: "up" } }] },
    ]);
  });
});

describe("PortForwarding: notices", () => {
  it("lists a port that appears without a notice and forwards nothing", async () => {
    const { hub, notices, ports } = setup();
    const vm1 = host("vm1");
    ports.watch(vm1.conn);
    list(vm1.socket, [3000, "node"]);
    list(vm1.socket, [3000, "node"], [8000, "python3"]);
    await settle();
    expect(notices).toEqual([]);
    expect(ports.lists()[0].rows.map((row) => [row.port, row.process])).toEqual([[3000, "node"], [8000, "python3"]]);
    expect(hub.of("POST")).toEqual([]);
  });

  it("offers the forward in a notice when the browser opened nothing", async () => {
    const { notices, opened, ports } = setup({ openWorks: false });
    await ports.forward("vm1", 8000, { open: true });
    expect(opened).toEqual(["http://localhost:8000"]);
    expect(notices).toHaveLength(1);
    expect(notices[0]).toMatchObject({ kind: "success", title: "vm1: port 8000 is forwarded", description: "http://localhost:8000" });
    notices[0].action!.run();
    await settle();
    expect(opened).toEqual(["http://localhost:8000", "http://localhost:8000"]);
  });
});

describe("PortForwarding: automatic forwarding", () => {
  it("forwards nothing without a click while the setting is off", async () => {
    const { hub, ports } = setup({ auto: false });
    const vm1 = host("vm1");
    ports.watch(vm1.conn);
    list(vm1.socket);
    list(vm1.socket, [8000, "python3"]);
    await settle();
    expect(hub.of("POST")).toEqual([]);
  });

  it("forwards a new port at once with the setting on, without a notice", async () => {
    const { hub, notices, opened, ports } = setup({ auto: true });
    const vm1 = host("vm1");
    ports.watch(vm1.conn);
    // The first list is not forwarded.
    list(vm1.socket, [3000, "node"]);
    await settle();
    expect(hub.of("POST")).toEqual([]);

    list(vm1.socket, [3000, "node"], [8000, "python3"]);
    await settle();
    expect(hub.of("POST")).toEqual([{ method: "POST", body: { target: "vm1", port: 8000, automatic: true } }]);
    expect(opened).toEqual([]);
    expect(notices).toEqual([]);
    expect(ports.lists()[0].rows[1].forward?.url).toBe("http://localhost:8000");
  });

  it("does not forward what was listening before a reconnect", async () => {
    const { hub, ports } = setup({ auto: true });
    const vm1 = host("vm1");
    ports.watch(vm1.conn);
    list(vm1.socket, [3000, "node"]);
    vm1.socket.drop();
    list(vm1.socket, [3000, "node"], [8000, "python3"]);
    await settle();
    expect(hub.of("POST")).toEqual([]);
    list(vm1.socket, [3000, "node"], [8000, "python3"], [9000, "ruby"]);
    await settle();
    expect(hub.of("POST").map((r) => r.body?.port)).toEqual([9000]);
  });

  it("forwards every new port, once per process", async () => {
    const { hub, ports } = setup({ auto: true });
    const vm1 = host("vm1");
    ports.watch(vm1.conn);
    list(vm1.socket);
    list(vm1.socket, [3000, "a"], [3001, "b"], [3002, "c"], [3003, "d"]);
    await settle();
    expect(hub.of("POST").map((r) => r.body?.port)).toEqual([3000, 3001, 3002, 3003]);

    // A forward the user stopped is not forwarded again for the same process.
    await ports.stop("vm1", 3000);
    list(vm1.socket);
    list(vm1.socket, [3000, "a"]);
    await settle();
    expect(hub.of("POST")).toHaveLength(4);
    list(vm1.socket);
    list(vm1.socket, [3000, "vite"]);
    await settle();
    expect(hub.of("POST")).toHaveLength(5);
  });
});

describe("PortForwarding: forward, open, stop", () => {
  it("sends one request for a port however often it is asked", async () => {
    const { hub, ports } = setup();
    const [a, b] = await Promise.all([ports.forward("vm1", 8000), ports.forward("vm1", 8000)]);
    expect(a).toEqual(b);
    expect(hub.of("POST")).toHaveLength(1);
    await ports.forward("vm1", 8000);
    expect(hub.of("POST")).toHaveLength(2);
  });

  it("shows the local server's error and hint as plain text", async () => {
    const { hub, notices, opened, ports } = setup();
    hub.fail = { status: 502, body: { error: "ssh: connect\u001b[0m failed", code: "unreachable", hint: "Check ‮the host" } };
    expect(await ports.forward("vm1", 8000, { open: true })).toBeNull();
    expect(opened).toEqual([]);
    expect(notices).toEqual([{
      id: "forward:vm1\n8000", kind: "error", title: "Port 8000 of vm1 was not forwarded",
      description: "ssh: connect[0m failed\nCheck the host",
    }]);
    expect(ports.lists()).toEqual([]);
  });

  it("stops a forward, and takes one stopped elsewhere as gone", async () => {
    const { hub, notices, ports } = setup();
    await ports.forward("vm1", 8000);
    await ports.forward("vm1", 9000);
    await ports.stop("vm1", 8000);
    expect(hub.of("DELETE")).toEqual([{ method: "DELETE", body: { target: "vm1", port: 8000 } }]);
    expect(ports.forwards().map((f) => f.remote)).toEqual([9000]);

    hub.forwards = [];
    await ports.stop("vm1", 9000);
    expect(ports.forwards()).toEqual([]);
    expect(notices).toEqual([]);
  });

  it("keeps a forward whose stop failed and says why", async () => {
    const { hub, notices, ports } = setup();
    await ports.forward("vm1", 8000);
    hub.fail = { status: 500, body: { error: "it hangs" } };
    await ports.stop("vm1", 8000);
    expect(ports.forwards()).toHaveLength(1);
    expect(notices).toMatchObject([{ kind: "error", title: "The forward of port 8000 of vm1 was not stopped", description: "it hangs" }]);
  });

  it("drops a read that started before a forward or stop here", async () => {
    const { hub, ports } = setup();
    let release!: () => void;
    hub.gate = new Promise<void>((resolve) => { release = resolve; });
    const stale = ports.refresh();
    hub.gate = undefined;
    await ports.forward("vm1", 8000);
    await settle();
    release();
    await stale;
    expect(ports.forwards().map((f) => f.remote)).toEqual([8000]);

    hub.gate = new Promise<void>((resolve) => { release = resolve; });
    const staleAgain = ports.refresh();
    hub.gate = undefined;
    await ports.stop("vm1", 8000);
    release();
    await staleAgain;
    expect(ports.forwards()).toEqual([]);
  });

  it("reads the forwards every few seconds while there is one", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const { hub, ports } = setup();
    await ports.refresh();
    vi.advanceTimersByTime(REFRESH_INTERVAL * 2);
    expect(hub.of("GET")).toHaveLength(1);

    await ports.forward("vm1", 8000);
    await settle();
    const before = hub.of("GET").length;
    hub.forwards[0].state = "down";
    vi.advanceTimersByTime(REFRESH_INTERVAL);
    await settle();
    expect(hub.of("GET")).toHaveLength(before + 1);
    expect(ports.forwards()[0].state).toBe("down");

    await ports.stop("vm1", 8000);
    const after = hub.of("GET").length;
    vi.advanceTimersByTime(REFRESH_INTERVAL * 3);
    await settle();
    expect(hub.of("GET")).toHaveLength(after);
  });
});

describe("PortForwarding: links of panes", () => {
  it("opens a local pane's links as they are", async () => {
    const { hub, opened, ports } = setup();
    ports.openLink("local", "http://localhost:5173/app");
    ports.openLink("local", "http://127.0.0.1:8000/");
    await settle();
    expect(opened).toEqual(["http://localhost:5173/app", "http://127.0.0.1:8000/"]);
    expect(hub.of("POST")).toEqual([]);
  });

  it("opens a host's links to other machines as they are", async () => {
    const { hub, opened, ports } = setup();
    ports.openLink(hostKey("vm1"), "https://example.com:5173/docs");
    ports.openLink(hostKey("vm1"), "http://10.0.0.5:8080/");
    await settle();
    expect(opened).toEqual(["https://example.com:5173/docs", "http://10.0.0.5:8080/"]);
    expect(hub.of("POST")).toEqual([]);
  });

  it("forwards the port of a host's loopback link first, then opens the forward with the path", async () => {
    const { hub, opened, ports } = setup();
    hub.localFor = () => 40001;
    ports.openLink(hostKey("vm1"), "http://localhost:5173/app?x=1#y");
    await settle();
    expect(hub.of("POST")).toEqual([{ method: "POST", body: { target: "vm1", port: 5173 } }]);
    expect(opened).toEqual(["http://localhost:40001/app?x=1#y"]);
  });

  it("verifies a forwarded port before opening, for every loopback host", async () => {
    const { hub, opened, ports } = setup();
    hub.localFor = () => 8001;
    await ports.forward("vm1", 8000);
    const posts = hub.of("POST").length;
    for (const host of ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"]) {
      ports.openLink(hostKey("vm1"), `http://${host}:8000/x`);
      await settle();
    }
    // Each explicit click verifies current backend state before opening.
    expect(opened).toEqual(Array(4).fill("http://localhost:8001/x"));
    expect(hub.of("POST")).toHaveLength(posts + 4);
    // Another host's same port is not that forward.
    ports.openLink(hostKey("vm2"), "http://localhost:8000/x");
    await settle();
    expect(hub.of("POST").at(-1)).toEqual({ method: "POST", body: { target: "vm2", port: 8000 } });
  });

  it("opens nothing when the forward fails, and says why", async () => {
    const { hub, notices, opened, ports } = setup();
    hub.fail = { status: 409, body: { error: "vm1 has 32 forwards already." } };
    ports.openLink(hostKey("vm1"), "http://localhost:5173/");
    await settle();
    expect(opened).toEqual([]);
    expect(notices).toMatchObject([{ kind: "error", title: "Port 5173 of vm1 was not forwarded", description: "vm1 has 32 forwards already." }]);
  });
});

const preference = (changes: Partial<SavedForward> = {}): SavedForward => ({
  target: "vm1", remote: 5173, local: 15173, label: "Frontend", enabled: true, ...changes,
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

/** Hold a selected response after the fake server has applied its action. */
function holdResponse(deps: PortForwardingDeps, match: (path: string, init?: RequestInit) => boolean) {
  const original = deps.fetch;
  const held = deferred<void>();
  deps.fetch = async (path, init) => {
    const res = await original(path, init);
    if (match(path, init)) await held.promise;
    return res;
  };
  return () => held.resolve();
}

describe("PortForwarding: saved mappings and explicit consent", () => {
  it("reads saved frontend/API mappings before connecting, without forwarding", async () => {
    const { hub, ports } = setup();
    hub.saved = [preference(), preference({ remote: 8000, local: 18000, label: "API", enabled: false })];
    await ports.refresh();
    expect(ports.lists()[0].rows.map((r) => [r.port, r.saved?.local, r.saved?.enabled, r.process]))
      .toEqual([[5173, 15173, true, undefined], [8000, 18000, false, undefined]]);
    expect(hub.requests.map((r) => [r.path, r.method])).toEqual([[FORWARDS_PATH, "GET"], [PREFERENCES_PATH, "GET"]]);
  });

  it("saves a disconnected manual mapping through PUT alone and preserves pause during metadata edits", async () => {
    const { hub, ports } = setup();
    const parsed = manualPort({ target: "vm1", remote: "5173", local: "15173", label: "Frontend", remember: true }).value!;
    expect(await ports.save(parsed.target, parsed.port, { local: parsed.local, label: parsed.label })).toBe(true);
    expect(hub.of("PUT", PREFERENCES_PATH)).toEqual([{ method: "PUT", body: { target: "vm1", port: 5173, local: 15173, label: "Frontend" } }]);
    expect(hub.of("POST")).toEqual([]);
    await ports.stop("vm1", 5173);
    await ports.save("vm1", 5173, { label: "API" });
    expect(ports.saved()[0]).toMatchObject({ local: 15173, enabled: false, label: "API" });
    expect(hub.of("PUT", PREFERENCES_PATH).at(-1)?.body).toEqual({ target: "vm1", port: 5173, label: "API" });
    await ports.save("vm1", 5173, { label: "" });
    expect(ports.saved()[0].label).toBe("");
    expect(hub.of("POST")).toEqual([]);
  });

  it("forwards without remembering unless opted in, then saves the actual allocated port", async () => {
    const { hub, ports } = setup();
    hub.localFor = () => 40123;
    await ports.forward("vm1", 5173);
    expect(hub.of("PUT", PREFERENCES_PATH)).toEqual([]);
    await ports.forward("vm1", 5173, { remember: true, label: "Frontend" });
    expect(hub.of("PUT", PREFERENCES_PATH)).toEqual([{ method: "PUT", body: { target: "vm1", port: 5173, local: 40123, label: "Frontend" } }]);
    expect(ports.saved()[0].local).toBe(40123);
  });

  it("keeps a live mapping visible and reports honestly when its Remember write fails", async () => {
    const { hub, ports, notices } = setup();
    hub.fail = { path: PREFERENCES_PATH, status: 500, body: { error: "Disk full", code: "storage_failure" } };
    expect(await ports.forward("vm1", 5173, { remember: true })).not.toBeNull();
    await settle();
    expect(ports.forwards()).toHaveLength(1);
    expect(ports.saved()).toEqual([]);
    expect(ports.errorOf("vm1", 5173)?.code).toBe("storage_failure");
    expect(notices).toMatchObject([{ kind: "error", title: "Port 5173 is forwarded but was not remembered" }]);
    await ports.refresh();
    expect(ports.errorOf("vm1", 5173)?.code).toBe("storage_failure");
  });

  it("durably stops, explicitly resumes at the saved port, then forgets without stopping", async () => {
    const { hub, ports } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    await ports.forward("vm1", 5173);
    await ports.stop("vm1", 5173);
    expect(ports.forwards()).toEqual([]);
    expect(ports.saved()[0]).toMatchObject({ local: 15173, enabled: false });
    expect(await ports.stop("vm1", 5173)).toBe(true);
    await ports.forward("vm1", 5173);
    expect(ports.saved()[0].enabled).toBe(true);
    expect(ports.forwards()[0].local).toBe(15173);
    const stops = hub.of("DELETE").length;
    await ports.forget("vm1", 5173);
    await ports.forget("vm1", 5173);
    expect(ports.saved()).toEqual([]);
    expect(ports.forwards()).toHaveLength(1);
    expect(hub.of("DELETE")).toHaveLength(stops);
    expect(hub.of("DELETE", PREFERENCES_PATH)).toHaveLength(2);
  });

  it("never reports a persistent Stop when the durable write fails", async () => {
    const { hub, ports } = setup();
    await ports.forward("vm1", 5173, { remember: true });
    hub.fail = { status: 500, body: { error: "Disk full", code: "storage_failure" } };
    expect(await ports.stop("vm1", 5173)).toBe(false);
    expect(ports.forwards()).toHaveLength(1);
    expect(ports.saved()[0].enabled).toBe(true);
  });

  it("preserves the old mapping on failed alternate and commits only a successful numeric choice", async () => {
    const { hub, ports, opened } = setup();
    hub.saved = [preference({ enabled: false })];
    await ports.refresh();
    hub.fail = { status: 409, body: { error: "Local port is occupied", code: "local_port_conflict" } };
    expect(await ports.forward("vm1", 5173, { local: 15174, open: true })).toBeNull();
    expect(ports.saved()[0]).toMatchObject({ local: 15173, enabled: false });
    expect(ports.errorOf("vm1", 5173)?.code).toBe("local_port_conflict");
    expect(opened).toEqual([]);
    await ports.forward("vm1", 5173, { local: 15175 });
    expect(hub.of("POST").map((r) => r.body?.local)).toEqual([15174, 15175]);
    expect(ports.saved()[0]).toMatchObject({ local: 15175, enabled: true });
    expect(ports.errorOf("vm1", 5173)).toBeUndefined();
    expect(hub.of("PUT", PREFERENCES_PATH)).toEqual([]);
  });

  it("refuses to change an active port through Forward or Save, requiring Stop first", async () => {
    const { ports } = setup();
    await ports.forward("vm1", 5173, { local: 15173, remember: true });
    expect(await ports.forward("vm1", 5173, { local: 15174 })).toBeNull();
    expect(ports.errorOf("vm1", 5173)?.code).toBe("active_mapping_conflict");
    expect(await ports.save("vm1", 5173, { local: 15174 })).toBe(false);
    expect(ports.forwards()[0].local).toBe(15173);
    expect(ports.saved()[0].local).toBe(15173);
    await ports.stop("vm1", 5173);
    expect(await ports.forward("vm1", 5173, { local: 15174 })).toMatchObject({ local: 15174 });
  });
});

describe("PortForwarding: refresh and mutation ordering", () => {
  it("keeps each resource on failed or malformed reads while accepting a valid other resource", async () => {
    const { hub, ports, deps } = setup();
    hub.saved = [preference()];
    await ports.forward("vm1", 8000);
    await ports.refresh();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    deps.fetch = async (path) => path === PREFERENCES_PATH ? json(200, { version: 8, forwards: [] }) : json(200, []);
    await ports.refresh();
    expect(ports.saved()).toHaveLength(1);
    expect(ports.forwards()).toEqual([]);
    deps.fetch = async (path) => path === PREFERENCES_PATH ? json(200, { version: 1, forwards: [] }) : json(503, {});
    await ports.refresh();
    expect(ports.saved()).toEqual([]);
    expect(warn).toHaveBeenCalledTimes(2);
    warn.mockRestore();
  });

  it("ignores a stale saved GET after Forget and an earlier read answered after a newer one", async () => {
    const { hub, ports } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    const gate = deferred<void>();
    hub.gate = gate.promise;
    const old = ports.refresh();
    hub.gate = undefined;
    await ports.forget("vm1", 5173);
    gate.resolve();
    await old;
    expect(ports.saved()).toEqual([]);
    const next = deferred<void>();
    hub.gate = next.promise;
    const stale = ports.refresh();
    hub.gate = undefined;
    hub.saved = [preference({ label: "Newer" })];
    await ports.refresh();
    next.resolve();
    await stale;
    expect(ports.saved()[0].label).toBe("Newer");
  });

  it("orders Stop behind opening and suppresses the earlier result and browser-open continuation", async () => {
    const { hub, ports, deps, opened } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    const opening = ports.forward("vm1", 5173, { open: true });
    await settle();
    const stopping = ports.stop("vm1", 5173);
    expect(hub.of("DELETE")).toHaveLength(0);
    release();
    expect(await opening).toBeNull();
    await stopping;
    await settle();
    expect(opened).toEqual([]);
    expect(ports.forwards()).toEqual([]);
    expect(ports.saved()[0].enabled).toBe(false);
    expect(hub.requests.filter((r) => r.method !== "GET").map((r) => r.method)).toEqual(["POST", "DELETE"]);
  });

  it("does not let an in-flight Forward and Remember recreate a subsequently forgotten rule", async () => {
    const { hub, ports, deps, opened } = setup();
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    const opening = ports.forward("vm1", 5173, { open: true, remember: true, label: "Old" });
    await settle();
    const forgetting = ports.forget("vm1", 5173);
    release();
    await Promise.all([opening, forgetting]);
    await settle();
    expect(opened).toEqual([]);
    expect(hub.of("PUT", PREFERENCES_PATH)).toEqual([]);
    expect(ports.saved()).toEqual([]);
    expect(ports.forwards()).toHaveLength(1);
  });

  it("orders a delayed Save before Forget without republishing its stale preference", async () => {
    const { hub, ports, deps } = setup();
    const release = holdResponse(deps, (_path, init) => init?.method === "PUT");
    const saving = ports.save("vm1", 5173);
    await settle();
    const forgetting = ports.forget("vm1", 5173);
    release();
    expect(await saving).toBe(false);
    await forgetting;
    await settle();
    expect(ports.saved()).toEqual([]);
    expect(hub.saved).toEqual([]);
  });

  it("allows another mapping to finish while a slow one has a queued Stop", async () => {
    const { ports, deps } = setup();
    const release = holdResponse(deps, (_path, init) => init?.method === "POST" && JSON.parse(String(init.body)).port === 5173);
    const slow = ports.forward("vm1", 5173);
    const stopping = ports.stop("vm1", 5173);
    expect(await ports.forward("vm1", 8000, { remember: true })).toMatchObject({ remote: 8000 });
    expect(ports.savedOf("vm1", 8000)).toBeDefined();
    release();
    await Promise.all([slow, stopping]);
  });

  it("does not open a remote link or blocked-browser notice after a later Stop", async () => {
    const { ports, deps, opened } = setup();
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    ports.openLink(hostKey("vm1"), "http://localhost:5173/path");
    await settle();
    const stop = ports.stop("vm1", 5173);
    release();
    await stop;
    await settle();
    expect(opened).toEqual([]);
    const blocked = setup({ openWorks: false });
    await blocked.ports.forward("vm1", 5173, { open: true });
    const notice = blocked.notices.at(-1)!;
    await blocked.ports.stop("vm1", 5173);
    blocked.hub.fail = { status: 409, body: { error: "Port occupied", code: "local_port_conflict" } };
    notice.action!.run();
    await settle();
    expect(blocked.opened).toHaveLength(1);
  });

  it("polls saved-only, watched and visible empty states, then discovers another window's changes", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const { hub, ports } = setup();
    ports.setViewOpen(true);
    await settle();
    hub.saved = [preference({ enabled: false })];
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    await settle();
    expect(ports.saved()[0].enabled).toBe(false);
    ports.setViewOpen(false);
    hub.saved[0].label = "Changed elsewhere";
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    await settle();
    expect(ports.saved()[0].label).toBe("Changed elsewhere");
    hub.saved = [];
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    await settle();
    expect(ports.saved()).toEqual([]);
    const before = hub.requests.length;
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(hub.requests).toHaveLength(before);
    const unwatch = ports.watch(host("vm2").conn);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    await settle();
    expect(hub.requests.length).toBeGreaterThan(before);
    unwatch();
    expect(hub.of("POST")).toEqual([]);
  });

  it("verifies explicit actions against the backend after another window stopped the cached active mapping", async () => {
    const { hub, ports, opened } = setup();
    await ports.forward("vm1", 5173, { remember: true });
    hub.forwards = [];
    hub.saved[0].enabled = false;
    hub.fail = { status: 409, body: { error: "A different service now owns the port", code: "local_port_conflict" } };
    expect(await ports.forward("vm1", 5173, { open: true })).toBeNull();
    await settle();
    expect(opened).toEqual([]);
    expect(ports.saved()[0].enabled).toBe(false);
    expect(ports.forwards()).toEqual([]);
    expect(hub.of("POST")).toHaveLength(2);
  });

  it("clears a local conflict after another window retries successfully", async () => {
    const { hub, ports } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    hub.fail = { status: 409, body: { error: "Port busy", code: "local_port_conflict" } };
    await ports.forward("vm1", 5173);
    await settle();
    expect(ports.errorOf("vm1", 5173)?.code).toBe("local_port_conflict");
    await hub.fetch(FORWARDS_PATH, { method: "POST", body: JSON.stringify({ target: "vm1", port: 5173, local: 15174 }) });
    await ports.refresh();
    expect(ports.errorOf("vm1", 5173)).toBeUndefined();
    expect(ports.lists()[0].rows[0]).not.toHaveProperty("error");
    expect(ports.saved()[0].local).toBe(15174);
  });

  it.each(["pause", "forget", "change local port"])("clears an obsolete forward error after another window chooses to %s", async (action) => {
    const { hub, ports } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    hub.fail = { status: 409, body: { error: "Port busy", code: "local_port_conflict" } };
    await ports.forward("vm1", 5173);
    await settle();
    hub.saved[0].label = "Only a label edit";
    await ports.refresh();
    expect(ports.errorOf("vm1", 5173)?.code).toBe("local_port_conflict");
    if (action === "pause") hub.saved[0].enabled = false;
    else if (action === "forget") hub.saved = [];
    else hub.saved[0].local = 15174;
    await ports.refresh();
    expect(ports.errorOf("vm1", 5173)).toBeUndefined();
    expect(hub.of("POST")).toHaveLength(1);
  });

  it("uses current server failures, then clears them when another window resolves the mapping", async () => {
    const { hub, ports } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    hub.fail = { status: 409, body: { error: "Port busy", code: "local_port_conflict" } };
    await ports.forward("vm1", 5173);
    await settle();
    hub.saved[0].lastError = { code: "unreachable", message: "The latest retry could not reach the host" };
    await ports.refresh();
    expect(ports.errorOf("vm1", 5173)?.code).toBe("unreachable");
    await hub.fetch(FORWARDS_PATH, { method: "POST", body: JSON.stringify({ target: "vm1", port: 5173 }) });
    await ports.refresh();
    expect(ports.errorOf("vm1", 5173)).toBeUndefined();
  });

  it("keeps a failed label edit visible until another window changes or removes its preference", async () => {
    const { hub, ports } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    hub.fail = { status: 500, body: { error: "Disk full", code: "storage_failure" } };
    await ports.save("vm1", 5173, { label: "API" });
    await settle();
    expect(ports.errorOf("vm1", 5173)?.code).toBe("storage_failure");
    hub.saved = [];
    await ports.refresh();
    expect(ports.errorOf("vm1", 5173)).toBeUndefined();
  });
});

describe("PortForwarding: pause-aware detection and links", () => {
  it("silently skips known paused mappings and respects a stale window's automatic refusal", async () => {
    const { hub, ports, notices } = setup({ auto: true });
    const vm = host("vm1");
    hub.saved = [preference({ enabled: false })];
    await ports.refresh();
    ports.watch(vm.conn);
    list(vm.socket);
    list(vm.socket, [5173, "vite"]);
    await settle();
    expect(hub.of("POST")).toEqual([]);
    expect(notices).toEqual([]);
    const stale = setup({ auto: true });
    stale.hub.saved = [preference({ enabled: false })];
    stale.ports.watch(vm.conn);
    list(vm.socket);
    list(vm.socket, [5173, "vite"]);
    await settle();
    expect(stale.hub.of("POST")).toEqual([{ method: "POST", body: { target: "vm1", port: 5173, automatic: true } }]);
    expect(stale.notices).toEqual([]);
    expect(stale.ports.saved()[0].enabled).toBe(false);
  });

  it("keeps automatic saved-port conflicts exact while allowing unrelated ephemeral forwards", async () => {
    const { hub, ports, notices } = setup({ auto: true });
    const vm = host("vm1");
    hub.saved = [preference()];
    await ports.refresh();
    ports.watch(vm.conn);
    list(vm.socket);
    hub.fail = { status: 409, body: { error: "Saved local port busy", code: "local_port_conflict" } };
    list(vm.socket, [5173, "vite"], [8000, "api"]);
    await settle();
    expect(hub.of("POST").map((r) => r.body)).toEqual([
      { target: "vm1", port: 5173, automatic: true }, { target: "vm1", port: 8000, automatic: true },
    ]);
    expect(ports.saved()[0].local).toBe(15173);
    expect(ports.forwards().map((f) => f.remote)).toEqual([8000]);
    expect(notices.some((n) => n.kind === "error")).toBe(true);
    expect(hub.of("PUT", PREFERENCES_PATH)).toEqual([]);
  });

  it("resumes a paused saved link explicitly while preserving HTTPS, path, query and fragment", async () => {
    const { hub, ports, opened } = setup({ auto: true });
    hub.saved = [preference({ enabled: false })];
    await ports.refresh();
    ports.openLink(hostKey("vm1"), "https://127.0.0.1:5173/app?x=1#top");
    await settle();
    expect(hub.of("POST")).toEqual([{ method: "POST", body: { target: "vm1", port: 5173 } }]);
    expect(opened).toEqual(["https://localhost:15173/app?x=1#top"]);
    expect(ports.saved()[0].enabled).toBe(true);
    expect(hub.of("PUT", PREFERENCES_PATH)).toEqual([]);
  });
});


describe("PortForwarding: every browser Open verifies current mapping", () => {
  it.each(["row", "link", "forward", "fallback", "system browser"] as const)("rejects a completed POST delivered after another controller's acknowledged Stop (%s)", async (action) => {
    const first = setup({ openWorks: action !== "fallback" });
    const second = setup({ hub: first.hub });
    const { hub, ports, deps, opened, notices } = first;
    let noticeAction: (() => void) | undefined;
    await ports.forward("vm1", 5173, { remember: true });
    await settle();
    const active = ports.forwardOf("vm1", 5173)!;
    if (action === "fallback") {
      ports.openLink(hostKey("vm1"), "https://localhost:5173/app?x=1#top");
      await settle();
      expect(opened).toEqual(["https://localhost:5173/app?x=1#top"]);
      noticeAction = notices.at(-1)!.action!.run;
      opened.length = 0;
    }
    await second.ports.refresh();
    const priorPosts = hub.of("POST").length;
    const priorMutations = hub.requests.filter((r) => r.method !== "GET").map((r) => r.method);
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    if (action !== "system browser") deps.reserveOpen = () => reservation;
    // The real controller POST reaches the fake backend and completes. Only
    // response delivery is held; a POST accepted after Stop would be Resume.
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    if (noticeAction) noticeAction();
    else if (action === "link") ports.openLink(hostKey("vm1"), "https://localhost:5173/app?x=1#top");
    else if (action === "forward") void ports.forward("vm1", 5173, { open: true });
    else ports.open(active);
    await settle();
    expect(hub.of("POST")).toHaveLength(priorPosts + 1);
    expect(hub.forwards).toEqual([active]);
    expect(hub.saved[0].enabled).toBe(true);
    expect(reservation.open).not.toHaveBeenCalled();
    expect(await second.ports.stop("vm1", 5173)).toBe(true);
    await settle();
    expect(hub.forwards).toEqual([]);
    expect(hub.saved[0].enabled).toBe(false);
    const repainted: (number | undefined)[] = [];
    deps.changed = () => repainted.push(ports.forwardOf("vm1", 5173)?.local);
    release();
    await settle();
    expect(reservation.open).not.toHaveBeenCalled();
    if (action !== "system browser") expect(reservation.close).toHaveBeenCalled();
    expect(opened).toEqual([]);
    expect(repainted).not.toContain(active.local);
    expect(ports.forwards()).toEqual([]);
    expect(ports.savedOf("vm1", 5173)?.enabled).toBe(false);
    expect(hub.requests.filter((r) => r.method !== "GET").map((r) => r.method))
      .toEqual([...priorMutations, "POST", "DELETE"]);
  });

  it.each(["remapped", "down"] as const)("does not navigate or repaint a delayed POST when its mapping is now %s", async (state) => {
    const { hub, ports, deps, opened } = setup();
    const second = setup({ hub });
    const active = (await ports.forward("vm1", 5173, { remember: true }))!;
    await settle();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = () => reservation;
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    ports.open(active);
    await settle();
    expect(hub.of("POST")).toHaveLength(2);
    if (state === "remapped") {
      expect(await second.ports.stop("vm1", 5173)).toBe(true);
      expect(await second.ports.forward("vm1", 5173, { local: 15174 })).toMatchObject({ local: 15174 });
    } else hub.forwards[0].state = "down";
    await settle();
    const repainted: unknown[] = [];
    deps.changed = () => repainted.push(ports.forwardOf("vm1", 5173));
    release();
    await settle();
    expect(reservation.open).not.toHaveBeenCalled();
    expect(reservation.close).toHaveBeenCalled();
    expect(opened).toEqual([]);
    expect(repainted).not.toContainEqual(active);
    expect(ports.forwards()).toEqual(hub.forwards);
    expect(hub.of("POST")).toHaveLength(state === "remapped" ? 3 : 2);
  });

  it.each(["HTTP error", "network error", "invalid JSON", "invalid active list"] as const)("never opens when final active validation fails with %s", async (problem) => {
    const { hub, ports, deps, notices, opened } = setup();
    const active = (await ports.forward("vm1", 5173))!;
    await settle();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = () => reservation;
    let validationFailed = false;
    deps.fetch = async (path, init) => {
      if (!validationFailed && path === FORWARDS_PATH && (init?.method ?? "GET") === "GET") {
        validationFailed = true;
        if (problem === "network error") throw new Error("Connection lost");
        if (problem === "invalid JSON") return new Response("not JSON");
        return problem === "HTTP error" ? json(503, { error: "Cannot verify the forward", code: "unreachable" })
          : json(200, [active, { ...active, remote: 8000, local: 0 }]);
      }
      return hub.fetch(path, init);
    };
    expect(await ports.forward("vm1", 5173, { open: true })).toBeNull();
    await settle();
    expect(validationFailed).toBe(true);
    expect(reservation.open).not.toHaveBeenCalled();
    expect(reservation.close).toHaveBeenCalled();
    expect(opened).toEqual([]);
    expect(notices).toContainEqual(expect.objectContaining({ kind: "error" }));
    expect(ports.forwards()).toEqual([active]);
    expect(hub.of("POST")).toHaveLength(2);
  });

  it("allows an explicit POST accepted after another controller's Stop to resume and open", async () => {
    const { hub, ports, deps } = setup();
    const second = setup({ hub });
    const active = (await ports.forward("vm1", 5173, { remember: true }))!;
    await settle();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = () => reservation;
    const acceptance = deferred<void>();
    deps.fetch = async (path, init) => {
      // Deliberately delay the request, unlike holdResponse: this POST must
      // be accepted after Stop and is therefore a legitimate explicit Resume.
      if (init?.method === "POST") await acceptance.promise;
      return hub.fetch(path, init);
    };
    ports.open(active);
    await settle();
    expect(hub.of("POST")).toHaveLength(1);
    expect(await second.ports.stop("vm1", 5173)).toBe(true);
    expect(hub.forwards).toEqual([]);
    expect(hub.saved[0].enabled).toBe(false);
    acceptance.resolve();
    await settle();
    expect(reservation.open).toHaveBeenCalledExactlyOnceWith(active.url);
    expect(reservation.close).not.toHaveBeenCalled();
    expect(ports.savedOf("vm1", 5173)?.enabled).toBe(true);
    expect(hub.requests.filter((r) => r.method !== "GET").map((r) => r.method))
      .toEqual(["POST", "PUT", "DELETE", "POST"]);
  });

  it("validates after Remember finishes so a delayed PUT cannot authorize a stopped forward", async () => {
    const { hub, ports, deps, opened } = setup();
    const second = setup({ hub });
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = () => reservation;
    const release = holdResponse(deps, (_path, init) => init?.method === "PUT");
    const opening = ports.forward("vm1", 5173, { open: true, remember: true });
    await settle();
    expect(hub.saved).toHaveLength(1);
    expect(await second.ports.stop("vm1", 5173)).toBe(true);
    const repainted: unknown[] = [];
    deps.changed = () => repainted.push(ports.forwardOf("vm1", 5173));
    release();
    expect(await opening).toBeNull();
    await settle();
    expect(reservation.open).not.toHaveBeenCalled();
    expect(reservation.close).toHaveBeenCalled();
    expect(opened).toEqual([]);
    expect(repainted.every((forward) => forward === undefined)).toBe(true);
    expect(ports.savedOf("vm1", 5173)?.enabled).toBe(false);
    expect(hub.of("POST")).toHaveLength(1);
  });

  it.each(["stop", "forget", "dispose"] as const)("ignores the final active JSON after a newer same-window %s", async (action) => {
    const { hub, ports, deps, opened, notices } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = () => reservation;
    const reading = deferred<void>();
    const delivery = deferred<void>();
    let held = false;
    deps.fetch = async (path, init) => {
      const response = await hub.fetch(path, init);
      if (!held && path === FORWARDS_PATH && (init?.method ?? "GET") === "GET") {
        held = true;
        const body = await response.json();
        // The response headers have arrived; the final asynchronous JSON read
        // must still respect a newer local generation before publishing it.
        response.json = async () => {
          reading.resolve();
          await delivery.promise;
          return body;
        };
      }
      return response;
    };
    const opening = ports.forward("vm1", 5173, { open: true });
    await reading.promise;
    expect(reservation.open).not.toHaveBeenCalled();
    const mutation = action === "dispose" ? ports.dispose() : ports[action]("vm1", 5173);
    expect(reservation.close).toHaveBeenCalled();
    const repainted: (number | undefined)[] = [];
    deps.changed = () => {
      // Forget may subsequently show the live forward through its own fresh
      // refresh; only publication by the superseded opening is forbidden.
      if (!hub.requests.some((r) => r.method === "DELETE")) repainted.push(ports.forwardOf("vm1", 5173)?.local);
    };
    delivery.resolve();
    expect(await opening).toBeNull();
    await mutation;
    await settle();
    expect(repainted).not.toContain(15173);
    expect(reservation.open).not.toHaveBeenCalled();
    expect(opened).toEqual([]);
    expect(notices).toEqual([]);
    if (action === "stop") expect(ports.forwards()).toEqual([]);
    if (action === "forget") expect(ports.saved()).toEqual([]);
  });

  it("lets unrelated keys finish during validation without repainting their older snapshot", async () => {
    const { hub, ports, deps, opened } = setup();
    await ports.forward("vm2", 5173);
    await settle();
    let held = false;
    const release = holdResponse(deps, (path, init) => {
      if (held || path !== FORWARDS_PATH || (init?.method ?? "GET") !== "GET") return false;
      held = true;
      return true;
    });
    const opening = ports.forward("vm1", 5173, { open: true });
    await settle();
    expect(held).toBe(true);
    expect(opened).toEqual([]);
    expect(await ports.stop("vm2", 5173)).toBe(true);
    expect(await ports.forward("vm1", 8000, { open: true, remember: true })).toMatchObject({ remote: 8000 });
    expect(opened).toEqual(["http://localhost:8000"]);
    const repainted: unknown[] = [];
    deps.changed = () => repainted.push(ports.forwards().map((f) => [f.target, f.remote]));
    release();
    expect(await opening).toMatchObject({ target: "vm1", remote: 5173 });
    await settle();
    expect(opened).toEqual(["http://localhost:8000", "http://localhost:5173"]);
    expect(repainted.length).toBeGreaterThan(0);
    for (const snapshot of repainted) expect(snapshot).toEqual([["vm1", 5173], ["vm1", 8000]]);
    expect(ports.savedOf("vm1", 8000)?.enabled).toBe(true);
    expect(hub.of("POST")).toHaveLength(3);
  });

  it("does not open the row's cached URL after another window stops and its port is occupied", async () => {
    const { hub, ports, opened } = setup();
    const active = (await ports.forward("vm1", 5173, { remember: true }))!;
    hub.forwards = [];
    hub.saved[0].enabled = false;
    hub.fail = { status: 409, body: { code: "local_port_conflict", error: "Another service owns this local port" } };
    ports.open(active);
    await settle();
    expect(hub.of("POST")).toHaveLength(2);
    expect(opened).toEqual([]);
  });

  it("verifies blocked-browser fallback after a cross-window Stop and opens nothing on conflict", async () => {
    const { hub, ports, opened, notices } = setup({ openWorks: false });
    hub.saved = [preference()];
    await ports.refresh();
    ports.openLink(hostKey("vm1"), "https://localhost:5173/app?x=1#top");
    await settle();
    const fallback = notices.at(-1)!.action!;
    hub.forwards = [];
    hub.saved[0].enabled = false;
    hub.fail = { status: 409, body: { code: "local_port_conflict", error: "Port occupied" } };
    fallback.run();
    await settle();
    expect(hub.of("POST")).toHaveLength(2);
    expect(opened).toEqual(["https://localhost:15173/app?x=1#top"]);
  });

  it("preserves rewritten URL details when a verified fallback uses a newer local mapping", async () => {
    const { hub, ports, opened, notices } = setup({ openWorks: false });
    hub.saved = [preference()];
    await ports.refresh();
    ports.openLink(hostKey("vm1"), "https://localhost:5173/app?x=1#top");
    await settle();
    const fallback = notices.at(-1)!.action!;
    hub.forwards = [];
    hub.saved[0].local = 15174;
    fallback.run();
    await settle();
    expect(opened).toEqual(["https://localhost:15173/app?x=1#top", "https://localhost:15174/app?x=1#top"]);
  });
});


it("does not mistake supervisor reconnects for successful persistence", async () => {
  const { hub, ports } = setup();
  hub.fail = { path: PREFERENCES_PATH, status: 500, body: { error: "Disk full", code: "storage_failure" } };
  await ports.forward("vm1", 5173, { remember: true });
  await settle();
  hub.forwards[0].state = "down";
  await ports.refresh();
  expect(ports.errorOf("vm1", 5173)?.code).toBe("storage_failure");
  await ports.save("vm1", 5173);
  hub.fail = { status: 500, body: { error: "Disk full", code: "storage_failure" } };
  await ports.stop("vm1", 5173);
  await settle();
  hub.forwards[0].state = "up";
  hub.saved[0].label = "Changed elsewhere";
  await ports.refresh();
  expect(ports.errorOf("vm1", 5173)?.code).toBe("storage_failure");
  expect(ports.saved()[0].enabled).toBe(true);
});

describe("PortForwarding: browser activation reservations", () => {
  it("reserves a blank tab during the click and navigates only after the verified response", async () => {
    const { hub, ports, deps, opened } = setup();
    hub.saved = [preference()];
    await ports.refresh();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = vi.fn(() => reservation);
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    ports.openLink(hostKey("vm1"), "https://localhost:5173/app?x=1#top");
    expect(deps.reserveOpen).toHaveBeenCalledTimes(1);
    await settle();
    expect(reservation.open).not.toHaveBeenCalled();
    release();
    await settle();
    expect(reservation.open).toHaveBeenCalledWith("https://localhost:15173/app?x=1#top");
    expect(reservation.close).not.toHaveBeenCalled();
    expect(opened).toEqual([]);
  });

  it.each(["stop", "forget", "dispose"] as const)("closes the waiting blank immediately on %s and never navigates afterward", async (action) => {
    const { ports, deps } = setup();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = () => reservation;
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    const opening = ports.forward("vm1", 5173, { open: true });
    await settle();
    const mutation = action === "dispose" ? ports.dispose() : ports[action]("vm1", 5173);
    expect(reservation.close).toHaveBeenCalled();
    release();
    await Promise.all([opening, mutation]);
    expect(reservation.open).not.toHaveBeenCalled();
  });

  it("closes a reserved tab on conflict without opening the unrelated service", async () => {
    const { hub, ports, deps, opened } = setup();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    deps.reserveOpen = () => reservation;
    hub.fail = { status: 409, body: { error: "Port in use", code: "local_port_conflict" } };
    await ports.forward("vm1", 5173, { open: true });
    expect(reservation.close).toHaveBeenCalled();
    expect(reservation.open).not.toHaveBeenCalled();
    expect(opened).toEqual([]);
  });

  it("reserves on the fallback click when the first popup was blocked, keeping the rewritten URL", async () => {
    const { hub, ports, deps, notices, opened } = setup({ openWorks: false });
    hub.saved = [preference()];
    await ports.refresh();
    const reservation = { open: vi.fn(() => true), close: vi.fn() };
    const reserve = vi.fn<NonNullable<PortForwardingDeps["reserveOpen"]>>()
      .mockReturnValueOnce(undefined).mockReturnValue(reservation);
    deps.reserveOpen = reserve;
    ports.openLink(hostKey("vm1"), "https://localhost:5173/app?x=1#top");
    await settle();
    const release = holdResponse(deps, (_path, init) => init?.method === "POST");
    notices.at(-1)!.action!.run();
    expect(reserve).toHaveBeenCalledTimes(2);
    await settle();
    expect(reservation.open).not.toHaveBeenCalled();
    release();
    await settle();
    expect(reservation.open).toHaveBeenCalledWith("https://localhost:15173/app?x=1#top");
    expect(opened).toHaveLength(1);
    expect(hub.of("POST")).toHaveLength(2);
  });
});
