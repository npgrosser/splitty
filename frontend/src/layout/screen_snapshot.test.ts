// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AddPanelOptions, DockviewApi, DockviewGroupPanel, IDockviewPanel } from "dockview-core";
import { applyScreenSnapshot } from "./screen_snapshot.ts";
import { gridArrangement, sameShape } from "./arrangement.ts";
import { ScreenBinding } from "../lib/connection/screen_binding.ts";
import type { LayoutNode, PaneState, ScreenState, ViewerRecord } from "../lib/protocol/types.ts";
import { testScreen, type TerminalStandIn, type TestScreen } from "../lib/testing/screen_layout.ts";
import { ANNOTATIONS_CAPABILITY } from "../lib/viewer/annotations.ts";
import { REVIEWS_CAPABILITY } from "../lib/viewer/server_reviews.ts";
import { VIEWER_CAPABILITY } from "../lib/viewer/limits.ts";
import { viewerRecordOf } from "../lib/viewer/panel.ts";
import { showContextMenu } from "../stores/ui.svelte.ts";

// The viewer pane's modules query media features when they load; jsdom has no matchMedia.
vi.hoisted(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (media: string) => ({
      matches: false, media, onchange: null,
      addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false,
    }),
  });
});

vi.mock("../stores/ui.svelte.ts", async (importOriginal) => ({
  ...await importOriginal<typeof import("../stores/ui.svelte.ts")>(),
  showContextMenu: vi.fn(),
}));

let screens: TestScreen[] = [];

afterEach(() => {
  vi.unstubAllGlobals();
  for (const screen of screens) screen.dispose();
  screens = [];
});

