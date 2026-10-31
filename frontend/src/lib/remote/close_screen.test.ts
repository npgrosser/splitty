import { describe, expect, it } from "vitest";
import { fakeEmulator, fakeServer } from "../testing/fake_server.ts";
import { ScreenViews } from "../connection/screen_sync.ts";
import { TerminalSession } from "../terminal/terminal_session.ts";
import { RemoteScreenSync } from "../mux/remote_screen_sync.ts";
import type { ConnKey } from "../connection/server_connection.ts";
import type { Screen } from "../../stores/screens.svelte.ts";
import {
  detachRemoteScreen, rememberTerminateConfirmed, removeRemoteScreen, terminateConfirmedBefore,
  terminateRemoteScreen, type CloseEffects,
} from "./close_screen.ts";

/**
 * A window with a local screen and the host screen `api` (`s1` on vm1) whose
 * two panes show `sess-a` (created) and `sess-b` (attached). Removing a
 * screen does what the workspace does: its binding goes first, then dockview
 * closes the layout's panels, whose renderers send `close_pane` through the
 * binding and release their sessions.
 */
async function world(options: { onlyRemote?: boolean } = {}) {
  const local = fakeServer("local");
  const host = fakeServer("host:vm1", "vm1");
  const screens: Screen[] = [
    ...(options.onlyRemote ? [] : [{ id: "screen-1", name: "Default", conn: "local" as const, wireId: "screen-1" }]),
    { id: "rs-a", name: "api", conn: "host:vm1", wireId: "s1" },
  ];
  const views = new ScreenViews(() => {});
  for (const s of screens) views.bind(s, s.conn === "local" ? local.conn : host.conn, false);

  const created = new TerminalSession(host.conn.sessions, "sess-a", fakeEmulator());
  created.wire();
  const pending = host.conn.sessions.attach("sess-b", 80, 24);
  host.socket.message({ type: "attached", sessionId: "sess-b" });
  const attached = new TerminalSession(host.conn.sessions, "sess-b", fakeEmulator());
  attached.wire(await pending);
  const panes = new Map([["terminal-a", created], ["terminal-b", attached]]);

  const sync = new RemoteScreenSync(host.conn, {
    screens: () => screens,
    snapshot: () => {},
    startFresh: () => {},
    changed: () => {},
  });
  sync.ready([{ id: "s1", name: "api", panes: 2 }, { id: "s2", name: "infra", panes: 1 }]);
  host.socket.message({ type: "mux.welcome", clientId: "c1", screens: [{ id: "s1" }, { id: "s2" }] });
  host.socket.sent = [];

  const log: string[] = [];
  const fx: CloseEffects = {
    screens: () => screens,
    createLocalScreen: () => {
      log.push("create local");
      screens.push({ id: "screen-2", name: "Screen 2", conn: "local", wireId: "screen-2" });
    },
    removeScreen: (key) => {
      log.push(`remove ${key}`);
      const binding = views.binding(key);
      views.remove(key);
      for (const [paneId, session] of key === "rs-a" ? panes : []) {
        binding?.sendOp({ type: "close_pane", paneId });
        binding?.release(session);
      }
      screens.splice(screens.findIndex((s) => s.id === key), 1);
    },
    connection: (key: ConnKey) => (key === "local" ? local.conn : key === "host:vm1" ? host.conn : undefined),
    sync: (key: ConnKey) => (key === "host:vm1" ? sync : undefined),
    panes: (key) => (key === "rs-a" ? panes.size : 0),
    releaseHost: (key) => log.push(`release ${key}`),
  };
  return { local, host, screens, sync, fx, log };
}

describe("closing a remote screen", () => {
  it("collapses by detaching only: nothing that ends a session reaches the host", async () => {
    const w = await world();
    detachRemoteScreen("rs-a", w.fx);

    expect(w.host.socket.sent).toEqual([{ type: "detach", id: "sess-a" }, { type: "detach", id: "sess-b" }]);
    expect(w.local.socket.sent).toEqual([]);
    expect(w.screens.map((s) => s.id)).toEqual(["screen-1"]);
    expect(w.log).toEqual(["remove rs-a", "release host:vm1"]);
    // It keeps running there, and the reopen offer lists it.
    expect(w.sync.available()).toEqual([
      { id: "s1", name: "api", panes: 2 },
      { id: "s2", name: "infra", panes: 1 },
    ]);
  });

  it("terminates with exactly one close_screen, under the host's id of the screen", async () => {
    const w = await world();
    expect(terminateRemoteScreen("rs-a", w.fx)).toBe(true);

    const ops = w.host.socket.sentOfType("mux.op");
    expect(ops).toEqual([{ type: "mux.op", screenId: "s1", op: { type: "close_screen", screenId: "s1" } }]);
    expect(w.host.socket.sent.filter((m) => m.type !== "mux.op" && m.type !== "detach")).toEqual([]);
    expect(w.local.socket.sent).toEqual([]);
    expect(w.sync.available().map((s) => s.id)).toEqual(["s2"]);
  });

  it("creates a local screen first when it is the window's last", async () => {
    const w = await world({ onlyRemote: true });
    detachRemoteScreen("rs-a", w.fx);

    expect(w.log).toEqual(["create local", "remove rs-a", "release host:vm1"]);
    expect(w.screens.map((s) => s.conn)).toEqual(["local"]);
  });

  it("changes nothing when terminating without the host's connection", async () => {
    const w = await world();
    w.fx.connection = () => undefined;
    expect(terminateRemoteScreen("rs-a", w.fx)).toBe(false);
    expect(w.screens).toHaveLength(2);
    expect(w.host.socket.sent).toEqual([]);
  });

  it("removes a screen the host never had without recording it, and leaves local screens alone", async () => {
    const w = await world();
    removeRemoteScreen("screen-1", w.fx);
    expect(w.screens).toHaveLength(2);

    removeRemoteScreen("rs-a", w.fx);
    expect(w.host.socket.sentOfType("mux.op")).toEqual([]);
    expect(w.screens.map((s) => s.id)).toEqual(["screen-1"]);
  });
});

describe("the first terminate", () => {
  it("asks until a terminate was confirmed once", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => { store.set(k, v); } };
    expect(terminateConfirmedBefore(storage)).toBe(false);
    rememberTerminateConfirmed(storage);
    expect(terminateConfirmedBefore(storage)).toBe(true);

    const broken = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); } };
    expect(terminateConfirmedBefore(broken)).toBe(false);
    expect(() => rememberTerminateConfirmed(broken)).not.toThrow();
  });
});
