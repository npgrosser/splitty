/**
 * The list of a viewer pane's annotations (`viewer-annotations`, Annotations
 * In The Pane): a column on the pane's right. Its header holds the counts,
 * the hand-over of the review and the choice to mark resolved annotations
 * as well (the eye in the pane's bar hides every mark); each
 * entry is a thread: its number, kind and where its passage is, the start of
 * its quote, the remark, its replies and a box to reply in. Choosing an entry
 * brings its passage into view. The open threads come first, the resolved
 * ones after them in a group that is folded until the user opens it.
 *
 * The entries arrive in the list's order (`review.ts`, `compareListPlaces`);
 * this module only shows them. Every text of an annotation and of a reply is
 * set as text, never as markup: an agent's reply is shown as it came, with
 * the title of the pane it came from.
 *
 * An entry keeps its element while it stays in the list, so that a reply
 * being written keeps its text, its selection and the keyboard while another
 * window or an agent changes the same thread.
 */

import type { AnnotationKind } from "../../lib/protocol/types.ts";
import { isMac } from "../../lib/shortcuts.ts";
import { safeText } from "../../lib/viewer/review.ts";

/** A reply as the list shows it. */
export interface ListReply {
  readonly id: string;
  readonly author: "user" | "agent";
  /** The title of an agent's pane, as plain text. */
  readonly title?: string;
  readonly body: string;
  /** The user handed it over to an agent. */
  readonly sent: boolean;
}

export interface ListEntry {
  readonly id: string;
  /** Absent until the server has given it. */
  readonly number?: number;
  readonly kind: AnnotationKind;
  /** A change with an empty replacement, which suggests deleting the passage. */
  readonly deletion: boolean;
  /** Where the passage is: its lines, "Passage" without lines, or "General" for a general comment. */
  readonly where: string;
  /**
   * Found by its quote, found as the passage it most likely became, not found
   * in the current version, or a general comment.
   */
  readonly state: "anchored" | "changed" | "stale" | "general";
  readonly resolved: boolean;
  /** The user handed the annotation itself over to an agent. */
  readonly sent: boolean;
  readonly quote?: string;
  readonly body?: string;
  readonly replacement?: string;
  readonly replies: readonly ListReply[];
}

export interface ListHeader {
  readonly open: number;
  readonly resolved: number;
  /** The main hand-over action, such as "Send to claude" or "Send Again to claude". */
  readonly send: string;
  readonly sendTitle: string;
  /** Whether the review holds anything: an open annotation. */
  readonly sendable: boolean;
  /** Whether annotations can be added and changed. */
  readonly writable: boolean;
  /** Whether a passage is selected that an annotation can move to. */
  readonly movable: boolean;
  readonly resolvedShown: boolean;
}

export interface AnnotationListActions {
  select(id: string): void;
  edit(id: string): void;
  remove(id: string): void;
  setResolved(id: string, resolved: boolean): void;
  /** Give the annotation the selected passage as its target. */
  move(id: string): void;
  /** Save a reply of the user, new or, with `replyId`, edited; returns why it cannot be saved. */
  reply(id: string, body: string, replyId?: string): string | undefined;
  removeReply(id: string, replyId: string): void;
  addComment(): void;
  send(anchor: HTMLElement): void;
  /** The menu of every way to hand the review over. */
  more(anchor: HTMLElement): void;
  close(): void;
  toggleResolved(): void;
}

const KIND_NAMES: Record<AnnotationKind, string> = {
  comment: "Comment",
  change: "Change",
};

/** The first characters of a quote as the list shows them. */
const QUOTE_PREVIEW = 160;

export const MOVE_TITLE = "Give this annotation the passage selected in the document";
export const MOVE_DISABLED_TITLE = "Select a passage in the document to move this annotation to it";

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

function preview(text: string): string {
  return text.length > QUOTE_PREVIEW ? `${text.slice(0, QUOTE_PREVIEW)}…` : text;
}

/** How the list names the author of a reply. */
export function replyAuthor(reply: Pick<ListReply, "author" | "title">): string {
  if (reply.author === "user") return "You";
  return reply.title ? `Agent (${reply.title})` : "Agent";
}

function count(n: number, one: string, many: string): string {
  return n === 1 ? `1 ${one}` : `${n} ${many}`;
}

