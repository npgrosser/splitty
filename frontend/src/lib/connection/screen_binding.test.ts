import { afterEach, describe, expect, it, vi } from "vitest";
import type { DockviewApi } from "dockview-core";
import { ScreenBinding, bindingForApi, bridgeDockviewToMux, registerScreenApi, unregisterScreenApi } from "./screen_binding.ts";
import { fakeEmulator, fakeServer } from "../testing/fake_server.ts";
import { TerminalSession } from "../terminal/terminal_session.ts";
import type { ServerConnection } from "./server_connection.ts";
import { layoutArrangement } from "../../layout/arrangement.ts";

/** One session wired by `create`, one by `attach`, as panes of a screen have them. */
async function twoSessions(conn: ServerConnection, socket: ReturnType<typeof fakeServer>["socket"]) {
  const created = new TerminalSession(conn.sessions, "sess-a", fakeEmulator());
  created.wire();

  const pending = conn.sessions.attach("sess-b", 80, 24);
  socket.message({ type: "attached", sessionId: "sess-b" });
  const attached = new TerminalSession(conn.sessions, "sess-b", fakeEmulator());
  attached.wire(await pending);
  return [created, attached];
}

describe("ScreenBinding", () => {
  // The snapshot flag used to be global and dropped every screen's ops.
  it("holds back layout ops of its own screen only during a snapshot", () => {
    const { socket, conn } = fakeServer("local");
    const one = new ScreenBinding("screen-1", conn, "screen-1");
    const two = new ScreenBinding("screen-2", conn, "screen-2");

    one.suppressOps = true;
    one.sendOp({ type: "close_pane", paneId: "terminal-1" });
    two.sendOp({ type: "close_pane", paneId: "terminal-2" });

    expect(socket.sentOfType("mux.op")).toEqual([
      { type: "mux.op", screenId: "screen-2", op: { type: "close_pane", paneId: "terminal-2" } },
    ]);
  });

  it("sends ops under the screen's id on its server", () => {
    const { socket, conn } = fakeServer("host:vm1");
    const binding = new ScreenBinding("rs-0123", conn, "screen-1");
    binding.sendOp({ type: "close_pane", paneId: "terminal-1" });
    binding.resync();
    expect(socket.sent.map((m) => ("screenId" in m ? m.screenId : undefined))).toEqual(["screen-1", "screen-1"]);
  });

  it("keeps floating ids per screen", () => {
    const { conn } = fakeServer("local");
    const one = new ScreenBinding("screen-1", conn, "screen-1");
    const two = new ScreenBinding("screen-2", conn, "screen-2");
    one.registerFloatingId("group-1", "float-1");
    two.clearFloatingIds();
    expect(one.floatingId("group-1")).toBe("float-1");
    expect(two.floatingId("group-1")).toBeUndefined();
  });

  // Dockview gives group header actions and the watermark API wrappers of
  // their own; a lookup by object missed them, and their new panes were
  // spawned without the server's workspace knowing them.
  it("is found from any API wrapper of its layout", () => {
    const { conn } = fakeServer("local");
    const binding = new ScreenBinding("screen-1", conn, "screen-1");
    const wrapper = (id: string) => ({ id }) as unknown as DockviewApi;

    registerScreenApi(wrapper("dv-test-1"), binding);
    expect(bindingForApi(wrapper("dv-test-1"))).toBe(binding);
    expect(bindingForApi(wrapper("dv-test-2"))).toBeUndefined();

    unregisterScreenApi(wrapper("dv-test-1"));
    expect(bindingForApi(wrapper("dv-test-1"))).toBeUndefined();
  });

  it("calls its activation listeners when the screen becomes active, and the others when it stops", () => {
    const { conn } = fakeServer("local");
    const binding = new ScreenBinding("screen-1", conn, "screen-1");
    let activations = 0;
    let deactivations = 0;
    binding.onActivate(() => activations++);
    binding.onDeactivate(() => deactivations++);
    binding.setActive(true);
    binding.setActive(true);
    binding.setActive(false);
    binding.setActive(false);
    expect(binding.isActive()).toBe(false);
    expect(activations).toBe(1);
    expect(deactivations).toBe(1);
  });

  describe("teardown", () => {
    it("closes the sessions of a local screen, as before", async () => {
      const { socket, conn } = fakeServer("local");
      const binding = new ScreenBinding("screen-1", conn, "screen-1");
      expect(binding.teardown).toBe("close");
      const sessions = await twoSessions(conn, socket);
      socket.sent = [];

      for (const session of sessions) binding.release(session);

      expect(socket.sent).toEqual([{ type: "close", id: "sess-a" }, { type: "close", id: "sess-b" }]);
    });

    // remote-screens D7: collapsing a remote screen must not end anything on the host.
    it("only detaches when the layout of a detach screen is removed", async () => {
      const { socket, conn } = fakeServer("host:vm1");
      const binding = new ScreenBinding("rs-0123", conn, "screen-1");
      expect(binding.teardown).toBe("detach");
      const sessions = await twoSessions(conn, socket);
      socket.sent = [];

      // Removing the screen: the binding goes first, then dockview closes the panels.
      binding.dispose();
      binding.sendOp({ type: "close_pane", paneId: "terminal-a" });
      binding.sendOp({ type: "close_screen", screenId: "screen-1" });
      for (const session of sessions) binding.release(session);

      expect(socket.sent).toEqual([{ type: "detach", id: "sess-a" }, { type: "detach", id: "sess-b" }]);
      expect(conn.sessions.get("sess-a")).toBeUndefined();
    });

    // workspace-model, Moving a Pane to Another Screen: the pane's panel leaves
    // this screen, its session keeps running in the target.
    it("lets go of a handed-over pane without close_pane and only detaches its session", async () => {
      const { socket, conn } = fakeServer("local");
      const binding = new ScreenBinding("screen-1", conn, "screen-1");
      const removed: ((e: { id: string }) => void)[] = [];
      const api = {
        onDidRemovePanel: (cb: (e: { id: string }) => void) => { removed.push(cb); return { dispose() {} }; },
        onDidMovePanel: () => ({ dispose() {} }),
        onDidActivePanelChange: () => ({ dispose() {} }),
      } as unknown as DockviewApi;
      bridgeDockviewToMux(binding, api);
      const [moved, staying] = await twoSessions(conn, socket);
      socket.sent = [];

      binding.handOver("terminal-a", "sess-a");
      for (const cb of removed) cb({ id: "terminal-a" });
      binding.release(moved);
      // Only once: a later panel with the same id closes as usual.
      for (const cb of removed) cb({ id: "terminal-a" });
      binding.release(staying);

      expect(socket.sent).toEqual([
        { type: "detach", id: "sess-a" },
        { type: "mux.op", screenId: "screen-1", op: { type: "close_pane", paneId: "terminal-a" } },
        { type: "close", id: "sess-b" },
      ]);
    });
  });
});