function layout(viewers = true) {
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
    conn: { mux: { viewers } },
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
    for (const [index, id] of [[1, "two"], [2, "three"]] as const) {
      expect(api.getPanel(id)!.group).toBe(firstGroup);
      expect(api.addPanel).toHaveBeenCalledWith(expect.objectContaining({
        id, position: { referenceGroup: firstGroup, direction: "within", index },
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

// viewer-panes, Viewer Capability: a pane of the role viewer is built as a
// viewer pane, in the grid and floating, and never starts a terminal session.
describe("viewer panes in a snapshot", () => {
  const plan = { source: { kind: "file" as const, path: "/home/me/app/PLAN.md" }, originPaneId: "p1", revision: 1 };
  const screenWith = (viewer: PaneState["viewer"], options: { revision?: number; activePaneId?: string } = {}): ScreenState => ({
    id: "screen", revision: options.revision ?? 1,
    root: { type: "split", id: "row", axis: "horizontal", sizes: [0.5, 0.5], children: [
      { type: "slot", id: "left", paneIds: ["p1"] },
      { type: "slot", id: "right", paneIds: ["p2", "v1"], activePaneId: options.activePaneId ?? "v1" },
    ] },
    floating: [{ id: "float", root: { type: "slot", id: "f", paneIds: ["v2"] }, bounds: { x: .1, y: .1, w: .5, h: .5 } }],
    panes: {
      p1: { id: "p1", sessionId: "session-p1", role: "terminal" },
      p2: { id: "p2", sessionId: "session-p2" },
      // A session the server should never name for a viewer pane is ignored.
      v1: { id: "v1", sessionId: "session-v1", role: "viewer", title: "PLAN.md", cwd: "/home/me/app", ...(viewer ? { viewer } : {}) },
      v2: { id: "v2", role: "viewer", title: "Selection from claude", cwd: "/home/me/app", viewer: { source: { kind: "text", title: "Selection from claude" }, originPaneId: "p1", revision: 1 } },
    },
    sessions: {},
  });
  const added = (api: DockviewApi, id: string) =>
    vi.mocked(api.addPanel).mock.calls.map(([options]) => options).find((options) => options.id === id)!;

  it("builds viewer panes in the grid and in floating groups", () => {
    const { api, binding } = layout();
    applyScreenSnapshot(api, binding, screenWith(plan));
    expect(added(api, "v1")).toMatchObject({
      component: "viewer", title: "PLAN.md",
      params: { viewer: { record: plan, supported: true, cwd: "/home/me/app" } },
      position: { referenceGroup: api.getPanel("p2")!.group, direction: "within" },
    });
    expect(added(api, "v2")).toMatchObject({
      component: "viewer", title: "Selection from claude",
      params: { viewer: { record: { source: { kind: "text", title: "Selection from claude" } }, supported: true } },
      floating: expect.any(Object),
    });
    for (const id of ["v1", "v2"]) expect(added(api, id).params).not.toHaveProperty("existingSessionId");
    expect(added(api, "p1")).toMatchObject({ component: "terminal", params: { existingSessionId: "session-p1" } });
    expect(added(api, "p2")).toMatchObject({ component: "terminal" });
  });

  it("builds a viewer pane that cannot be shown for a missing record or a server without viewers", () => {
    const missing = layout();
    applyScreenSnapshot(missing.api, missing.binding, screenWith(undefined));
    expect(added(missing.api, "v1")).toMatchObject({ component: "viewer", title: "PLAN.md", params: { viewer: { supported: true } } });
    expect((added(missing.api, "v1").params as { viewer: object }).viewer).not.toHaveProperty("record");

    const old = layout(false);
    applyScreenSnapshot(old.api, old.binding, screenWith(plan));
    expect(added(old.api, "v1")).toMatchObject({ component: "viewer", params: { viewer: { record: plan, supported: false } } });
    expect(added(old.api, "v2")).toMatchObject({ component: "viewer", params: { viewer: { supported: false } } });
  });

  it("shows a viewer at once whose revision rose: it was opened again", () => {
    const { api, binding } = layout();
    applyScreenSnapshot(api, binding, screenWith(plan, { activePaneId: "p2" }));
    // The window shows the terminal tab, which a rebuild keeps in front.
    (api as unknown as { groups: unknown[] }).groups = [{ activePanel: { id: "p2" } }];

    applyScreenSnapshot(api, binding, screenWith(plan, { revision: 2, activePaneId: "p2" }));
    const order = (id: string) => vi.mocked(api.getPanel(id)!.api.setActive).mock.invocationCallOrder.at(-1) ?? 0;
    expect(order("p2")).toBeGreaterThan(0);
    expect(order("v1")).toBe(0);

    applyScreenSnapshot(api, binding, screenWith({ ...plan, revision: 2 }, { revision: 3, activePaneId: "v1" }));
    expect(order("v1")).toBeGreaterThan(order("p2"));
  });
});

// mux-protocol, Job Snapshots And Background Rendering, for viewer panes: a
// snapshot that adds or removes viewer panes or changes a viewer's record is
// applied to the panels that are there.
describe("snapshots applied in place", () => {
  const PLAN = "/home/me/app/PLAN.md";
  const NOTES = "/home/me/app/NOTES.md";
  const terminal = (id: string): PaneState => ({ id, role: "terminal", sessionId: `session-${id}` });
  const viewer = (id: string, path: string, record: Partial<ViewerRecord> = {}): PaneState => ({
    id, role: "viewer", title: path.split("/").pop(), cwd: "/home/me/app",
    viewer: { source: { kind: "file", path }, originPaneId: "t1", revision: 1, ...record },
  });
  const slot = (id: string, paneIds: string[], activePaneId?: string): LayoutNode =>
    ({ type: "slot", id, paneIds, ...(activePaneId ? { activePaneId } : {}) });
  const row = (...children: LayoutNode[]): LayoutNode =>
    ({ type: "split", id: "row", axis: "horizontal", sizes: children.map(() => 1 / children.length), children });
  const state = (root: LayoutNode, panes: PaneState[]): ScreenState => ({
    id: "s1", revision: 1, root, floating: [], panes: Object.fromEntries(panes.map(pane => [pane.id, pane])), sessions: {},
  });

  function open(capabilities?: string[]): TestScreen & { added: string[]; removed: string[] } {
    const screen = testScreen({ capabilities });
    screens.push(screen);
    const added: string[] = [];
    const removed: string[] = [];
    screen.api.onDidAddPanel(panel => added.push(panel.id));
    screen.api.onDidRemovePanel(panel => removed.push(panel.id));
    return Object.assign(screen, { added, removed });
  }

  /** The shown element of a viewer pane's panel. */
  const pane = (screen: TestScreen, id: string) => screen.content(id).element;
  const grid = (screen: TestScreen) => gridArrangement(screen.api.toJSON().grid);

  it("keeps every panel and the keyboard focus when a viewer's revision rises", () => {
    const screen = open();
    const { api, binding } = screen;
    const layout = row(slot("left", ["t1"]), slot("right", ["v1"]));
    applyScreenSnapshot(api, binding, state(layout, [terminal("t1"), viewer("v1", PLAN)]));
    api.getPanel("t1")!.api.setActive();
    const shell = screen.content<TerminalStandIn>("t1");
    const document = screen.content("v1");
    shell.input.focus();
    screen.added.length = 0;

    applyScreenSnapshot(api, binding, state(layout, [terminal("t1"), viewer("v1", PLAN, { revision: 2, originPaneId: "t2" })]));
    expect(screen.added).toEqual([]);
    expect(screen.removed).toEqual([]);
    expect(screen.content("t1")).toBe(shell);
    expect(screen.content("v1")).toBe(document);
    expect(viewerRecordOf(api.getPanel("v1"))).toMatchObject({ revision: 2, originPaneId: "t2" });
    expect(api.activePanel?.id).toBe("t1");
    expect(globalThis.document.activeElement).toBe(shell.input);
  });

  // viewer-annotations: another window's annotation reaches the pane's list
  // and marks, and the layout stays as it is.
  it("keeps every panel and the shown document when only a viewer's annotations change", { timeout: 30_000 }, async () => {
    const screen = open([VIEWER_CAPABILITY, ANNOTATIONS_CAPABILITY, REVIEWS_CAPABILITY]);
    const { api, binding } = screen;
    const layout = row(slot("left", ["t1"]), slot("right", ["v1"]));
    // As the connection's snapshot handler does: the annotations first, then the layout.
    const apply = (next: ScreenState) => {
      binding.conn.annotations.snapshot(next);
      applyScreenSnapshot(api, binding, next);
    };
    apply(state(layout, [terminal("t1"), viewer("v1", PLAN)]));
    screen.answerReads({ v1: "# Plan\n\nThe first step.\n" });
    // The view's libraries load with the first viewer pane: slow while other test files run.
    await vi.waitFor(() => expect(pane(screen, "v1").querySelector(".viewer-rendered")).not.toBeNull(), { timeout: 15_000 });
    api.getPanel("t1")!.api.setActive();
    const shell = screen.content<TerminalStandIn>("t1");
    shell.input.focus();
    const content = screen.content("v1");
    const rendered = pane(screen, "v1").querySelector(".viewer-rendered");
    screen.added.length = 0;

    const annotations = [{
      id: "ann-0123456789abcdef", kind: "comment" as const, body: "Which step?",
      target: { view: "rendered" as const, quote: "The first step.", prefix: "Plan\n", suffix: "\n", startLine: 3, endLine: 3 },
    }];
    // An annotation op raises only the screen's revision.
    apply({ ...state(layout, [terminal("t1"), viewer("v1", PLAN, { annotations })]), revision: 2 });
    expect(screen.added).toEqual([]);
    expect(screen.removed).toEqual([]);
    expect(screen.content("t1")).toBe(shell);
    expect(screen.content("v1")).toBe(content);
    expect(pane(screen, "v1").querySelector(".viewer-rendered")).toBe(rendered);
    expect(viewerRecordOf(api.getPanel("v1"))).not.toHaveProperty("annotations");
    expect(pane(screen, "v1").querySelectorAll(".viewer-annotation")).toHaveLength(1);
    expect(rendered!.shadowRoot!.querySelector("[data-splitty-mark]")!.textContent).toBe("The first step.");
    expect(api.activePanel?.id).toBe("t1");
    expect(globalThis.document.activeElement).toBe(shell.input);
    expect(screen.socket.sentOfType("mux.op")).toEqual([]);
  });

  it("adds a viewer beside a terminal, and one as a tab of its group, without taking the front or the focus", () => {
    const screen = open();
    const { api, binding } = screen;
    applyScreenSnapshot(api, binding, state(slot("main", ["t1"]), [terminal("t1")]));
    const shell = screen.content<TerminalStandIn>("t1");
    shell.input.focus();
    screen.added.length = 0;

    const plan = viewer("v1", PLAN);
    applyScreenSnapshot(api, binding, state(row(slot("main", ["t1"]), slot("docs", ["v1"], "v1")), [terminal("t1"), plan]));
    expect(screen.added).toEqual(["v1"]);
    expect(screen.removed).toEqual([]);
    expect(screen.content("t1")).toBe(shell);
    expect(sameShape(grid(screen), { type: "split", axis: "horizontal", sizes: [], children: [
      { type: "slot", paneIds: ["t1"] }, { type: "slot", paneIds: ["v1"] },
    ] })).toBe(true);
    expect(api.getPanel("v1")!.api.isVisible).toBe(true);
    expect(api.activePanel?.id).toBe("t1");
    expect(document.activeElement).toBe(shell.input);

    // The next document of that terminal joins the viewer's group, in front
    // of it, while the terminal's group stays maximized and active.
    api.getPanel("t1")!.group.api.maximize();
    applyScreenSnapshot(api, binding, state(
      row(slot("main", ["t1"]), slot("docs", ["v1", "v2"], "v2")),
      [terminal("t1"), plan, viewer("v2", NOTES)],
    ));
    expect(screen.added).toEqual(["v1", "v2"]);
    expect(screen.removed).toEqual([]);
    expect(api.getPanel("v2")!.group).toBe(api.getPanel("v1")!.group);
    expect(api.getPanel("v1")!.group.activePanel?.id).toBe("v2");
    expect(api.hasMaximizedGroup()).toBe(true);
    expect(api.activePanel?.id).toBe("t1");
    expect(document.activeElement).toBe(shell.input);
  });

  it("removes a viewer without closing another panel", () => {
    const screen = open();
    const { api, binding } = screen;
    const panes = [terminal("t1"), viewer("v1", PLAN), viewer("v2", NOTES)];
    applyScreenSnapshot(api, binding, state(row(slot("main", ["t1"]), slot("docs", ["v1", "v2"], "v2")), panes));
    const shell = screen.content("t1");
    const plan = screen.content("v1");
    screen.added.length = 0;

    applyScreenSnapshot(api, binding, state(row(slot("main", ["t1"]), slot("docs", ["v1"], "v1")), panes.slice(0, 2)));
    expect(screen.removed).toEqual(["v2"]);
    expect(screen.added).toEqual([]);
    expect(screen.content("t1")).toBe(shell);
    expect(screen.content("v1")).toBe(plan);
    expect(api.getPanel("v1")!.api.isVisible).toBe(true);
  });

  it("keeps a viewer's mode and wrap choice through a rebuild", { timeout: 30_000 }, async () => {
    const screen = open();
    const { api, binding } = screen;
    const panes = [terminal("t1"), viewer("v1", PLAN)];
    applyScreenSnapshot(api, binding, state(row(slot("main", ["t1"]), slot("docs", ["v1"])), panes));
    screen.answerReads({ v1: "# Plan\n\nThe first step.\n" });
    // The view's libraries load with the first viewer pane: slow while other test files run.
    await vi.waitFor(() => expect(pane(screen, "v1").querySelector(".viewer-rendered")).not.toBeNull(), { timeout: 15_000 });

    // The source, unwrapped, as the bar and the pane's menu choose it.
    pane(screen, "v1").querySelector<HTMLButtonElement>(".viewer-bar-button")!.click();
    pane(screen, "v1").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    const items = vi.mocked(showContextMenu).mock.calls.at(-1)![0];
    items.find(item => item.label === "Unwrap Lines")!.action();
    expect(pane(screen, "v1").querySelector(".cm-content")!.classList.contains("cm-lineWrapping")).toBe(false);
    const before = screen.content("v1");
    const shell = screen.content("t1");

    // Another client splits the terminal: the screen is built anew.
    applyScreenSnapshot(api, binding, state(
      row(slot("main", ["t1"]), slot("docs", ["v1"]), slot("more", ["t2"])),
      [...panes, terminal("t2")],
    ));
    expect(screen.content("t1")).not.toBe(shell);
    expect(screen.content("v1")).not.toBe(before);
    const rebuilt = pane(screen, "v1");
    expect(rebuilt.querySelector(".viewer-rendered")).toBeNull();
    expect(rebuilt.querySelector(".cm-editor")!.textContent).toContain("# Plan");
    expect(rebuilt.querySelector(".cm-content")!.classList.contains("cm-lineWrapping")).toBe(false);
    expect(rebuilt.querySelector(".viewer-bar-button")!.textContent).toBe("Show Rendered");
  });

  it("builds the first snapshot on a new connection anew, so that its terminals attach again", () => {
    const screen = open();
    const { api, binding } = screen;
    const unchanged = state(slot("main", ["t1"]), [terminal("t1")]);
    applyScreenSnapshot(api, binding, unchanged);
    const shell = screen.content("t1");
    applyScreenSnapshot(api, binding, unchanged);
    expect(screen.content("t1")).toBe(shell);

    screen.reconnect("client-2");
    applyScreenSnapshot(api, binding, unchanged);
    expect(screen.content("t1")).not.toBe(shell);
  });
});