/** The reply box of one thread, which outlives the renderings of its entry. */
class ReplyBox {
  readonly element: HTMLElement;
  private readonly text: HTMLTextAreaElement;
  private readonly submit: HTMLButtonElement;
  private readonly problem: HTMLElement;
  /** The reply of the user being edited, or undefined for a new one. */
  private editing: string | undefined;

  constructor(doc: Document, private readonly save: (body: string, replyId?: string) => string | undefined, private readonly done: () => void) {
    this.element = element(doc, "div", "viewer-reply-box");
    this.element.hidden = true;
    this.text = element(doc, "textarea", "viewer-form-text viewer-reply-text");
    this.text.rows = 2;
    this.text.placeholder = "Reply…";
    this.problem = element(doc, "p", "viewer-form-problem viewer-reply-problem");
    this.problem.setAttribute("role", "alert");
    this.problem.hidden = true;
    const actions = element(doc, "div", "viewer-reply-actions");
    const hint = element(doc, "span", "viewer-form-hint viewer-reply-hint", `${isMac ? "⌘" : "Ctrl+"}Enter saves, Esc cancels`);
    const cancel = button(doc, "viewer-form-button viewer-reply-cancel", "Cancel", "Discard this reply", () => this.cancel());
    this.submit = button(doc, "viewer-form-button viewer-form-save viewer-reply-submit", "Reply", "Save this reply", () => this.commit());
    actions.append(hint, cancel, this.submit);
    this.element.append(this.text, this.problem, actions);
    this.text.addEventListener("input", () => {
      this.problem.hidden = true;
    });
    this.element.addEventListener("keydown", (event) => {
      event.stopPropagation();
      if (event.isComposing) return;
      if (event.key === "Escape") {
        event.preventDefault();
        this.cancel();
      } else if (event.key === "Enter" && (isMac ? event.metaKey : event.ctrlKey)) {
        event.preventDefault();
        this.commit();
      }
    });
    // Writing a reply chooses nothing: the passage stays where it is.
    this.element.addEventListener("click", (event) => event.stopPropagation());
    this.element.addEventListener("dblclick", (event) => event.stopPropagation());
    // The text field's own menu, not the pane's.
    this.element.addEventListener("contextmenu", (event) => event.stopPropagation());
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  /** Name the thread for the text field's label. */
  label(name: string): void {
    this.text.setAttribute("aria-label", `Reply to ${name}`);
  }

  /** Open the box for a new reply, or with the text of the user's reply `replyId` to edit it. */
  start(reply?: { readonly id: string; readonly body: string }): void {
    const switched = this.editing !== reply?.id;
    this.editing = reply?.id;
    if (reply && (switched || !this.open)) this.text.value = reply.body;
    else if (switched) this.text.value = "";
    this.submit.textContent = reply ? "Save" : "Reply";
    this.problem.hidden = true;
    this.element.hidden = false;
    this.text.focus({ preventScroll: true });
    this.text.setSelectionRange(this.text.value.length, this.text.value.length);
    this.text.scrollIntoView?.({ block: "nearest" });
  }

  /** The thread no longer holds the reply being edited: what is written becomes a new reply. */
  keep(replyIds: ReadonlySet<string>): void {
    if (this.editing && !replyIds.has(this.editing)) {
      this.editing = undefined;
      this.submit.textContent = "Reply";
    }
  }

  private commit(): void {
    const problem = this.save(this.text.value, this.editing);
    this.problem.textContent = problem ?? "";
    this.problem.hidden = !problem;
    if (problem) return;
    this.close();
  }

  private cancel(): void {
    this.close();
  }

  private close(): void {
    const focused = this.element.contains(this.element.ownerDocument.activeElement);
    this.text.value = "";
    this.editing = undefined;
    this.problem.hidden = true;
    this.element.hidden = true;
    if (focused) this.done();
  }
}

/** One thread of the list, kept while it is listed. */
class EntryView {
  readonly element: HTMLLIElement;
  private readonly content: HTMLElement;
  private readonly replies: HTMLOListElement;
  private readonly actionsRow: HTMLElement;
  private readonly moveButton: HTMLButtonElement;
  readonly box: ReplyBox;
  private entry: ListEntry;