const leaf = (views: string[], size: number, activeView = views[0]) =>
  ({ type: "leaf", data: { views, activeView, id: views[0] }, size });

/**
 * A screen whose grid is `p1` and `p2` as tabs on top of `p3`: what the user
 * made by dragging `p2` into `p1`'s group. Each group shows its first tab.
 */
function bridgedScreen(capabilities = ["layout-arrange-v1"]) {
  vi.useFakeTimers();
  const { socket, conn } = fakeServer("host:vm1");
  socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities });
  const binding = new ScreenBinding("rs-1", conn, "screen-1");
  const moved: (() => void)[] = [];
  const activated: (() => void)[] = [];
  const removed: ((e: { id: string }) => void)[] = [];
  const layout = {
    maximized: false,
    groups: [
      { panels: ["p1", "p2"], shown: "p1", location: "grid" },
      { panels: ["p3"], shown: "p3", location: "grid" },
    ],
  };
  const api = {
    onDidRemovePanel: (cb: (e: { id: string }) => void) => { removed.push(cb); return { dispose() {} }; },
    onDidMovePanel: (cb: () => void) => { moved.push(cb); return { dispose() {} }; },
    onDidActivePanelChange: (cb: () => void) => { activated.push(cb); return { dispose() {} }; },
    hasMaximizedGroup: () => layout.maximized,
    get groups() {
      return layout.groups.map((group) => ({
        panels: group.panels.map((id) => ({ id })),
        activePanel: { id: group.shown },
        api: { location: { type: group.location } },
      }));
    },
    toJSON: () => ({ grid: {
      orientation: "VERTICAL",
      root: { type: "branch", size: 600, data: layout.groups.map((group) => leaf(group.panels, 300, group.shown)) },
    } }),
  } as unknown as DockviewApi;
  const docListeners = new Map<string, () => void>();
  let pressed: ((event: { target: unknown }) => void) | undefined;
  const element = {
    ownerDocument: {
      addEventListener: (type: string, cb: () => void) => docListeners.set(type, cb),
      removeEventListener: (type: string) => docListeners.delete(type),
    },
    addEventListener: (_type: string, cb: (event: { target: unknown }) => void) => { pressed = cb; },
    removeEventListener: () => {},
  } as unknown as HTMLElement;
  const dispose = bridgeDockviewToMux(binding, api, element);
  socket.sent = [];
  const move = () => { for (const cb of moved) cb(); };
  /** The user shows `paneId` in its group. */
  const select = (paneId: string) => {
    for (const group of layout.groups) if (group.panels.includes(paneId)) group.shown = paneId;
    for (const cb of activated) cb();
  };
  /** The user closes `paneId`, and dockview shows `neighbour` in its place. */
  const close = (paneId: string, neighbour: string) => {
    for (const group of layout.groups) {
      if (!group.panels.includes(paneId)) continue;
      group.panels = group.panels.filter((id) => id !== paneId);
      group.shown = neighbour;
    }
    for (const cb of removed) cb({ id: paneId });
  };
  const dragDivider = (selector: string | null) => {
    pressed?.({ target: { closest: (s: string) => (s === selector ? {} : null) } });
    docListeners.get("pointerup")?.();
  };
  const sent = () => socket.sentOfType("mux.op").map((m) => m.op);
  return { socket, binding, layout, move, select, close, dragDivider, sent, dispose };
}

