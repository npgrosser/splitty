import { describe, expect, it } from "vitest";
import { firstViewerOf, viewerDirectoryOf, viewerPaneTitle } from "./panel.ts";

const viewer = (id: string, originPaneId?: string) => ({
  id,
  params: { viewer: { record: { source: { kind: "file", path: `/home/me/app/${id}.md` }, revision: 1, ...(originPaneId ? { originPaneId } : {}) }, supported: true } },
  view: { contentComponent: "viewer" },
});
const terminal = (id: string) => ({ id, params: {}, view: { contentComponent: "terminal" } });

// viewer-panes, Opening A Viewer: viewers opened beside a terminal collect in
// the group of its first viewer.
describe("firstViewerOf", () => {
  it("finds the first viewer of the terminal in layout order", () => {
    const panels = [terminal("p1"), viewer("v1", "p2"), viewer("v2", "p1"), viewer("v3", "p1")];
    expect(firstViewerOf("p1", panels)?.id).toBe("v2");
    expect(firstViewerOf("p1", panels, ["v3", "p1", "v2"])?.id).toBe("v3");
    expect(firstViewerOf("p2", panels)?.id).toBe("v1");
  });

  it("ranks panels the order does not hold after it", () => {
    // v1 floats, v2 is in the grid: the grid comes first.
    const panels = [viewer("v1", "p1"), terminal("p1"), viewer("v2", "p1")];
    expect(firstViewerOf("p1", panels, ["p1", "v2"])?.id).toBe("v2");
    // An order that names only panes that are gone still ranks the rest.
    expect(firstViewerOf("p1", panels, ["gone", "v9"])?.id).toBe("v1");
  });

  it("finds nothing for a terminal without viewers", () => {
    expect(firstViewerOf("p1", [terminal("p1"), viewer("v1"), viewer("v2", "p2")])).toBeUndefined();
    // A terminal panel is never taken for a viewer, whatever its params say.
    expect(firstViewerOf("p1", [{ ...terminal("t2"), params: viewer("x", "p1").params }])).toBeUndefined();
  });
});

describe("viewer panels", () => {
  it("names a viewer pane after its document and gives its directory", () => {
    expect(viewerPaneTitle({ source: { kind: "file", path: "/home/me/app/PLAN.md" }, revision: 1 })).toBe("PLAN.md");
    expect(viewerPaneTitle({ source: { kind: "text", title: "Selection from claude" }, revision: 1 })).toBe("Selection from claude");
    expect(viewerPaneTitle(undefined)).toBe("Viewer");
    expect(viewerDirectoryOf(viewer("PLAN"))).toBe("/home/me/app");
    expect(viewerDirectoryOf(terminal("p1"))).toBeUndefined();
  });
});
