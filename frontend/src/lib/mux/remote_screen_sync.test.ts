import { describe, expect, it } from "vitest";
import { fakeServer, type FakeSocket } from "../testing/fake_server.ts";
import type { ServerConnection } from "../connection/server_connection.ts";
import type { ScreenState } from "../protocol/types.ts";
import { connectScreenSync } from "../connection/screen_sync.ts";
import { RemoteScreenSync, type RemoteScreenRef } from "./remote_screen_sync.ts";

interface Rig {
  socket: FakeSocket;
  conn: ServerConnection;
  sync: RemoteScreenSync;
  screens: RemoteScreenRef[];
  /** Screens whose first pane and `create_screen` were requested. */
  fresh: string[];
  applied: string[];
  changes: number;
}

/** A host connection with the window's screens `screens`; one local screen as well. */
function rig(screens: RemoteScreenRef[] = []): Rig {
  const { socket, conn } = fakeServer("host:vm1", "vm1");
  const r: Rig = {
    socket, conn, screens: [{ id: "screen-local", name: "Default", conn: "local", wireId: "screen-local" }, ...screens],
    fresh: [], applied: [], changes: 0,
  } as unknown as Rig;
  r.sync = new RemoteScreenSync(conn, {
    screens: () => r.screens,
    snapshot: (key, state) => r.applied.push(`${key}:${state.id}`),
    startFresh: (key) => {
      r.fresh.push(key);
      const screen = r.screens.find((s) => s.id === key);
      if (screen) conn.mux.sendOp(screen.wireId, { type: "create_screen", screenId: screen.wireId, newPaneId: "terminal-new" });
    },
    changed: () => r.changes++,
  });
  connectScreenSync(conn, r.sync);
  return r;
}

const NAMES: Record<string, string> = { s1: "api", s2: "infra", "screen-new": "Screen 2" };

const welcome = (r: Rig, ids: string[]) =>
  r.socket.message({ type: "mux.welcome", clientId: "c1", screens: ids.map((id) => ({ id, name: NAMES[id] })) });

const snapshot = (id: string, name = "api"): Partial<ScreenState> => ({
  id, name, revision: 1,
  root: { type: "slot", id: "root", paneIds: ["terminal-1"] },
  floating: [], panes: { "terminal-1": { id: "terminal-1", sessionId: "sess-1" } }, sessions: {},
});

const resyncs = (r: Rig) => r.socket.sentOfType("mux.resync").map((m) => m.screenId);
const creates = (r: Rig) => r.socket.sentOfType("mux.op").filter((m) => m.op.type === "create_screen").map((m) => m.screenId);

