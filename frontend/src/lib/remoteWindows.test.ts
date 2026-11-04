import { describe, it, expect, vi, beforeAll, afterAll, beforeEach } from "vitest";
import type { RemoteProgress } from "./remoteWindows.ts";

// Tauri's `listen`: keeps the handler so a test can emit, and hands out an
// unlisten function per registration.
const tauriEvents = vi.hoisted(() => ({
  handler: null as ((event: { payload: unknown }) => void) | null,
  unlisten: [] as (() => void)[],
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn((name: string, handler: (event: { payload: unknown }) => void) => {
    if (name === "remote-progress") tauriEvents.handler = handler;
    const unlisten = vi.fn();
    tauriEvents.unlisten.push(unlisten);
    return Promise.resolve(unlisten);
  }),
}));

// `platform.ts` reads the page's globals when it loads.
vi.stubGlobal("window", { location: { search: "" }, matchMedia: () => ({ matches: false }) });
const { createRemoteWindows, toOpenError, parseKnownHosts } = await import("./remoteWindows.ts");
vi.unstubAllGlobals();

const hostsJson = [
  { target: "vm1", source: "recent", lastConnected: 1_790_000_000 },
  { target: "api.devpod", source: "ssh-config" },
];

describe("the Tauri bridge", () => {
  let invoke: ReturnType<typeof vi.fn>;
  const bridge = () => createRemoteWindows("tauri", { __SPLITTY_TAURI__: { invoke } });

  beforeEach(() => {
    invoke = vi.fn(() => Promise.resolve());
    tauriEvents.handler = null;
    tauriEvents.unlisten = [];
  });

  it("is available", () => {
    expect(bridge().available).toBe(true);
  });

  it("lists the known hosts with remote_hosts", async () => {
    invoke.mockResolvedValueOnce(hostsJson);

    expect(await bridge().hosts()).toEqual(hostsJson);
    expect(invoke).toHaveBeenCalledWith("remote_hosts", undefined);
  });

  it("opens and cancels with the target as an argument", async () => {
    await bridge().open("vm1");
    await bridge().cancel("vm1");

    expect(invoke).toHaveBeenCalledWith("remote_open", { target: "vm1" });
    expect(invoke).toHaveBeenCalledWith("remote_cancel", { target: "vm1" });
  });

  it("sends confirmed update intent once and preserves the native offer", async () => {
    const migration = { available: true, fromVersion: "0.1.27", toVersion: "0.2.0-dev.2", sessions: 2 };
    invoke.mockRejectedValueOnce({ message: "Update required", migration: { ...migration, token: "not-renderer-authority" } });
    const api = bridge();
    await expect(api.open("vm1")).rejects.toEqual({ message: "Update required", migration });
    await api.open("vm1", { update: true });
    await api.open("vm1");
    expect(invoke.mock.calls).toEqual([
      ["remote_open", { target: "vm1" }], ["remote_open", { target: "vm1", update: true }], ["remote_open", { target: "vm1" }],
    ]);
  });

  it("rejects an open with the command's error, hint included", async () => {
    invoke.mockRejectedValueOnce({ message: "Host key verification failed.", hint: "Run `ssh vm1` once in a terminal." });

    await expect(bridge().open("vm1")).rejects.toEqual({
      message: "Host key verification failed.",
      hint: "Run `ssh vm1` once in a terminal.",
    });
  });

  it("rejects an open with a plain string error as its message", async () => {
    invoke.mockRejectedValueOnce("not a valid target");

    await expect(bridge().open("-oProxyCommand=x")).rejects.toEqual({ message: "not a valid target" });
  });

  it("polls only pending targets and stops delivering after unsubscribe", async () => {
    vi.useFakeTimers();
    let finish!: () => void;
    const progress = { target: "vm1", step: "install", message: "Installing Splitty on vm1" };
    invoke.mockImplementation((command: string) => command === "remote_open"
      ? new Promise<void>(resolve => { finish = resolve; }) : Promise.resolve(progress));
    const api = bridge(), seen: RemoteProgress[] = [];
    const unsubscribe = api.onProgress(value => seen.push(value));
    const opening = api.open("vm1");
    await vi.advanceTimersByTimeAsync(150);
    expect(seen).toEqual([progress]);
    await vi.advanceTimersByTimeAsync(150);
    expect(seen).toHaveLength(1);
    unsubscribe(); finish(); await opening;
    const count = invoke.mock.calls.length;
    await vi.advanceTimersByTimeAsync(500);
    expect(invoke.mock.calls).toHaveLength(count);
    vi.useRealTimers();
  });

  it("allows unsubscribe before an open without registering event queues", () => {
    const api = bridge(), seen = vi.fn();
    api.onProgress(seen)();
    expect(tauriEvents.unlisten).toHaveLength(0);
    expect(seen).not.toHaveBeenCalled();
  });
});

