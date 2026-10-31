import { describe, expect, it } from "vitest";
import { arrangementSlots, gridArrangement, layoutArrangement, sameArrangement, withActivePane } from "./arrangement.ts";
import type { LayoutNode } from "../lib/protocol/types.ts";

const leaf = (views: string[], size: number, activeView = views[0]) =>
  ({ type: "leaf" as const, data: { views, activeView, id: `group-${views.join("-")}` }, size });

describe("arrangement of the grid", () => {
  // The picture of the 2026-10-30 report after the drag: two tabs on top, a row below.
  it("reads rows, columns, tabs and sizes from dockview's grid", () => {
    const grid = {
      orientation: "VERTICAL",
      root: {
        type: "branch" as const, size: 1000, data: [
          leaf(["agent", "ui"], 600, "ui"),
          { type: "branch" as const, size: 400, data: [leaf(["zsh"], 300), leaf([], 100), leaf(["htop"], 700)] },
        ],
      },
    };
    expect(gridArrangement(grid)).toEqual({
      type: "split", axis: "vertical", sizes: [0.6, 0.4], children: [
        { type: "slot", paneIds: ["agent", "ui"], activePaneId: "ui" },
        // The empty group is left out, its share goes to the others.
        { type: "split", axis: "horizontal", sizes: [0.3, 0.7], children: [
          { type: "slot", paneIds: ["zsh"], activePaneId: "zsh" },
          { type: "slot", paneIds: ["htop"], activePaneId: "htop" },
        ] },
      ],
    });
  });

  it("collapses a branch with one group and reads an empty grid as an empty slot", () => {
    const one = { orientation: "HORIZONTAL", root: { type: "branch" as const, size: 800, data: [leaf(["p1", "p2"], 800)] } };
    expect(gridArrangement(one)).toEqual({ type: "slot", paneIds: ["p1", "p2"], activePaneId: "p1" });
    const none = { orientation: "HORIZONTAL", root: { type: "branch" as const, size: 800, data: [leaf([], 800)] } };
    expect(gridArrangement(none)).toEqual({ type: "slot", paneIds: [] });
  });

  // The server wraps a moved pane's target in a split of its own; dockview
  // keeps one row. They are the same grid.
  it("matches a snapshot that nests splits of the same axis", () => {
    const root: LayoutNode = {
      type: "split", id: "split-a", axis: "horizontal", sizes: [0.5, 0.5], children: [
        { type: "slot", id: "slot-1", paneIds: ["p1"] },
        { type: "split", id: "split-b", axis: "horizontal", sizes: [0.5, 0.5], children: [
          { type: "slot", id: "slot-2", paneIds: ["p2"] },
          { type: "slot", id: "slot-3", paneIds: ["p3"], activePaneId: "p3" },
        ] },
      ],
    };
    const grid = {
      orientation: "HORIZONTAL",
      root: { type: "branch" as const, size: 600, data: [leaf(["p1"], 400), leaf(["p2"], 200), leaf(["p3"], 200)] },
    };
    expect(layoutArrangement(root)).toEqual({
      type: "split", axis: "horizontal", sizes: [0.5, 0.25, 0.25], children: [
        { type: "slot", paneIds: ["p1"] },
        { type: "slot", paneIds: ["p2"] },
        { type: "slot", paneIds: ["p3"], activePaneId: "p3" },
      ],
    });
    // 400/800 against 0.5 and so on: the same within the tolerance.
    const wider = { ...grid, root: { ...grid.root, data: [leaf(["p1"], 401), leaf(["p2"], 200), leaf(["p3"], 199)] } };
    expect(sameArrangement(gridArrangement(wider), layoutArrangement(root))).toBe(true);
  });

  it("tells apart sizes beyond the tolerance, another tab order and another active tab", () => {
    const base = gridArrangement({
      orientation: "HORIZONTAL",
      root: { type: "branch", size: 1000, data: [leaf(["p1", "p2"], 500, "p2"), leaf(["p3"], 500)] },
    });
    const variants = [
      { type: "branch" as const, size: 1000, data: [leaf(["p1", "p2"], 520, "p2"), leaf(["p3"], 480)] },
      { type: "branch" as const, size: 1000, data: [leaf(["p2", "p1"], 500, "p2"), leaf(["p3"], 500)] },
      { type: "branch" as const, size: 1000, data: [leaf(["p1", "p2"], 500, "p1"), leaf(["p3"], 500)] },
      { type: "branch" as const, size: 1000, data: [leaf(["p1", "p2"], 500, "p2"), leaf(["p3"], 500)] },
    ];
    expect(variants.map(root => sameArrangement(gridArrangement({ orientation: "VERTICAL", root }), base)))
      .toEqual([false, false, false, false]);
    expect(variants.map(root => sameArrangement(gridArrangement({ orientation: "HORIZONTAL", root }), base)))
      .toEqual([false, false, false, true]);
    // A slot without an active pane on the server matches any active tab.
    expect(sameArrangement({ type: "slot", paneIds: ["p1", "p2"] }, { type: "slot", paneIds: ["p1", "p2"], activePaneId: "p2" }))
      .toBe(true);
  });

  // mux-protocol, Selected Tabs Reach the Server: a sent select_tab is what the server has.
  it("replaces the active tab of the slot that holds a pane, and lists the slots", () => {
    const root = gridArrangement({
      orientation: "HORIZONTAL",
      root: { type: "branch", size: 1000, data: [leaf(["p1", "p2"], 500), leaf(["p3"], 500)] },
    });
    const selected = withActivePane(root, "p2");
    expect(arrangementSlots(selected)).toEqual([
      { type: "slot", paneIds: ["p1", "p2"], activePaneId: "p2" },
      { type: "slot", paneIds: ["p3"], activePaneId: "p3" },
    ]);
    expect(arrangementSlots(root)[0].activePaneId).toBe("p1");
    expect(withActivePane(root, "p9")).toEqual(root);
  });
});
