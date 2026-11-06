import type { DockviewApi } from "dockview-core";
import { describe, expect, it, vi } from "vitest";

const sessions: Record<string, unknown> = {};
vi.mock("../lib/connection/screen_binding.ts", () => ({
  sessionForPane: (_api: unknown, paneId: string) => sessions[paneId],
}));

import { extractTemplate } from "./template_extract.ts";

const terminal = (id: string) => ({ id, params: {}, view: { contentComponent: "terminal" } });
const viewer = (id: string) => ({ id, params: { viewer: { supported: true } }, view: { contentComponent: "viewer" } });

interface Group {
  id: string;
  panels: object[];
  floating?: boolean;
}

function apiWith(groups: Group[], root: unknown): DockviewApi {
  return {
    width: 1000,
    height: 800,
    groups: groups.map((group) => ({
      id: group.id,
      panels: group.panels,
      api: { location: { type: group.floating ? "floating" : "grid" } },
      element: {
        getBoundingClientRect: () => ({ left: 100, top: 80, width: 500, height: 400 }),
        closest: () => ({ getBoundingClientRect: () => ({ left: 0, top: 0 }) }),
      },
    })),
    toJSON: () => ({ grid: { orientation: "HORIZONTAL", root } }),
  } as unknown as DockviewApi;
}

const leaf = (id: string, size: number) => ({ type: "leaf", data: { id, views: [] }, size });

// viewer-panes: a template starts terminals only, so viewer panes are not
// part of one.
describe("extractTemplate with viewer panes", () => {
  it("leaves viewer panes out of their group", () => {
    sessions.t1 = { lastCwd: "/home/me/app" };
    const api = apiWith(
      [{ id: "g1", panels: [terminal("t1"), viewer("v1")] }, { id: "g2", panels: [terminal("t2")] }],
      { type: "branch", data: [leaf("g1", 600), leaf("g2", 400)], size: 800 },
    );
    const template = extractTemplate(api, "work");
    expect(template.panes).toEqual({ shell: { dir: "/home/me/app" }, shell1: {} });
    expect(template.layout).toEqual({ split: "horizontal", children: [{ slot: ["shell"], ratio: 0.6 }, { slot: ["shell1"], ratio: 0.4 }] });
  });

  it("drops a group of viewer panes only, and its siblings share its space", () => {
    const api = apiWith(
      [{ id: "g1", panels: [terminal("t1")] }, { id: "g2", panels: [viewer("v1")] }, { id: "g3", panels: [terminal("t3")] }],
      { type: "branch", data: [leaf("g1", 300), leaf("g2", 400), leaf("g3", 300)], size: 1000 },
    );
    expect(extractTemplate(api, "work").layout).toEqual({
      split: "horizontal", children: [{ slot: ["shell"], ratio: 0.5 }, { slot: ["shell1"], ratio: 0.5 }],
    });

    const pair = apiWith(
      [{ id: "g1", panels: [terminal("t1")] }, { id: "g2", panels: [viewer("v1"), viewer("v2")] }],
      { type: "branch", data: [leaf("g1", 500), { type: "branch", data: [leaf("g2", 400)], size: 500 }], size: 1000 },
    );
    expect(extractTemplate(pair, "work").layout).toEqual({ slot: ["shell"] });
  });

  it("leaves out a floating group of viewer panes only", () => {
    const api = apiWith(
      [
        { id: "g1", panels: [terminal("t1")] },
        { id: "f1", panels: [viewer("v1")], floating: true },
        { id: "f2", panels: [viewer("v2"), terminal("t2")], floating: true },
      ],
      leaf("g1", 1000),
    );
    const template = extractTemplate(api, "work");
    expect(template.layout).toEqual({ slot: ["shell"] });
    expect(template.floating).toEqual([{ layout: { slot: ["shell1"] }, bounds: { x: 0.1, y: 0.1, w: 0.5, h: 0.5 } }]);
  });
});