  constructor(private readonly doc: Document, entry: ListEntry, private readonly actions: AnnotationListActions) {
    this.entry = entry;
    const item = element(doc, "li", "viewer-annotation");
    this.element = item;
    item.tabIndex = 0;
    this.content = element(doc, "div", "viewer-annotation-content");
    this.replies = element(doc, "ol", "viewer-replies");
    this.actionsRow = element(doc, "div", "viewer-annotation-actions");
    const reply = button(doc, "viewer-annotation-action viewer-annotation-reply", "Reply", "Reply in this thread", () => this.box.start());
    this.moveButton = button(doc, "viewer-annotation-action viewer-annotation-move", "Move to Selection", MOVE_TITLE, () => this.actions.move(this.entry.id));
    // Pressing it must not end the selection it moves the annotation to.
    this.moveButton.addEventListener("pointerdown", (event) => event.preventDefault());
    this.moveButton.addEventListener("mousedown", (event) => event.preventDefault());
    this.actionsRow.append(reply, this.moveButton);
    this.box = new ReplyBox(doc, (body, replyId) => this.actions.reply(this.entry.id, body, replyId), () => item.focus({ preventScroll: true }));
    item.append(this.content, this.replies, this.actionsRow, this.box.element);

    item.addEventListener("click", () => this.actions.select(this.entry.id));
    item.addEventListener("keydown", (event) => {
      if (event.target !== item) return;
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        this.actions.select(this.entry.id);
      } else if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        this.actions.remove(this.entry.id);
      }
    });
    item.addEventListener("dblclick", () => this.actions.edit(this.entry.id));
  }

  update(entry: ListEntry, header: ListHeader, active: boolean): void {
    this.entry = entry;
    const doc = this.doc;
    const item = this.element;
    const name = entry.number !== undefined ? `#${entry.number}` : "this annotation";
    item.dataset.id = entry.id;
    item.dataset.kind = entry.kind;
    item.dataset.state = entry.state;
    item.dataset.status = entry.resolved ? "resolved" : "open";
    if (entry.number !== undefined) item.dataset.number = String(entry.number);
    else delete item.dataset.number;
    if (entry.deletion) item.dataset.deletion = "";
    else delete item.dataset.deletion;
    if (entry.sent) item.dataset.sent = "";
    else delete item.dataset.sent;
    item.classList.toggle("active", active);
    item.classList.toggle("stale", entry.state === "stale");
    item.classList.toggle("changed", entry.state === "changed");
    item.classList.toggle("resolved", entry.resolved);
    item.setAttribute("aria-label", `${entry.number !== undefined ? `#${entry.number} ` : ""}${KIND_NAMES[entry.kind]}, ${entry.where}`);

    const head = element(doc, "div", "viewer-annotation-head");
    if (entry.number !== undefined) head.append(element(doc, "span", "viewer-annotation-number", `#${entry.number}`));
    head.append(element(doc, "span", "viewer-annotation-kind", entry.deletion ? "Deletion" : KIND_NAMES[entry.kind]));
    head.append(element(doc, "span", "viewer-annotation-where", entry.where));
    if (entry.state === "changed") {
      const badge = element(doc, "span", "viewer-annotation-badge viewer-annotation-changed", "changed");
      badge.title = "The passage changed since it was annotated; it is marked where it most likely is now";
      head.append(badge);
    } else if (entry.state === "stale") {
      const badge = element(doc, "span", "viewer-annotation-badge viewer-annotation-not-found", "not found");
      badge.title = "The passage is not in the current version of the document";
      head.append(badge);
    }
    if (entry.sent) {
      const badge = element(doc, "span", "viewer-annotation-badge viewer-annotation-sent", "sent");
      badge.title = "Handed over to an agent";
      head.append(badge);
    }
    const tools = element(doc, "span", "viewer-annotation-tools");
    if (header.writable) {
      tools.append(
        button(doc, "viewer-annotation-tool", "Edit", "Edit this annotation", () => this.actions.edit(entry.id)),
        button(
          doc,
          "viewer-annotation-tool viewer-annotation-resolve",
          entry.resolved ? "Reopen" : "Resolve",
          entry.resolved ? "Reopen this annotation" : "Resolve this annotation",
          () => this.actions.setResolved(entry.id, !entry.resolved),
        ),
      );
    }
    tools.append(button(doc, "viewer-annotation-tool", "Delete", "Delete this annotation", () => this.actions.remove(entry.id)));
    head.append(tools);

    const parts: HTMLElement[] = [head];
    if (entry.quote !== undefined) parts.push(element(doc, "div", "viewer-annotation-quote", preview(entry.quote)));
    if (entry.kind === "change") {
      parts.push(entry.deletion
        ? element(doc, "div", "viewer-annotation-deletion", "Delete this passage")
        : element(doc, "div", "viewer-annotation-replacement", preview(entry.replacement ?? "")));
    }
    if (entry.body) parts.push(element(doc, "div", "viewer-annotation-body", entry.body));
    this.content.replaceChildren(...parts);

    this.replies.hidden = entry.replies.length === 0;
    this.replies.replaceChildren(...entry.replies.map((reply) => this.reply(entry, reply, header)));
    this.box.keep(new Set(entry.replies.filter((reply) => reply.author === "user").map((reply) => reply.id)));
    this.box.label(name);

    // A resolved thread is reopened before it goes on.
    const open = !entry.resolved && header.writable;
    this.actionsRow.hidden = !open;
    if (!open && this.box.open) this.box.element.hidden = true;
    this.setMovable(header.movable);
  }

  setMovable(movable: boolean): void {
    this.moveButton.disabled = !movable;
    this.moveButton.title = movable ? MOVE_TITLE : MOVE_DISABLED_TITLE;
  }

  private reply(entry: ListEntry, reply: ListReply, header: ListHeader): HTMLElement {
    const doc = this.doc;
    const item = element(doc, "li", "viewer-reply");
    item.dataset.id = reply.id;
    item.dataset.author = reply.author;
    if (reply.sent) item.dataset.sent = "";
    const head = element(doc, "div", "viewer-reply-head");
    head.append(element(doc, "span", "viewer-reply-author", replyAuthor(reply)));
    if (reply.sent) {
      const badge = element(doc, "span", "viewer-annotation-badge viewer-reply-sent", "sent");
      badge.title = "Handed over to an agent";
      head.append(badge);
    }
    if (header.writable) {
      const tools = element(doc, "span", "viewer-reply-tools");
      if (reply.author === "user" && !entry.resolved) {
        tools.append(button(doc, "viewer-annotation-tool", "Edit", "Edit this reply", () => this.box.start({ id: reply.id, body: reply.body })));
      }
      tools.append(button(doc, "viewer-annotation-tool", "Remove", "Remove this reply", () => this.actions.removeReply(entry.id, reply.id)));
      head.append(tools);
    }
    // An agent's text as text, never markup, with the characters a reader
    // would not see, or would see reordered, shown as U+FFFD.
    item.append(head, element(doc, "div", "viewer-reply-body", reply.author === "agent" ? safeText(reply.body) : reply.body));
    return item;
  }
}

