import { afterEach, describe, expect, it, vi } from "vitest";
import { SCRATCHPAD_LIMIT } from "../scratchpad.ts";
import { FakeSocket } from "../testing/fake_server.ts";
import {
  ReviewAnswerError, ReviewAnswers, appendedScratchpad, pastedText, scratchpadProblem, sendReview, type Review, type ReviewTarget,
} from "./handover.ts";

const REVIEW: Review = {
  text: "# Review of PLAN.md\n\nDocument: /home/me/app/PLAN.md\n\n## #1 Line 30, comment\n> retry three times\n\nWhy three?\n",
  fileName: "review-PLAN.md",
};

function target(options: { bracketedPaste?: boolean; scratchpad?: string | null; upload?: (file: File) => Promise<string> } = {}) {
  let scratchpad = options.scratchpad === undefined ? "" : options.scratchpad;
  const calls: string[] = [];
  const pasted: string[] = [];
  const uploaded: File[] = [];
  const fake: ReviewTarget = {
    paneId: "t1",
    title: "claude",
    bracketedPaste: () => options.bracketedPaste ?? true,
    paste: (text) => {
      calls.push("paste");
      pasted.push(text);
    },
    reveal: () => calls.push("reveal"),
    upload: async (file) => {
      calls.push("upload");
      uploaded.push(file);
      return options.upload ? options.upload(file) : "/home/me/.cache/splitty/uploads/review-PLAN.md";
    },
    ...(scratchpad !== null ? {
      scratchpad: {
        text: () => scratchpad!,
        set: (text: string) => {
          calls.push("scratchpad");
          scratchpad = text;
        },
        open: () => calls.push("open"),
      },
    } : {}),
  };
  return { fake, calls, pasted, uploaded, scratchpad: () => scratchpad };
}

function notices() {
  return { sent: vi.fn(), info: vi.fn(), error: vi.fn() };
}

describe("sendReview", () => {
  // viewer-annotations, Handing A Review To A Terminal: Send to the agent.
  it("pastes the review without Enter, gives the terminal the keyboard and says it went out", async () => {
    const terminal = target();
    const told = notices();
    await expect(sendReview(REVIEW, terminal.fake, "paste", told)).resolves.toBe(true);
    expect(terminal.pasted).toEqual([REVIEW.text.replace(/\n$/, "")]);
    expect(terminal.pasted[0].endsWith("\n")).toBe(false);
    expect(terminal.calls).toEqual(["paste", "reveal"]);
    expect(told.sent).toHaveBeenCalledWith("The review is at the prompt of claude");
  });

  // viewer-annotations, Handing A Review To A Terminal: Undo after sending.
  it("offers no undo, since the annotations stay", async () => {
    const terminal = target();
    const told = notices();
    await sendReview(REVIEW, terminal.fake, "paste", told);
    expect(told.sent).toHaveBeenCalledTimes(1);
    expect(told.sent.mock.calls[0]).toHaveLength(1);
    expect(terminal.calls).toEqual(["paste", "reveal"]);
  });

  // viewer-annotations, Handing A Review To A Terminal: A program without bracketed paste.
  it("stores a review of several lines as a file for a program without bracketed paste and inserts its path", async () => {
    const terminal = target({ bracketedPaste: false, upload: async () => "/home/me/.cache/splitty/uploads/review PLAN.md" });
    const told = notices();
    await expect(sendReview(REVIEW, terminal.fake, "paste", told)).resolves.toBe(true);
    expect(terminal.calls).toEqual(["upload", "paste", "reveal"]);
    expect(terminal.uploaded[0].name).toBe("review-PLAN.md");
    expect(await terminal.uploaded[0].text()).toBe(REVIEW.text);
    expect(terminal.pasted).toEqual(["'/home/me/.cache/splitty/uploads/review PLAN.md'"]);
    expect(told.sent.mock.calls[0][0]).toContain("takes no bracketed paste");
  });

  it("pastes a review of one line also without bracketed paste", async () => {
    const terminal = target({ bracketedPaste: false });
    await sendReview({ text: "# Review of a\n", fileName: "review-a.md" }, terminal.fake, "paste", notices());
    expect(terminal.pasted).toEqual(["# Review of a"]);
  });

  it("stores the review as a file on request", async () => {
    const terminal = target();
    const told = notices();
    await sendReview(REVIEW, terminal.fake, "file", told);
    expect(terminal.calls).toEqual(["upload", "paste", "reveal"]);
    expect(terminal.pasted[0]).toBe("/home/me/.cache/splitty/uploads/review-PLAN.md");
    expect(told.sent.mock.calls[0][0]).toBe("The review is stored as a file, its path is at the prompt of claude");
  });

  it("says why when the upload fails, and that nothing went out", async () => {
    const terminal = target({ upload: async () => { throw new Error("The connection changed during the upload."); } });
    const told = notices();
    await expect(sendReview(REVIEW, terminal.fake, "file", told)).resolves.toBe(false);
    expect(terminal.pasted).toEqual([]);
    expect(told.sent).not.toHaveBeenCalled();
    expect(told.error).toHaveBeenCalledWith("The review was not sent", "The connection changed during the upload.");
  });

  // viewer-annotations, Handing A Review To A Terminal: Into the scratchpad.
  it("appends the review to the scratchpad, opens its area and sends the terminal nothing", async () => {
    const terminal = target({ scratchpad: "Earlier notes\n\n" });
    const told = notices();
    await expect(sendReview(REVIEW, terminal.fake, "scratchpad", told)).resolves.toBe(true);
    expect(terminal.scratchpad()).toBe(`Earlier notes\n\n${REVIEW.text}`);
    expect(terminal.calls).toEqual(["scratchpad", "open"]);
    expect(terminal.pasted).toEqual([]);
    expect(told.sent).toHaveBeenCalledWith("The review is in the scratchpad of claude");
  });

  it("does not append a review the scratchpad cannot keep", async () => {
    const full = "x".repeat(SCRATCHPAD_LIMIT - 10);
    const terminal = target({ scratchpad: full });
    const told = notices();
    expect(scratchpadProblem(REVIEW, terminal.fake)).toContain("does not fit");
    await expect(sendReview(REVIEW, terminal.fake, "scratchpad", told)).resolves.toBe(false);
    expect(terminal.scratchpad()).toBe(full);
    expect(told.sent).not.toHaveBeenCalled();
    expect(told.error).toHaveBeenCalledWith("The review was not sent", expect.stringContaining("64 KiB"));
    expect(scratchpadProblem(REVIEW, target({ scratchpad: null }).fake)).toBe("The terminal's server keeps no scratchpads.");
  });
});

