import { describe, expect, it } from "vitest";
import { ScreenStartup, type StartupEffects } from "./screen_startup.ts";

/** Records every effect and keeps the local screen list like the store does. */
function harness(initial: string[] = ["screen-1"]) {
  const local = [...initial];
  const log: string[] = [];
  const fx: StartupEffects = {
    localScreenIds: () => [...local],
    createLocalScreen: (id) => { local.push(id); log.push(`create-local ${id}`); },
    removeLocalScreen: (id) => { local.splice(local.indexOf(id), 1); log.push(`remove-local ${id}`); },
    resync: (id) => log.push(`resync ${id}`),
    startFresh: (id) => log.push(`create_screen ${id}`),
  };
  const startup = new ScreenStartup(initial, fx);
  const created = () => log.filter((l) => l.startsWith("create_screen"));
  return { startup, local, log, created };
}

describe("ScreenStartup", () => {
  it("adopts a CLI job screen from its actual-ID nudge without creating another process", () => {
    const { startup, local, log, created } = harness();
    startup.welcome([{ id: "user", name: "User" }]);
    startup.nudge("job-screen");
    expect(log).toContain("resync job-screen");
    startup.snapshot("job-screen", "Backend");
    startup.viewReady("job-screen");
    expect(local).toEqual(["user", "job-screen"]);
    expect(created()).toEqual([]);
  });
  // mux-protocol, Page reload against an existing workspace: the placeholder's
  // view is ready before the welcome, the server already has screen-1.
  it("adopts the server's screens on a reload instead of re-creating them", () => {
    const { startup, log, created, local } = harness();
    startup.viewReady("screen-1");
    startup.welcome([{ id: "screen-1", name: "Default" }, { id: "screen-b", name: "logs" }]);
    startup.viewReady("screen-b");

    expect(created()).toEqual([]);
    expect(log).toContain("resync screen-1");
    expect(log).toContain("resync screen-b");
    expect(local).toEqual(["screen-1", "screen-b"]);
  });

  // workspace-model, Move to a new screen: the move op creates it on the server.
  it("sends no create_screen for a screen a moved pane creates", () => {
    const { startup, created, local } = harness();
    startup.welcome([{ id: "screen-1", name: "Default" }]);
    startup.expect("screen-moved");
    local.push("screen-moved");
    startup.viewReady("screen-moved");

    expect(created()).toEqual([]);
    expect(local).toEqual(["screen-1", "screen-moved"]);
  });

  it("removes a placeholder the server does not list", () => {
    const { startup, created, local } = harness();
    startup.viewReady("screen-1");
    startup.welcome([{ id: "screen-b", name: "logs" }]);

    expect(created()).toEqual([]);
    expect(local).toEqual(["screen-b"]);
  });

  it("removes an unlisted placeholder whose view was not ready yet", () => {
    const { startup, created, local } = harness();
    startup.welcome([{ id: "screen-b", name: "logs" }]);
    startup.viewReady("screen-b");

    expect(created()).toEqual([]);
    expect(local).toEqual(["screen-b"]);
  });

  // mux-protocol, First start against an empty workspace.
  it("creates exactly one screen when the server has none", () => {
    const { startup, created } = harness();
    startup.viewReady("screen-1");
    expect(created()).toEqual([]);

    startup.welcome([]);
    expect(created()).toEqual(["create_screen screen-1"]);
  });

  it("starts the placeholder when its view gets ready after an empty welcome", () => {
    const { startup, created } = harness();
    startup.welcome([]);
    startup.viewReady("screen-1");
    expect(created()).toEqual(["create_screen screen-1"]);
  });

  it("sends nothing before the welcome", () => {
    const { startup, log } = harness();
    startup.viewReady("screen-1");
    expect(log).toEqual([]);
  });

  // A listed screen whose view is ready before its snapshot waits for it.
  it("waits for the snapshot of a listed screen", () => {
    const { startup, created } = harness();
    startup.welcome([{ id: "screen-x", name: "x" }]);
    startup.viewReady("screen-x");
    startup.snapshot("screen-x", "x");
    expect(created()).toEqual([]);
  });

  it("treats a screen first seen in a snapshot as the server's", () => {
    const { startup, created, local } = harness();
    startup.welcome([]);
    startup.viewReady("screen-1");
    startup.snapshot("screen-z", "from another client");
    startup.viewReady("screen-z");
    expect(local).toContain("screen-z");
    expect(created()).toEqual(["create_screen screen-1"]);
  });

  it("still starts screens the user creates after the welcome", () => {
    const { startup, created } = harness();
    startup.viewReady("screen-1");
    startup.welcome([{ id: "screen-1", name: "Default" }]);
    startup.viewReady("screen-2");
    expect(created()).toEqual(["create_screen screen-2"]);
  });

  // mux-protocol, Reconnect restores state.
  it("resyncs every listed screen on a reconnect", () => {
    const { startup, log, created } = harness();
    startup.viewReady("screen-1");
    startup.welcome([]);
    log.length = 0;

    startup.welcome([{ id: "screen-1" }, { id: "screen-2" }]);
    expect(log.filter((l) => l.startsWith("resync"))).toEqual(["resync screen-1", "resync screen-2"]);
    expect(created()).toEqual([]);
  });

  it("keeps screens on a reconnect even when a later welcome omits them", () => {
    const { startup, local } = harness();
    startup.viewReady("screen-1");
    startup.welcome([]);
    startup.welcome([{ id: "screen-9" }]);
    expect(local).toContain("screen-1");
  });

  // mux-protocol, Nudge triggers resync.
  it("resyncs a nudged screen", () => {
    const { startup, log } = harness();
    startup.nudge("screen-1");
    startup.nudge(undefined);
    expect(log).toEqual(["resync screen-1"]);
  });
});
