import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AnnotationReply, MuxOperation, PaneState, ScreenState, SentEntry, ViewerAnnotation } from "../protocol/types.ts";
import {
  ANNOTATION_COUNT_LIMIT,
  ANNOTATION_SETTLE_MS,
  REPLY_LIMIT,
  ViewerAnnotations,
  annotationProblem,
  isAnnotationId,
  isBlank,
  isPending,
  isReplyId,
  newAnnotationId,
  newReplyId,
  paneProblem,
  pendingEntries,
  receivedAnnotations,
  registerAnnotationList,
  replyProblem,
  reviewSelection,
  showAnnotationList,
} from "./annotations.ts";

const PLAN = { source: { kind: "file" as const, path: "/home/me/app/PLAN.md" }, revision: 1 };

let serial = 0;
/** Entries of `mark_sent` that name only their IDs; the store marks by ID, the server checks the content. */
function entries(...ids: string[]): SentEntry[] {
  return ids.map((each) => ({ id: each }));
}

function id(): string {
  return `ann-${(serial++).toString(16).padStart(16, "0")}`;
}

function replyId(): string {
  return `rep-${(serial++).toString(16).padStart(16, "0")}`;
}

/** A comment on a passage, as a question is made. */
function question(overrides: Partial<ViewerAnnotation> = {}): ViewerAnnotation {
  return {
    id: id(),
    kind: "comment",
    body: "Why three?",
    target: { view: "source", quote: "retry three times", prefix: "2. ", suffix: " on failure", startLine: 30, endLine: 30 },
    ...overrides,
  };
}

function comment(body = "Looks good overall."): ViewerAnnotation {
  return { id: id(), kind: "comment", body };
}

function agentReply(body = "The list is shared by two windows.", title = "claude"): AnnotationReply {
  return { id: replyId(), author: "agent", body, from: { paneId: "t1", title } };
}

