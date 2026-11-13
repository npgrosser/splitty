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
  // mux-protocol, Server-Chosen Screen Names: a new screen shows a placeholder
  // that is never sent, until its server names it.
  it("gives new screens a placeholder until their server names them", () => {
    const first = store.getScreens()[0];
    expect(first).toMatchObject({ name: store.PLACEHOLDER_NAME, provisional: true });
    const local = store.createScreen();
    const remote = store.addRemoteScreen("host:vm1", "a");
    expect(local.provisional).toBe(true);
    expect(remote).toMatchObject({ name: store.PLACEHOLDER_NAME, provisional: true });
    expect(store.nameToSend(local.id, true)).toBeUndefined();

    store.setScreenName(local.id, "Screen 2");
    const named = store.getScreens().find((s) => s.id === local.id)!;
    expect(named.name).toBe("Screen 2");
    expect(named.provisional).toBeUndefined();
    expect(store.nameToSend(local.id, true)).toBe("Screen 2");
  });

  it("names a placeholder for an older server when its create goes out", () => {
    store.setScreenName(store.getScreens()[0].id, "Screen 1");
    const local = store.createScreen();
    expect(store.nameToSend(local.id, false)).toBe("Screen 2");
    expect(store.getScreens().find((s) => s.id === local.id)).toMatchObject({ name: "Screen 2" });
    // Placeholders reserve no name; known names on the same host do.
    store.addRemoteScreen("host:vm1", "pending");
    const remote = store.addRemoteScreen("host:vm1", "b");
    expect(store.nameToSend(remote.id, false, ["Screen 1", "Screen 3"])).toBe("Screen 2");
    expect(store.nextScreenName("host:vm2")).toBe("Screen 1");
  });

  it("keeps a name the user gave or a template brought", () => {
    const named = store.createScreen("Work");
    expect(named.provisional).toBeUndefined();
    expect(store.nameToSend(named.id, true)).toBe("Work");
    const remote = store.addRemoteScreen("host:vm1", "a");
    store.renameScreen(remote.id, "Logs");
    expect(store.nameToSend(remote.id, true)).toBe("Logs");
  });

  it("preserves restored and custom names, including the old Default name", () => {
    const original = store.getScreens()[0];
    store.createScreen("Default", "restored-local");
    store.removeScreen(original.id);
    store.createScreen("Work", "named-local");
    store.addRemoteScreen("host:vm1", "restored", { name: "Screen 7" });
    store.addRemoteScreen("host:vm1", "named", { name: "Logs" });

    expect(store.getScreens().map((s) => s.name)).toEqual(["Default", "Work", "Screen 7", "Logs"]);
    expect(store.nextScreenName()).toBe("Screen 1");
    expect(store.nextScreenName("host:vm1")).toBe("Screen 1");
  });
});
