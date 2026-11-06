/**
 * Handing a viewer pane's review to a terminal or to a process that waits
 * for it (`viewer-annotations`, Handing A Review To A Terminal and Waiting
 * For A Review; D4 and D9 of review-threads).
 *
 * The review is pasted as the user's own paste would be, without Enter, and
 * the terminal takes the keyboard. A program that has not enabled bracketed
 * paste would run each line of a review as a command, so a review of several
 * lines goes to it as a file instead: stored on the terminal's server with
 * `upload` and its quoted path inserted, as a drop with path insertion does.
 * A review can also be stored as a file on purpose, or appended to the
 * terminal's scratchpad, separated by a blank line, while it stays within
 * the scratchpad's limit.
 *
 * Sending keeps the annotations: the caller marks the entries the review
 * held as handed over once it went out (`mark_sent`), and nothing is taken
 * back, so there is no undo. Copying and saving the review mark nothing,
 * since nothing reaches an agent.
 *
 * A process that waits for a review is answered with `viewer.review_answer`
 * over the pane's connection (`ReviewAnswers`); the server acknowledges every
 * answer with `viewer.review_answered`, and only an accepted one counts.
 *
 * Everything that touches a terminal comes in as a `ReviewTarget`, so that
 * this module knows no panes, sessions or emulators.
 */

import type { ConnSocket } from "../protocol/socket.ts";
import type { ReviewOutcome } from "../protocol/types.ts";
import { SCRATCHPAD_LIMIT, byteLength } from "../scratchpad.ts";
import { quoteUploadPath } from "../terminal/image_paste.ts";

/** How long the notice of a change the user can take back offers its undo. */
export const UNDO_MS = 10_000;
/** The longest review that answers a waiting process, in bytes of UTF-8. */
export const REVIEW_ANSWER_LIMIT = 1024 * 1024;
/** How long an answer waits for the server's acknowledgement. */
export const ANSWER_TIMEOUT_MS = 15_000;

/** A terminal pane a review can go to. */
export interface ReviewTarget {
  readonly paneId: string;
  /** How the window names it. */
  readonly title: string;
  /** Whether the terminal's program enabled bracketed paste. */
  bracketedPaste(): boolean;
  /** Paste `text` as the user's own paste would; nothing is submitted. */
  paste(text: string): void;
  /** Show the pane and give its terminal the keyboard. */
  reveal(): void;
  /** Store a file on the terminal's server; resolves with its path there. */
  upload(file: File): Promise<string>;
  /** The terminal's scratchpad, where its server keeps scratchpads. */
  readonly scratchpad?: {
    text(): string;
    /** Replace its text, as an edit of the user, saved at once. */
    set(text: string): void;
    /** Show the pane with its scratchpad area open. */
    open(): void;
  };
}

export interface Review {
  /** The review as Markdown, ending with a line break. */
  readonly text: string;
  /** What a file of it is called, `review-<document>.md`. */
  readonly fileName: string;
}

/** What the user is told. */
export interface HandoverNotices {
  /** The review went out. */
  sent(message: string): void;
  info(message: string): void;
  error(message: string, detail?: string): void;
}

export type SendForm = "paste" | "file" | "scratchpad";

/** The text a paste takes: the review without its final line breaks, so that nothing reads as Enter. */
export function pastedText(review: Review): string {
  return review.text.replace(/\n+$/, "");
}

/** Whether a paste of the review would reach a program without bracketed paste as several lines. */
function needsFile(review: Review, target: ReviewTarget): boolean {
  return !target.bracketedPaste() && pastedText(review).includes("\n");
}

/** The scratchpad text after appending the review, separated by a blank line. */
export function appendedScratchpad(existing: string, review: Review): string {
  const kept = existing.replace(/\s+$/, "");
  return kept ? `${kept}\n\n${review.text}` : review.text;
}

/** Why the review cannot go to the target's scratchpad, or undefined when it can. */
export function scratchpadProblem(review: Review, target: ReviewTarget): string | undefined {
  if (!target.scratchpad) return "The terminal's server keeps no scratchpads.";
  if (byteLength(appendedScratchpad(target.scratchpad.text(), review)) > SCRATCHPAD_LIMIT) {
    return `The review does not fit into the scratchpad, which keeps at most ${SCRATCHPAD_LIMIT / 1024} KiB.`;
  }
  return undefined;
}

/**
 * Send `review` to `target` in `form`. Resolves with whether it went out, so
 * that the caller marks what it held. A paste that a program without
 * bracketed paste would run line by line goes as a file, and the notice says so.
 */
