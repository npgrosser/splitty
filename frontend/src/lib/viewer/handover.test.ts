import { describe, expect, it, vi } from "vitest";
import { SCRATCHPAD_LIMIT } from "../scratchpad.ts";
import { appendedScratchpad, pastedText, scratchpadProblem, sendReview, type Review, type ReviewTarget } from "./handover.ts";

const REVIEW: Review = {
  text: "# Review of PLAN.md\n\nDocument: /home/me/app/PLAN.md\n\n## 1. Line 30, question\n> retry three times\n\nWhy three?\n",
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

function round() {
  return { end: vi.fn(), restore: vi.fn() };
}

function notices() {
  return { sent: vi.fn(), info: vi.fn(), error: vi.fn() };
}

describe("sendReview", () => {
  // viewer-annotations, Handing A Review To A Terminal: Send to the agent.
  it("pastes the review without Enter, gives the terminal the keyboard and ends the round", async () => {
    const terminal = target();
    const sent = round();
    const told = notices();
    await expect(sendReview(REVIEW, terminal.fake, "paste", sent, told)).resolves.toBe(true);
    expect(terminal.pasted).toEqual([REVIEW.text.replace(/\n$/, "")]);
    expect(terminal.pasted[0].endsWith("\n")).toBe(false);
    expect(terminal.calls).toEqual(["paste", "reveal"]);
    expect(sent.end).toHaveBeenCalledTimes(1);
    expect(told.sent).toHaveBeenCalledWith("The review is at the prompt of claude", expect.any(Function));
  });

  // viewer-annotations, Handing A Review To A Terminal: Undo after sending.
  it("offers to put the sent annotations back, and leaves the paste where it is", async () => {
    const terminal = target();
    const sent = round();
    const told = notices();
    await sendReview(REVIEW, terminal.fake, "paste", sent, told);
    const undo = told.sent.mock.calls[0][1] as () => void;
    undo();
    expect(sent.restore).toHaveBeenCalledTimes(1);
    expect(terminal.calls).toEqual(["paste", "reveal"]);
  });

  // viewer-annotations, Handing A Review To A Terminal: A program without bracketed paste.
  it("stores a review of several lines as a file for a program without bracketed paste and inserts its path", async () => {
    const terminal = target({ bracketedPaste: false, upload: async () => "/home/me/.cache/splitty/uploads/review PLAN.md" });
    const sent = round();
    const told = notices();
    await expect(sendReview(REVIEW, terminal.fake, "paste", sent, told)).resolves.toBe(true);
    expect(terminal.calls).toEqual(["upload", "paste", "reveal"]);
    expect(terminal.uploaded[0].name).toBe("review-PLAN.md");
    expect(await terminal.uploaded[0].text()).toBe(REVIEW.text);
    expect(terminal.pasted).toEqual(["'/home/me/.cache/splitty/uploads/review PLAN.md'"]);
    expect(told.sent.mock.calls[0][0]).toContain("takes no bracketed paste");
    expect(sent.end).toHaveBeenCalledTimes(1);
  });

  it("pastes a review of one line also without bracketed paste", async () => {
    const terminal = target({ bracketedPaste: false });
    await sendReview({ text: "# Review of a\n", fileName: "review-a.md" }, terminal.fake, "paste", round(), notices());
    expect(terminal.pasted).toEqual(["# Review of a"]);
  });

  it("stores the review as a file on request", async () => {
    const terminal = target();
    const told = notices();
    await sendReview(REVIEW, terminal.fake, "file", round(), told);
    expect(terminal.calls).toEqual(["upload", "paste", "reveal"]);
    expect(terminal.pasted[0]).toBe("/home/me/.cache/splitty/uploads/review-PLAN.md");
    expect(told.sent.mock.calls[0][0]).toBe("The review is stored as a file, its path is at the prompt of claude");
  });

  it("keeps the round when the upload fails, and says why", async () => {
    const terminal = target({ upload: async () => { throw new Error("The connection changed during the upload."); } });
    const sent = round();
    const told = notices();
    await expect(sendReview(REVIEW, terminal.fake, "file", sent, told)).resolves.toBe(false);
    expect(terminal.pasted).toEqual([]);
    expect(sent.end).not.toHaveBeenCalled();
    expect(told.error).toHaveBeenCalledWith("The review was not sent", "The connection changed during the upload.");
  });

  // viewer-annotations, Handing A Review To A Terminal: Into the scratchpad.
  it("appends the review to the scratchpad, opens its area and sends the terminal nothing", async () => {
    const terminal = target({ scratchpad: "Earlier notes\n\n" });
    const sent = round();
    await expect(sendReview(REVIEW, terminal.fake, "scratchpad", sent, notices())).resolves.toBe(true);
    expect(terminal.scratchpad()).toBe(`Earlier notes\n\n${REVIEW.text}`);
    expect(terminal.calls).toEqual(["scratchpad", "open"]);
    expect(terminal.pasted).toEqual([]);
    expect(sent.end).toHaveBeenCalledTimes(1);
  });

  it("does not append a review the scratchpad cannot keep", async () => {
    const full = "x".repeat(SCRATCHPAD_LIMIT - 10);
    const terminal = target({ scratchpad: full });
    const sent = round();
    const told = notices();
    expect(scratchpadProblem(REVIEW, terminal.fake)).toContain("does not fit");
    await expect(sendReview(REVIEW, terminal.fake, "scratchpad", sent, told)).resolves.toBe(false);
    expect(terminal.scratchpad()).toBe(full);
    expect(sent.end).not.toHaveBeenCalled();
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
