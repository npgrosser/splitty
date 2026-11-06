import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MuxOperation, PaneState, ScreenState, ViewerAnnotation } from "../protocol/types.ts";
import {
  ANNOTATION_COUNT_LIMIT,
  ANNOTATION_SETTLE_MS,
  ViewerAnnotations,
  annotationProblem,
  isAnnotationId,
  isBlank,
  newAnnotationId,
  paneProblem,
  receivedAnnotations,
  registerAnnotationList,
  showAnnotationList,
} from "./annotations.ts";

const PLAN = { source: { kind: "file" as const, path: "/home/me/app/PLAN.md" }, revision: 1 };

let serial = 0;
function id(): string {
  return `ann-${(serial++).toString(16).padStart(16, "0")}`;
}

function question(overrides: Partial<ViewerAnnotation> = {}): ViewerAnnotation {
  return {
    id: id(),
    kind: "question",
    body: "Why three?",
    target: { view: "source", quote: "retry three times", prefix: "2. ", suffix: " on failure", startLine: 30, endLine: 30 },
    ...overrides,
  };
}

function comment(body = "Looks good overall."): ViewerAnnotation {
  return { id: id(), kind: "comment", body };
}

function screen(annotations: Record<string, ViewerAnnotation[] | undefined>, screenId = "screen-1"): ScreenState {
  const panes: Record<string, PaneState> = {
    t1: { id: "t1", role: "terminal" },
  };
  for (const [paneId, list] of Object.entries(annotations)) {
    panes[paneId] = { id: paneId, role: "viewer", viewer: { ...PLAN, ...(list ? { annotations: list } : {}) } };
  }
  return { id: screenId, root: { type: "slot", id: "s1", paneIds: Object.keys(panes) }, panes, sessions: {} } as unknown as ScreenState;
}