// mux-protocol, Window Layout Changes Reach the Server
describe("arrangement after a gesture", () => {
  afterEach(() => vi.useRealTimers());

  const screen = bridgedScreen;

  it("sends the grid once for the moves of one drop, and no move_pane", () => {
    const { move, sent } = screen();
    move();
    move();
    expect(sent()).toEqual([]);
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([{
      type: "arrange_panes",
      root: { type: "split", axis: "vertical", sizes: [0.5, 0.5], children: [
        { type: "slot", paneIds: ["p1", "p2"], activePaneId: "p1" },
        { type: "slot", paneIds: ["p3"], activePaneId: "p3" },
      ] },
    }]);
    // The same grid again changes nothing.
    move();
    vi.advanceTimersByTime(100);
    expect(sent()).toHaveLength(1);
  });

  it("sends the grid at the end of a divider drag, not for other presses", () => {
    const { dragDivider, sent } = screen();
    dragDivider(null);
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([]);
    dragDivider(".dv-sash");
    vi.advanceTimersByTime(100);
    expect(sent().map((op) => op.type)).toEqual(["arrange_panes"]);
  });

  it("sends nothing when the server already has that grid", () => {
    const { binding, move, sent } = screen();
    binding.arrangement = layoutArrangement({
      type: "split", id: "split-1", axis: "vertical", sizes: [0.5, 0.5], children: [
        { type: "slot", id: "slot-1", paneIds: ["p1", "p2"] },
        { type: "slot", id: "slot-2", paneIds: ["p3"] },
      ],
    });
    move();
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([]);
  });

  it("sends nothing for a move of a snapshot, a host without the capability, a maximized or popout group", () => {
    const during = screen();
    during.binding.suppressOps = true;
    during.move();
    during.binding.suppressOps = false;
    vi.advanceTimersByTime(100);
    expect(during.sent()).toEqual([]);

    const old = screen(["scratchpad-v1"]);
    old.move();
    vi.advanceTimersByTime(100);
    expect(old.sent()).toEqual([]);

    const maximized = screen();
    maximized.layout.maximized = true;
    maximized.move();
    vi.advanceTimersByTime(100);
    expect(maximized.sent()).toEqual([]);

    const popout = screen();
    popout.layout.groups[1].location = "popout";
    popout.move();
    vi.advanceTimersByTime(100);
    expect(popout.sent()).toEqual([]);
  });

  it("drops a pending report when the bridge goes away", () => {
    const { move, sent, dispose } = screen();
    move();
    dispose();
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([]);
  });
});

