import { afterEach, describe, expect, it } from "vitest";
import { MAX_KEPT_STATES, dropViewerStates, keepViewerState, takeViewerState } from "./viewer_view_state.ts";

afterEach(() => {
  dropViewerStates("screen-1", () => false);
  dropViewerStates("screen-2", () => false);
});

describe("viewer view states", () => {
  it("gives a kept state to the pane's next view once", () => {
    keepViewerState("v1", "screen-1", { mode: "source", wrap: false, position: { line: 120 } });
    expect(takeViewerState("v1")).toEqual({ mode: "source", wrap: false, position: { line: 120 } });
    expect(takeViewerState("v1")).toBeUndefined();
    expect(takeViewerState("v2")).toBeUndefined();
  });

  it("keeps the newest state of a pane", () => {
    keepViewerState("v1", "screen-1", { mode: "source" });
    keepViewerState("v1", "screen-1", { mode: "rendered", position: { fraction: 0.5 } });
    expect(takeViewerState("v1")).toEqual({ mode: "rendered", position: { fraction: 0.5 } });
  });

  it("drops the states of the panes a screen no longer holds, and only that screen's", () => {
    keepViewerState("v1", "screen-1", { mode: "source" });
    keepViewerState("v2", "screen-1", { mode: "source" });
    keepViewerState("v3", "screen-2", { mode: "source" });
    dropViewerStates("screen-1", (paneId) => paneId === "v2");
    expect(takeViewerState("v1")).toBeUndefined();
    expect(takeViewerState("v2")).toEqual({ mode: "source" });
    expect(takeViewerState("v3")).toEqual({ mode: "source" });
  });

  it("holds a bounded number of states and drops the oldest first", () => {
    for (let i = 0; i <= MAX_KEPT_STATES; i++) keepViewerState(`v${i}`, "screen-1", { mode: "source", position: { line: i } });
    expect(takeViewerState("v0")).toBeUndefined();
    expect(takeViewerState("v1")).toEqual({ mode: "source", position: { line: 1 } });
    expect(takeViewerState(`v${MAX_KEPT_STATES}`)).toEqual({ mode: "source", position: { line: MAX_KEPT_STATES } });
  });
});