describe("the Electron bridge", () => {
  let invoke: ReturnType<typeof vi.fn>;
  let progressHandler: ((progress: unknown) => void) | null;
  let unsubscribe: ReturnType<typeof vi.fn>;
  const bridge = () => createRemoteWindows("electron", {
    __SPLITTY_ELECTRON__: {
      invoke,
      onRemoteProgress: (handler: (progress: unknown) => void) => {
        progressHandler = handler;
        return unsubscribe;
      },
    },
  });

  beforeEach(() => {
    invoke = vi.fn(() => Promise.resolve({ ok: true }));
    progressHandler = null;
    unsubscribe = vi.fn();
  });

  it("uses the remote channels", async () => {
    invoke.mockResolvedValueOnce(hostsJson);

    expect(await bridge().hosts()).toEqual(hostsJson);
    await bridge().open("vm1");
    await bridge().cancel("vm1");

    expect(invoke.mock.calls).toEqual([["remote:hosts"], ["remote:open", "vm1"], ["remote:cancel", "vm1"]]);
  });

  it("passes one explicit confirmation and keeps migration metadata in the result", async () => {
    const migration = { available: true, fromVersion: "0.1.30", toVersion: "0.2.0-dev.2", sessions: 1 };
    invoke.mockResolvedValueOnce({ ok: false, error: { message: "Update required", migration } });
    const api = bridge();
    await expect(api.open("vm1")).rejects.toEqual({ message: "Update required", migration });
    await api.open("vm1", { update: true });
    await api.open("vm1");
    expect(invoke.mock.calls).toEqual([["remote:open", "vm1"], ["remote:open", "vm1", { update: true }], ["remote:open", "vm1"]]);
  });

  /// `ipcRenderer.invoke` rejections lose their fields, so the main process
  /// resolves with `{ ok: false, error }` and the bridge turns that into the
  /// rejection the contract names.
  it("rejects an open the main process answered with an error", async () => {
    invoke.mockResolvedValueOnce({ ok: false, error: { message: "ssh: connect to host vm1: timed out", hint: "Check the host name." } });

    await expect(bridge().open("vm1")).rejects.toEqual({
      message: "ssh: connect to host vm1: timed out",
      hint: "Check the host name.",
    });
  });

  it("rejects a cancelled open with its message", async () => {
    invoke.mockResolvedValueOnce({ ok: false, error: { message: "cancelled" } });

    await expect(bridge().open("vm1")).rejects.toEqual({ message: "cancelled" });
  });

  it("rejects an open whose channel failed", async () => {
    invoke.mockRejectedValueOnce(new Error("IPC channel not allowed: remote:open"));

    await expect(bridge().open("vm1")).rejects.toEqual({ message: "IPC channel not allowed: remote:open" });
  });

  it("delivers well-formed progress and unsubscribes", () => {
    const seen: RemoteProgress[] = [];
    const off = bridge().onProgress((progress) => seen.push(progress));

    progressHandler!({ target: "vm1", step: "tunnel", message: "Opening the tunnel" });
    progressHandler!("garbage");
    off();

    expect(seen).toEqual([{ target: "vm1", step: "tunnel", message: "Opening the tunnel" }]);
    expect(unsubscribe).toHaveBeenCalledOnce();
  });

  it("is unavailable with a preload that predates remote windows", () => {
    const old = createRemoteWindows("electron", { __SPLITTY_ELECTRON__: { invoke } });

    expect(old.available).toBe(false);
  });
});

describe("outside a desktop shell", () => {
  it("is unavailable in a browser", async () => {
    const web = createRemoteWindows("web", {});

    expect(web.available).toBe(false);
    expect(await web.hosts()).toEqual([]);
    await expect(web.open("vm1")).rejects.toHaveProperty("message");
  });

  describe("in a remote session", () => {
    beforeAll(() => {
      vi.stubGlobal("window", {
        location: { search: "?host=vm1" },
        matchMedia: () => ({ matches: false }),
        __SPLITTY_TAURI__: { invoke: vi.fn() },
      });
    });
    afterAll(() => {
      vi.unstubAllGlobals();
    });

    /// access-control, Remote Window Isolation: the host's UI in a remote
    /// window offers no way to open further hosts.
    it("is unavailable, although Tauri's bridge is injected", async () => {
      vi.resetModules();
      const { remoteWindows } = await import("./remoteWindows.ts");

      expect(remoteWindows.available).toBe(false);
    });
  });
});

describe("toOpenError", () => {
  it("keeps a message and a non-empty hint", () => {
    expect(toOpenError({ message: "failed", hint: "" })).toEqual({ message: "failed" });
    expect(toOpenError(new Error("boom"))).toEqual({ message: "boom" });
  });

  it("falls back to a generic message", () => {
    expect(toOpenError(undefined).message).toBeTruthy();
    expect(toOpenError({ hint: "only a hint" }).message).toBeTruthy();
  });
});

describe("parseKnownHosts", () => {
  it("drops entries that do not have the documented shape", () => {
    expect(parseKnownHosts([
      { target: "vm1", source: "recent", lastConnected: 5 },
      { target: "vm2", source: "elsewhere" },
      { source: "recent" },
      null,
      { target: "api.devpod", source: "ssh-config", lastConnected: "yesterday" },
    ])).toEqual([
      { target: "vm1", source: "recent", lastConnected: 5 },
      { target: "api.devpod", source: "ssh-config" },
    ]);
    expect(parseKnownHosts({ hosts: [] })).toEqual([]);
  });
});