describe("RemoteScreenSync", () => {
  it("routes job snapshots only to open host screens and discovers new ones on refresh", () => {
    const r = rig([{ id: "remote-view", name: "api", conn: "host:vm1", wireId: "s1" }]);
    welcome(r, ["s1"]);
    const jobSnapshot = (id: string) => ({ ...snapshot(id),
      panes: { "terminal-1": { id: "terminal-1", sessionId: "sess-1", jobId: "job" } },
      jobs: { job: { jobId: "job", state: "running" } } });
    r.socket.message({ type: "mux.screen_snapshot", screen: jobSnapshot("screen-local") });
    r.socket.message({ type: "mux.screen_snapshot", screen: jobSnapshot("new-job-screen") });
    expect(r.applied).toEqual([]);
    r.socket.message({ type: "mux.screen_snapshot", screen: jobSnapshot("s1") });
    expect(r.applied).toEqual(["remote-view:s1"]);
    welcome(r, ["s1", "new-job-screen"]);
    expect(r.sync.available().map(s => s.id)).toEqual(["new-job-screen"]);
    expect(r.screens).toHaveLength(2);
    expect(creates(r)).toEqual([]);
  });
  it("creates nothing from a welcome or a snapshot", () => {
    const r = rig();
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }]);
    welcome(r, ["s1", "s2"]);
    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("s1") });
    r.socket.message({ type: "mux.nudge", reason: "cli_op", screenId: "s2" });

    expect(r.fresh).toEqual([]);
    expect(r.applied).toEqual([]);
    expect(r.socket.sent).toEqual([]);
    expect(r.screens).toHaveLength(1);
  });

  it("resyncs the open screens on every welcome, also after a reconnect", () => {
    const r = rig([{ id: "rs-a", name: "api", conn: "host:vm1", wireId: "s1" }]);
    welcome(r, ["s1", "s2"]);
    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("s1") });
    expect(resyncs(r)).toEqual(["s1"]);
    expect(r.applied).toEqual(["rs-a:s1"]);

    // A view that becomes ready after the welcome needs no second resync.
    r.sync.viewReady("rs-a");
    expect(resyncs(r)).toEqual(["s1"]);

    r.socket.drop();
    welcome(r, ["s1"]);
    expect(resyncs(r)).toEqual(["s1", "s1"]);
  });

  it("resyncs an opened screen when its view becomes ready after the welcome", () => {
    const r = rig();
    welcome(r, ["s1"]);
    r.screens.push({ id: "rs-a", name: "api", conn: "host:vm1", wireId: "s1" });
    r.sync.viewReady("rs-a");
    expect(resyncs(r)).toEqual(["s1"]);
  });

  it("flags a screen bound after the welcome that the host did not list (a remembered screen)", () => {
    const r = rig();
    welcome(r, ["s1"]);
    r.screens.push({ id: "rs-gone", name: "old", conn: "host:vm1", wireId: "s7" });
    r.sync.viewReady("rs-gone");
    expect(r.sync.isMissing("rs-gone")).toBe(true);
    expect(resyncs(r)).toEqual(["s7"]);

    // The host had it after all.
    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("s7", "old") });
    expect(r.sync.isMissing("rs-gone")).toBe(false);
  });

  // workspace-model, Move to a new screen on a host: the move op creates it.
  it("neither flags nor creates a screen a moved pane creates", () => {
    const r = rig();
    welcome(r, ["s1"]);
    r.sync.adopt("s9", "Screen 2");
    r.screens.push({ id: "rs-moved", name: "Screen 2", conn: "host:vm1", wireId: "s9" });
    r.sync.viewReady("rs-moved");

    expect(r.sync.isMissing("rs-moved")).toBe(false);
    expect(creates(r)).toEqual([]);
    expect(r.fresh).toEqual([]);
  });

  it("flags open screens the host no longer lists, until a snapshot shows them again", () => {
    const r = rig([
      { id: "rs-a", name: "api", conn: "host:vm1", wireId: "s1" },
      { id: "rs-b", name: "infra", conn: "host:vm1", wireId: "s2" },
    ]);
    welcome(r, ["s1"]);
    expect(r.sync.isMissing("rs-a")).toBe(false);
    expect(r.sync.isMissing("rs-b")).toBe(true);
    expect(resyncs(r)).toEqual(["s1"]);

    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("s2") });
    expect(r.sync.isMissing("rs-b")).toBe(false);
  });

  it("sends create_screen exactly once for a new screen: after the welcome and its view, never again on reconnect", () => {
    const r = rig();
    r.screens.push({ id: "rs-new", name: "Screen 2", conn: "host:vm1", wireId: "screen-new" });
    r.sync.addNew("rs-new");
    r.sync.viewReady("rs-new");
    expect(creates(r)).toEqual([]);

    r.sync.ready([]);
    welcome(r, []);
    expect(r.fresh).toEqual(["rs-new"]);
    expect(creates(r)).toEqual(["screen-new"]);
    expect(r.sync.isPending("rs-new")).toBe(false);

    r.socket.drop();
    r.sync.ready([{ id: "screen-new", name: "Screen 2", panes: 1 }]);
    welcome(r, ["screen-new"]);
    r.sync.viewReady("rs-new");
    expect(creates(r)).toEqual(["screen-new"]);
    expect(resyncs(r)).toEqual(["screen-new"]);
  });

  it("waits for the answer to the reopen offer before creating a new screen", () => {
    const r = rig();
    r.screens.push({ id: "rs-new", name: "Screen 2", conn: "host:vm1", wireId: "screen-new" });
    r.sync.addNew("rs-new");
    r.sync.viewReady("rs-new");
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }, { id: "s2", name: "infra", panes: 1 }]);
    welcome(r, ["s1", "s2"]);

    expect(r.sync.needsChoice("rs-new")).toBe(true);
    expect(r.sync.available()).toEqual([
      { id: "s1", name: "api", panes: 3 },
      { id: "s2", name: "infra", panes: 1 },
    ]);
    expect(creates(r)).toEqual([]);

    r.sync.decide("rs-new");
    expect(r.sync.needsChoice("rs-new")).toBe(false);
    expect(creates(r)).toEqual(["screen-new"]);
    r.sync.decide("rs-new");
    expect(creates(r)).toEqual(["screen-new"]);
  });

  it("honors explicit New Screen in a host directory before connecting, without reopening other screens", () => {
    const r = rig([{ id: "rs-new", name: "Screen 2", conn: "host:vm1", wireId: "screen-new" }]);
    r.sync.addNew("rs-new");
    r.sync.decide("rs-new");
    r.sync.viewReady("rs-new");
    expect(creates(r)).toEqual([]);
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }]);
    welcome(r, ["s1"]);
    expect(creates(r)).toEqual(["screen-new"]);
    expect(r.sync.needsChoice("rs-new")).toBe(false);
    expect(r.sync.available()).toEqual([{ id: "s1", name: "api", panes: 3 }]);
    expect(resyncs(r)).toEqual([]);
  });

  it("creates a new screen whose offer lost its last entry to another new screen", () => {
    const r = rig();
    r.screens.push({ id: "rs-1", name: "Screen 2", conn: "host:vm1", wireId: "screen-n1" });
    r.screens.push({ id: "rs-2", name: "Screen 3", conn: "host:vm1", wireId: "screen-n2" });
    r.sync.addNew("rs-1");
    r.sync.addNew("rs-2");
    r.sync.viewReady("rs-1");
    r.sync.viewReady("rs-2");
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }]);
    welcome(r, ["s1"]);
    expect(r.sync.needsChoice("rs-1") && r.sync.needsChoice("rs-2")).toBe(true);

    // The second new screen opens `api` in its place.
    r.screens.splice(r.screens.findIndex((s) => s.id === "rs-2"), 1, { id: "rs-api", name: "api", conn: "host:vm1", wireId: "s1" });
    r.sync.forget("rs-2");
    r.sync.refresh();
    expect(creates(r)).toEqual(["screen-n1"]);
  });

  it("keeps the host's screens the window does not show available, detached ones included", () => {
    const r = rig([{ id: "rs-a", name: "api", conn: "host:vm1", wireId: "s1" }]);
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }, { id: "s2", name: "infra", panes: 1 }]);
    welcome(r, ["s1", "s2"]);
    expect(r.sync.available().map((s) => s.id)).toEqual(["s2"]);

    // Detach: the screen leaves the window and keeps running on the host.
    r.sync.detached("rs-a", 4);
    r.screens.splice(1, 1);
    expect(r.sync.available()).toEqual([
      { id: "s1", name: "api", panes: 4 },
      { id: "s2", name: "infra", panes: 1 },
    ]);

    r.sync.terminated("s2");
    expect(r.sync.available().map((s) => s.id)).toEqual(["s1"]);
  });

  it("takes host names as data: a welcome entry without a name is listed by its id", () => {
    const r = rig();
    r.socket.message({ type: "mux.welcome", clientId: "c1", screens: [{ id: "s1" }, { id: 5 }, null] });
    expect(r.sync.available()).toEqual([{ id: "s1", name: "s1", panes: 0 }]);
  });

  it("requests a fresh snapshot when a detached screen is reopened on the same connection", () => {
    const r = rig([{ id: "rs-a", name: "api", conn: "host:vm1", wireId: "s1" }]);
    welcome(r, ["s1"]);
    r.sync.viewReady("rs-a");
    expect(resyncs(r)).toEqual(["s1"]);
    r.sync.detached("rs-a", 1);
    r.screens.splice(1, 1);
    r.screens.push({ id: "rs-reopened", name: "api", conn: "host:vm1", wireId: "s1" });
    r.sync.viewReady("rs-reopened");
    r.sync.viewReady("rs-reopened");
    expect(resyncs(r)).toEqual(["s1", "s1"]);
    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("s1") });
    expect(r.applied).toEqual(["rs-reopened:s1"]);
    expect(creates(r)).toEqual([]);
  });

  it("does not publish an unobserved directory as empty, but a verified empty welcome replaces old entries", () => {
    const r = rig();
    expect(r.sync.catalog()).toBeUndefined();
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }]);
    expect(r.sync.catalog()).toEqual([{ id: "s1", name: "api", panes: 3 }]);
    r.socket.drop();
    expect(r.sync.catalog()).toHaveLength(1);
    welcome(r, []);
    expect(r.sync.catalog()).toEqual([]);
    expect(creates(r)).toEqual([]);
  });

  it("keeps open-screen names and pane counts in the directory through detach, but removes terminated screens", () => {
    const r = rig([{ id: "rs-a", name: "api", conn: "host:vm1", wireId: "s1" }]);
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }]);
    welcome(r, ["s1"]);
    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("s1", "Backend") });
    expect(r.sync.catalog()).toEqual([{ id: "s1", name: "Backend", panes: 1 }]);

    r.screens[1] = { ...r.screens[1], name: "Web application" };
    r.sync.updateOpen("rs-a", 2);
    expect(r.sync.catalog()).toEqual([{ id: "s1", name: "Web application", panes: 2 }]);
    r.sync.detached("rs-a", 2);
    r.screens.splice(1, 1);
    expect(r.sync.catalog()).toEqual(r.sync.available());
    const copy = r.sync.catalog()!;
    copy[0].name = "must not mutate sync";
    expect(r.sync.catalog()?.[0].name).toBe("Web application");
    r.sync.terminated("s1");
    expect(r.sync.catalog()).toEqual([]);
  });

  it("never adds another connection's snapshots or unconfirmed local names to the catalog", () => {
    const r = rig([{ id: "rs-new", name: "new", conn: "host:vm1", wireId: "s1" }]);
    r.sync.addNew("rs-new");
    r.sync.ready([{ id: "s1", name: "api", panes: 3 }]);
    r.sync.updateOpen("rs-new", 9);
    r.sync.updateOpen("screen-local", 9);
    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("screen-local") });
    r.socket.message({ type: "mux.screen_snapshot", screen: snapshot("unknown") });
    expect(r.sync.catalog()).toEqual([{ id: "s1", name: "api", panes: 3 }]);
  });
});
