import { describe, expect, it, vi } from "vitest";
import { mountManagedJob } from "./managed_job_mount.ts";

describe("managed pane mounting", () => {
  it("reloads failed, interrupted and missing sessions without attaching or creating a shell", () => {
    const actions = { attach: vi.fn(async () => "attached"), showOutcome: vi.fn() };
    for (const state of ["failed", "interrupted", "starting", undefined]) {
      expect(mountManagedJob({ jobId: "job", jobState: state, existingSessionId: "old" }, actions)).toEqual({});
    }
    expect(mountManagedJob({ jobId: "job", jobState: "running" }, actions)).toEqual({});
    expect(actions.attach).not.toHaveBeenCalled();
    expect(actions.showOutcome).toHaveBeenCalledTimes(5);
  });
  it("reattaches running and exited jobs to exactly the known session without a fallback on failure", async () => {
    const actions = { attach: vi.fn(async (id: string) => id), showOutcome: vi.fn() };
    for (const state of ["running", "stopping", "exited"]) {
      await expect(mountManagedJob({ jobId: "job", jobState: state, existingSessionId: "session" }, actions)?.promise).resolves.toBe("session");
    }
    const failed = { attach: vi.fn(async () => { throw new Error("session unavailable"); }), showOutcome: vi.fn() };
    await expect(mountManagedJob({ jobId: "job", jobState: "exited", existingSessionId: "session" }, failed)?.promise).rejects.toThrow("session unavailable");
    expect(failed.attach).toHaveBeenCalledTimes(1);
    expect(mountManagedJob({}, actions)).toBeUndefined();
  });
});
