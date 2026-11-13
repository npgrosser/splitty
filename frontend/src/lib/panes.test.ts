import { afterEach, describe, expect, it, vi } from "vitest";
import type { DockviewApi, DockviewGroupPanel } from "dockview-core";
import { addTerminalPane, addViewerPane, openViewerFile } from "./panes.ts";
import { ScreenBinding, registerScreenApi, unregisterScreenApi } from "./connection/screen_binding.ts";
import { fakeServer } from "./testing/fake_server.ts";

vi.mock("./shell_profiles.ts", () => ({ getDefaultShellProfileId: () => "local-default" }));

const layouts: DockviewApi[] = [];
afterEach(() => { for (const api of layouts.splice(0)) unregisterScreenApi(api); });

function setup() {
  const { socket, conn } = fakeServer("host:vm1");
  const binding = new ScreenBinding("remote-screen", conn, "host-screen");
  let activePanel = { id: "active-pane" };
  const group = { panels: [activePanel] };
  const api = {
    id: "tab-test", get activePanel() { return activePanel; }, panels: [activePanel],
    addPanel: vi.fn(() => { activePanel = { id: "new-pane" }; group.panels.push(activePanel); }),
    getPanel: (id: string) => group.panels.some((panel) => panel.id === id) ? { id, group } : undefined,
  } as unknown as DockviewApi;
  registerScreenApi(api, binding);
  layouts.push(api);
  return { api, socket, binding };
}

describe("terminal pane layout operations", () => {
  it("leaves launch inheritance to capable hosts and preserves explicit cwd", () => {
    const { api, socket, binding } = setup();
    binding.conn.sessions.serverDerivedPaneState = true;
    vi.spyOn(binding, "sessionForPane").mockReturnValue({ lastCwd: "/stale" } as ReturnType<typeof binding.sessionForPane>);
    addTerminalPane(api, { id: "new-pane" });
    expect(socket.sentOfType("mux.op")[0].op).toMatchObject({ cwd: undefined, shellProfileId: undefined });
    addTerminalPane(api, { id: "explicit", dir: "/chosen", shellProfileId: "host-fish" });
    expect(socket.sentOfType("mux.op")[1].op).toMatchObject({ cwd: "/chosen", shellProfileId: "host-fish" });
  });

  it("opens a keyboard tab in the active pane's slot on its host", () => {
    const { api, socket } = setup();
    addTerminalPane(api, { id: "new-pane", dir: "/work" });
    expect(socket.sentOfType("mux.op")).toEqual([{
      type: "mux.op", screenId: "host-screen",
      op: { type: "new_tab", paneId: "active-pane", newPaneId: "new-pane", cwd: "/work", shellProfileId: undefined },
    }]);
  });

  // mux-protocol, Selected Tabs Reach the Server: new_tab makes the new tab active on the server.
  it("knows the new tab as its slot's active one on the server", () => {
    const { api, binding } = setup();
    binding.noteActiveTab(["active-pane"], "active-pane");
    addTerminalPane(api, { id: "new-pane" });
    expect([...binding.activeTabs]).toEqual(["new-pane"]);
  });

  // workspace-model, Screen Creation: a server that removed the screen with its
  // last pane creates it anew, so the op carries the name the window shows.
  it("re-creates an empty screen around the new pane under the window's name", async () => {
    const { createScreen, removeScreen } = await import("../stores/screens.svelte.ts");
    createScreen("Logs", "remote-screen", { activate: false });
    const { socket, conn } = fakeServer("host:vm1");
    socket.message({ type: "mux.welcome", clientId: "c1", screens: [] });
    const binding = new ScreenBinding("remote-screen", conn, "host-screen");
    const api = { id: "empty-test", activePanel: undefined, panels: [], addPanel: vi.fn() } as unknown as DockviewApi;
    registerScreenApi(api, binding);
    layouts.push(api);
    addTerminalPane(api, { id: "first-pane" });
    expect(socket.sentOfType("mux.op")).toEqual([{
      type: "mux.op", screenId: "host-screen",
      op: { type: "create_screen", screenId: "host-screen", name: "Logs", newPaneId: "first-pane", shellProfileId: undefined },
    }]);
    removeScreen("remote-screen");
  });

  it("targets the clicked group's tab even when another group is active", () => {
    const { api, socket } = setup();
    const group = { activePanel: { id: "other-pane" } } as DockviewGroupPanel;
    addTerminalPane(api, { id: "new-pane", group });
    expect(api.addPanel).toHaveBeenCalledWith(expect.objectContaining({ position: { referenceGroup: group, direction: "within" } }));
    expect(socket.sentOfType("mux.op")[0].op).toMatchObject({ type: "new_tab", paneId: "other-pane" });
  });

  it.each([["right", "horizontal"], ["below", "vertical"]] as const)("still splits %s along the %s axis", (direction, axis) => {
    const { api, socket } = setup();
    addTerminalPane(api, { id: "new-pane", direction });
    expect(socket.sentOfType("mux.op")[0].op).toMatchObject({ type: "split_pane", paneId: "active-pane", axis });
  });
});

