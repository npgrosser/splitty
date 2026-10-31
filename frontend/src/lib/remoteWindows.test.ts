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
  const bridge = () => createRemoteWindows("tauri", { __TAURI_INTERNALS__: { invoke } });

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

  it("delivers progress until unsubscribed", async () => {
    const seen: RemoteProgress[] = [];
    const unsubscribe = bridge().onProgress((progress) => seen.push(progress));
    await vi.waitFor(() => expect(tauriEvents.handler).not.toBeNull());

    tauriEvents.handler!({ payload: { target: "vm1", step: "install", message: "Installing Splitty on vm1" } });
    tauriEvents.handler!({ payload: { target: "vm1" } });
    unsubscribe();

    expect(seen).toEqual([{ target: "vm1", step: "install", message: "Installing Splitty on vm1" }]);
    await vi.waitFor(() => expect(tauriEvents.unlisten[0]).toHaveBeenCalledOnce());
  });

  it("drops the listener when unsubscribed before it was registered", async () => {
    const unsubscribe = bridge().onProgress(() => {});
    unsubscribe();

    await vi.waitFor(() => expect(tauriEvents.unlisten).toHaveLength(1));
    await vi.waitFor(() => expect(tauriEvents.unlisten[0]).toHaveBeenCalledOnce());
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
        __TAURI_INTERNALS__: { invoke: vi.fn() },
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