// mux-protocol, Selected Tabs Reach the Server
describe("selected tabs", () => {
  afterEach(() => vi.useRealTimers());

  // The snapshot's screen: `p1` and `p2` as tabs with `p1` active, above `p3`.
  function screen(capabilities = ["layout-arrange-v1", "tab-select-v1"]) {
    const shown = bridgedScreen(capabilities);
    shown.binding.arrangement = layoutArrangement({
      type: "split", id: "split-1", axis: "vertical", sizes: [0.5, 0.5], children: [
        { type: "slot", id: "slot-1", paneIds: ["p1", "p2"], activePaneId: "p1" },
        { type: "slot", id: "slot-2", paneIds: ["p3"] },
      ],
    });
    shown.binding.noteActiveTab(["p1", "p2"], "p1");
    return shown;
  }

  it("sends one select_tab for a selected tab, and nothing for the tab the server has", () => {
    const { select, sent } = screen();
    select("p2");
    select("p1");
    select("p2");
    expect(sent()).toEqual([]);
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([{ type: "select_tab", paneId: "p2" }]);

    select("p2");
    select("p3");
    vi.advanceTimersByTime(100);
    expect(sent()).toHaveLength(1);
  });

  it("sends nothing during a snapshot, to a host without the capability, or for a popout group", () => {
    const during = screen();
    during.binding.suppressOps = true;
    during.select("p2");
    during.binding.suppressOps = false;
    vi.advanceTimersByTime(100);
    expect(during.sent()).toEqual([]);

    const old = screen(["layout-arrange-v1"]);
    old.select("p2");
    vi.advanceTimersByTime(100);
    expect(old.sent()).toEqual([]);

    const popout = screen();
    popout.layout.groups[0].location = "popout";
    popout.select("p2");
    vi.advanceTimersByTime(100);
    expect(popout.sent()).toEqual([]);
  });

  it("sends the neighbour dockview shows after a close", () => {
    const { layout, close, sent } = screen();
    layout.groups[0].panels.push("p4");
    close("p1", "p4");
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([
      { type: "close_pane", paneId: "p1" },
      { type: "select_tab", paneId: "p4" },
    ]);
  });

  it("leaves a sent tab out of the next drag's comparison", () => {
    const { select, dragDivider, sent } = screen();
    select("p2");
    vi.advanceTimersByTime(100);
    dragDivider(".dv-sash");
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([{ type: "select_tab", paneId: "p2" }]);
  });

  it("reports a drag's tabs with its arrangement, not again as select_tab", () => {
    const { layout, move, sent } = screen();
    // `p3` dragged into the top group, where it is shown.
    layout.groups = [{ panels: ["p1", "p2", "p3"], shown: "p3", location: "grid" }];
    move();
    vi.advanceTimersByTime(100);
    expect(sent()).toEqual([{
      type: "arrange_panes",
      root: { type: "slot", paneIds: ["p1", "p2", "p3"], activePaneId: "p3" },
    }]);
  });
});
