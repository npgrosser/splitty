/**
 * The banner of a viewer pane whose document a process waits for
 * (`viewer-annotations`, Waiting For A Review; D9 of review-threads): it says
 * which pane waits and offers to approve the document or to send the review
 * to the waiting process. While an answer is on its way, both are closed.
 * The waiting pane's title is shown as text.
 */

import type { ReviewWait } from "../../lib/protocol/types.ts";

export interface ReviewBannerActions {
  approve(): void;
  send(): void;
}

export interface BannerState {
  readonly wait: ReviewWait | undefined;
  /** An answer is on its way. */
  readonly answering: boolean;
  /** Whether the review holds anything: an open annotation. */
  readonly sendable: boolean;
}

export const SEND_REVIEW_TITLE = "Answer the waiting command with the review of the open annotations";
export const NOTHING_TO_SEND = "Annotate the document first, or approve it";

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
  private readonly approveButton: HTMLButtonElement;
  private readonly sendButton: HTMLButtonElement;

  constructor(doc: Document, actions: ReviewBannerActions) {
    this.element = element(doc, "div", "viewer-review-wait");
    this.element.setAttribute("role", "status");
    this.element.hidden = true;
    this.message = element(doc, "span", "viewer-review-wait-text");
    this.approveButton = element(doc, "button", "viewer-review-wait-button viewer-review-wait-approve", "Approve");
    this.approveButton.type = "button";
    this.approveButton.title = "Approve the document: the waiting command goes on without a review";
    this.approveButton.addEventListener("click", () => actions.approve());
    this.sendButton = element(doc, "button", "viewer-review-wait-button viewer-review-wait-send", "Send Review");
    this.sendButton.type = "button";
    this.sendButton.addEventListener("click", () => actions.send());
    this.element.append(this.message, this.approveButton, this.sendButton);
  }

  show(state: BannerState): void {
    const { wait } = state;
    this.element.hidden = !wait;
    if (!wait) return;
    this.message.textContent = waitMessage(wait);
    this.element.dataset.waitId = wait.id;
    this.approveButton.disabled = state.answering;
    this.sendButton.disabled = state.answering || !state.sendable;
    this.sendButton.title = state.sendable ? SEND_REVIEW_TITLE : NOTHING_TO_SEND;
    this.element.setAttribute("aria-busy", String(state.answering));
  }
}
