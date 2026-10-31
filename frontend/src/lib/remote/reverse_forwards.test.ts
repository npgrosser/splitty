import { afterEach, describe, expect, it, vi } from "vitest";
import { REFRESH_INTERVAL } from "./port_forwarding.ts";
import {
  ADMIN_PORTS, REVERSE_FORWARDS_PATH, REVERSE_PREFERENCES_PATH, ReverseForwarding, parseReverseForwards,
  parseSavedReverseForwards, shareInput, shareLists, shareWarning, type ReverseForward, type SavedReverseForward,
} from "./reverse_forwards.ts";

type RequestBody = { target: string; port: number; local?: number };

/** Both reverse-forward APIs as R5 describes them. */
class FakeHub {
  forwards: ReverseForward[] = [];
  saved: SavedReverseForward[] = [];
  requests: { path: string; method: string; body?: RequestBody }[] = [];
  fail: { status: number; body?: unknown; path?: string; raw?: string } | undefined;
  /** Answer POST with another mapping than requested. */
  skew = false;

  fetch = async (path: string, init?: RequestInit): Promise<Response> => {
    expect([REVERSE_FORWARDS_PATH, REVERSE_PREFERENCES_PATH]).toContain(path);
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) as RequestBody : undefined;
    this.requests.push({ path, method, ...(body ? { body } : {}) });
    if (method !== "GET") expect(init?.headers).toEqual({ "Content-Type": "application/json" });
    if (method === "GET") {
      return json(200, structuredClone(path === REVERSE_FORWARDS_PATH ? this.forwards : { version: 1, forwards: this.saved }));
    }
    if (this.fail && (!this.fail.path || this.fail.path === path)) {
      const { status, body: failure, raw } = this.fail;
      this.fail = undefined;
      return raw === undefined ? json(status, failure) : new Response(raw, { status });
    }
    const { target, port } = body!;
    const active = this.forwards.find((f) => f.target === target && f.remote === port);
    const saved = this.saved.find((f) => f.target === target && f.remote === port);
    if (path === REVERSE_PREFERENCES_PATH) {
      if (method === "DELETE") {
        this.saved = this.saved.filter((f) => f !== saved);
        return new Response(null, { status: 204 });
      }
      const rule: SavedReverseForward = { target, remote: port, local: body!.local ?? saved?.local ?? active?.local ?? port, enabled: saved?.enabled ?? true };
      this.saved = [...this.saved.filter((f) => f !== saved), rule];
      return json(200, rule);
    }
    if (method === "POST") {
      if (active && active.local !== body!.local) return json(409, { code: "active_mapping_conflict", error: "Shared with another local port." });
      const forward = active ?? { target, remote: port, local: body!.local!, state: "up" as const };
      if (!active) this.forwards.push(forward);
      if (saved) { saved.enabled = true; saved.local = forward.local; delete saved.lastError; }
      return json(200, this.skew ? { ...forward, local: forward.local + 1 } : forward);
    }
    if (saved) { saved.enabled = false; delete saved.lastError; }
    if (!active && !saved) return json(404, { error: "Nothing is shared there." });
    this.forwards = this.forwards.filter((f) => f !== active);
    return new Response(null, { status: 204 });
  };

  of(method: string, path = REVERSE_FORWARDS_PATH) {
    return this.requests.filter((r) => r.method === method && r.path === path).map((r) => r.body);
  }

  mutations() {
    return this.requests.filter((r) => r.method !== "GET");
  }
}

function json(status: number, body: unknown): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await new Promise<void>((resolve) => queueMicrotask(resolve));
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

let instances: ReverseForwarding[] = [];

function setup(hub = new FakeHub()) {
  const changed = vi.fn();
  const sharing = new ReverseForwarding({ fetch: hub.fetch, changed });
  instances.push(sharing);
  return { hub, sharing, changed };
}

