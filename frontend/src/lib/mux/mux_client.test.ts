import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MuxClient, PaneSpawnRejectedError, PaneSpawnedTimeoutError, createScreenOp, openViewerOp, splitOp } from "./mux_client.ts";
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

  it("discards create_screen while disconnected instead of replaying it on the next welcome", () => {
    welcome();
    socket.drop();
    mux.sendOp("screen-c", createScreenOp("screen-c"));
    expect(socket.sentOfType("mux.op")).toEqual([]);

    welcome();
    expect(socket.sentOfType("mux.op")).toEqual([]);
    mux.sendOp("screen-d", createScreenOp("screen-d"));
    expect(socket.sentOfType("mux.op").map((m) => m.screenId)).toEqual(["screen-d"]);
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

  // viewer-panes, Viewer Capability (Host without viewers): the flag follows
  // every welcome; a refused open_viewer spawns nothing, so the screen resyncs.
  it("offers viewer panes only for a server that announces them, and resyncs a refused one", () => {
    welcome();
    expect(mux.viewers).toBe(false);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["scratchpad-v1", "viewer-v1"] });
    expect(mux.viewers).toBe(true);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["scratchpad-v1"] });
    expect(mux.viewers).toBe(false);

    socket.message({ type: "mux.op_failed", op: "open_viewer", error: "no screen" });
    expect(socket.sentOfType("mux.resync")).toEqual([]);
    socket.message({ type: "mux.op_failed", op: "open_viewer", screenId: "screen-1", paneId: "viewer-0123456789abcdef", error: "the path must be absolute" });
    expect(socket.sentOfType("mux.resync")).toEqual([{ type: "mux.resync", screenId: "screen-1" }]);
  });

  // viewer-annotations, Annotations Capability (Host without annotations): the
  // flag follows every welcome; a rejected annotation op goes to the listeners
  // and resyncs its screen, and rejects no spawn waiter of the same pane.
  it("offers annotations only for a server that announces them, and resyncs a rejected annotation op", async () => {
    welcome();
    expect(mux.annotations).toBe(false);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["viewer-v1", "viewer-annotations-v1"] });
    expect(mux.annotations).toBe(true);
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities: ["viewer-v1"] });
    expect(mux.viewers).toBe(true);
    expect(mux.annotations).toBe(false);

    const failures: unknown[] = [];
    const stop = mux.onOpFailed((failure) => failures.push(failure));
    let settled = false;
    const spawn = mux.waitForPaneSpawned("viewer-0123456789abcdef", 60_000);
    void spawn.then(() => { settled = true; }, () => { settled = true; });
    socket.message({ type: "mux.op_failed", op: "put_annotations", screenId: "screen-1", paneId: "viewer-0123456789abcdef", error: "pane viewer-0123456789abcdef holds 100 annotations" });
    socket.message({ type: "mux.op_failed", op: "remove_annotations", paneId: 7, error: "no screen" });
    expect(failures).toEqual([
      { op: "put_annotations", screenId: "screen-1", paneId: "viewer-0123456789abcdef", error: "pane viewer-0123456789abcdef holds 100 annotations" },
      { op: "remove_annotations", error: "no screen" },
    ]);
    expect(socket.sentOfType("mux.resync")).toEqual([{ type: "mux.resync", screenId: "screen-1" }]);
    await Promise.resolve();
    expect(settled).toBe(false);

    // The operations on threads go the same way.
    for (const op of ["put_replies", "remove_replies", "mark_sent"]) {
      socket.message({ type: "mux.op_failed", op, screenId: "screen-2", paneId: "viewer-0123456789abcdef", error: `${op} refused` });
    }
    expect(failures.slice(2)).toEqual(["put_replies", "remove_replies", "mark_sent"].map((op) => (
      { op, screenId: "screen-2", paneId: "viewer-0123456789abcdef", error: `${op} refused` }
    )));
    expect(socket.sentOfType("mux.resync").map((message) => message.screenId)).toEqual(["screen-1", "screen-2", "screen-2", "screen-2"]);
    await Promise.resolve();
    expect(settled).toBe(false);

    stop();
    socket.message({ type: "mux.op_failed", op: "put_annotations", screenId: "screen-1", paneId: "v1", error: "again" });
    expect(failures).toHaveLength(5);
    socket.message({ type: "mux.pane_spawned", paneId: "viewer-0123456789abcdef", sessionId: "" });
  });

  // viewer-annotations, Saving Annotations Through The Window: snapshots carry
  // a viewer pane's annotations, malformed entries dropped, absent while empty.
  it("keeps a viewer pane's annotations from a snapshot by the server's rules", () => {
    const seen: ScreenState[] = [];
    mux.setHandlers({ onSnapshot: (screen) => seen.push(screen) });
    const annotation = { id: "ann-0123456789abcdef", kind: "comment", body: "Looks good." };
    const viewer = (annotations: unknown) => ({ role: "viewer", viewer: { source: { kind: "file", path: "/home/me/app/PLAN.md" }, revision: 1, annotations } });
    const panes: Record<string, unknown> = {
      v1: viewer([annotation, { id: "ann-bad", kind: "comment", body: "x" }, { ...annotation, body: "same ID" }]),
      v2: viewer([]),
      v3: viewer("not a list"),
    };
    socket.message({ type: "mux.screen_snapshot", screen: { id: "screen-1", root: { type: "slot", id: "s1", paneIds: Object.keys(panes) }, panes } });
    const got = seen[0].panes;
    expect(got.v1.viewer!.annotations).toEqual([annotation]);
    expect(got.v2.viewer).not.toHaveProperty("annotations");
    expect(got.v3.viewer).not.toHaveProperty("annotations");
  });

  // viewer-annotations, Threads and Waiting For A Review: numbers, statuses,
  // hand-over marks, replies, the counter and the wait arrive checked;
  // malformed entries are dropped and the kinds of earlier builds are read.
  it("keeps the threads, the counter and the wait of a viewer record by the server's rules", () => {
    const seen: ScreenState[] = [];
    mux.setHandlers({ onSnapshot: (screen) => seen.push(screen) });
    const target = { view: "source", quote: "retry three times", prefix: "", suffix: "", startLine: 30, endLine: 30 };
    const agent = { id: "rep-00000000000000a1", author: "agent", body: "Done.", from: { paneId: "p1", title: "claude\u001b[2J" } };
    const user = { id: "rep-00000000000000a2", author: "user", body: "Thanks.", sent: true };
    const threads = [
      { id: "ann-0000000000000001", number: 1, kind: "comment", body: "Why?", target, status: "resolved", sent: true, replies: [agent, user] },
      { id: "ann-0000000000000002", number: 2, kind: "question", body: "Asked before threads.", target },
      { id: "ann-0000000000000003", number: 3, kind: "delete", target },
      { id: "ann-0000000000000004", number: -1, kind: "comment", body: "Bad number." },
      { id: "ann-0000000000000005", kind: "comment", body: "Bad status.", status: "done" },
    ];
    const record = (extra: Record<string, unknown>) => ({
      role: "viewer",
      viewer: { source: { kind: "text", title: "Plan" }, revision: 1, ...extra },
    });
    const wait = { id: "wait-0123456789abcdef", paneId: "p1", title: " claude\u0007 " };
    const panes: Record<string, unknown> = {
      v1: record({ annotations: threads, lastAnnotationNumber: 5, reviewWait: wait }),
      v2: record({ lastAnnotationNumber: 0, reviewWait: { ...wait, id: "wait-1" } }),
      v3: record({ lastAnnotationNumber: 1.5, reviewWait: { ...wait, paneId: "" } }),
      v4: record({ lastAnnotationNumber: "3", reviewWait: { ...wait, paneId: "p".repeat(257) } }),
      v5: record({ reviewWait: { id: wait.id, title: "claude" } }),
    };
    socket.message({ type: "mux.screen_snapshot", screen: { id: "screen-1", root: { type: "slot", id: "s1", paneIds: Object.keys(panes) }, panes } });
    const got = seen[0].panes;
    expect(got.v1.viewer).toEqual({
      source: { kind: "text", title: "Plan" },
      revision: 1,
      annotations: [
        { ...threads[0], replies: [{ ...agent, from: { paneId: "p1", title: "claude[2J" } }, user] },
        { id: "ann-0000000000000002", number: 2, kind: "comment", body: "Asked before threads.", target },
        { id: "ann-0000000000000003", number: 3, kind: "change", replacement: "", target },
      ],
      lastAnnotationNumber: 5,
      reviewWait: { id: wait.id, paneId: "p1", title: "claude" },
    });
    for (const paneId of ["v2", "v3", "v4", "v5"]) {
      expect(got[paneId].viewer, paneId).toEqual({ source: { kind: "text", title: "Plan" }, revision: 1 });
    }
  });

  // viewer-panes, Viewer Panes and Viewer Capability: a malformed record
  // leaves a viewer pane that says it cannot be shown; Document Text Outside
  // Snapshots: a text a snapshot carries anyway goes no further.
  it("keeps a viewer pane's record from a snapshot only when it is well formed", () => {
    const seen: ScreenState[] = [];
    mux.setHandlers({ onSnapshot: (screen) => seen.push(screen) });
    const file = (path: unknown) => ({ source: { kind: "file", path }, revision: 1 });
    const panes: Record<string, unknown> = {
      v1: { role: "viewer", title: "PLAN.md", cwd: "/home/me/app", viewer: { source: { kind: "file", path: "/home/me/app/PLAN.md" }, originPaneId: "p1", revision: 2, extra: 1 } },
      v2: { role: "viewer", viewer: { source: { kind: "text", title: "Selection from claude", text: "the selected text" }, revision: 1 } },
      plan: { role: "viewer", viewer: { source: { kind: "text", title: "Plan", format: "markdown" }, revision: 1 } },
      odd: { role: "viewer", viewer: { source: { kind: "text", title: "Notes", format: "html" }, revision: 1 } },
      relative: { role: "viewer", viewer: file("docs/plan.md") },
      dotted: { role: "viewer", viewer: file("/home/me/app/../secrets.txt") },
      control: { role: "viewer", viewer: file("/home/me/a\nb") },
      long: { role: "viewer", viewer: file(`/${"a".repeat(4096)}`) },
      url: { role: "viewer", viewer: { source: { kind: "url", url: "https://example.com" }, revision: 1 } },
      revision: { role: "viewer", viewer: { source: { kind: "file", path: "/home/me/a" }, revision: -1 } },
      fraction: { role: "viewer", viewer: { source: { kind: "file", path: "/home/me/a" }, revision: 1.5 } },
      untitled: { role: "viewer", viewer: { source: { kind: "text", title: "" }, revision: 1 } },
      titled: { role: "viewer", viewer: { source: { kind: "text", title: "x".repeat(201) }, revision: 1 } },
      missing: { role: "viewer" },
      p1: { role: "terminal", viewer: file("/home/me/a") },
    };
    socket.message({ type: "mux.screen_snapshot", screen: { id: "screen-1", root: { type: "slot", id: "s1", paneIds: Object.keys(panes) }, panes } });

    const got = seen[0].panes;
    expect(got.v1).toEqual({
      id: "v1", title: "PLAN.md", cwd: "/home/me/app", role: "viewer",
      viewer: { source: { kind: "file", path: "/home/me/app/PLAN.md" }, originPaneId: "p1", revision: 2 },
    });
    expect(got.v2.viewer).toEqual({ source: { kind: "text", title: "Selection from claude" }, revision: 1 });
    expect(got.plan.viewer).toEqual({ source: { kind: "text", title: "Plan", format: "markdown" }, revision: 1 });
    // Another format shows the text as plain text.
    expect(got.odd.viewer).toEqual({ source: { kind: "text", title: "Notes" }, revision: 1 });
    for (const id of ["relative", "dotted", "control", "long", "url", "revision", "fraction", "untitled", "titled", "missing"]) {
      expect(got[id].role, id).toBe("viewer");
      expect(got[id], id).not.toHaveProperty("viewer");
    }
    expect(got.p1).not.toHaveProperty("viewer");
  });

  // viewer-panes, Opening A Viewer: only the fields given are sent.
  it("builds open_viewer ops", () => {
    expect(openViewerOp("viewer-0123456789abcdef", { kind: "file", path: "/home/me/app/PLAN.md" }, { paneId: "p1", placement: "right" })).toEqual({
      type: "open_viewer", paneId: "p1", placement: "right", newPaneId: "viewer-0123456789abcdef", source: { kind: "file", path: "/home/me/app/PLAN.md" },
    });
    expect(openViewerOp("viewer-0123456789abcdef", { kind: "text", title: "Notes", text: "a" }, { cwd: "/home/me" })).toEqual({
      type: "open_viewer", newPaneId: "viewer-0123456789abcdef", source: { kind: "text", title: "Notes", text: "a" }, cwd: "/home/me",
    });
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

it("revokes pane-spawn waits on disconnect before a timeout can trigger a fallback create", async () => {
  const wait = mux.waitForPaneSpawned("p"); const failed = expect(wait).rejects.toThrow("outcome is unknown");
  socket.drop(); socket.message({ type: "mux.pane_spawned", paneId: "p", sessionId: "late" }); await failed;
});

it("gates template actions on the capability without a legacy spawn fallback", async () => {
  welcome();
  const op = { type: "apply_template" as const, template: { name: "T", panes: { a: {} }, layout: { slot: ["a"] } }, paneIds: { a: "p" } };
  expect(() => mux.sendOp("s", op)).toThrow("Update");
  await expect(mux.extractTemplate("s", "T")).rejects.toThrow("Update");
  expect(socket.sent).toEqual([]);
});
it("retains template spawn results until delayed mount and rejects every pending pane on rejection", async () => {
  socket.message({ type: "mux.welcome", capabilities: ["server-templates-v1"], screens: [] });
  mux.sendOp("s", { type: "apply_template", template: { name: "T", panes: { a: {}, b: {} }, layout: { slot: ["a", "b"] } }, paneIds: { a: "p", b: "q" } });
  socket.message({ type: "mux.pane_spawned", paneId: "p", sessionId: "session" });
  await expect(mux.waitForPaneSpawned("p")).resolves.toBe("session");
  const pending = mux.waitForPaneSpawned("q");
  socket.message({ type: "mux.op_failed", op: "apply_template", screenId: "s", error: "invalid template" });
  await expect(pending).rejects.toThrow("invalid template");
  expect(socket.sentOfType("mux.resync")).toEqual([{ type: "mux.resync", screenId: "s" }]);
});
it("extracts on its connection and propagates errors and disconnects", async () => {
  socket.message({ type: "mux.welcome", capabilities: ["server-templates-v1"], screens: [] });
  const first = mux.extractTemplate("s", "Saved", "/project");
  const request = socket.sentOfType("mux.extract_template")[0];
  expect(request).toMatchObject({ screenId: "s", name: "Saved", root: "/project" });
  socket.message({ type: "mux.template_extracted", requestId: request.requestId, error: "no tiled terminals" });
  await expect(first).rejects.toThrow("no tiled terminals");
  const second = mux.extractTemplate("s", "Saved");
  socket.drop();
  await expect(second).rejects.toThrow("Connection ended");
});
