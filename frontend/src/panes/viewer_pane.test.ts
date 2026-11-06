// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { LayoutNode, PaneState, ScreenState } from "../lib/protocol/types.ts";
import type { TestScreen } from "../lib/testing/screen_layout.ts";

// The view's modules query media features when they load; jsdom has no matchMedia.
vi.hoisted(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (media: string) => ({
      matches: false, media, onchange: null,
      addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false,
    }),
  });
});

let screens: TestScreen[] = [];

afterEach(() => {
  for (const screen of screens) screen.dispose();
  screens = [];
  vi.doUnmock("./viewer_view.ts");
  vi.restoreAllMocks();
});

const viewer = (id: string, path: string): PaneState => ({
  id, role: "viewer", title: path.split("/").pop(), cwd: "/home/me/app",
  viewer: { source: { kind: "file", path }, originPaneId: "t1", revision: 1 },
});
const slot = (id: string, paneIds: string[]): LayoutNode => ({ type: "slot", id, paneIds, activePaneId: paneIds[0] });
const row = (...children: LayoutNode[]): LayoutNode =>
  ({ type: "split", id: "row", axis: "horizontal", sizes: children.map(() => 1 / children.length), children });
const state = (root: LayoutNode, panes: PaneState[]): ScreenState => ({
  id: "s1", revision: 1, root, floating: [], panes: Object.fromEntries(panes.map(pane => [pane.id, pane])), sessions: {},
});

/**
 * A screen with the viewer panes of `panes`, in a window that has not loaded
 * the view yet: the modules are loaded anew, after the view's mock.
 */
async function open(root: LayoutNode, panes: PaneState[]) {
  vi.resetModules();
  const { testScreen } = await import("../lib/testing/screen_layout.ts");
  const { applyScreenSnapshot } = await import("../layout/screen_snapshot.ts");
  const { VIEW_LOAD_FAILED } = await import("./viewer_pane.ts");
  const screen = testScreen();
  screens.push(screen);
  const apply = (root: LayoutNode, panes: PaneState[]) => applyScreenSnapshot(screen.api, screen.binding, state(root, panes));
  apply(root, panes);
  return { screen, apply, VIEW_LOAD_FAILED };
}

// Each test loads the pane's modules anew, and with them the view's libraries:
// slow while other test files run.
const LOAD = { timeout: 15_000 };

/** Until the view's load has settled for every pane that waits for it. */
async function settled(): Promise<void> {
  await import("./viewer_view.ts").catch(() => undefined);
  await new Promise(resolve => setTimeout(resolve, 0));
}

describe("ViewerPaneRenderer", { timeout: 30_000 }, () => {
  it("says so when its view cannot be loaded, and a later pane tries again", async () => {
    vi.doMock("./viewer_view.ts", () => {
      throw new Error("The module is gone.");
    });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const plan = viewer("v1", "/home/me/app/PLAN.md");
    const { screen, apply, VIEW_LOAD_FAILED } = await open(slot("docs", ["v1"]), [plan]);
    const element = screen.content("v1").element;
    await vi.waitFor(() => expect(element.querySelector(".viewer-notice-text")?.textContent).toBe(VIEW_LOAD_FAILED), LOAD);
    expect(element.querySelector(".viewer-bar")).toBeNull();
    expect(warn).toHaveBeenCalledWith("pane v1: the viewer could not be loaded", expect.any(Error));

    vi.doUnmock("./viewer_view.ts");
    apply(row(slot("docs", ["v1"]), slot("more", ["v2"])), [plan, viewer("v2", "/home/me/app/NOTES.md")]);
    const next = screen.content("v2").element;
    await vi.waitFor(() => expect(next.querySelector(".viewer-bar-label")?.textContent).toBe("/home/me/app/NOTES.md"), LOAD);
  });

  it("builds no view for a pane closed while the view loads", async () => {
    let release!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    vi.doMock("./viewer_view.ts", async (importOriginal) => {
      await gate;
      return importOriginal();
    });
    const { screen } = await open(
      row(slot("docs", ["v1"]), slot("more", ["v2"])),
      [viewer("v1", "/home/me/app/PLAN.md"), viewer("v2", "/home/me/app/NOTES.md")],
    );
    const closed = screen.content("v1").element;
    const kept = screen.content("v2").element;
    expect(closed.childElementCount).toBe(0);
    expect(kept.childElementCount).toBe(0);

    screen.api.getPanel("v1")!.api.close();
    release();
    await settled();
    expect(kept.querySelector(".viewer-bar-label")?.textContent).toBe("/home/me/app/NOTES.md");
    expect(closed.childElementCount).toBe(0);
  });
});
