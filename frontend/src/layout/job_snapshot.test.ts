import { afterEach, describe, expect, it, vi } from "vitest";
import { jobCanAttach, jobParams, topology } from "./job_snapshot.ts";
import { applyScreenSnapshot } from "./screen_snapshot.ts";
import { screenState } from "../lib/mux/incoming.ts";
import type { ScreenState } from "../lib/protocol/types.ts";
import type { DockviewApi } from "dockview-core";
import { ScreenBinding } from "../lib/connection/screen_binding.ts";

afterEach(() => vi.unstubAllGlobals());

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
    expect(topology(next, new Set(["job"]))).toEqual(topology(old, new Set()));
    expect(topology(next, new Set())).not.toEqual(topology(old, new Set()));
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
    class Element { isConnected = true; focus = vi.fn(); }
    const focused = new Element();
    vi.stubGlobal("HTMLElement", Element);
    vi.stubGlobal("document", { activeElement: focused });
    const frames: (() => void)[] = [];
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => frames.push(cb));
    const group = { activePanel: undefined as unknown, api: { setSize: vi.fn() } };
    const panels = new Map<string, any>();
    let active: any;
    const panel = (id: string, params = {}) => {
      const result: any = { id, params, group, api: {
        close: vi.fn(() => panels.delete(id)), setActive: vi.fn(() => { active = result; group.activePanel = result; }),
      } };
      panels.set(id, result); return result;
    };
    const user = panel("user"); active = user; group.activePanel = user;
    const api = { get panels() { return [...panels.values()]; }, get activePanel() { return active; }, groups: [group],
      getPanel: (id: string) => panels.get(id), addPanel: (options: any) => { const p = panel(options.id, options.params); p.api.setActive(); return p; }, width: 800, height: 600 } as unknown as DockviewApi;
    const binding = { suppressOps: false, suppressFocus: false, isActive: () => true,
      sessionForPane: vi.fn(), clearFloatingIds: vi.fn(), registerFloatingId: vi.fn(),
      activeTabs: new Set(), noteActiveTab: ScreenBinding.prototype.noteActiveTab } as unknown as ScreenBinding;
    applyScreenSnapshot(api, binding, placedScreen());
    expect(user.api.close).not.toHaveBeenCalled();
    expect(active).toBe(user);
    expect(focused.focus).toHaveBeenCalled();
    expect(panels.get("job").params.jobId).toBe("job-id");
    const job = panels.get("job");
    // A newer explicit user choice must survive later metadata snapshots and
    // previously scheduled animation-frame callbacks.
    job.api.setActive();
    applyScreenSnapshot(api, binding, placedScreen("exited"));
    expect(panels.get("job")).toBe(job);
    expect(active).toBe(job);
    for (const frame of frames) frame();
    expect(active).toBe(job);
    expect(binding.suppressOps).toBe(false);
    focused.isConnected = false;
    const calls = focused.focus.mock.calls.length;
    const removed = screen(); delete removed.panes.job; removed.root.paneIds = ["user"]; removed.jobs = {};
    applyScreenSnapshot(api, binding, removed);
    expect(panels.has("job")).toBe(false);
    expect(focused.focus.mock.calls.length).toBe(calls);
  });
});