describe("annotation rules", () => {
  it("makes IDs of the form the server takes", () => {
    const made = new Set(Array.from({ length: 50 }, () => newAnnotationId()));
    expect(made.size).toBe(50);
    for (const each of made) expect(isAnnotationId(each)).toBe(true);
    expect(isAnnotationId("ann-0123456789ABCDEF")).toBe(false);
    expect(isAnnotationId("ann-0123")).toBe(false);
    expect(isAnnotationId("note-0123456789abcdef")).toBe(false);
    expect(isAnnotationId(7)).toBe(false);
  });

  it("counts Unicode White_Space as blank, and U+FEFF only for what the window sends", () => {
    expect(isBlank(" \t\n\r\u000b\u000c")).toBe(true);
    expect(isBlank("\u0085   　")).toBe(true);
    expect(isBlank("﻿")).toBe(true);
    expect(isBlank("﻿", false)).toBe(false);
    expect(isBlank("​")).toBe(false);
    expect(isBlank(" a ")).toBe(false);
  });

  it("says why the server would refuse an annotation", () => {
    expect(annotationProblem(question())).toBeUndefined();
    expect(annotationProblem(comment())).toBeUndefined();
    expect(annotationProblem({ ...comment(), id: "ann-1" })).toBe("The annotation has no valid ID.");
    expect(annotationProblem({ ...comment(), body: " \u0085 " })).toBe("A comment needs a remark.");
    expect(annotationProblem({ ...question(), body: "﻿" })).toBe("A question needs a remark.");
    expect(annotationProblem({ ...question(), body: "﻿" }, false)).toBeUndefined();
    expect(annotationProblem({ ...question(), kind: "change" })).toBe("A suggested change needs a replacement.");
    expect(annotationProblem({ ...question(), kind: "change", replacement: "retry twice" })).toBeUndefined();
    expect(annotationProblem({ id: id(), kind: "delete" })).toBe("A suggested deletion needs a passage.");
    expect(annotationProblem({ id: id(), kind: "change", replacement: "x" })).toBe("A suggested change needs a passage.");
    expect(annotationProblem({ ...question({ kind: "delete", body: undefined }) })).toBeUndefined();

    const target = question().target!;
    expect(annotationProblem(question({ target: { ...target, quote: "" } }))).toBe("The passage is empty.");
    for (const lines of [{ startLine: 0, endLine: 1 }, { startLine: 3, endLine: 2 }, { startLine: 1, endLine: 10_000_001 }, { startLine: 1.5, endLine: 2 }]) {
      expect(annotationProblem(question({ target: { ...target, ...lines } })), JSON.stringify(lines)).toBe("The lines of the passage are invalid.");
    }
    expect(annotationProblem(question({ target: { ...target, startLine: 10_000_000, endLine: 10_000_000 } }))).toBeUndefined();
  });

  it("measures the limits in bytes of UTF-8", () => {
    const target = question().target!;
    expect(annotationProblem(comment("é".repeat(8192)))).toBeUndefined();
    expect(annotationProblem(comment(`${"é".repeat(8192)}a`))).toBe("The remark is too long: it may hold at most 16 KB.");
    expect(annotationProblem(question({ target: { ...target, quote: "x".repeat(16_385) } }))).toBe("The passage is too long to annotate: it may hold at most 16 KB.");
    expect(annotationProblem(question({ target: { ...target, prefix: "€".repeat(86) } }))).toBe("The context of the passage is too long.");
    expect(annotationProblem(question({ kind: "change", replacement: "x".repeat(16_385) }))).toBe("The replacement is too long: it may hold at most 16 KB.");

    // viewer-annotations, Annotations On A Viewer Pane: One annotation too many.
    expect(paneProblem(Array.from({ length: 100 }, () => comment()))).toBeUndefined();
    expect(paneProblem(Array.from({ length: 101 }, () => comment()))).toBe("A pane holds at most 100 annotations.");
    expect(paneProblem(Array.from({ length: 17 }, () => comment("x".repeat(16_000))))).toBe("The annotations of this pane would hold more than 256 KB of text.");
  });

  it("keeps a snapshot's annotations by the server's rules and drops malformed ones", () => {
    const kept = question();
    const fromServer = { ...comment(), body: "﻿" };
    const general = comment();
    const received = receivedAnnotations([
      { ...kept, extra: 1, target: { ...kept.target, extra: 2 } },
      kept,
      { id: "ann-bad", kind: "comment", body: "x" },
      { id: id(), kind: "praise", body: "x" },
      { id: id(), kind: "comment" },
      { id: id(), kind: "comment", body: 7 },
      { id: id(), kind: "delete", target: { view: "both", quote: "q", prefix: "", suffix: "", startLine: 1, endLine: 1 } },
      { id: id(), kind: "delete", target: { view: "source", quote: "q", startLine: 1, endLine: 1 } },
      fromServer,
      { ...general, replacement: null, target: null },
      "not an annotation",
    ]);
    expect(received).toEqual([kept, fromServer, general]);
    expect(received[0]).not.toHaveProperty("extra");
    expect(received[0].target).not.toHaveProperty("extra");
    expect(receivedAnnotations(undefined)).toEqual([]);
    expect(receivedAnnotations({ length: 1 })).toEqual([]);
  });

  it("stops a snapshot's list at the pane's limits", () => {
    expect(receivedAnnotations(Array.from({ length: 120 }, () => comment()))).toHaveLength(ANNOTATION_COUNT_LIMIT);
    const big = Array.from({ length: 20 }, () => comment("x".repeat(16_000)));
    const small = comment("short");
    const received = receivedAnnotations([...big, small]);
    // 16 of 16,000 bytes fit into 262,144; the 17th does not, a short one after it does.
    expect(received).toHaveLength(17);
    expect(received.at(-1)).toEqual(small);
  });
});