interface FakePanel {
  id: string;
  title?: string;
  group: FakeGroup;
  params: Record<string, unknown>;
  view: { contentComponent: string };
  api: { setActive: ReturnType<typeof vi.fn> };
}

interface FakeGroup {
  id: string;
  api: { location: { type: "grid" | "floating" | "popout" } };
  panels: FakePanel[];
}

const PLAN = { kind: "file" as const, path: "/home/me/app/PLAN.md" };
let viewerLayouts = 0;

/** A screen of a host with viewer panes: terminal `p1` in the grid, `p4` in a floating group. */
function viewerSetup(capabilities = ["viewer-v1"]) {
  const { socket, conn } = fakeServer("host:vm1");
  socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities });
  const binding = new ScreenBinding("remote-screen", conn, "host-screen");
  const grid: FakeGroup = { id: "g1", api: { location: { type: "grid" } }, panels: [] };
  const floating: FakeGroup = { id: "g2", api: { location: { type: "floating" } }, panels: [] };
  const panels = new Map<string, FakePanel>();
  const place = (id: string, group: FakeGroup, fields: { component?: string; params?: Record<string, unknown>; title?: string } = {}) => {
    const panel: FakePanel = {
      id, group, title: fields.title, params: fields.params ?? {},
      view: { contentComponent: fields.component ?? "terminal" }, api: { setActive: vi.fn() },
    };
    group.panels.push(panel);
    panels.set(id, panel);
    return panel;
  };
  place("p1", grid, { title: "claude" });
  place("p4", floating);
  let groups = 2;
  const api = {
    id: `viewer-layout-${++viewerLayouts}`,
    get activePanel() { return panels.get("p1"); },
    get panels() { return [...panels.values()]; },
    getPanel: (id: string) => panels.get(id),
    // A tab joins its group; any other placement makes a group of its own.
    addPanel: vi.fn((options: { id: string; component: string; params: Record<string, unknown>; title: string; position?: { referenceGroup?: FakeGroup } }) =>
      place(options.id, options.position?.referenceGroup ?? { id: `g${++groups}`, api: { location: { type: "grid" } }, panels: [] }, options)),
  } as unknown as DockviewApi;
  registerScreenApi(api, binding);
  layouts.push(api);
  const ops = () => socket.sentOfType("mux.op").map((message) => message.op);
  return { api, socket, binding, grid, floating, place, ops };
}

