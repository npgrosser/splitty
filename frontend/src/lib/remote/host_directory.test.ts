import { describe, expect, it, vi } from "vitest";
import { HostDirectory, type HostCatalog } from "./host_directory.ts";
import type { HostScreen } from "./remote_socket.ts";

const DIRECTORY = "/api/remote/directory";
const screen = (id = "s1", name = "API", panes = 1): HostScreen => ({ id, name, panes });
const entry = (target = "vm1", lastSeen = 100, screens = [screen()]): HostCatalog => ({ target, lastSeen, screens });
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { "Content-Type": "application/json" },
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

/** A window's directory over a hub that answers `hosts` (and `order`) for the merged directory. */
function setup(hosts: unknown[] = [], order?: unknown[]) {
  const observe = vi.fn();
  const changed = vi.fn();
  const fetch = vi.fn(async (path: string, init?: RequestInit) => {
    expect(path).toBe(DIRECTORY);
    expect(init?.method).toBeUndefined();
    return json({ version: 1, hosts, ...(order ? { order } : {}) });
  });
  const directory = new HostDirectory({ fetch, observe, changed });
  return { directory, fetch, observe, changed };
}

describe("a hub without hub-host-decisions-v1", () => {
  it("gives the window no directory: nothing is read, merged or written", async () => {
    const { directory, fetch, observe } = setup([entry()]);
    await directory.load();
    await directory.refresh();
    directory.observe("vm1", [screen()]);
    expect(fetch).not.toHaveBeenCalled();
    expect(observe).not.toHaveBeenCalled();
    expect(directory.mergedHosts()).toBeUndefined();
    expect(directory.entries()).toEqual([]);
    expect(directory.known()).toEqual([]);
  });
});