export class AnnotationList {
  readonly element: HTMLElement;
  private readonly countLabel: HTMLElement;
  private readonly sendButton: HTMLButtonElement;
  private readonly moreButton: HTMLButtonElement;
  private readonly resolvedMarksButton: HTMLButtonElement;
  private readonly addButton: HTMLButtonElement;
  private readonly openList: HTMLOListElement;
  private readonly resolvedGroup: HTMLDetailsElement;
  private readonly resolvedSummary: HTMLElement;
  private readonly resolvedList: HTMLOListElement;
  private readonly empty: HTMLElement;
  private readonly views = new Map<string, EntryView>();
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
    this.sendButton = button(doc, "viewer-annotations-primary", "Send", "", (node) => this.actions.send(node));
    this.moreButton = button(doc, "viewer-annotations-more", "▾", "More ways to hand the review over", (node) => this.actions.more(node));
    this.moreButton.setAttribute("aria-label", "More ways to hand the review over");
    send.append(this.sendButton, this.moreButton);

    const options = element(doc, "div", "viewer-annotations-options");
    this.resolvedMarksButton = button(doc, "viewer-annotations-toggle viewer-annotations-show-resolved", "Show Resolved", "Mark the passages of resolved annotations as well", () => this.actions.toggleResolved());
    options.append(this.resolvedMarksButton);

