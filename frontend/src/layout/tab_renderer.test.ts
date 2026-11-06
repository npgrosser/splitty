// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DockviewApi, TabPartInitParameters } from "dockview-core";
import { ScreenBinding, registerScreenApi, unregisterScreenApi } from "../lib/connection/screen_binding.ts";
import type { ScreenState, ViewerAnnotation } from "../lib/protocol/types.ts";
import { fakeServer } from "../lib/testing/fake_server.ts";
import { registerAnnotationList } from "../lib/viewer/annotations.ts";
import { SplittyTabRenderer } from "./tab_renderer.ts";

// The platform's modules query media features when they load; jsdom has no matchMedia.
vi.hoisted(() => {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (media: string) => ({
      matches: false, media, onchange: null,
      addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false,
    }),
  });
});

const PLAN = { source: { kind: "file" as const, path: "/home/me/app/PLAN.md" }, revision: 1 };

const cleanups: (() => void)[] = [];

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
});

function tab(paneId: string, params: Record<string, unknown>) {
  const { conn } = fakeServer("host:tabs", "tabs");
  const binding = new ScreenBinding("screen-key", conn, "screen-1");
  const containerApi = { id: `dv-${paneId}` } as unknown as DockviewApi;
  registerScreenApi(containerApi, binding);
  const api = { id: paneId, title: "PLAN.md", setActive: vi.fn(), onDidTitleChange: () => ({ dispose() {} }), group: { panels: [] } };
  const renderer = new SplittyTabRenderer();
  renderer.init({ api, containerApi, params, title: "PLAN.md" } as unknown as TabPartInitParameters);
  cleanups.push(() => {
    renderer.dispose();
    unregisterScreenApi(containerApi);
    conn.dispose();
  });
  return { renderer, conn, api };
}

function screen(annotations: ViewerAnnotation[]): ScreenState {
  return {
    id: "screen-1",
    root: { type: "slot", id: "s1", paneIds: ["v1"] },
    panes: { v1: { id: "v1", role: "viewer", viewer: { ...PLAN, annotations } } },
    sessions: {},
  } as unknown as ScreenState;
}

const comment = (n: number): ViewerAnnotation => ({ id: `ann-${n.toString(16).padStart(16, "0")}`, kind: "comment", body: `Remark ${n}.` });

// viewer-annotations, Annotations In The Pane: a tab's marker with the number of the pane's open annotations.
describe("the tab of a viewer pane", () => {
  it("shows how many open annotations the pane holds, and opens its list on a click", () => {
    const { renderer, conn, api } = tab("v1", { viewer: { record: PLAN, supported: true } });
    const marker = renderer.element.querySelector<HTMLElement>(".splitty-tab-annotations")!;
    expect(marker.hidden).toBe(true);

    conn.annotations.snapshot(screen([comment(1), comment(2), comment(3), { ...comment(4), status: "resolved" }]));
    expect(marker.hidden).toBe(false);
    expect(marker.textContent).toBe("3");
    expect(marker.getAttribute("aria-label")).toBe("3 open annotations");

    const show = vi.fn();
    cleanups.push(registerAnnotationList("v1", show));
    marker.click();
    expect(api.setActive).toHaveBeenCalled();
    expect(show).toHaveBeenCalledTimes(1);

    // Resolved ones are not counted.
    conn.annotations.snapshot(screen([{ ...comment(4), status: "resolved" }]));
    expect(marker.hidden).toBe(true);

    conn.annotations.snapshot(screen([comment(1)]));
    expect(marker.getAttribute("aria-label")).toBe("1 open annotation");

    conn.annotations.snapshot(screen([]));
    expect(marker.hidden).toBe(true);
  });

  it("is not on a terminal pane's tab", () => {
    const { renderer } = tab("t1", {});
    expect(renderer.element.querySelector(".splitty-tab-annotations")).toBeNull();
  });
});
