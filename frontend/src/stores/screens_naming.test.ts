import { beforeEach, describe, expect, it, vi } from "vitest";

let store: typeof import("./screens.svelte.ts");

beforeEach(async () => {
  vi.resetModules();
  store = await import("./screens.svelte.ts");
});

describe("default screen names", () => {
  it("discovers a background job screen without changing this window's selected screen", () => {
    const selected = store.getActiveScreenId();
    store.createScreen("Backend", "job-screen", { activate: false });
    expect(store.getActiveScreenId()).toBe(selected);
    expect(store.getScreens().some(screen => screen.id === "job-screen")).toBe(true);
    store.setActiveScreen("job-screen");
    store.createScreen("Another backend", "job-screen-2", { activate: false });
    expect(store.getActiveScreenId()).toBe("job-screen");
  });
  // workspace-model, Move to a new screen: the server places it after the source.
  it("places a local screen after a given one", () => {
    const first = store.getScreens()[0].id;
    store.createScreen("last", "screen-last");
    store.createScreen("moved", "screen-moved", { activate: false, after: first });
    expect(store.getScreens().map((s) => s.id)).toEqual([first, "screen-moved", "screen-last"]);
  });
  it("starts fresh local screens at Screen 1", () => {
    expect(store.getScreens().map((s) => s.name)).toEqual(["Screen 1"]);
    expect(store.createScreen().name).toBe("Screen 2");
  });

  it("numbers local and each remote host independently", () => {
    store.createScreen();
    expect(store.addRemoteScreen("host:vm1", "a").name).toBe("Screen 1");
    expect(store.addRemoteScreen("host:vm2", "a").name).toBe("Screen 1");
    expect(store.addRemoteScreen("host:vm1", "b").name).toBe("Screen 2");
    expect(store.createScreen().name).toBe("Screen 3");
    expect(store.addRemoteScreen("host:vm2", "b").name).toBe("Screen 2");
  });

  it("reuses the lowest number a closed screen freed within its connection", () => {
    const remote = store.addRemoteScreen("host:vm1", "a");
    store.addRemoteScreen("host:vm1", "b");
    const local = store.createScreen();
    store.createScreen();

    store.removeScreen(remote.id);
    expect(store.addRemoteScreen("host:vm1", "c").name).toBe("Screen 1");
    store.removeScreen(local.id);
    expect(store.createScreen().name).toBe("Screen 2");
  });

  it("skips unopened names known on the same host", () => {
    store.addRemoteScreen("host:vm1", "a");
    store.addRemoteScreen("host:vm2", "a", { name: "Screen 3" });
    expect(store.nextScreenName("host:vm1", ["Screen 2", "Screen 4"])).toBe("Screen 3");
    expect(store.nextScreenName("host:vm2", ["Screen 1", "Screen 2"])).toBe("Screen 4");
    expect(store.nextScreenName()).toBe("Screen 2");
  });

  it("preserves restored and custom names, including the old Default name", () => {
    const original = store.getScreens()[0];
    store.createScreen("Default", "restored-local");
    store.removeScreen(original.id);
    store.createScreen("Work", "named-local");
    store.addRemoteScreen("host:vm1", "restored", { name: "Screen 7" });
    store.addRemoteScreen("host:vm1", "named", { name: "Logs" });

    expect(store.getScreens().map((s) => s.name)).toEqual(["Default", "Work", "Screen 7", "Logs"]);
    expect(store.createScreen().name).toBe("Screen 1");
    expect(store.addRemoteScreen("host:vm1", "new").name).toBe("Screen 1");
  });
});