export async function sendReview(review: Review, target: ReviewTarget, form: SendForm, notices: HandoverNotices): Promise<boolean> {
  if (form === "scratchpad") {
    const problem = scratchpadProblem(review, target);
    if (problem || !target.scratchpad) {
      notices.error("The review was not sent", problem);
      return false;
    }
    target.scratchpad.set(appendedScratchpad(target.scratchpad.text(), review));
    target.scratchpad.open();
    notices.sent(`The review is in the scratchpad of ${target.title}`);
    return true;
  }

  const asFile = form === "file" || needsFile(review, target);
  if (!asFile) {
    target.paste(pastedText(review));
    target.reveal();
    notices.sent(`The review is at the prompt of ${target.title}`);
    return true;
  }

  let path: string;
  try {
    path = await target.upload(new File([review.text], review.fileName, { type: "text/markdown" }));
  } catch (error) {
    notices.error("The review was not sent", error instanceof Error ? error.message : String(error));
    return false;
  }
  target.paste(quoteUploadPath(path));
  target.reveal();
  notices.sent(form === "file"
    ? `The review is stored as a file, its path is at the prompt of ${target.title}`
    : `The review went as a file: the program in ${target.title} takes no bracketed paste, so its path is at the prompt`);
  return true;
}

/** An answer to a process that waits for a review of a viewer pane. */
export interface ReviewAnswerRequest {
  /** The pane's screen, by its id on the server. */
  readonly screenId: string;
  readonly paneId: string;
  readonly waitId: string;
  readonly outcome: ReviewOutcome;
  /** The review, for `feedback`. */
  readonly review?: string;
}

/** The server did not acknowledge an answer: the connection ended, or no acknowledgement came in time. */
export class ReviewAnswerError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ReviewAnswerError";
  }
}

interface PendingAnswer {
  readonly promise: Promise<boolean>;
  settle(accepted: boolean | Error): void;
}

/**
 * The answers of one connection to processes waiting for reviews. An answer
 * resolves with whether the server accepted it, which only the first answer
 * to a wait is; it fails when the connection ends or no acknowledgement
 * comes within `ANSWER_TIMEOUT_MS`. A second answer to the same wait while
 * the first is on its way is the first.
 */
export class ReviewAnswers {
  private readonly pending = new Map<string, PendingAnswer>();
  private readonly unsubscribes: (() => void)[];

  constructor(private readonly socket: ConnSocket, private readonly timeoutMs = ANSWER_TIMEOUT_MS) {
    this.unsubscribes = [
      socket.onMessage((msg) => {
        if (msg.type !== "viewer.review_answered") return;
        const { paneId, waitId } = msg as { paneId?: unknown; waitId?: unknown };
        if (typeof paneId !== "string" || typeof waitId !== "string") return;
        this.pending.get(key(paneId, waitId))?.settle((msg as { accepted?: unknown }).accepted === true);
      }),
      socket.onDisconnect(() => {
        for (const answer of [...this.pending.values()]) {
          answer.settle(new ReviewAnswerError("The connection to the server ended before it took the answer."));
        }
      }),
    ];
  }

  answer(request: ReviewAnswerRequest): Promise<boolean> {
    const id = key(request.paneId, request.waitId);
    const known = this.pending.get(id);
    if (known) return known.promise;
    let settle!: (accepted: boolean | Error) => void;
    const promise = new Promise<boolean>((resolve, reject) => {
      const timer = setTimeout(() => settle(new ReviewAnswerError("The server did not take the answer in time.")), this.timeoutMs);
      settle = (accepted) => {
        clearTimeout(timer);
        if (this.pending.get(id)?.promise === promise) this.pending.delete(id);
        if (accepted instanceof Error) reject(accepted);
        else resolve(accepted);
      };
    });
    this.pending.set(id, { promise, settle });
    this.socket.send({
      type: "viewer.review_answer",
      screenId: request.screenId,
      paneId: request.paneId,
      waitId: request.waitId,
      outcome: request.outcome,
      ...(request.outcome === "feedback" && request.review !== undefined ? { review: request.review } : {}),
    });
    return promise;
  }

  dispose(): void {
    for (const unsubscribe of this.unsubscribes) unsubscribe();
    this.unsubscribes.length = 0;
    for (const answer of [...this.pending.values()]) answer.settle(new ReviewAnswerError("The connection was closed."));
  }
}

function key(paneId: string, waitId: string): string {
  return `${paneId}\u0000${waitId}`;
}