// viewer-panes, Opening A Viewer: the window's panel and the op it sends agree
// with what the server builds.
describe("viewer pane layout operations", () => {
  it("opens a viewer to the right of a terminal with its own id", () => {
    const { api, ops } = viewerSetup();
    const id = addViewerPane(api, { source: PLAN, anchor: "p1" });
    expect(id).toMatch(/^viewer-[0-9a-f]{16}$/);
    expect(api.addPanel).toHaveBeenCalledWith({
      id, component: "viewer", title: "PLAN.md", renderer: "always",
      params: { viewer: { record: { source: PLAN, revision: 1, originPaneId: "p1" }, supported: true } },
      position: { referencePanel: "p1", direction: "right" },
    });
    expect(ops()).toEqual([{ type: "open_viewer", paneId: "p1", placement: "right", newPaneId: id, source: PLAN }]);
  });

  // Directional placement in a floating group; As a tab of a floating group.
  it.each(["below", "tab"] as const)("adds a viewer next to a floating pane as a tab of its group (%s)", (placement) => {
    const { api, binding, floating, ops } = viewerSetup();
    const id = addViewerPane(api, { source: PLAN, anchor: "p4", placement })!;
    expect(api.addPanel).toHaveBeenCalledWith(expect.objectContaining({ id, position: { referenceGroup: floating, direction: "within" } }));
    expect(floating.panels.map((panel) => panel.id)).toEqual(["p4", id]);
    expect(ops()).toEqual([{ type: "open_viewer", paneId: "p4", placement: "tab", newPaneId: id, source: PLAN }]);
    // The server makes the new tab its slot's active one.
    expect(binding.activeTabs.has(id)).toBe(true);
  });

  it("adds a viewer without an anchor at the right of the layout", () => {
    const { api, ops } = viewerSetup();
    const id = addViewerPane(api, { source: PLAN });
    expect(api.addPanel).toHaveBeenCalledWith(expect.objectContaining({ id, position: { direction: "right" } }));
    expect(ops()).toEqual([{ type: "open_viewer", newPaneId: id, source: PLAN }]);
  });

  // A text document; Document Text Outside Snapshots: the text goes only into the op.
  it("sends a text with its title and directory, and keeps the text out of the panel", () => {
    const { api, ops } = viewerSetup();
    const source = { kind: "text" as const, title: "Selection from claude", text: "the answer\n\ud800" };
    const id = addViewerPane(api, { source, anchor: "p1", cwd: "/home/me/app" })!;
    expect(ops()).toEqual([{
      type: "open_viewer", paneId: "p1", placement: "right", newPaneId: id, cwd: "/home/me/app",
      source: { kind: "text", title: "Selection from claude", text: "the answer\n\ufffd" },
    }]);
    expect(api.getPanel(id)!.title).toBe("Selection from claude");
    expect(JSON.stringify(api.getPanel(id)!.params)).not.toContain("the answer");
    expect(api.getPanel(id)!.params).toEqual({
      viewer: { record: { source: { kind: "text", title: "Selection from claude" }, revision: 1, originPaneId: "p1" }, supported: true, cwd: "/home/me/app" },
    });
  });

  // Viewer Capability; A relative path; A text over the limit; A path with a dot segment.
  it("adds nothing the server would refuse", () => {
    const { api, ops } = viewerSetup();
    for (const source of [
      { kind: "file" as const, path: "docs/plan.md" },
      { kind: "file" as const, path: "/home/me/app/../secrets.txt" },
      { kind: "file" as const, path: "//server/share/plan.md" },
      { kind: "text" as const, title: "Notes", text: "x".repeat(262_145) },
      { kind: "text" as const, title: "", text: "x" },
    ]) {
      expect(addViewerPane(api, { source, anchor: "p1" }), JSON.stringify(source).slice(0, 60)).toBeUndefined();
    }
    expect(addViewerPane(api, { source: PLAN, anchor: "gone" })).toBeUndefined();
    expect(api.addPanel).not.toHaveBeenCalled();
    expect(ops()).toEqual([]);

    // A relative directory is left out; the viewer opens.
    expect(addViewerPane(api, { source: { kind: "text", title: "Notes", text: "x" }, cwd: "relative/dir" })).toBeDefined();
    expect(ops()[0]).not.toHaveProperty("cwd");
  });

  it("adds no viewer on a server without viewer panes", () => {
    const { api, ops } = viewerSetup(["scratchpad-v1"]);
    expect(addViewerPane(api, { source: PLAN, anchor: "p1" })).toBeUndefined();
    expect(api.addPanel).not.toHaveBeenCalled();
    expect(ops()).toEqual([]);
  });

  it("passes a viewer anchor's origin on", () => {
    const { api, grid, place, ops } = viewerSetup();
    place("v1", grid, { component: "viewer", params: { viewer: { record: { source: PLAN, revision: 1, originPaneId: "p1" }, supported: true } } });
    const id = addViewerPane(api, { source: { kind: "file", path: "/home/me/app/TASKS.md" }, anchor: "v1", placement: "tab" })!;
    expect(api.getPanel(id)!.params).toMatchObject({ viewer: { record: { originPaneId: "p1" } } });
    expect(ops()).toEqual([{ type: "open_viewer", paneId: "v1", placement: "tab", newPaneId: id, source: { kind: "file", path: "/home/me/app/TASKS.md" } }]);
  });

  // A relative link of a rendered document: an open viewer of the file comes
  // to the front, or a new one opens as a tab.
  it("shows a linked file in the viewer that has it, or in a new tab", () => {
    const { api, grid, place, ops } = viewerSetup();
    const v1 = place("v1", grid, { component: "viewer", params: { viewer: { record: { source: PLAN, revision: 1 }, supported: true } } });
    expect(openViewerFile(api, PLAN.path, "p1")).toBe("v1");
    expect(v1.api.setActive).toHaveBeenCalled();
    expect(ops()).toEqual([]);

    const id = openViewerFile(api, "/home/me/app/TASKS.md", "v1");
    expect(ops()).toEqual([{ type: "open_viewer", paneId: "v1", placement: "tab", newPaneId: id, source: { kind: "file", path: "/home/me/app/TASKS.md" } }]);
  });

  // Viewers beside a terminal collect in the group of its first viewer, as the
  // server groups the CLI's viewers per terminal.
  it("adds a viewer beside a terminal with viewers as a tab of the first one", () => {
    const { api, binding, ops } = viewerSetup();
    const first = addViewerPane(api, { source: PLAN, anchor: "p1" })!;
    const group = api.getPanel(first)!.group;
    const notes = { kind: "text" as const, title: "Notes", text: "x" };
    const second = addViewerPane(api, { source: notes, anchor: "p1", cwd: "/home/me/app" })!;

    expect(api.getPanel(second)!.group).toBe(group);
    expect(api.addPanel).toHaveBeenLastCalledWith(expect.objectContaining({ id: second, position: { referenceGroup: group, direction: "within" } }));
    expect(ops()[1]).toEqual({ type: "open_viewer", paneId: first, placement: "tab", newPaneId: second, source: notes, cwd: "/home/me/app" });
    // The viewer anchor passes the terminal on as origin.
    expect(api.getPanel(second)!.params).toMatchObject({ viewer: { record: { originPaneId: "p1" } } });
    expect(binding.activeTabs.has(second)).toBe(true);
  });

  it("joins the first of the terminal's viewers in layout order", () => {
    const { api, binding, floating, place, ops } = viewerSetup();
    const record = { viewer: { record: { source: PLAN, revision: 1, originPaneId: "p1" }, supported: true } };
    place("floating-viewer", floating, { component: "viewer", params: record });
    place("grid-viewer", { id: "g9", api: { location: { type: "grid" } }, panels: [] }, { component: "viewer", params: record });
    // The grid as last arranged comes before the floating groups.
    binding.arrangement = { type: "split", axis: "horizontal", sizes: [0.5, 0.5], children: [
      { type: "slot", paneIds: ["p1"] }, { type: "slot", paneIds: ["grid-viewer"] },
    ] };
    addViewerPane(api, { source: { kind: "file", path: "/home/me/app/TASKS.md" }, anchor: "p1" });
    expect(ops()[0]).toMatchObject({ paneId: "grid-viewer", placement: "tab" });

    binding.arrangement = undefined;
    addViewerPane(api, { source: { kind: "file", path: "/home/me/app/NOTES.md" }, anchor: "p1" });
    expect(ops()[1]).toMatchObject({ paneId: "floating-viewer", placement: "tab" });
  });

  it("groups only beside a terminal, and only for a directional placement", () => {
    const { api, grid, place, ops } = viewerSetup();
    place("v4", grid, { component: "viewer", params: { viewer: { record: { source: PLAN, revision: 1, originPaneId: "p4" }, supported: true } } });
    // Another terminal's viewer is not this terminal's.
    addViewerPane(api, { source: PLAN, anchor: "p1" });
    expect(ops()[0]).toMatchObject({ paneId: "p1", placement: "right" });
    // A tab beside the terminal stays in the terminal's group.
    addViewerPane(api, { source: PLAN, anchor: "p1", placement: "tab" });
    expect(ops()[1]).toMatchObject({ paneId: "p1", placement: "tab" });
    // Beside a viewer, a directional placement stays one.
    addViewerPane(api, { source: PLAN, anchor: "v4", placement: "below" });
    expect(ops()[2]).toMatchObject({ paneId: "v4", placement: "below" });
  });

  // workspace-model, Split Session Inheritance (Split of a viewer pane).
  it("starts a terminal split off a viewer pane in the viewer's directory", () => {
    const { api, grid, place, ops } = viewerSetup();
    place("v1", grid, { component: "viewer", params: { viewer: { record: { source: PLAN, revision: 1 }, supported: true } } });
    addTerminalPane(api, { id: "terminal-new", direction: "right", referencePanel: "v1" });
    expect(ops()).toEqual([expect.objectContaining({ type: "split_pane", paneId: "v1", newPaneId: "terminal-new", cwd: "/home/me/app" })]);
  });
});