describe("hub-owned directory", () => {
  it("drops a previous hub's snapshot but keeps the last good one on a reconnect to the same kind", async () => {
    let hosts = [entry("first")];
    const fetch = vi.fn(async () => json({ version: 1, hosts }));
    const directory = new HostDirectory({ fetch, changed: vi.fn() });
    directory.useServerDirectory(true);
    await directory.refresh();
    expect(directory.mergedHosts()?.map((host) => host.target)).toEqual(["first"]);
    directory.useServerDirectory(true);
    expect(directory.mergedHosts()?.map((host) => host.target)).toEqual(["first"]);

    hosts = [entry("second")];
    directory.useServerDirectory(false);
    expect(directory.mergedHosts()).toBeUndefined();
    expect(directory.entries()).toEqual([]);
    directory.useServerDirectory(true);
    expect(directory.mergedHosts()).toEqual([]);
    await directory.refresh();
    expect(directory.entries().map((host) => host.target)).toEqual(["second"]);
  });

  it("reads the merged directory and sends ordered observations without persistence requests", async () => {
    const { directory, fetch, observe } = setup([{ ...entry(), alias: "Work" }], ["vm1"]);
    const order = vi.fn();
    const ordered = new HostDirectory({ fetch, observe, order, changed: vi.fn() });
    ordered.useServerDirectory(true);
    await ordered.load();
    expect(ordered.mergedHosts()).toEqual([{ ...entry(), alias: "Work" }]);
    expect(order).toHaveBeenCalledWith(["vm1"]);
    ordered.observe("vm1", [screen()]);
    ordered.observe("vm1", [screen()]);
    ordered.observe("vm1", [screen("renamed")]);
    ordered.observe("vm1", []);
    expect(observe.mock.calls).toEqual([["vm1", [screen()]], ["vm1", [screen("renamed")]], ["vm1", []]]);
    expect(fetch.mock.calls).toEqual([[DIRECTORY]]);
    ordered.useServerDirectory(true); // reconnect resends the current observation
    ordered.observe("vm1", []);
    expect(observe).toHaveBeenCalledTimes(4);
    directory.dispose();
  });

  it("discards reads overtaken by an invalidation or forgetting a host", async () => {
    const first = deferred<Response>(); const second = deferred<Response>();
    const fetch = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const directory = new HostDirectory({ fetch, changed: vi.fn() });
    directory.useServerDirectory(true);
    const old = directory.refresh(); const fresh = directory.refresh();
    second.resolve(json({ version: 1, hosts: [entry("fresh")] })); await fresh;
    first.resolve(json({ version: 1, hosts: [entry("old")] })); await old;
    expect(directory.mergedHosts()?.map(h => h.target)).toEqual(["fresh"]);
    const pending = deferred<Response>(); fetch.mockReturnValueOnce(pending.promise);
    const refresh = directory.refresh(); directory.setHidden("fresh", true);
    pending.resolve(json({ version: 1, hosts: [entry("fresh")] })); await refresh;
    expect(directory.mergedHosts()).toEqual([]);
  });

  it("discards invalid records, deduplicates ids per host, and treats names as plain text", async () => {
    const { directory } = setup([
      null, "bad", entry("-oProxyCommand=sh"), entry("vm1 "), entry("vm 1"), entry("vm‮1"),
      { ...entry(), screens: [
        null, {}, screen(""), screen("x".repeat(257)), screen("\ud800"),
        screen("negative", "bad", -1), screen("fraction", "bad", 1.5), screen("large", "bad", 1e20),
        { id: "bad-name", name: {}, panes: 1 },
        screen("s1", "<b>API</b>\n\u0000‮!", 2), screen("s1", "duplicate", 99),
        { id: "s2", panes: 0 },
      ] },
      entry("vm1", 300),
      entry("vm2", 200, [screen("s1", "same id, different host")]),
    ]);
    directory.useServerDirectory(true);
    await directory.load();
    expect(directory.entries()).toEqual([
      entry("vm2", 200, [screen("s1", "same id, different host")]),
      entry("vm1", 100, [screen("s1", "<b>API</b>!", 2), screen("s2", "", 0)]),
    ]);
  });

  it("accepts canonical SSH targets and refuses malformed targets on both reads and observations", async () => {
    const valid = ["vm1", "dev@api.example:2222", "[::1]", "user@[2001:db8::1]:22", "[::ffff:127.0.0.1]"];
    const invalid = ["vm1;id", "a@@vm1", "@vm1", "vm1:", "vm1:0", "vm1:65536", "vm1:22:33", "::1",
      "[::1]trailing", "[::1]:0", "[bad]", "[127.0.0.1]", "host..name", "host-", "a".repeat(64), "vm\ud800"];
    const { directory, observe } = setup([...valid, ...invalid].map((target) => entry(target)));
    directory.useServerDirectory(true);
    await directory.load();
    expect(directory.entries().map((e) => e.target).sort()).toEqual([...valid].sort());
    for (const target of invalid) directory.observe(target, []);
    expect(observe).not.toHaveBeenCalled();
    for (const target of valid) directory.observe(target, []);
    expect(observe.mock.calls.map(([target]) => target)).toEqual(valid);
  });

  it("lists the merged directory's hosts and does not expose mutable records", async () => {
    const { directory } = setup([
      { target: "vm1", source: "recent", lastConnected: 123, lastSeen: 100, screens: [screen()] },
      { target: "alias", source: "ssh-config", screens: [] },
    ]);
    directory.useServerDirectory(true);
    await directory.load();
    expect(directory.known()).toEqual([
      { target: "vm1", source: "recent", alias: undefined, lastConnected: 123 },
      { target: "alias", source: "ssh-config", alias: undefined, lastConnected: undefined },
    ]);
    directory.entries()[0].screens[0].name = "changed";
    directory.known()[0].target = "other";
    expect(directory.entries()).toEqual([entry()]);
    expect(directory.known()[0].target).toBe("vm1");
  });
});

describe("forgotten hosts", () => {
  it("sends no observation for a forgotten host until it is added again", async () => {
    const { directory, observe } = setup([entry()]);
    directory.useServerDirectory(true);
    await directory.load();
    directory.observe("vm1", [screen()]);
    directory.setHidden("vm1", true);
    expect(directory.entries()).toEqual([]);
    expect(directory.known()).toEqual([]);
    directory.observe("vm1", [screen("other")]);
    expect(observe).toHaveBeenCalledTimes(1);
    directory.setHidden("vm1", false);
    directory.observe("vm1", [screen()]);
    expect(observe).toHaveBeenCalledTimes(2);
  });

  it("does nothing once disposed", async () => {
    const { directory, fetch, observe } = setup([entry()]);
    directory.useServerDirectory(true);
    directory.dispose();
    await directory.refresh();
    directory.observe("vm1", [screen()]);
    expect(fetch).not.toHaveBeenCalled();
    expect(observe).not.toHaveBeenCalled();
  });
});
