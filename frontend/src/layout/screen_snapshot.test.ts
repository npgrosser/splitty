import { afterEach, describe, expect, it, vi } from "vitest";
import type { AddPanelOptions, DockviewApi, DockviewGroupPanel, IDockviewPanel } from "dockview-core";
import { applyScreenSnapshot } from "./screen_snapshot.ts";
import { ScreenBinding } from "../lib/connection/screen_binding.ts";
import type { ScreenState } from "../lib/protocol/types.ts";

afterEach(() => vi.unstubAllGlobals());

function layout() {
  vi.stubGlobal("HTMLElement", class {});
  vi.stubGlobal("document", { activeElement: null });
  vi.stubGlobal("requestAnimationFrame", vi.fn());
  const panels = new Map<string, IDockviewPanel>();
  const api = {
    width: 800, height: 600, groups: [],
    get panels() { return [...panels.values()]; },
    getPanel: (id: string) => panels.get(id),
    addPanel: vi.fn((options: AddPanelOptions) => {
      const within = options.position && "referenceGroup" in options.position ? options.position.referenceGroup : undefined;
      const group = within ?? { id: `group-${panels.size}`, api: { setSize: vi.fn() } };
      const panel = {
        id: options.id, group: group as DockviewGroupPanel,
        api: { setActive: vi.fn(), close: vi.fn(() => panels.delete(options.id)) },
      } as unknown as IDockviewPanel;
      panels.set(panel.id, panel);
      return panel;
    }),
  } as unknown as DockviewApi;
  const binding = {
    suppressOps: false, suppressFocus: false, sessionForPane: vi.fn(),
    clearFloatingIds: vi.fn(), registerFloatingId: vi.fn(),
    activeTabs: new Set(), noteActiveTab: ScreenBinding.prototype.noteActiveTab,
  } as unknown as ScreenBinding;
  return { api, binding };
}

describe("tab snapshots on another client", () => {
  it.each([false, true])("restores tabs into one group (floating=%s)", floating => {
    const { api, binding } = layout();
    const slot = { type: "slot" as const, id: "tabs", paneIds: ["one", "two", "three"], activePaneId: "three" };
    const screen: ScreenState = {
      id: "screen", revision: 2,
      root: floating ? { type: "slot", id: "empty", paneIds: [] } : slot,
      floating: floating ? [{ id: "float", root: slot, bounds: { x: .1, y: .1, w: .5, h: .5 } }] : [],
      panes: Object.fromEntries(slot.paneIds!.map(id => [id, { id, sessionId: `session-${id}` }])),
      sessions: {},
    };
    applyScreenSnapshot(api, binding, screen);
    expect(api.addPanel).toHaveBeenCalledTimes(3);
    const firstGroup = api.getPanel("one")!.group;
    for (const id of ["two", "three"]) {
      expect(api.getPanel(id)!.group).toBe(firstGroup);
      expect(api.addPanel).toHaveBeenCalledWith(expect.objectContaining({
        id, position: { referenceGroup: firstGroup, direction: "within" },
        params: expect.objectContaining({ existingSessionId: `session-${id}` }),
      }));
    }
    expect(api.getPanel("three")!.api.setActive).toHaveBeenCalled();
    if (floating) expect(binding.registerFloatingId).toHaveBeenCalledWith(String(firstGroup.id), "float");
    // mux-protocol, Window Layout Changes Reach the Server: a later drag is compared with it.
    expect(binding.arrangement).toEqual(floating
      ? { type: "slot", paneIds: [] }
      : { type: "slot", paneIds: ["one", "two", "three"], activePaneId: "three" });
  });
});

// mux-protocol, Selected Tabs Reach the Server (A slot without an active pane).
describe("the tab a fresh build shows", () => {
  const tabs = (activePaneId?: string) => ({ type: "slot" as const, id: "tabs", paneIds: ["one", "two", "three"], activePaneId });
  const screenWith = (slot: ReturnType<typeof tabs>, floating: boolean): ScreenState => ({
    id: "screen", revision: 2,
    root: floating ? { type: "slot", id: "empty", paneIds: [] } : slot,
    floating: floating ? [{ id: "float", root: slot, bounds: { x: .1, y: .1, w: .5, h: .5 } }] : [],
    panes: Object.fromEntries(slot.paneIds.map(id => [id, { id }])),
    sessions: {},
  });
  const selected = (api: DockviewApi) =>
    ["one", "two", "three"].filter(id => vi.mocked(api.getPanel(id)!.api.setActive).mock.calls.length > 0);

  it.each([false, true])("is the slot's first pane when it names none or an unknown one (floating=%s)", floating => {
    for (const active of [undefined, "gone"]) {
      const { api, binding } = layout();
      applyScreenSnapshot(api, binding, screenWith(tabs(active), floating));
      expect(selected(api)).toEqual(["one"]);
    }
  });

  it.each([false, true])("is the slot's active pane when it names one (floating=%s)", floating => {
    const { api, binding } = layout();
    applyScreenSnapshot(api, binding, screenWith(tabs("two"), floating));
    expect(selected(api)).toEqual(["two"]);
  });

  it("tells the binding which tabs the server has as active", () => {
    const { api, binding } = layout();
    binding.activeTabs.add("stale");
    applyScreenSnapshot(api, binding, {
      id: "screen", revision: 3,
      root: { type: "split", id: "row", axis: "horizontal", sizes: [0.5, 0.5], children: [
        { type: "slot", id: "left", paneIds: ["one", "two"], activePaneId: "two" },
        { type: "slot", id: "right", paneIds: ["three"] },
      ] },
      floating: [{ id: "float", root: { type: "slot", id: "f", paneIds: ["four", "five"], activePaneId: "five" }, bounds: { x: .1, y: .1, w: .5, h: .5 } }],
      panes: Object.fromEntries(["one", "two", "three", "four", "five"].map(id => [id, { id }])),
      sessions: {},
    });
    expect([...binding.activeTabs].sort()).toEqual(["five", "two"]);
  });
});

// workspace-model, Moving a Pane to Another Screen: no session ends, also on a
// client that only learns of the move from the source screen's snapshot.
describe("panes that leave a screen", () => {
  it("let go of the sessions the server listed, and end only a pane's it never did", () => {
    const { api, binding } = layout();
    const sessions = new Map<string, { id: string; detach: ReturnType<typeof vi.fn> }>();
    vi.mocked(binding.sessionForPane).mockImplementation(((paneId: string) => {
      if (!sessions.has(paneId)) sessions.set(paneId, { id: `session-${paneId}`, detach: vi.fn() });
      return sessions.get(paneId);
    }) as never);
    const screen = (paneIds: string[]): ScreenState => ({
      id: "screen", revision: 1,
      root: { type: "slot", id: "tabs", paneIds, activePaneId: paneIds[0] },
      floating: [],
      panes: Object.fromEntries(paneIds.map(id => [id, { id, sessionId: `session-${id}` }])),
      sessions: {},
    });
    applyScreenSnapshot(api, binding, screen(["stays", "moves"]));
    api.addPanel({ id: "local", component: "terminal" });

    applyScreenSnapshot(api, binding, screen(["stays"]));

    expect(sessions.get("moves")?.detach).toHaveBeenCalled();
    expect(sessions.get("stays")?.detach).toHaveBeenCalled();
    expect(sessions.get("local")?.detach).not.toHaveBeenCalled();
    expect(api.getPanel("moves")).toBeUndefined();
  });
});
