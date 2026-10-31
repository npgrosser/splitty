import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ScreenState } from "./protocol/types.ts";
import {
  byteLength, FONT_SIZE_MAX, FONT_SIZE_MIN, SAVE_DELAY_MS, SCRATCHPAD_LIMIT, Scratchpads, SETTLE_MS, steppedFontSize, withinLimit,
} from "./scratchpad.ts";

function screen(id: string, sessions: Record<string, string | undefined>, panes: Record<string, string> = {}): ScreenState {
  return {
    id,
    revision: 1,
    root: { type: "slot", id: "slot", paneIds: Object.keys(panes) },
    floating: [],
    panes: Object.fromEntries(Object.entries(panes).map(([pane, session]) => [pane, { id: pane, sessionId: session, role: "terminal" as const }])),
    sessions: Object.fromEntries(Object.entries(sessions).map(([session, scratchpad]) => [
      session,
      { id: session, status: "running" as const, ...(scratchpad === undefined ? {} : { scratchpad }) },
    ])),
  };
}

let saved: [string, string, string][];
let pads: Scratchpads;

beforeEach(() => {
  vi.useFakeTimers();
  saved = [];
  pads = new Scratchpads((screenId, sessionId, text) => saved.push([screenId, sessionId, text]));
});

afterEach(() => vi.useRealTimers());

describe("scratchpads", () => {
  // session-scratchpad, Saving Through The Window
  it("shows an edit at once and saves the whole text once typing pauses", () => {
    const seen: string[] = [];
    pads.subscribe(() => seen.push(pads.text("sess-1")));
    pads.edit("screen-1", "sess-1", "a");
    pads.edit("screen-1", "sess-1", "ab");
    expect(pads.text("sess-1")).toBe("ab");
    expect(pads.unsaved("sess-1")).toBe(true);
    vi.advanceTimersByTime(SAVE_DELAY_MS - 1);
    expect(saved).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(saved).toEqual([["screen-1", "sess-1", "ab"]]);
    expect(pads.unsaved("sess-1")).toBe(false);
    expect(seen).toEqual(["a", "ab"]);
  });

  it("saves at once on flush, and only once", () => {
    pads.edit("screen-1", "sess-1", "draft");
    pads.flush("sess-1");
    pads.flush("sess-1");
    vi.advanceTimersByTime(SAVE_DELAY_MS * 4);
    expect(saved).toEqual([["screen-1", "sess-1", "draft"]]);
  });

  it("takes a snapshot's texts and panes", () => {
    pads.snapshot(screen("screen-1", { "sess-1": "from the server", "sess-2": undefined }, { "terminal-1": "sess-1" }));
    expect(pads.text("sess-1")).toBe("from the server");
    expect(pads.text("sess-2")).toBe("");
    expect(pads.sessionOf("terminal-1")).toBe("sess-1");
    pads.link("terminal-2", "sess-2");
    expect(pads.sessionOf("terminal-2")).toBe("sess-2");
  });

  // session-scratchpad, Concurrent Edits (Unsaved edit is kept)
  it("keeps an unsaved edit over a newer server text, then saves it", () => {
    pads.snapshot(screen("screen-1", { "sess-1": "old" }));
    pads.edit("screen-1", "sess-1", "mine");
    pads.snapshot(screen("screen-1", { "sess-1": "theirs" }));
    expect(pads.text("sess-1")).toBe("mine");
    vi.advanceTimersByTime(SAVE_DELAY_MS);
    expect(saved).toEqual([["screen-1", "sess-1", "mine"]]);
  });

  it("lets a saved text outlast snapshots the server made before the save", () => {
    pads.edit("screen-1", "sess-1", "saved");
    pads.flush("sess-1");
    pads.snapshot(screen("screen-1", { "sess-1": "older" }));
    expect(pads.text("sess-1")).toBe("saved");
    vi.advanceTimersByTime(SETTLE_MS);
    pads.snapshot(screen("screen-1", { "sess-1": "later, from another window" }));
    expect(pads.text("sess-1")).toBe("later, from another window");
  });

  it("forgets a session its screen no longer lists, unless another screen does", () => {
    pads.snapshot(screen("screen-1", { "sess-1": "moves", "sess-2": "closes" }));
    pads.snapshot(screen("screen-2", { "sess-1": "moves" }));
    pads.snapshot(screen("screen-1", {}));
    expect(pads.text("sess-1")).toBe("moves");
    expect(pads.text("sess-2")).toBe("");
  });

  // session-scratchpad, Scratchpad Per Session: at most 64 KiB of UTF-8
  it("keeps edits within the limit, at a character boundary", () => {
    const fits = "ä".repeat(SCRATCHPAD_LIMIT / 2);
    expect(withinLimit(fits)).toBe(fits);
    const cut = withinLimit("a" + fits);
    expect(byteLength(cut)).toBeLessThanOrEqual(SCRATCHPAD_LIMIT);
    expect(cut.endsWith("ä")).toBe(true);
    expect(byteLength(withinLimit("🙂".repeat(SCRATCHPAD_LIMIT)))).toBe(SCRATCHPAD_LIMIT);
    expect(pads.edit("screen-1", "sess-1", "x".repeat(SCRATCHPAD_LIMIT + 10))).toHaveLength(SCRATCHPAD_LIMIT);
  });
});

describe("the scratchpad font size", () => {
  it("steps by one with the wheel and stays within its limits", () => {
    expect(steppedFontSize(12, -100)).toBe(13);
    expect(steppedFontSize(12, 100)).toBe(11);
    expect(steppedFontSize(FONT_SIZE_MAX, -100)).toBe(FONT_SIZE_MAX);
    expect(steppedFontSize(FONT_SIZE_MIN, 100)).toBe(FONT_SIZE_MIN);
  });
});
