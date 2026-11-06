/**
 * The banner of a viewer pane whose document a process waits for
 * (`viewer-annotations`, Waiting For A Review; D9 of review-threads, D1, D3
 * and D6 of review-verdict): it says which pane waits, holds a field for an
 * overall comment, and offers the two verdicts, Approve and Request Changes.
 * It says so when the document changed while the process waited. While an
 * answer is on its way, both verdicts are closed. The waiting pane's title is
 * shown as text.
 */

import type { ReviewWait } from "../../lib/protocol/types.ts";

export interface ReviewBannerActions {
  approve(): void;
  requestChanges(): void;
  /** The text of the overall comment changed. */
  overallChanged(text: string): void;
}

export interface BannerState {
  readonly wait: ReviewWait | undefined;
  /** An answer is on its way. */
  readonly answering: boolean;
  /** The window saw the document change while the process waits. */
  readonly changed: boolean;
}

export const APPROVE_TITLE = "Approve the document: the waiting command goes on, with new comments as notes";
export const REQUEST_CHANGES_TITLE = "Request changes: the waiting command gets the review and revises the document";
export const OVERALL_PLACEHOLDER = "Overall comment (optional)";
export const CHANGED_NOTICE = "The document changed since the review began";

function element<K extends keyof HTMLElementTagNameMap>(doc: Document, tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = doc.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** What the banner says for `wait`. */
export function waitMessage(wait: ReviewWait): string {
  return wait.title ? `${wait.title} waits for a review` : "A command waits for a review";
}

export class ReviewBanner {
  readonly element: HTMLElement;
  private readonly message: HTMLElement;
  private readonly note: HTMLElement;
  private readonly field: HTMLTextAreaElement;
  private readonly approveButton: HTMLButtonElement;
  private readonly changesButton: HTMLButtonElement;

  constructor(doc: Document, actions: ReviewBannerActions, overall = "") {
    this.element = element(doc, "div", "viewer-review-wait");
    this.element.setAttribute("role", "region");
    this.element.setAttribute("aria-label", "Waiting review");
    this.element.hidden = true;
    const row = element(doc, "div", "viewer-review-wait-row");
    this.message = element(doc, "span", "viewer-review-wait-text");
    this.message.setAttribute("role", "status");
    this.note = element(doc, "span", "viewer-review-wait-note", CHANGED_NOTICE);
    this.note.hidden = true;
    this.approveButton = element(doc, "button", "viewer-review-wait-button viewer-review-wait-approve", "Approve");
    this.approveButton.type = "button";
    this.approveButton.title = APPROVE_TITLE;
    this.approveButton.addEventListener("click", () => actions.approve());
    this.changesButton = element(doc, "button", "viewer-review-wait-button viewer-review-wait-changes", "Request Changes");
    this.changesButton.type = "button";
    this.changesButton.title = REQUEST_CHANGES_TITLE;
    this.changesButton.addEventListener("click", () => actions.requestChanges());
    row.append(this.message, this.note, this.approveButton, this.changesButton);
    this.field = element(doc, "textarea", "viewer-review-wait-comment");
    this.field.rows = 1;
    this.field.placeholder = OVERALL_PLACEHOLDER;
    this.field.setAttribute("aria-label", OVERALL_PLACEHOLDER);
    this.field.value = overall;
    this.field.addEventListener("input", () => actions.overallChanged(this.field.value));
    this.element.append(row, this.field);
  }

  /** The text of the overall comment. */
  get overall(): string {
    return this.field.value;
  }

  /** Empty the overall comment once it is saved. */
  clearOverall(): void {
    this.field.value = "";
  }

  show(state: BannerState): void {
    const { wait } = state;
    this.element.hidden = !wait;
    if (!wait) return;
    this.message.textContent = waitMessage(wait);
    this.element.dataset.waitId = wait.id;
    this.note.hidden = !state.changed;
    this.approveButton.disabled = state.answering;
    this.changesButton.disabled = state.answering;
    this.field.disabled = state.answering;
    this.element.setAttribute("aria-busy", String(state.answering));
  }
}
