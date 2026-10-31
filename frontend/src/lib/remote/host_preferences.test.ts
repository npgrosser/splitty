import { describe, expect, it, vi } from "vitest";
import { HostPreferences, parseHostPreference } from "./host_preferences.ts";

const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
function setup() {
  let rows: { target: string; alias?: string; hidden?: boolean }[] = [];
  const fetch = vi.fn(async (_path: string, init?: RequestInit): Promise<Response> => {
    if (!init?.method) return json({ version: 1, hosts: rows });
    const body = JSON.parse(String(init.body));
    const existing = rows.find((row) => row.target === body.target) ?? { target: body.target };
    const next = init.method === "DELETE" ? { target: body.target, hidden: true } : { ...existing, ...body };
    if (!next.alias) delete next.alias;
    if (!next.hidden) delete next.hidden;
    rows = [...rows.filter((row) => row.target !== body.target), next];
    return init.method === "DELETE" ? new Response(null, { status: 204 }) : json(next);
  });
  const visibility = vi.fn();
  const changed = vi.fn();
  return { preferences: new HostPreferences({ fetch, visibility, changed }), fetch, visibility, changed };
}

describe("host display preferences", () => {
  it("keeps display aliases separate from the SSH address and clears optional aliases", async () => {
    const { preferences, fetch } = setup();
    await preferences.setAlias("me@192.0.2.44", "  Home Lab  ");
    expect(preferences.entries()).toEqual([{ target: "me@192.0.2.44", alias: "Home Lab" }]);
    expect(JSON.parse(String(fetch.mock.calls[0][1]?.body))).toEqual({ target: "me@192.0.2.44", alias: "Home Lab" });
    await preferences.setAlias("me@192.0.2.44", " ");
    expect(preferences.entries()).toEqual([{ target: "me@192.0.2.44" }]);
  });

  it("removes durably before hiding and requires an explicit reveal to re-add", async () => {
    const { preferences, visibility, fetch } = setup();
    await preferences.setAlias("vm1", "VM");
    await preferences.forget("vm1");
    expect(preferences.isHidden("vm1")).toBe(true);
    expect(visibility).toHaveBeenLastCalledWith("vm1", true);
    await preferences.load();
    expect(preferences.entries()).toEqual([{ target: "vm1", hidden: true }]);
    await preferences.reveal("vm1");
    expect(preferences.isHidden("vm1")).toBe(false);
    expect(visibility).toHaveBeenLastCalledWith("vm1", false);
    expect(fetch.mock.calls.find(([, init]) => init?.method === "DELETE")?.[0]).toBe("/api/remote/hosts");
  });

  it("preserves visible data and exposes failed writes to the editor", async () => {
    const { preferences, fetch, visibility } = setup();
    await preferences.setAlias("vm1", "Work");
    fetch.mockResolvedValueOnce(json({ error: "cannot write" }, 500));
    await expect(preferences.forget("vm1")).rejects.toThrow("500");
    expect(preferences.entries()).toEqual([{ target: "vm1", alias: "Work" }]);
    expect(visibility).not.toHaveBeenCalledWith("vm1", true);
    fetch.mockResolvedValueOnce(json({ error: "cannot write" }, 500));
    await expect(preferences.setAlias("vm1", "New name")).rejects.toThrow("500");
    expect(preferences.entries()[0].alias).toBe("Work");
  });

  it("does not let a slow initial read undo a removal", async () => {
    const { preferences, fetch } = setup();
    let finish!: (value: Response) => void;
    fetch.mockImplementationOnce(() => new Promise<Response>((resolve) => { finish = resolve; }));
    const reading = preferences.load();
    await preferences.forget("vm1");
    finish(json({ version: 1, hosts: [{ target: "vm1", alias: "Old name" }] }));
    await reading;
    expect(preferences.entries()).toEqual([{ target: "vm1", hidden: true }]);
  });

  it("does not let a read begun during a save replace its confirmed result", async () => {
    const { preferences, fetch } = setup();
    let save!: (value: Response) => void;
    let read!: (value: Response) => void;
    fetch.mockImplementationOnce(() => new Promise<Response>((resolve) => { save = resolve; }));
    const saving = preferences.setAlias("vm1", "New");
    fetch.mockImplementationOnce(() => new Promise<Response>((resolve) => { read = resolve; }));
    const reading = preferences.load();
    save(json({ target: "vm1", alias: "New" }));
    await saving;
    read(json({ version: 1, hosts: [{ target: "vm1", alias: "Old" }] }));
    await reading;
    expect(preferences.entries()[0].alias).toBe("New");
  });

  it("validates aliases before sending and accepts Unicode names", async () => {
    const { preferences, fetch } = setup();
    await expect(preferences.setAlias("vm1", "a".repeat(201))).rejects.toThrow("200");
    await expect(preferences.setAlias("vm1", "bad\u0000name")).rejects.toThrow("control");
    expect(fetch).not.toHaveBeenCalled();
    await preferences.setAlias("vm1", "😀".repeat(200));
    expect(preferences.entries()[0].alias).toBe("😀".repeat(200));
    expect(parseHostPreference({ target: "-bad", alias: "Work" })).toBeUndefined();
  });
});

it("merges untouched hidden hosts from a late load after another host was edited", async () => {
  const { preferences, fetch } = setup();
  let finish!: (value: Response) => void;
  fetch.mockImplementationOnce(() => new Promise<Response>((resolve) => { finish = resolve; }));
  const loading = preferences.load();
  await preferences.setAlias("vm1", "New");
  finish(json({ version: 1, hosts: [{ target: "vm1", alias: "Old" }, { target: "vm2", hidden: true }] }));
  await loading;
  expect(preferences.isHidden("vm2")).toBe(true);
  expect(preferences.entries().find((row) => row.target === "vm1")?.alias).toBe("New");
});

it("explicitly reveals a target before a slow initial load has identified it as hidden", async () => {
  const { preferences, fetch } = setup();
  let finish!: (value: Response) => void;
  fetch.mockImplementationOnce(() => new Promise<Response>((resolve) => { finish = resolve; }));
  const loading = preferences.load();
  await preferences.reveal("vm1");
  finish(json({ version: 1, hosts: [{ target: "vm1", hidden: true }] }));
  await loading;
  expect(preferences.isHidden("vm1")).toBe(false);
  expect(JSON.parse(String(fetch.mock.calls[1][1]?.body))).toEqual({ target: "vm1", hidden: false });
});

it("orders writes for the same host while allowing another host to proceed", async () => {
  const { preferences, fetch } = setup();
  let finish!: (value: Response) => void;
  fetch.mockImplementationOnce(() => new Promise<Response>((resolve) => { finish = resolve; }));
  const saving = preferences.setAlias("vm1", "Work");
  const forgetting = preferences.forget("vm1");
  await preferences.setAlias("vm2", "Other");
  expect(fetch.mock.calls).toHaveLength(2);
  expect(fetch.mock.calls.some(([, init]) => init?.method === "DELETE")).toBe(false);
  finish(json({ target: "vm1", alias: "Work" }));
  await Promise.all([saving, forgetting]);
  expect(preferences.isHidden("vm1")).toBe(true);
  expect(preferences.entries().find((row) => row.target === "vm2")?.alias).toBe("Other");
});
