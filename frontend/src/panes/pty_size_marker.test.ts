import { describe, expect, it } from "vitest";
import { markerLayout } from "./pty_size_marker.ts";

const cell = { width: 8, height: 16 };
// A pane of 120×40 cells with 4 px of padding left and top.
const box = { width: 4 + 120 * 8, height: 4 + 40 * 16, padLeft: 4, padTop: 4, padRight: 0 };
const pane = { cols: 120, rows: 40 };

describe("markerLayout", () => {
  it("draws nothing while the PTY has the pane's size or is unknown", () => {
    expect(markerLayout({ pane, pty: null }, cell, box, 0)).toBeNull();
    expect(markerLayout({ pane, pty: { ...pane, policy: "active" } }, cell, box, 0)).toBeNull();
  });

  it("hatches the width a narrower PTY leaves (Narrower PTY)", () => {
    const layout = markerLayout({ pane, pty: { cols: 100, rows: 40, policy: "active" } }, cell, box, 0);
    expect(layout?.right).toEqual({ left: 4 + 800, top: 0, width: 160, height: box.height });
    expect(layout?.width).toBe(800);
    expect(layout?.bottom).toBeNull();
    expect(layout?.shift).toBe(0);
    expect(layout?.label).toBe("100×40");
    expect(layout?.takeover).toBe(true);
    expect(layout?.title).toContain("Click to use this window's size");
  });

  it("hatches below a shorter PTY, as wide as the terminal", () => {
    const layout = markerLayout({ pane, pty: { cols: 100, rows: 30, policy: "latest" } }, cell, box, 0);
    expect(layout?.bottom).toEqual({ left: 0, top: 4 + 480, width: 4 + 800, height: 160 });
  });

  it("moves a taller PTY up as far as its cursor needs", () => {
    const taller = { pane, pty: { cols: 120, rows: 50, policy: "active" as const } };
    expect(markerLayout(taller, cell, box, 5)?.shift).toBe(0);
    expect(markerLayout(taller, cell, box, 44)?.shift).toBe(5 * 16);
    expect(markerLayout(taller, cell, box, 49)?.shift).toBe(10 * 16);
    expect(markerLayout(taller, cell, box, 49)?.right).toBeNull();
    expect(markerLayout(taller, cell, box, 49)?.bottom).toBeNull();
  });

  it("keeps a scrollbar that does not overlay the grid next to a narrower terminal", () => {
    const withScrollbar = { ...box, width: box.width + 14 };
    const layout = markerLayout({ pane, pty: { cols: 100, rows: 40, policy: "active" } }, cell, withScrollbar, 0);
    expect(layout?.width).toBe(800 + 14);
    expect(layout?.right?.left).toBe(4 + 800 + 14);
  });

  it("offers no takeover under `smallest`", () => {
    const layout = markerLayout({ pane, pty: { cols: 100, rows: 30, policy: "smallest" } }, cell, box, 0);
    expect(layout?.takeover).toBe(false);
    expect(layout?.title).not.toContain("Click");
  });
});