describe("helpers", () => {
  it("pastes no final line break, which would read as Enter", () => {
    expect(pastedText({ text: "a\nb\n\n", fileName: "x" })).toBe("a\nb");
  });

  it("separates an appended review by one blank line", () => {
    expect(appendedScratchpad("", REVIEW)).toBe(REVIEW.text);
    expect(appendedScratchpad("notes  \n\n\n", REVIEW)).toBe(`notes\n\n${REVIEW.text}`);
  });
});

// viewer-annotations, Waiting For A Review (D9 of review-threads): the window's answer and the server's acknowledgement.
describe("ReviewAnswers", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  const request = { screenId: "screen-1", paneId: "v1", waitId: "wait-0123456789abcdef" } as const;

  it("sends feedback with the review and resolves with whether the server took it", async () => {
    const socket = new FakeSocket();
    const answers = new ReviewAnswers(socket);
    const answered = answers.answer({ ...request, outcome: "feedback", review: REVIEW.text });
    expect(socket.sentOfType("viewer.review_answer")).toEqual([
      { type: "viewer.review_answer", screenId: "screen-1", paneId: "v1", waitId: request.waitId, outcome: "feedback", review: REVIEW.text },
    ]);
    // Another wait's acknowledgement, and a malformed one, settle nothing.
    socket.message({ type: "viewer.review_answered", paneId: "v1", waitId: "wait-ffffffffffffffff", accepted: false });
    socket.message({ type: "viewer.review_answered", paneId: 7, waitId: request.waitId, accepted: false });
    socket.message({ type: "viewer.review_answered", paneId: "v1", waitId: request.waitId, accepted: true });
    await expect(answered).resolves.toBe(true);
    answers.dispose();
  });

  it("sends an approval without a review", async () => {
    const socket = new FakeSocket();
    const answers = new ReviewAnswers(socket);
    const answered = answers.answer({ ...request, outcome: "approved", review: "ignored" });
    expect(socket.sentOfType("viewer.review_answer")[0]).not.toHaveProperty("review");
    socket.message({ type: "viewer.review_answered", paneId: "v1", waitId: request.waitId, accepted: "yes" });
    await expect(answered).resolves.toBe(false);
    answers.dispose();
  });

  // viewer-annotations, Waiting For A Review: Two windows answer.
  it("resolves with false for an answer that came second", async () => {
    const socket = new FakeSocket();
    const answers = new ReviewAnswers(socket);
    const answered = answers.answer({ ...request, outcome: "feedback", review: REVIEW.text });
    socket.message({ type: "viewer.review_answered", paneId: "v1", waitId: request.waitId, accepted: false });
    await expect(answered).resolves.toBe(false);
    answers.dispose();
  });

  it("sends one answer per wait while it is on its way", async () => {
    const socket = new FakeSocket();
    const answers = new ReviewAnswers(socket);
    const first = answers.answer({ ...request, outcome: "approved" });
    const second = answers.answer({ ...request, outcome: "feedback", review: REVIEW.text });
    expect(second).toBe(first);
    expect(socket.sentOfType("viewer.review_answer")).toHaveLength(1);
    socket.message({ type: "viewer.review_answered", paneId: "v1", waitId: request.waitId, accepted: true });
    await expect(first).resolves.toBe(true);
    // Once it is settled, a new answer goes out.
    const third = answers.answer({ ...request, outcome: "approved" });
    expect(socket.sentOfType("viewer.review_answer")).toHaveLength(2);
    answers.dispose();
    await expect(third).rejects.toThrow("The connection was closed.");
  });

  it("fails when the connection ends or no acknowledgement comes in time", async () => {
    vi.useFakeTimers();
    const socket = new FakeSocket();
    const answers = new ReviewAnswers(socket, 1000);
    const dropped = answers.answer({ ...request, outcome: "approved" });
    socket.drop();
    await expect(dropped).rejects.toThrow(ReviewAnswerError);
    await expect(dropped).rejects.toThrow("The connection to the server ended before it took the answer.");

    const late = answers.answer({ ...request, outcome: "approved" });
    const failed = expect(late).rejects.toThrow("The server did not take the answer in time.");
    await vi.advanceTimersByTimeAsync(1000);
    await failed;

    const disposed = answers.answer({ ...request, outcome: "approved" });
    answers.dispose();
    await expect(disposed).rejects.toThrow(ReviewAnswerError);
  });
});
