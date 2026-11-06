// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { layoutArrangement, sameShape, withoutPanes } from "./arrangement.ts";
import { jobCanAttach, jobParams } from "./job_snapshot.ts";
import { applyScreenSnapshot } from "./screen_snapshot.ts";
import { screenState } from "../lib/mux/incoming.ts";
import type { LayoutNode, ScreenState } from "../lib/protocol/types.ts";
import { testScreen, type TerminalStandIn, type TestScreen } from "../lib/testing/screen_layout.ts";

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

let layouts: TestScreen[] = [];

afterEach(() => {
  vi.unstubAllGlobals();
  for (const layout of layouts) layout.dispose();
  layouts = [];
});

function screen(state: "running" | "failed" | "interrupted" | "exited" = "running"): ScreenState {
  return { id: "s", revision: 2, root: { type: "slot", id: "slot", paneIds: ["user", "job"], activePaneId: "user" },
    floating: [], panes: { user: { id: "user", sessionId: "u" }, job: { id: "job", sessionId: "j", jobId: "job-id" } },
    sessions: {}, jobs: { "job-id": { jobId: "job-id", state, error: state === "failed" ? "missing program" : null } } };
}

describe("managed job snapshots", () => {
  it("preserves job identity through the untrusted wire boundary and fails closed on unknown states", () => {
    const parsed = screenState(screen("failed"))!;
    expect(jobParams(parsed, "job")).toEqual({ jobId: "job-id", jobState: "failed", jobError: "missing program" });
    const raw = { ...screen(), jobs: { "job-id": { state: "not-a-state", error: "\x1b[31muntrusted" } } };
    expect(jobParams(screenState(raw)!, "job").jobState).toBe("interrupted");
    expect(jobCanAttach(jobParams(screenState(raw)!, "job").jobState)).toBe(false);
  });
  it("never turns an unavailable job into a fresh terminal", () => {
    for (const state of ["starting", "failed", "interrupted", undefined]) expect(jobCanAttach(state)).toBe(false);
    for (const state of ["running", "stopping", "exited"]) expect(jobCanAttach(state)).toBe(true);
    expect(jobParams(screen("failed"), "job")).toEqual({ jobId: "job-id", jobState: "failed", jobError: "missing program" });
  });

  it("recognizes additive job topology without hiding moves of existing panes", () => {
    const old = { type: "slot" as const, id: "a", paneIds: ["user"] };
    const next = { type: "split" as const, id: "root", axis: "horizontal" as const, sizes: [.5, .5], children: [old, {type: "slot" as const, id: "b", paneIds: ["job"]}] };
    const shape = (node: LayoutNode, omit: string[]) => withoutPanes(layoutArrangement(node), new Set(omit));
    expect(sameShape(shape(next, ["job"]), shape(old, []))).toBe(true);
    expect(sameShape(shape(next, []), shape(old, []))).toBe(false);
  });

  it.each(["tab", "left", "right", "above", "below"])("keeps the existing renderer, active tab and actual focus for background %s updates", (placement) => {
    const placedScreen = (state: "running" | "exited" = "running") => {
      const result = screen(state);
      if (placement !== "tab") {
        const user = { type: "slot" as const, id: "user-slot", paneIds: ["user"], activePaneId: "user" };
        const job = { type: "slot" as const, id: "job-slot", paneIds: ["job"], activePaneId: "job" };
        result.root = { type: "split", id: "split", axis: placement === "left" || placement === "right" ? "horizontal" : "vertical", children: placement === "left" || placement === "above" ? [job, user] : [user, job], sizes: [.5, .5] };
      }
      return result;
    };
    const userOnly = () => {
      const result = screen();
      delete result.panes.job;
      result.root = { type: "slot", id: "slot", paneIds: ["user"], activePaneId: "user" };
      result.jobs = {};
      return result;
    };
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => frames.push(cb));
    const layout = testScreen();
    layouts.push(layout);
    const { api, binding } = layout;
    // The window shows the user's terminal, which has the keyboard.
    applyScreenSnapshot(api, binding, userOnly());
    const user = layout.content<TerminalStandIn>("user");
    user.input.focus();

    applyScreenSnapshot(api, binding, placedScreen());
    expect(layout.content("user")).toBe(user);
    expect(api.activePanel?.id).toBe("user");
    expect(document.activeElement).toBe(user.input);
    expect(api.getPanel("job")!.params?.jobId).toBe("job-id");
    const job = layout.content("job");
    // A newer explicit user choice must survive later metadata snapshots and
    // previously scheduled animation-frame callbacks.
    api.getPanel("job")!.api.setActive();
    applyScreenSnapshot(api, binding, placedScreen("exited"));
    expect(layout.content("job")).toBe(job);
    expect(api.activePanel?.id).toBe("job");
    for (const frame of frames.splice(0)) frame(0);
    expect(api.activePanel?.id).toBe("job");
    expect(binding.suppressOps).toBe(false);

    applyScreenSnapshot(api, binding, userOnly());
    expect(api.getPanel("job")).toBeUndefined();
    expect(layout.content("user")).toBe(user);
  });
});