function userReply(body = "Then compute it per connection.", sent = false): AnnotationReply {
  return { id: replyId(), author: "user", body, ...(sent ? { sent: true as const } : {}) };
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
    const replies = new Set(Array.from({ length: 50 }, () => newReplyId()));
    expect(replies.size).toBe(50);
    for (const each of replies) expect(isReplyId(each)).toBe(true);
    expect(isReplyId("ann-0123456789abcdef")).toBe(false);
  });

  it("counts Unicode White_Space as blank, and U+FEFF only for what the window sends", () => {
    expect(isBlank(" \t\n\r\u000b\u000c")).toBe(true);
    expect(isBlank("\u0085   　")).toBe(true);
    expect(isBlank("﻿")).toBe(true);
    expect(isBlank("﻿", false)).toBe(false);
    expect(isBlank("​")).toBe(false);
    expect(isBlank(" a ")).toBe(false);
  });

  it("says why the server would refuse an annotation", () => {
    expect(annotationProblem(question())).toBeUndefined();
    expect(annotationProblem(comment())).toBeUndefined();
    expect(annotationProblem({ ...comment(), id: "ann-1" })).toBe("The annotation has no valid ID.");
    expect(annotationProblem({ ...comment(), kind: "question" as never })).toBe("The annotation has an unknown kind.");
    expect(annotationProblem({ ...comment(), body: " \u0085 " })).toBe("A comment needs a remark.");
    expect(annotationProblem({ ...question(), body: "﻿" })).toBe("A comment needs a remark.");
    expect(annotationProblem({ ...question(), body: "﻿" }, false)).toBeUndefined();
    expect(annotationProblem({ ...question(), kind: "change" })).toBe("A suggested change needs a replacement.");
    expect(annotationProblem({ ...question(), kind: "change", replacement: "retry twice" })).toBeUndefined();
    // viewer-annotations, Annotations On A Viewer Pane: A suggested deletion.
    expect(annotationProblem(question({ kind: "change", body: undefined, replacement: "" }))).toBeUndefined();
    expect(annotationProblem({ id: id(), kind: "change", replacement: "" })).toBe("A suggested change needs a passage.");

    const target = question().target!;
    expect(annotationProblem(question({ target: { ...target, quote: "" } }))).toBe("The passage is empty.");
    for (const lines of [{ startLine: 0, endLine: 1 }, { startLine: 3, endLine: 2 }, { startLine: 1, endLine: 10_000_001 }, { startLine: 1.5, endLine: 2 }]) {
      expect(annotationProblem(question({ target: { ...target, ...lines } })), JSON.stringify(lines)).toBe("The lines of the passage are invalid.");
    }
    expect(annotationProblem(question({ target: { ...target, startLine: 10_000_000, endLine: 10_000_000 } }))).toBeUndefined();
  });

  it("says why the server would refuse a reply of the user", () => {
    expect(replyProblem({ id: replyId(), body: "Fine." })).toBeUndefined();
    expect(replyProblem({ id: "rep-1", body: "Fine." })).toBe("The reply has no valid ID.");
    expect(replyProblem({ id: replyId(), body: " \n " })).toBe("A reply needs a text.");
    expect(replyProblem({ id: replyId(), body: "﻿" })).toBe("A reply needs a text.");
    expect(replyProblem({ id: replyId(), body: "﻿" }, false)).toBeUndefined();
    expect(replyProblem({ id: replyId(), body: "é".repeat(8192) })).toBeUndefined();
    expect(replyProblem({ id: replyId(), body: `${"é".repeat(8192)}a` })).toBe("The reply is too long: it may hold at most 16 KB.");
  });

  it("measures the limits in bytes of UTF-8, replies included", () => {
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
    // Sixteen remarks fit; the replies of a seventeenth thread do not.
    const threads = Array.from({ length: 16 }, () => comment("x".repeat(16_000)));
    expect(paneProblem(threads)).toBeUndefined();
    expect(paneProblem([...threads, { ...comment("short"), replies: [agentReply("y".repeat(6_200))] }])).toBe(
      "The annotations of this pane would hold more than 256 KB of text.",
    );
  });

  it("keeps a snapshot's annotations by the server's rules and drops malformed ones", () => {
    const kept = question({ number: 1 });
    const fromServer = { ...comment(), body: "﻿" };
    const general = comment();
    const received = receivedAnnotations([
      { ...kept, extra: 1, target: { ...kept.target, extra: 2 } },
      kept,
      { id: "ann-bad", kind: "comment", body: "x" },
      { id: id(), kind: "praise", body: "x" },
      { id: id(), kind: "comment" },
      { id: id(), kind: "comment", body: 7 },
      { id: id(), kind: "change", replacement: "", target: { view: "both", quote: "q", prefix: "", suffix: "", startLine: 1, endLine: 1 } },
      { id: id(), kind: "change", replacement: "", target: { view: "source", quote: "q", startLine: 1, endLine: 1 } },
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

  it("reads the kinds of earlier builds: a question as a comment, a deletion as a change with an empty replacement", () => {
    const target = question().target!;
    const [asked, deleted, deletedWithText] = [id(), id(), id()];
    expect(receivedAnnotations([
      { id: asked, kind: "question", body: "Why?", target },
      { id: deleted, kind: "delete", target },
      { id: deletedWithText, kind: "delete", body: "Gone.", replacement: "stray", target },
    ])).toEqual([
      { id: asked, kind: "comment", body: "Why?", target },
      { id: deleted, kind: "change", replacement: "", target },
      { id: deletedWithText, kind: "change", body: "Gone.", replacement: "", target },
    ]);
  });

  it("keeps numbers, statuses and hand-over marks, and drops annotations whose fields are malformed", () => {
    const resolved = question({ number: 2, status: "resolved", sent: true });
    const open = comment();
    const received = receivedAnnotations([
      resolved,
      { ...open, status: "open", sent: "yes" },
      { ...comment(), number: 0 },
      { ...comment(), number: 1.5 },
      { ...comment(), number: "3" },
      { ...comment(), status: "closed" },
    ]);
    expect(received).toEqual([resolved, open]);
  });

  it("keeps replies by the server's rules, with an agent's title as plain text", () => {
    const user = userReply("Then compute it per connection.", true);
    const agent = agentReply();
    const annotation = question({ number: 1 });
    const [received] = receivedAnnotations([{
      ...annotation,
      replies: [
        agent,
        { ...user, from: { paneId: "t1", title: "forged" } },
        { id: replyId(), author: "agent", body: "Marked.", sent: true, from: { paneId: "t2", title: "\u001b]0;evil\u0007 agent‮" } },
        { id: replyId(), author: "agent", body: "Long title.", from: { paneId: "t2", title: "é".repeat(200) } },
        { id: replyId(), author: "system", body: "x" },
        { id: replyId(), author: "user", body: " " },
        { id: "rep-1", author: "user", body: "x" },
        { ...agent, body: "Same ID again." },
        "not a reply",
      ],
    }]);
    const replies = received.replies!;
    expect(replies.slice(0, 2)).toEqual([agent, user]);
    expect(replies[2]).toEqual({ id: replies[2].id, author: "agent", body: "Marked.", from: { paneId: "t2", title: "]0;evil agent" } });
    expect(new TextEncoder().encode(replies[3].from!.title).length).toBeLessThanOrEqual(256);
    expect(replies[3].from!.title).toBe("é".repeat(128));
    expect(replies).toHaveLength(4);
  });

  it("keeps each reply ID once per pane and at most 50 replies per thread", () => {
    const shared = agentReply();
    const many = Array.from({ length: 60 }, () => userReply());
    const [first, second] = receivedAnnotations([
      { ...comment(), replies: [shared, ...many] },
      { ...comment(), replies: [shared, agentReply("Another.")] },
    ]);
    expect(first.replies).toHaveLength(REPLY_LIMIT);
    expect(first.replies![0]).toEqual(shared);
    expect(second.replies!.map((reply) => reply.body)).toEqual(["Another."]);
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

describe("pending entries and the review selection", () => {
  it("names the user's entries that were not handed over", () => {
    const agent = agentReply();
    const mine = userReply();
    const old = userReply("Earlier.", true);
    expect(pendingEntries(question({ replies: [agent, old, mine] }))).toEqual([expect.stringMatching(/^ann-/), mine.id]);
    const handed = question({ sent: true, replies: [agent, old] });
    expect(pendingEntries(handed)).toEqual([]);
    expect(isPending(handed)).toBe(false);
    expect(isPending(question({ sent: true, replies: [mine] }))).toBe(true);
    // A resolved annotation is in no review.
    expect(isPending(question({ status: "resolved" }))).toBe(false);
  });

  it("holds the open annotations with a pending entry, each with its whole thread", () => {
    const one = question({ number: 1, sent: true, replies: [agentReply()] });
    const two = question({ number: 2, sent: true, replies: [agentReply(), userReply()] });
    const three = comment();
    const resolved = { ...comment("Settled."), status: "resolved" as const };
    expect(reviewSelection([one, two, resolved, three])).toEqual({ annotations: [two, three], again: false });
  });

  // viewer-annotations, Handing A Review To A Terminal: Sent again.
  it("Sent again: holds every open annotation when none is pending", () => {
    const one = question({ number: 1, sent: true });
    const two = { ...comment(), number: 2, sent: true as const, replies: [agentReply("Done.")] };
    const resolved = { ...comment("Old."), number: 3, status: "resolved" as const };
    expect(reviewSelection([one, two, resolved])).toEqual({ annotations: [one, two], again: true });
    expect(reviewSelection([resolved])).toEqual({ annotations: [], again: true });
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

  it("counts all annotations of a pane, and its open ones for the tab's marker", () => {
    store.snapshot(screen({ v1: [question(), { ...comment(), status: "resolved" }, comment()] }));
    expect(store.count("v1")).toBe(3);
    expect(store.openCount("v1")).toBe(2);
  });

  // viewer-annotations, Saving Annotations Through The Window: shown at once, sent without waiting.
  it("shows the window's own annotation at once, sends it, and asks for its number", () => {
    store.snapshot(screen({ v1: [] }));
    const annotation = question({ body: "  Why three?  " });
    expect(store.put("screen-1", "v1", [annotation])).toBeUndefined();
    expect(store.list("v1")).toEqual([annotation]);
    expect(sent).toEqual([{ screenId: "screen-1", op: { type: "put_annotations", paneId: "v1", annotations: [annotation] } }]);
    // Only the server numbers it; it sends this window nothing for its own operation.
    expect(resyncs).toEqual(["screen-1"]);
  });

  it("sends only the fields an annotation's kind uses, and well-formed text", () => {
    const change = question({ kind: "change", body: " \n ", replacement: "retry \ud800twice" });
    const deletion = question({ kind: "change", replacement: "" });
    const general = { ...comment(), replacement: "ignored" };
    store.put("screen-1", "v1", [change, deletion, general]);
    const op = sent[0].op as Extract<MuxOperation, { type: "put_annotations" }>;
    expect(op.annotations[0]).toEqual({ id: change.id, kind: "change", replacement: "retry �twice", target: change.target });
    expect(op.annotations[1]).toEqual({ id: deletion.id, kind: "change", body: "Why three?", replacement: "", target: deletion.target });
    expect(op.annotations[2]).not.toHaveProperty("replacement");
    expect(store.list("v1")).toEqual(op.annotations);
  });

  it("puts an annotation without its number and thread, and keeps both from the server", () => {
    const agent = agentReply();
    const held = question({ number: 3, sent: true, replies: [agent] });
    store.snapshot(screen({ v1: [held] }));
    store.put("screen-1", "v1", [{ ...store.get("v1", held.id)!, status: "resolved" }]);
    const op = sent[0].op as Extract<MuxOperation, { type: "put_annotations" }>;
    expect(op.annotations[0]).not.toHaveProperty("number");
    expect(op.annotations[0]).not.toHaveProperty("replies");
    expect(op.annotations[0]).toMatchObject({ status: "resolved", sent: true });
    expect(store.get("v1", held.id)).toEqual({ ...held, status: "resolved" });
    expect(store.openCount("v1")).toBe(0);
    // Replacing a held annotation needs no number from the server.
    expect(resyncs).toEqual([]);
  });

  // viewer-annotations, Saving Annotations Through The Window: An edit after sending.
  it("puts an edit of a handed-over annotation as not handed over", () => {
    const held = question({ number: 1, sent: true });
    store.snapshot(screen({ v1: [held] }));
    const { sent: _sent, ...edited } = { ...held, body: "Why not twice?" };
    store.put("screen-1", "v1", [edited]);
    const op = sent[0].op as Extract<MuxOperation, { type: "put_annotations" }>;
    expect(op.annotations[0]).not.toHaveProperty("sent");
    expect(isPending(store.get("v1", held.id)!)).toBe(true);
    expect(reviewSelection(store.list("v1")).annotations.map((each) => each.id)).toEqual([held.id]);
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

  it("counts the thread a replaced annotation keeps toward the pane's limit", () => {
    const threads = Array.from({ length: 15 }, () => comment("x".repeat(16_000)));
    const held = { ...comment("short"), replies: [agentReply("y".repeat(16_000))] };
    store.snapshot(screen({ v1: [...threads, held] }));
    expect(store.put("screen-1", "v1", [{ ...held, body: "z".repeat(16_000) }])).toBe(
      "The annotations of this pane would hold more than 256 KB of text.",
    );
    expect(sent).toEqual([]);
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
  it("An older snapshot arrives: keeps showing its own annotation", () => {
    store.snapshot(screen({ v1: [] }));
    const annotation = question();
    store.put("screen-1", "v1", [annotation]);
    expect(resyncs).toEqual(["screen-1"]);

    store.snapshot(screen({ v1: [] }));
    expect(store.list("v1")).toEqual([annotation]);

    // The hold ends; the window asks for the screen once more and shows what the server holds.
    vi.advanceTimersByTime(ANNOTATION_SETTLE_MS + 100);
    expect(resyncs).toEqual(["screen-1", "screen-1"]);
    store.snapshot(screen({ v1: [{ ...annotation, number: 1 }] }));
    expect(store.list("v1")).toEqual([{ ...annotation, number: 1 }]);
  });

  it("drops its own state once a snapshot shows it, and follows later snapshots", () => {
    const annotation = question();
    store.put("screen-1", "v1", [annotation]);
    store.snapshot(screen({ v1: [{ ...annotation, number: 1 }] }));
    // Another window removes it: the next snapshot shows that at once.
    store.snapshot(screen({ v1: [] }));
    expect(store.list("v1")).toEqual([]);
    vi.advanceTimersByTime(ANNOTATION_SETTLE_MS + 100);
    expect(resyncs).toEqual(["screen-1"]);
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

  it("asks for the new number of an annotation put back after its removal, and shows neither its old number nor its old thread", () => {
    const annotation = question({ number: 1, replies: [agentReply()] });
    store.snapshot(screen({ v1: [annotation] }));
    store.remove("screen-1", "v1", [annotation.id]);
    const { number: _number, replies: _replies, ...own } = annotation;
    expect(store.put("screen-1", "v1", [own], { restore: true })).toBeUndefined();
    // The last snapshot still holds it as #1 with the agent's reply; the server adds it anew.
    expect(store.list("v1")).toEqual([own]);
    expect(resyncs).toEqual(["screen-1"]);
    store.snapshot(screen({ v1: [{ ...own, number: 2 }] }));
    expect(store.list("v1")).toEqual([{ ...own, number: 2 }]);
  });

  // viewer-annotations, Concurrent Annotation Edits: An agent replies while the user edits.
  it("An agent replies while the user edits: shows the new remark and the agent's reply", () => {
    const held = question({ number: 1, sent: true });
    store.snapshot(screen({ v1: [held] }));
    store.put("screen-1", "v1", [{ ...store.get("v1", held.id)!, body: "New remark." }]);
    const agent = agentReply("On it.");
    // A snapshot the server made before the window's operation arrived, with the agent's reply.
    store.snapshot(screen({ v1: [{ ...held, replies: [agent] }] }));
    expect(store.get("v1", held.id)).toEqual({ ...held, body: "New remark.", replies: [agent] });
    // The server kept the reply when the operation replaced the annotation.
    store.snapshot(screen({ v1: [{ ...held, body: "New remark.", replies: [agent] }] }));
    expect(store.get("v1", held.id)).toEqual({ ...held, body: "New remark.", replies: [agent] });
  });

  // viewer-annotations, Saving Annotations Through The Window: Two windows add at the same time.
  it("shows another window's annotation next to its own before the server has both", () => {
    const own = question();
    const theirs = comment("From the other window.");
    store.snapshot(screen({ v1: [] }));
    store.put("screen-1", "v1", [own]);
    store.snapshot(screen({ v1: [{ ...theirs, number: 1 }] }));
    expect(store.list("v1")).toEqual([{ ...theirs, number: 1 }, own]);
    store.snapshot(screen({ v1: [{ ...theirs, number: 1 }, { ...own, number: 2 }] }));
    expect(store.list("v1").map((each) => each.number)).toEqual([1, 2]);
  });

  // viewer-annotations, Saving Annotations Through The Window: A second window sees the annotation.
  it("shows an annotation another window added with the next snapshot", () => {
    store.snapshot(screen({ v1: [] }));
    const theirs = comment("From the other window.");
    store.snapshot(screen({ v1: [theirs] }));
    expect(store.list("v1")).toEqual([theirs]);
    expect(sent).toEqual([]);
  });

  it("resolves once its annotations have the numbers the server gave them, or after a while", async () => {
    store.snapshot(screen({ v1: [] }));
    const own = question();
    store.put("screen-1", "v1", [own]);
    let numbered = false;
    const waiting = store.whenNumbered("v1", [own.id]).then(() => { numbered = true; });
    await Promise.resolve();
    expect(numbered).toBe(false);
    store.snapshot(screen({ v1: [{ ...own, number: 4 }] }));
    await waiting;
    expect(store.get("v1", own.id)?.number).toBe(4);

    const other = comment();
    store.put("screen-1", "v1", [other]);
    const late = store.whenNumbered("v1", [other.id], 500);
    vi.advanceTimersByTime(500);
    await expect(late).resolves.toBeUndefined();
    await expect(store.whenNumbered("v1", [own.id, "ann-ffffffffffffffff"])).resolves.toBeUndefined();
  });

  describe("replies", () => {
    // viewer-annotations, Threads: The user answers an agent.
    it("shows the user's reply after the agent's at once, sends it, and keeps it over an older snapshot", () => {
      const agent = agentReply();
      const held = question({ number: 2, sent: true, replies: [agent] });
      store.snapshot(screen({ v1: [held] }));
      const mine = { id: newReplyId(), body: "Then compute it per connection." };
      expect(store.putReplies("screen-1", "v1", held.id, [mine])).toBeUndefined();
      expect(store.get("v1", held.id)?.replies).toEqual([agent, { ...mine, author: "user" }]);
      expect(sent).toEqual([{
        screenId: "screen-1",
        op: { type: "put_replies", paneId: "v1", annotationId: held.id, replies: [{ id: mine.id, author: "user", body: mine.body }] },
      }]);
      store.snapshot(screen({ v1: [held] }));
      expect(store.get("v1", held.id)?.replies).toHaveLength(2);
      // The reply is pending: the annotation goes into the next review with its thread.
      expect(reviewSelection(store.list("v1")).annotations.map((each) => each.id)).toEqual([held.id]);
      expect(resyncs).toEqual([]);
    });

    it("replaces the user's reply in its place", () => {
      const first = userReply("First.", true);
      const agent = agentReply();
      const held = question({ replies: [first, agent] });
      store.snapshot(screen({ v1: [held] }));
      store.putReplies("screen-1", "v1", held.id, [{ id: first.id, body: "First, edited." }]);
      expect(store.get("v1", held.id)?.replies).toEqual([{ id: first.id, author: "user", body: "First, edited." }, agent]);
    });

    // viewer-annotations, Threads: An agent's reply is not edited.
    it("refuses what the server would refuse, and changes nothing then", () => {
      const agent = agentReply();
      const held = question({ replies: [agent] });
      store.snapshot(screen({ v1: [held] }));
      expect(store.putReplies("screen-1", "v1", held.id, [{ id: agent.id, body: "Rewritten." }])).toBe("An agent's reply cannot be changed.");
      expect(store.putReplies("screen-1", "v1", held.id, [{ id: newReplyId(), body: " \n" }])).toBe("A reply needs a text.");
      const twice = { id: newReplyId(), body: "Twice." };
      expect(store.putReplies("screen-1", "v1", held.id, [twice, twice])).toBe("The same reply is saved twice.");
      expect(store.putReplies("screen-1", "v1", "ann-ffffffffffffffff", [{ id: newReplyId(), body: "x" }])).toBe("The annotation is no longer there.");
      const many = Array.from({ length: 51 }, () => ({ id: newReplyId(), body: "x" }));
      expect(store.putReplies("screen-1", "v1", held.id, many)).toBe("At most 50 replies can be saved at once.");
      expect(store.putReplies("screen-1", "v1", held.id, many.slice(0, 50))).toBe("A thread holds at most 50 replies.");
      expect(store.get("v1", held.id)?.replies).toEqual([agent]);
      expect(sent).toEqual([]);
    });

    it("removes replies of either author at once and keeps them removed over an older snapshot", () => {
      const agent = agentReply();
      const mine = userReply();
      const held = question({ replies: [agent, mine] });
      store.snapshot(screen({ v1: [held] }));
      store.removeReplies("screen-1", "v1", held.id, [agent.id, mine.id, "rep-unknown"]);
      expect(store.get("v1", held.id)).not.toHaveProperty("replies");
      expect(sent).toEqual([{ screenId: "screen-1", op: { type: "remove_replies", paneId: "v1", annotationId: held.id, ids: [agent.id, mine.id] } }]);
      store.snapshot(screen({ v1: [held] }));
      expect(store.get("v1", held.id)).not.toHaveProperty("replies");
    });

    it("drops the holds of a removed annotation's replies", () => {
      const held = question();
      store.snapshot(screen({ v1: [held] }));
      store.putReplies("screen-1", "v1", held.id, [{ id: newReplyId(), body: "Draft." }]);
      store.remove("screen-1", "v1", [held.id]);
      store.snapshot(screen({ v1: [] }));
      expect(store.list("v1")).toEqual([]);
    });
  });

  describe("hand-over marks", () => {
    it("marks the annotation and the user's replies, never an agent's, and keeps the marks over an older snapshot", () => {
      const agent = agentReply();
      const mine = userReply();
      const held = question({ number: 1, replies: [agent, mine] });
      store.snapshot(screen({ v1: [held] }));
      store.markSent("screen-1", "v1", entries(agent.id, "ann-ffffffffffffffff", mine.id, held.id, held.id));
      expect(sent).toEqual([{ screenId: "screen-1", op: { type: "mark_sent", paneId: "v1", entries: entries(mine.id, held.id) } }]);
      const marked = store.get("v1", held.id)!;
      expect(marked.sent).toBe(true);
      expect(marked.replies).toEqual([agent, { ...mine, sent: true }]);
      expect(isPending(marked)).toBe(false);

      store.snapshot(screen({ v1: [held] }));
      expect(store.get("v1", held.id)).toEqual(marked);
      store.snapshot(screen({ v1: [{ ...held, sent: true, replies: [agent, { ...mine, sent: true }] }] }));
      expect(store.get("v1", held.id)).toEqual(marked);
      vi.advanceTimersByTime(ANNOTATION_SETTLE_MS + 100);
      expect(resyncs).toEqual(["screen-1"]);
    });

    it("marks what the window just put, without asking twice", () => {
      store.snapshot(screen({ v1: [] }));
      const own = question();
      store.put("screen-1", "v1", [own]);
      const mine = { id: newReplyId(), body: "Also this." };
      store.markSent("screen-1", "v1", entries(own.id));
      expect(store.get("v1", own.id)?.sent).toBe(true);
      // An older snapshot without the mark keeps it.
      store.snapshot(screen({ v1: [{ ...own, number: 1 }] }));
      expect(store.get("v1", own.id)).toEqual({ ...own, number: 1, sent: true });
      expect(store.putReplies("screen-1", "v1", own.id, [mine])).toBeUndefined();
      store.markSent("screen-1", "v1", entries(mine.id));
      expect(store.get("v1", own.id)?.replies).toEqual([{ ...mine, author: "user", sent: true }]);
    });

    // viewer-annotations, Handing A Review To A Terminal: Added while sending.
    it("Added while sending: leaves another window's annotation pending", () => {
      const own = question({ number: 1 });
      store.snapshot(screen({ v1: [own] }));
      const review = reviewSelection(store.list("v1"));
      const theirs = { ...comment("From window B."), number: 2 };
      store.snapshot(screen({ v1: [own, theirs] }));
      store.markSent("screen-1", "v1", entries(...review.annotations.flatMap(pendingEntries)));
      expect(sent.at(-1)).toEqual({ screenId: "screen-1", op: { type: "mark_sent", paneId: "v1", entries: entries(own.id) } });
      expect(store.get("v1", own.id)?.sent).toBe(true);
      expect(isPending(store.get("v1", theirs.id)!)).toBe(true);
      expect(reviewSelection(store.list("v1"))).toEqual({ annotations: [theirs], again: false });
    });
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
    store.putReplies("screen-1", "v1", kept.id, [{ id: newReplyId(), body: "x" }]);
    store.failed({ op: "put_replies", screenId: "screen-1", paneId: "v1", error: "rejected reply" });
    expect(store.list("v1")).toEqual([kept]);
    expect(rejected).toHaveBeenCalledTimes(2);
    store.failed({ op: "arrange_panes", screenId: "screen-1", paneId: "v1", error: "other" });
    expect(rejected).toHaveBeenCalledTimes(2);
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
