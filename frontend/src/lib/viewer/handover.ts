/**
 * Handing a viewer pane's review to a terminal (`viewer-annotations`,
 * Handing A Review To A Terminal; D9 of review-pane).
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
 * Sending ends the round: the annotations that went out are removed, and for
 * ten seconds a notice offers to put them back. An annotation added in the
 * meantime is not part of the round and stays. Copying and saving the review
 * keep the annotations, since nothing reaches the agent.
 *
 * Everything that touches a terminal comes in as a `ReviewTarget`, so that
 * this module knows no panes, sessions or emulators.
 */

import { SCRATCHPAD_LIMIT, byteLength } from "../scratchpad.ts";
import { quoteUploadPath } from "../terminal/image_paste.ts";

/** How long the notice after a send offers to put the annotations back. */
export const UNDO_MS = 10_000;

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

/** The round a send ends: the annotations the review holds. */
export interface Round {
  /** Remove the annotations that were sent. */
  end(): void;
  /** Put them back. */
  restore(): void;
}

/** What the user is told; `sent` offers the undo for `UNDO_MS`. */
export interface HandoverNotices {
  sent(message: string, undo: () => void): void;
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
 * Send `review` to `target` in `form`. Resolves with whether it was sent, and
 * so the round ended. A paste that a program without bracketed paste would
 * run line by line goes as a file, and the notice says so.
 */
export async function sendReview(review: Review, target: ReviewTarget, form: SendForm, round: Round, notices: HandoverNotices): Promise<boolean> {
  const finish = (message: string) => {
    round.end();
    notices.sent(message, () => round.restore());
  };

  if (form === "scratchpad") {
    const problem = scratchpadProblem(review, target);
    if (problem || !target.scratchpad) {
      notices.error("The review was not sent", problem);
      return false;
    }
    target.scratchpad.set(appendedScratchpad(target.scratchpad.text(), review));
    target.scratchpad.open();
    finish(`The review is in the scratchpad of ${target.title}`);
    return true;
  }

  const asFile = form === "file" || needsFile(review, target);
  if (!asFile) {
    target.paste(pastedText(review));
    target.reveal();
    finish(`The review is at the prompt of ${target.title}`);
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
  finish(form === "file"
    ? `The review is stored as a file, its path is at the prompt of ${target.title}`
    : `The review went as a file: the program in ${target.title} takes no bracketed paste, so its path is at the prompt`);
  return true;
}
