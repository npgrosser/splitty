/**
 * The list of a viewer pane's annotations (`viewer-annotations`, Annotations
 * In The Pane; D7 of review-pane): a column on the pane's right. Its header
 * holds the count and the hand-over of the review; each entry shows its kind,
 * where its passage is, the start of its quote and the remark, and offers to
 * edit and delete it. Choosing an entry brings its passage into view.
 *
 * The entries arrive in the list's order (`review.ts`, `compareListPlaces`);
 * this module only shows them.
 */

import type { AnnotationKind } from "../../lib/protocol/types.ts";

export interface ListEntry {
  readonly id: string;
  readonly kind: AnnotationKind;
  /** Where the passage is: its lines, or "General" for a general comment. */
  readonly where: string;
  /** The passage was not found in the current version. */
  readonly stale: boolean;
  readonly quote?: string;
  readonly body?: string;
  readonly replacement?: string;
}

export interface ListHeader {
  readonly count: number;
  /** The main hand-over action, such as "Send to claude"; absent while nothing can be sent. */
  readonly send?: string;
  /** Whether annotations can be added. */
  readonly writable: boolean;
}

export interface AnnotationListActions {
  select(id: string): void;
  edit(id: string): void;
  remove(id: string): void;
  addComment(): void;
  send(anchor: HTMLElement): void;
  /** The menu of every way to hand the review over. */
  more(anchor: HTMLElement): void;
  close(): void;
}

const KIND_NAMES: Record<AnnotationKind, string> = {
  comment: "Comment",
  question: "Question",
  change: "Change",
  delete: "Deletion",
};

/** The first lines of a quote as the list shows them. */
const QUOTE_PREVIEW = 160;

function element<K extends keyof HTMLElementTagNameMap>(doc: Document, tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = doc.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function button(doc: Document, className: string, label: string, title: string, run: (button: HTMLButtonElement) => void): HTMLButtonElement {
  const node = element(doc, "button", className, label);
  node.type = "button";
  node.title = title;
  node.addEventListener("click", (event) => {
    event.stopPropagation();
    run(node);
  });
  return node;
}

export class AnnotationList {
  readonly element: HTMLElement;
  private readonly countLabel: HTMLElement;
  private readonly sendButton: HTMLButtonElement;
  private readonly moreButton: HTMLButtonElement;
  private readonly addButton: HTMLButtonElement;
  private readonly entries: HTMLElement;
  private readonly empty: HTMLElement;
  private active: string | undefined;

  constructor(private readonly doc: Document, private readonly actions: AnnotationListActions) {
    this.element = element(doc, "aside", "viewer-annotations");
    this.element.setAttribute("aria-label", "Annotations");
    this.element.hidden = true;

    const header = element(doc, "div", "viewer-annotations-header");
    this.countLabel = element(doc, "span", "viewer-annotations-count");
    const close = button(doc, "viewer-annotations-icon", "×", "Hide annotations", () => this.actions.close());
    close.setAttribute("aria-label", "Hide annotations");
    header.append(this.countLabel, close);

    const send = element(doc, "div", "viewer-annotations-send");
    this.sendButton = button(doc, "viewer-annotations-primary", "Send", "Paste the review into the terminal, without submitting it", (node) => this.actions.send(node));
    this.moreButton = button(doc, "viewer-annotations-more", "▾", "More ways to hand the review over", (node) => this.actions.more(node));
    this.moreButton.setAttribute("aria-label", "More ways to hand the review over");
    send.append(this.sendButton, this.moreButton);

    this.entries = element(doc, "ol", "viewer-annotations-list");
    this.empty = element(doc, "p", "viewer-annotations-empty", "No annotations yet. Select a passage to annotate it, or add a general comment.");
    this.addButton = button(doc, "viewer-annotations-add", "Add Comment", "Add a comment on the whole document", () => this.actions.addComment());
    this.element.append(header, send, this.entries, this.empty, this.addButton);
    // The column's own menu: none. The pane's would act on the document.
    this.element.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
  }

  get shown(): boolean {
    return !this.element.hidden;
  }

  setShown(shown: boolean): void {
    this.element.hidden = !shown;
  }

  /** Show `entries` in the list's order; `active` is the entry the user chose last. */
  render(header: ListHeader, entries: readonly ListEntry[], active?: string): void {
    this.active = active;
    this.countLabel.textContent = header.count === 1 ? "1 annotation" : `${header.count} annotations`;
    this.sendButton.textContent = header.send ?? "Send";
    this.sendButton.disabled = !header.send || header.count === 0;
    this.moreButton.disabled = header.count === 0;
    this.addButton.hidden = !header.writable;
    this.empty.hidden = entries.length > 0;
    this.entries.replaceChildren(...entries.map((entry) => this.entry(entry)));
  }

  /** Bring an entry into view and show it as the chosen one. */
  reveal(id: string): void {
    this.active = id;
    for (const item of this.entries.children) {
      const match = (item as HTMLElement).dataset.id === id;
      item.classList.toggle("active", match);
      if (match) (item as HTMLElement).scrollIntoView?.({ block: "nearest" });
    }
  }

  private entry(entry: ListEntry): HTMLElement {
    const doc = this.doc;
    const item = element(doc, "li", "viewer-annotation");
    item.dataset.id = entry.id;
    item.dataset.kind = entry.kind;
    item.tabIndex = 0;
    if (entry.id === this.active) item.classList.add("active");
    if (entry.stale) item.classList.add("stale");

    const head = element(doc, "div", "viewer-annotation-head");
    head.append(element(doc, "span", "viewer-annotation-kind", KIND_NAMES[entry.kind]));
    const where = element(doc, "span", "viewer-annotation-where", entry.stale ? `${entry.where}, not found` : entry.where);
    if (entry.stale) where.title = "The passage is not in the current version of the document";
    head.append(where);
    const tools = element(doc, "span", "viewer-annotation-tools");
    tools.append(
      button(doc, "viewer-annotation-tool", "Edit", "Edit this annotation", () => this.actions.edit(entry.id)),
      button(doc, "viewer-annotation-tool", "Delete", "Delete this annotation", () => this.actions.remove(entry.id)),
    );
    head.append(tools);
    item.append(head);

    if (entry.quote !== undefined) {
      const quote = entry.quote.length > QUOTE_PREVIEW ? `${entry.quote.slice(0, QUOTE_PREVIEW)}…` : entry.quote;
      item.append(element(doc, "div", "viewer-annotation-quote", quote));
    }
    if (entry.kind === "change" && entry.replacement !== undefined) {
      const text = entry.replacement.length > QUOTE_PREVIEW ? `${entry.replacement.slice(0, QUOTE_PREVIEW)}…` : entry.replacement;
      item.append(element(doc, "div", "viewer-annotation-replacement", text));
    }
    if (entry.body) item.append(element(doc, "div", "viewer-annotation-body", entry.body));

    const select = () => this.actions.select(entry.id);
    item.addEventListener("click", select);
    item.addEventListener("keydown", (event) => {
      if (event.target !== item) return;
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        select();
      } else if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        this.actions.remove(entry.id);
      }
    });
    item.addEventListener("dblclick", () => this.actions.edit(entry.id));
    return item;
  }
}