afterEach(() => {
  for (const instance of instances) instance.dispose();
  instances = [];
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const rule = (remote: number, local = remote, enabled = true, target = "vm1"): SavedReverseForward => ({ target, remote, local, enabled });
const active = (remote: number, local = remote, state: "up" | "down" = "up", target = "vm1"): ReverseForward => ({ target, remote, local, state });

describe("shared ports: reads", () => {
  it("parses active shares in order and refuses a list with an invalid entry", () => {
    expect(parseReverseForwards([active(9000, 9000, "down", "vm2"), active(8080), { ...active(3000), state: "weird" }]))
      .toEqual([active(3000), active(8080), active(9000, 9000, "down", "vm2")]);
    expect(parseReverseForwards([active(8080), { target: "vm1", remote: 0, local: 1 }])).toBeNull();
    expect(parseReverseForwards([{ ...active(8080), target: "-oProxyCommand=x" }])).toBeNull();
    expect(parseReverseForwards({})).toBeNull();
  });

  it("parses saved rules with a plain-text lastError and refuses invalid documents", () => {
    const lastError = { code: "host_port_conflict", message: "Port 8080 is in use on vm1.‮", hint: "Choose another port.", extra: 1 };
    expect(parseSavedReverseForwards({ version: 1, forwards: [rule(8080, 18080, false), { ...rule(3000), lastError }] })).toEqual([
      { ...rule(3000), lastError: { code: "host_port_conflict", message: "Port 8080 is in use on vm1.", hint: "Choose another port." } },
      rule(8080, 18080, false),
    ]);
    expect(parseSavedReverseForwards({ version: 2, forwards: [] })).toBeNull();
    expect(parseSavedReverseForwards({ version: 1, forwards: [rule(1), rule(1)] })).toBeNull();
    expect(parseSavedReverseForwards({ version: 1, forwards: [{ ...rule(1), enabled: "yes" }] })).toBeNull();
    expect(parseSavedReverseForwards({ version: 1, forwards: Array.from({ length: 33 }, (_, i) => rule(i + 1)) })).toBeNull();
    expect(parseSavedReverseForwards({ version: 1, forwards: Array.from({ length: 32 }, (_, i) => rule(i + 1)) })).toHaveLength(32);
  });

  it("merges active shares and saved rules per host, each host port once", () => {
    const lists = shareLists(
      [active(11434, 11434, "down"), active(5432, 15432, "up", "db")],
      [rule(11434, 11434), rule(8080, 18080, false), rule(5432, 5432, true, "db")],
      (target, port) => target === "vm1" && port === 8080 ? "share" : undefined,
      (target, port) => target === "db" ? { code: "forward", message: `failed ${port}` } : undefined,
    );
    expect(lists).toEqual([
      // The active share's local port wins over the saved one.
      { target: "db", rows: [{ remote: 5432, local: 15432, forward: active(5432, 15432, "up", "db"), saved: rule(5432, 5432, true, "db"), error: { code: "forward", message: "failed 5432" } }] },
      { target: "vm1", rows: [
        { remote: 8080, local: 18080, saved: rule(8080, 18080, false), pending: "share" },
        { remote: 11434, local: 11434, forward: active(11434, 11434, "down"), saved: rule(11434) },
      ] },
    ]);
    expect(shareLists([], [])).toEqual([]);
  });
});

describe("shared ports: consent", () => {
  it("always states who can use the port, naming the host and the local port", () => {
    expect(shareWarning("vm1", 11434)).toEqual({
      statement: "Programs running as you on vm1 can use localhost:11434 of this machine. Most local services ask for no password.",
    });
    expect(shareWarning("vm1", null).statement).toBe("Programs running as you on vm1 can use the local port you share. Most local services ask for no password.");
    expect(shareWarning("vm1", null).strong).toBeUndefined();
  });

  it("warns more strongly for the local ports of administrative services only", () => {
    expect([...ADMIN_PORTS.keys()]).toEqual([22, 2375, 2376, 3389, 5900, 6443, 10250]);
    for (const port of ADMIN_PORTS.keys()) {
      const warning = shareWarning("vm1", port);
      expect(warning.statement).toContain(`localhost:${port}`);
      expect(warning.strong).toContain(`Port ${port} is usually`);
      expect(warning.strong).toContain("gives programs on vm1 control of this machine");
    }
    expect(shareWarning("vm1", 2375).strong).toBe("Port 2375 is usually the Docker API. Sharing it gives programs on vm1 control of this machine.");
    for (const port of [1, 21, 23, 80, 2377, 5901, 8080, 11434, 65535]) expect(shareWarning("vm1", port).strong).toBeUndefined();
  });

  it("takes the host port from the local port unless given, and only whole port numbers", () => {
    expect(shareInput({ local: " 11434 ", remote: "", remember: false })).toEqual({ value: { port: 11434, local: 11434, remember: false } });
    expect(shareInput({ local: "5432", remote: "15432", remember: true })).toEqual({ value: { port: 15432, local: 5432, remember: true } });
    for (const local of ["", "0", "65536", "1e3", "80.0", "localhost:80"]) expect(shareInput({ local, remote: "", remember: false }).error).toMatch(/^Local port/);
    for (const remote of ["0", "65536", "-1", "0x50"]) expect(shareInput({ local: "80", remote, remember: false }).error).toMatch(/^Host port/);
  });
});

describe("shared ports: requests", () => {
  it("reads both lists passively and starts nothing", async () => {
    const hub = new FakeHub();
    hub.saved = [rule(11434), rule(8080, 8080, false)];
    const { sharing, changed } = setup(hub);
    await sharing.refresh();
    expect(hub.mutations()).toEqual([]);
    expect(hub.requests.map((r) => r.path).sort()).toEqual([REVERSE_PREFERENCES_PATH, REVERSE_FORWARDS_PATH].sort());
    expect(sharing.lists()[0].rows.map((row) => row.remote)).toEqual([8080, 11434]);
    expect(changed).toHaveBeenCalled();
  });

  it("shares with exactly target, host port and local port, and remembers only when asked", async () => {
    const { hub, sharing } = setup();
    expect(await sharing.share("vm1", 11434, 11434)).toEqual(active(11434));
    expect(hub.of("POST")).toEqual([{ target: "vm1", port: 11434, local: 11434 }]);
    expect(hub.of("PUT", REVERSE_PREFERENCES_PATH)).toEqual([]);

    expect(await sharing.share("vm1", 15432, 5432, { remember: true })).toEqual(active(15432, 5432));
    const order = hub.mutations().map((r) => `${r.method} ${r.path}`);
    expect(order.slice(-2)).toEqual([`POST ${REVERSE_FORWARDS_PATH}`, `PUT ${REVERSE_PREFERENCES_PATH}`]);
    expect(hub.of("PUT", REVERSE_PREFERENCES_PATH)).toEqual([{ target: "vm1", port: 15432, local: 5432 }]);
    expect(sharing.savedOf("vm1", 15432)).toEqual(rule(15432, 5432));
    expect(sharing.errorOf("vm1", 15432)).toBeUndefined();
  });

  it("keeps the share running and says so when Remember fails", async () => {
    const { hub, sharing } = setup();
    hub.fail = { status: 409, path: REVERSE_PREFERENCES_PATH, body: { code: "preference_limit", error: "Too many saved shares.", hint: "Forget one first." } };
    expect(await sharing.share("vm1", 11434, 11434, { remember: true })).toEqual(active(11434));
    await settle();
    expect(sharing.forwardOf("vm1", 11434)).toEqual(active(11434));
    expect(sharing.savedOf("vm1", 11434)).toBeUndefined();
    expect(sharing.errorOf("vm1", 11434)).toEqual({ code: "preference_limit", message: "Shared, but not remembered: Too many saved shares.", hint: "Forget one first." });
    expect(sharing.lists()[0].rows[0].error?.code).toBe("preference_limit");
  });

  it("maps the server's error, code and hint, and failures without a JSON answer", async () => {
    const { hub, sharing } = setup();
    hub.fail = { status: 502, body: { error: "This host is not supported.", code: "unsupported_host", hint: "Sharing needs a Linux host." } };
    expect(await sharing.share("mac", 8080, 8080)).toBeNull();
    expect(sharing.errorOf("mac", 8080)).toEqual({ code: "unsupported_host", message: "This host is not supported.", hint: "Sharing needs a Linux host." });

    hub.fail = { status: 409, body: { error: "Port 8080 is in use on vm1.", code: "host_port_conflict" } };
    await sharing.share("vm1", 8080, 8080);
    expect(sharing.errorOf("vm1", 8080)).toEqual({ code: "host_port_conflict", message: "Port 8080 is in use on vm1." });

    hub.fail = { status: 500, raw: "<html>oops</html>" };
    await sharing.share("vm1", 8081, 8081);
    expect(sharing.errorOf("vm1", 8081)).toEqual({ code: "internal", message: "The local Splitty server answered HTTP 500." });

    const offline = setup({ fetch: () => Promise.reject(new TypeError("Failed to fetch")) } as unknown as FakeHub).sharing;
    await offline.share("vm1", 8082, 8082);
    expect(offline.errorOf("vm1", 8082)?.message).toBe("The local Splitty server did not answer (TypeError: Failed to fetch).");
  });

  it("refuses an answer for another mapping and invalid input without a request", async () => {
    const { hub, sharing } = setup();
    hub.skew = true;
    expect(await sharing.share("vm1", 8080, 8080)).toBeNull();
    expect(sharing.errorOf("vm1", 8080)?.message).toBe("The local Splitty server answered with a different mapping.");
    const before = hub.mutations().length;
    expect(await sharing.share("vm1", 0, 8080)).toBeNull();
    expect(await sharing.share("vm1", 8080, 70000)).toBeNull();
    expect(await sharing.share("-oProxyCommand=x", 8080, 8080)).toBeNull();
    expect(hub.mutations()).toHaveLength(before);
  });

  it("stops and pauses a saved share, resumes it at its saved port, and forgets without stopping", async () => {
    const hub = new FakeHub();
    hub.saved = [rule(11434, 21434)];
    hub.forwards = [active(11434, 21434)];
    const { sharing } = setup(hub);
    await sharing.refresh();

    expect(await sharing.stop("vm1", 11434)).toBe(true);
    expect(hub.of("DELETE")).toEqual([{ target: "vm1", port: 11434 }]);
    await settle();
    expect(sharing.forwardOf("vm1", 11434)).toBeUndefined();
    expect(sharing.savedOf("vm1", 11434)).toEqual(rule(11434, 21434, false));
    expect(sharing.lists()[0].rows[0]).toMatchObject({ remote: 11434, local: 21434 });

    // Resume is a plain POST with the rule's local port.
    const row = sharing.lists()[0].rows[0];
    expect(await sharing.share("vm1", row.remote, row.local)).toEqual(active(11434, 21434));
    expect(hub.of("POST")).toEqual([{ target: "vm1", port: 11434, local: 21434 }]);
    expect(sharing.savedOf("vm1", 11434)?.enabled).toBe(true);

    expect(await sharing.forget("vm1", 11434)).toBe(true);
    expect(hub.of("DELETE", REVERSE_PREFERENCES_PATH)).toEqual([{ target: "vm1", port: 11434 }]);
    await settle();
    expect(sharing.savedOf("vm1", 11434)).toBeUndefined();
    expect(sharing.forwardOf("vm1", 11434)).toEqual(active(11434, 21434));
  });

  it("takes a share stopped elsewhere as gone, and keeps one whose stop failed", async () => {
    const hub = new FakeHub();
    const { sharing } = setup(hub);
    expect(await sharing.stop("vm1", 8080)).toBe(true);

    hub.forwards = [active(8080)];
    await sharing.refresh();
    hub.fail = { status: 500, body: { error: "Could not save.", code: "storage_failure" } };
    expect(await sharing.stop("vm1", 8080)).toBe(false);
    await settle();
    expect(sharing.forwardOf("vm1", 8080)).toEqual(active(8080));
    expect(sharing.errorOf("vm1", 8080)).toEqual({ code: "storage_failure", message: "Could not save." });
  });

  it("remembers an active share with its local port through PUT alone", async () => {
    const hub = new FakeHub();
    hub.forwards = [active(8080, 18080)];
    const { sharing } = setup(hub);
    await sharing.refresh();
    expect(await sharing.remember("vm1", 8080, 18080)).toBe(true);
    expect(hub.mutations()).toEqual([{ path: REVERSE_PREFERENCES_PATH, method: "PUT", body: { target: "vm1", port: 8080, local: 18080 } }]);
    expect(sharing.savedOf("vm1", 8080)).toEqual(rule(8080, 18080));
  });

  it("shows a failed restore only while the share is not active", async () => {
    const hub = new FakeHub();
    const lastError = { code: "host_port_conflict", message: "Port 8080 is in use on vm1." };
    hub.saved = [{ ...rule(8080), lastError }];
    const { sharing } = setup(hub);
    await sharing.refresh();
    expect(sharing.lists()[0].rows[0].error).toEqual(lastError);
    hub.forwards = [active(8080)];
    await sharing.refresh();
    expect(sharing.lists()[0].rows[0].error).toBeUndefined();
  });

  it("drops a failure once another window changes its mapping", async () => {
    const hub = new FakeHub();
    const { sharing } = setup(hub);
    hub.fail = { status: 409, body: { error: "In use.", code: "host_port_conflict" } };
    await sharing.share("vm1", 8080, 8080);
    await settle();
    expect(sharing.errorOf("vm1", 8080)).toBeDefined();
    await sharing.refresh();
    expect(sharing.errorOf("vm1", 8080)).toBeDefined();
    hub.forwards = [active(8080)];
    await sharing.refresh();
    expect(sharing.errorOf("vm1", 8080)).toBeUndefined();
  });

  it("drops a read that started before an action here", async () => {
    const hub = new FakeHub();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const fetch = hub.fetch;
    hub.fetch = async (path, init) => {
      if ((init?.method ?? "GET") === "GET" && !hub.requests.some((r) => r.method !== "GET")) {
        const res = await fetch(path, init);
        await gate;
        return res;
      }
      return fetch(path, init);
    };
    const { sharing } = setup(hub);
    const stale = sharing.refresh();
    await sharing.share("vm1", 8080, 8080);
    release();
    await stale;
    await settle();
    expect(sharing.forwardOf("vm1", 8080)).toEqual(active(8080));
  });

  it("polls while the view is open or something is shared or saved, and not otherwise", async () => {
    vi.useFakeTimers();
    const hub = new FakeHub();
    const { sharing } = setup(hub);
    await sharing.refresh();
    const reads = () => hub.requests.filter((r) => r.method === "GET").length;
    let before = reads();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(reads()).toBe(before);

    sharing.setViewOpen(true);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(reads()).toBeGreaterThan(before);
    sharing.setViewOpen(false);
    before = reads();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL * 2);
    expect(reads()).toBe(before);

    hub.saved = [rule(11434)];
    await sharing.refresh();
    before = reads();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL);
    expect(reads()).toBeGreaterThan(before);
    expect(hub.mutations()).toEqual([]);
  });
});
