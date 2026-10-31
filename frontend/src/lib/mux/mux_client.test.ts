import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MuxClient, PaneSpawnRejectedError, PaneSpawnedTimeoutError, createScreenOp, splitOp } from "./mux_client.ts";
import { FakeSocket } from "../testing/fake_server.ts";
import type { ScreenInfo, ScreenState } from "../protocol/types.ts";

let socket: FakeSocket;
let mux: MuxClient;

beforeEach(() => {
  socket = new FakeSocket();
  mux = new MuxClient(socket);
});

function welcome(screens: unknown = []) {
  socket.message({ type: "mux.welcome", clientId: "c1", screens, serverMode: "daemon" });
}

describe("MuxClient", () => {
  // mux-protocol, Client Resynchronization
  it("holds create_screen until the welcome, then sends it in order", () => {
    mux.sendOp("screen-a", createScreenOp("screen-a", "A", "terminal-a"));
    mux.sendOp("screen-x", { type: "rename_screen", screenId: "screen-x", name: "X" });
    mux.sendOp("screen-b", createScreenOp("screen-b", "B", "terminal-b"));

    expect(socket.sentOfType("mux.op").map((m) => m.op.type)).toEqual(["rename_screen"]);

    welcome();

    expect(socket.sentOfType("mux.op").map((m) => m.screenId)).toEqual(["screen-x", "screen-a", "screen-b"]);
  });

  it("holds create_screen again after the connection dropped, until the next welcome", () => {
    welcome();
    socket.drop();
    mux.sendOp("screen-c", createScreenOp("screen-c"));
    expect(socket.sentOfType("mux.op")).toEqual([]);

    welcome();
    expect(socket.sentOfType("mux.op").map((m) => m.screenId)).toEqual(["screen-c"]);
  });

  it("keeps the welcome's state and hands its screens to the handler", () => {
    const seen: ScreenInfo[][] = [];
    mux.setHandlers({ onWelcome: (_id, screens) => seen.push(screens) });

    welcome([{ id: "screen-1", name: "Work" }]);

    expect(mux.clientId).toBe("c1");
    expect(mux.serverMode).toBe("daemon");
    expect(seen).toEqual([[{ id: "screen-1", name: "Work" }]]);
  });

  it("resolves a spawn waiter with the session of its pane only", async () => {
    const pending = mux.waitForPaneSpawned("terminal-1");
    socket.message({ type: "mux.pane_spawned", paneId: "terminal-2", sessionId: "sess-2" });
    socket.message({ type: "mux.pane_spawned", paneId: "terminal-1", sessionId: "sess-1" });
    await expect(pending).resolves.toBe("sess-1");
  });

  // workspace-model, Identifier Assignment: Pane ID already in use
  it("rejects a spawn waiter when the server reports its op failed", async () => {
    const pending = mux.waitForPaneSpawned("terminal-1");
    socket.message({ type: "mux.op_failed", op: "split_pane", paneId: "terminal-1", error: "pane terminal-1 already exists" });
    await expect(pending).rejects.toBeInstanceOf(PaneSpawnRejectedError);
  });

  describe("timeout", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it("rejects a spawn waiter nobody answers", async () => {
      const pending = mux.waitForPaneSpawned("terminal-1", 1000);
      const assertion = expect(pending).rejects.toBeInstanceOf(PaneSpawnedTimeoutError);
      vi.advanceTimersByTime(1000);
      await assertion;
    });
  });

  it("checks a snapshot before handing it on", () => {
    const seen: ScreenState[] = [];
    mux.setHandlers({ onSnapshot: (screen) => seen.push(screen) });

    // Missing panes, sessions and floating; a split with a bad child.
    socket.message({
      type: "mux.screen_snapshot",
      screen: {
        id: "screen-1",
        root: { type: "split", id: "n1", axis: "vertical", children: [{ type: "slot", id: "s1", paneIds: ["terminal-1", 7] }, 42] },
      },
    });
    // No layout: dropped.
    socket.message({ type: "mux.screen_snapshot", screen: { id: "screen-2" } });
    socket.message({ type: "mux.screen_snapshot", screen: null });

    expect(seen).toHaveLength(1);
    expect(seen[0].panes).toEqual({});
    expect(seen[0].sessions).toEqual({});
    expect(seen[0].floating).toEqual([]);
    expect(seen[0].root.children).toEqual([{ type: "slot", id: "s1", paneIds: ["terminal-1"], activePaneId: undefined }]);
  });

  // session-scratchpad, Scratchpad Capability (Host without scratchpads)
  it("offers scratchpads only for a server that announces them", () => {
    welcome();
    expect(mux.scratchpads).toBe(false);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["managed-jobs-v1", "scratchpad-v1"] });
    expect(mux.scratchpads).toBe(true);
  });

  // mux-protocol, Window Layout Changes Reach the Server (Host without the
  // capability, Rejected arrangement).
  it("takes arrangements only from a server that announces them, and resyncs a rejected one", () => {
    welcome();
    expect(mux.arrangements).toBe(false);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["scratchpad-v1", "layout-arrange-v1"] });
    expect(mux.arrangements).toBe(true);

    socket.message({ type: "mux.op_failed", op: "split_pane", screenId: "screen-1", error: "pane not found" });
    socket.message({ type: "mux.op_failed", op: "arrange_panes", error: "no screen" });
    expect(socket.sentOfType("mux.resync")).toEqual([]);
    socket.message({ type: "mux.op_failed", op: "arrange_panes", screenId: "screen-1", error: "the arrangement misses pane p3" });
    expect(socket.sentOfType("mux.resync")).toEqual([{ type: "mux.resync", screenId: "screen-1" }]);
  });

  // mux-protocol, Selected Tabs Reach the Server (Host without the capability).
  it("takes selected tabs only from a server that announces them", () => {
    welcome();
    expect(mux.tabSelections).toBe(false);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["layout-arrange-v1", "tab-select-v1"] });
    expect(mux.tabSelections).toBe(true);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["layout-arrange-v1"] });
    expect(mux.tabSelections).toBe(false);
  });

  // session-scratchpad, Saving Through The Window: the text keeps its lines, bounded.
  it("keeps a session's scratchpad from a snapshot, bounded", () => {
    const seen: ScreenState[] = [];
    mux.setHandlers({ onSnapshot: (screen) => seen.push(screen) });
    const screen = (scratchpad: unknown) => ({
      id: "screen-1",
      root: { type: "slot", id: "s1", paneIds: ["terminal-1"] },
      sessions: { "sess-1": { status: "running", scratchpad } },
    });
    socket.message({ type: "mux.screen_snapshot", screen: screen("first\n\tsecond") });
    socket.message({ type: "mux.screen_snapshot", screen: screen("x".repeat(70_000)) });
    socket.message({ type: "mux.screen_snapshot", screen: screen(7) });
    expect(seen[0].sessions["sess-1"].scratchpad).toBe("first\n\tsecond");
    expect(seen[1].sessions["sess-1"].scratchpad).toHaveLength(64 * 1024);
    expect(seen[2].sessions["sess-1"]).not.toHaveProperty("scratchpad");
  });

  it("treats a welcome without a screen list as an empty one", () => {
    const seen: ScreenInfo[][] = [];
    mux.setHandlers({ onWelcome: (_id, screens) => seen.push(screens) });
    welcome(null);
    welcome([{ name: "no id" }, { id: "screen-1", name: 5 }]);
    expect(seen).toEqual([[], [{ id: "screen-1", name: undefined }]]);
  });

  it("builds split ops on the axis of the direction", () => {
    expect(splitOp("a", "b", "right")).toMatchObject({ type: "split_pane", axis: "horizontal", newPaneId: "b" });
    expect(splitOp("a", "b", "below")).toMatchObject({ type: "split_pane", axis: "vertical" });
  });

  it("stops listening when disposed", () => {
    const seen: string[] = [];
    mux.setHandlers({ onNudge: (reason) => seen.push(reason) });
    mux.dispose();
    socket.message({ type: "mux.nudge", reason: "late", screenId: "screen-1" });
    expect(seen).toEqual([]);
  });
});