    const scroll = element(doc, "div", "viewer-annotations-scroll");
    this.openList = element(doc, "ol", "viewer-annotations-list");
    this.openList.setAttribute("aria-label", "Open annotations");
    this.resolvedGroup = element(doc, "details", "viewer-annotations-resolved");
    this.resolvedSummary = element(doc, "summary", "viewer-annotations-resolved-summary");
    this.resolvedSummary.tabIndex = 0;
    this.resolvedList = element(doc, "ol", "viewer-annotations-list viewer-annotations-resolved-list");
    this.resolvedList.setAttribute("aria-label", "Resolved annotations");
    this.resolvedGroup.append(this.resolvedSummary, this.resolvedList);
    this.resolvedGroup.hidden = true;
    this.empty = element(doc, "p", "viewer-annotations-empty", "No annotations yet. Select a passage to annotate it, or add a general comment.");
    scroll.append(this.openList, this.resolvedGroup, this.empty);
    this.addButton = button(doc, "viewer-annotations-add", "Add Comment", "Add a comment on the whole document", () => this.actions.addComment());
    this.element.append(header, send, options, scroll, this.addButton);
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
    this.countLabel.textContent = header.open + header.resolved === 0 ? "No annotations"
      : header.resolved === 0 ? count(header.open, "open annotation", "open annotations")
      : `${header.open} open, ${header.resolved} resolved`;
    this.sendButton.textContent = header.send;
    this.sendButton.title = header.sendTitle;
    this.sendButton.disabled = !header.sendable;
    this.moreButton.disabled = header.open + header.resolved === 0;
    this.resolvedMarksButton.setAttribute("aria-pressed", String(header.resolvedShown));
    this.resolvedMarksButton.disabled = header.resolved === 0;
    this.addButton.hidden = !header.writable;
    this.empty.hidden = entries.length > 0;

    const listed = new Set(entries.map((entry) => entry.id));
    for (const [id, view] of this.views) {
      if (listed.has(id)) continue;
      view.element.remove();
      this.views.delete(id);
    }
    const viewOf = (entry: ListEntry) => {
      let view = this.views.get(entry.id);
      if (!view) {
        view = new EntryView(this.doc, entry, this.actions);
        this.views.set(entry.id, view);
      }
      view.update(entry, header, entry.id === active);
      return view.element;
    };
    const open = entries.filter((entry) => !entry.resolved).map(viewOf);
    const resolved = entries.filter((entry) => entry.resolved).map(viewOf);
    place(this.openList, open);
    place(this.resolvedList, resolved);
    this.openList.hidden = open.length === 0;
    this.resolvedGroup.hidden = resolved.length === 0;
    this.resolvedSummary.textContent = `Resolved (${resolved.length})`;
  }

  /** Whether a passage is selected that an annotation can move to. */
  setMovable(movable: boolean): void {
    for (const view of this.views.values()) view.setMovable(movable);
  }

  /** Bring an entry into view and show it as the chosen one; a resolved one opens its group. */
  reveal(id: string): void {
    this.active = id;
    for (const [each, view] of this.views) {
      const match = each === id;
      view.element.classList.toggle("active", match);
      if (!match) continue;
      if (this.resolvedList.contains(view.element)) this.resolvedGroup.open = true;
      view.element.scrollIntoView?.({ block: "nearest" });
    }
  }

  /** Open the reply box of an entry and give it the keyboard. */
  startReply(id: string): void {
    this.views.get(id)?.box.start();
  }

  /**
   * Give the keyboard to an entry, as after it moved between the groups; to
   * the folded group's summary for an entry that went into it.
   */
  focusEntry(id: string): void {
    const view = this.views.get(id);
    if (!view) return;
    const folded = this.resolvedList.contains(view.element) && !this.resolvedGroup.open;
    const target = folded ? this.resolvedSummary : view.element;
    target.focus({ preventScroll: true });
    target.scrollIntoView?.({ block: "nearest" });
  }
}

/** Put `items` into `list` in their order, moving as few of them as it takes. */
function place(list: HTMLElement, items: readonly HTMLElement[]): void {
  items.forEach((item, i) => {
    const at = list.children[i];
    if (at !== item) list.insertBefore(item, at ?? null);
  });
  while (list.children.length > items.length) list.lastElementChild!.remove();
}