describe("ViewerAnnotations", () => {
  let sent: { screenId: string; op: MuxOperation }[];
  let resyncs: string[];
  let store: ViewerAnnotations;

  beforeEach(() => {
    vi.useFakeTimers();
    sent = [];
    resyncs = [];
    store = new ViewerAnnotations({ send: (screenId, op) => sent.push({ screenId, op }), resync: (screenId) => resyncs.push(screenId) });
  });

  afterEach(() => {
    store.dispose();
    vi.useRealTimers();
  });

  it("shows a snapshot's annotations per viewer pane, in their order", () => {
    const first = question();
    const second = comment();
    const changes = vi.fn();
    store.subscribe(changes);
    store.snapshot(screen({ v1: [first, second], v2: undefined }));
    expect(store.list("v1")).toEqual([first, second]);
    expect(store.count("v2")).toBe(0);
    expect(changes).toHaveBeenCalledTimes(1);

    // The same list again changes nothing, and keeps the array.
    const list = store.list("v1");
    store.snapshot(screen({ v1: [first, second], v2: undefined }));
    expect(store.list("v1")).toBe(list);
    expect(changes).toHaveBeenCalledTimes(1);
  });

  // viewer-annotations, Saving Annotations Through The Window: shown at once, sent without waiting.
  it("shows the window's own annotation at once and sends it", () => {
    store.snapshot(screen({ v1: [] }));
    const annotation = question({ body: "  Why three?  " });
    expect(store.put("screen-1", "v1", [annotation])).toBeUndefined();
    expect(store.list("v1")).toEqual([annotation]);
    expect(sent).toEqual([{ screenId: "screen-1", op: { type: "put_annotations", paneId: "v1", annotations: [annotation] } }]);
  });

  it("sends only the fields an annotation's kind uses, and well-formed text", () => {
    const change = question({ kind: "change", body: " \n ", replacement: "retry \ud800twice" });
    const deletion = { ...question({ kind: "delete" }), replacement: "ignored" };
    store.put("screen-1", "v1", [change, deletion]);
    const op = sent[0].op as Extract<MuxOperation, { type: "put_annotations" }>;
    expect(op.annotations[0]).toEqual({ id: change.id, kind: "change", replacement: "retry �twice", target: change.target });
    expect(op.annotations[1]).not.toHaveProperty("replacement");
    expect(store.list("v1")).toEqual(op.annotations);
  });

  it("refuses what the server would refuse, and changes nothing then", () => {
    const blank = { ...comment(), body: "﻿" };
    expect(store.put("screen-1", "v1", [blank])).toBe("A comment needs a remark.");
    const twice = comment();
    expect(store.put("screen-1", "v1", [twice, twice])).toBe("The same annotation is saved twice.");
    expect(store.put("screen-1", "v1", Array.from({ length: 101 }, () => comment()))).toBe("At most 100 annotations can be saved at once.");
    store.snapshot(screen({ v1: Array.from({ length: 100 }, () => comment()) }));
    // viewer-annotations, Annotations On A Viewer Pane: One annotation too many.
    expect(store.put("screen-1", "v1", [comment()])).toBe("A pane holds at most 100 annotations.");
    // Replacing one of the 100 stays within the limit.
    const replaced = { ...store.list("v1")[5], body: "Changed." };
    expect(store.put("screen-1", "v1", [replaced])).toBeUndefined();
    expect(store.list("v1")[5]).toEqual(replaced);
    expect(store.count("v1")).toBe(100);
    expect(sent).toHaveLength(1);
  });

  it("puts back what the server kept before by the server's rules", () => {
    const kept = { ...comment(), body: "﻿" };
    expect(store.put("screen-1", "v1", [kept], { restore: true })).toBeUndefined();
    expect(store.list("v1")).toEqual([kept]);
  });

  it("replaces an annotation in its place and removes others at once", () => {
    const [a, b, c] = [question(), comment(), comment("Third.")];
    store.snapshot(screen({ v1: [a, b, c] }));
    store.put("screen-1", "v1", [{ ...b, body: "Changed." }]);
    expect(store.list("v1").map((each) => each.body)).toEqual(["Why three?", "Changed.", "Third."]);
    store.remove("screen-1", "v1", [a.id, "ann-unknown", c.id, a.id]);
    expect(store.list("v1").map((each) => each.id)).toEqual([b.id]);
    expect(sent.at(-1)).toEqual({ screenId: "screen-1", op: { type: "remove_annotations", paneId: "v1", ids: [a.id, c.id] } });
  });

  it("removes more than 100 annotations in several operations", () => {
    const ids = Array.from({ length: 150 }, () => id());
    store.remove("screen-1", "v1", ids);
    expect(sent.map(({ op }) => (op as { ids: string[] }).ids.length)).toEqual([100, 50]);
  });

  // viewer-annotations, Concurrent Annotation Edits: An older snapshot arrives.
  it("keeps showing its own annotation over a snapshot made before the operation arrived", () => {
    store.snapshot(screen({ v1: [] }));
    const annotation = question();
    store.put("screen-1", "v1", [annotation]);

    store.snapshot(screen({ v1: [] }));
    expect(store.list("v1")).toEqual([annotation]);
    expect(resyncs).toEqual([]);

    // The hold ends; the window asks for the screen once more and shows what the server holds.
    vi.advanceTimersByTime(ANNOTATION_SETTLE_MS + 100);
    expect(resyncs).toEqual(["screen-1"]);
    store.snapshot(screen({ v1: [annotation] }));
    expect(store.list("v1")).toEqual([annotation]);
  });

  it("drops its own state once a snapshot shows it, and follows later snapshots", () => {
    const annotation = question();
    store.put("screen-1", "v1", [annotation]);
    store.snapshot(screen({ v1: [annotation] }));
    // Another window removes it: the next snapshot shows that at once.
    store.snapshot(screen({ v1: [] }));
    expect(store.list("v1")).toEqual([]);
    vi.advanceTimersByTime(ANNOTATION_SETTLE_MS + 100);
    expect(resyncs).toEqual([]);
  });

  it("keeps a removal over an older snapshot, then shows what the server holds", () => {
    const annotation = question();
    store.snapshot(screen({ v1: [annotation] }));
    store.remove("screen-1", "v1", [annotation.id]);
    store.snapshot(screen({ v1: [annotation] }));
    expect(store.list("v1")).toEqual([]);
    vi.advanceTimersByTime(ANNOTATION_SETTLE_MS + 100);
    // A snapshot after the hold shows the server's state, whatever it is.
    store.snapshot(screen({ v1: [annotation] }));
    expect(store.list("v1")).toEqual([annotation]);
    expect(resyncs).toEqual(["screen-1"]);
  });

  // viewer-annotations, Saving Annotations Through The Window: Two windows add at the same time.
  it("shows another window's annotation next to its own before the server has both", () => {
    const own = question();
    const theirs = comment("From the other window.");
    store.snapshot(screen({ v1: [] }));
    store.put("screen-1", "v1", [own]);
    store.snapshot(screen({ v1: [theirs] }));
    expect(store.list("v1")).toEqual([theirs, own]);
    store.snapshot(screen({ v1: [theirs, own] }));
    expect(store.list("v1")).toEqual([theirs, own]);
  });

  // viewer-annotations, Saving Annotations Through The Window: A second window sees the annotation.
  it("shows an annotation another window added with the next snapshot", () => {
    store.snapshot(screen({ v1: [] }));
    const theirs = comment("From the other window.");
    store.snapshot(screen({ v1: [theirs] }));
    expect(store.list("v1")).toEqual([theirs]);
    expect(sent).toEqual([]);
  });

  it("drops its own state when the server rejects the operation, and says so", () => {
    const kept = comment();
    store.snapshot(screen({ v1: [kept] }));
    const rejected = vi.fn();
    store.onRejected(rejected);
    store.put("screen-1", "v1", [question()]);
    expect(store.count("v1")).toBe(2);
    store.failed({ op: "put_annotations", screenId: "screen-1", paneId: "v1", error: "pane v1 holds 100 annotations" });
    expect(store.list("v1")).toEqual([kept]);
    expect(rejected).toHaveBeenCalledWith("v1", "pane v1 holds 100 annotations");
    store.failed({ op: "arrange_panes", screenId: "screen-1", paneId: "v1", error: "other" });
    expect(rejected).toHaveBeenCalledTimes(1);
  });

  it("forgets a pane its screen no longer lists, unless another screen does", () => {
    const annotation = question();
    store.snapshot(screen({ v1: [annotation], v2: [comment()] }));
    // v1 moved to another screen.
    store.snapshot(screen({ v1: [annotation] }, "screen-2"));
    store.snapshot(screen({}));
    expect(store.list("v1")).toEqual([annotation]);
    expect(store.count("v2")).toBe(0);
    store.snapshot(screen({}, "screen-2"));
    expect(store.count("v1")).toBe(0);
  });

  it("counts the annotations of the panes the window shows", () => {
    store.snapshot(screen({ v1: [question(), comment()], v2: [comment()] }));
    expect(store.shownCount()).toBe(0);
    const one = store.mount("v1");
    const two = store.mount("v1");
    expect(store.shownCount()).toBe(2);
    one();
    one();
    expect(store.shownCount()).toBe(2);
    two();
    expect(store.shownCount()).toBe(0);
  });

  it("opens a mounted pane's list for the tab's marker", () => {
    const show = vi.fn();
    const unregister = registerAnnotationList("v1", show);
    expect(showAnnotationList("v1")).toBe(true);
    expect(show).toHaveBeenCalledTimes(1);
    unregister();
    expect(showAnnotationList("v1")).toBe(false);
  });
});
