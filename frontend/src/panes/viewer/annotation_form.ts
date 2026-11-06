/**
 * The form in which an annotation is written (D6 of review-pane, D1 of
 * review-threads): its kind, a comment or a suggested change, the passage
 * exactly as it is stored, the remark and, for a suggested change, the
 * replacement, which starts as the passage. An empty replacement suggests
 * deleting the passage, and the form says so. A general comment is a
 * comment. Ctrl+Enter or Cmd+Enter saves, Escape cancels.
 *
 * The passage is shown read-only and scrolls when it is long: in the rendered
 * view a selection also takes text the document hides with CSS, and the form
 * is where the user sees all of it before it goes into a review. The form
 * keeps what the user writes while another window changes the same
 * annotation; saving then replaces that change.
 */

import type { AnnotationKind } from "../../lib/protocol/types.ts";
import { isMac } from "../../lib/shortcuts.ts";

export interface FormDraft {
  readonly kind: AnnotationKind;
  readonly body: string;
  readonly replacement: string;
}

export interface AnnotationFormOptions {
  /** The kind the form starts with. */
  readonly kind: AnnotationKind;
  /** The passage as it is stored; absent for a general comment, which is a comment. */
  readonly quote?: string;
  /** Where the passage is, as the list names it. */
  readonly where?: string;
  readonly body?: string;
  /** The replacement of a change being edited; a new change starts with the passage. */
  readonly replacement?: string;
  /** The annotation exists already. */
  readonly editing: boolean;
  /** Save the draft; returns why it cannot be saved, or undefined once it is saved. */
  save(draft: FormDraft): string | undefined;
  cancel(): void;
}

const KIND_LABELS: Record<AnnotationKind, string> = {
  comment: "Comment",
  change: "Change",
};

const REMARK_LABELS: Record<AnnotationKind, string> = {
  comment: "Comment",
  change: "Reason (optional)",
};

export const DELETION_NOTE = "An empty replacement suggests deleting the passage.";

function element<K extends keyof HTMLElementTagNameMap>(doc: Document, tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const node = doc.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export class AnnotationForm {
  readonly element: HTMLElement;
  private kind: AnnotationKind;
  private readonly kindButtons = new Map<AnnotationKind, HTMLButtonElement>();
  private readonly title: HTMLElement;
  private readonly remark: HTMLTextAreaElement;
  private readonly remarkLabel: HTMLElement;
  private readonly replacement: HTMLTextAreaElement;
  private readonly replacementBox: HTMLElement;
  private readonly deletionNote: HTMLElement;
  private readonly problem: HTMLElement;
  /** The replacement holds what the user or the annotation gave it, so switching kinds leaves it as it is. */
  private replacementGiven: boolean;

  constructor(doc: Document, private readonly options: AnnotationFormOptions) {
    this.kind = options.quote === undefined ? "comment" : options.kind;
    this.element = element(doc, "div", "viewer-annotation-form");
    this.element.setAttribute("role", "dialog");

    const header = element(doc, "div", "viewer-form-header");
    this.title = element(doc, "span", "viewer-form-title");
    header.append(this.title);
    // A general comment has one kind.
    if (options.quote !== undefined) {
      const kinds = element(doc, "div", "viewer-form-kinds");
      kinds.setAttribute("role", "radiogroup");
      kinds.setAttribute("aria-label", "Kind");
      for (const kind of ["comment", "change"] as const) {
        const button = element(doc, "button", "viewer-form-kind", KIND_LABELS[kind]);
        button.type = "button";
        button.dataset.kind = kind;
        button.setAttribute("role", "radio");
        button.addEventListener("click", () => this.setKind(kind));
        this.kindButtons.set(kind, button);
        kinds.append(button);
      }
      header.append(kinds);
    }
    this.element.append(header);

    if (options.quote !== undefined) {
      const label = element(doc, "div", "viewer-form-label", options.where ? `Passage, ${options.where}` : "Passage");
      const quote = element(doc, "pre", "viewer-form-quote", options.quote);
      // Read with the keyboard as well: it scrolls when it is long.
      quote.tabIndex = 0;
      quote.setAttribute("aria-label", "The annotated passage as it is stored");
      this.element.append(label, quote);
    }

    this.replacementBox = element(doc, "div", "viewer-form-field viewer-form-replacement");
    this.replacementBox.append(element(doc, "div", "viewer-form-label", "Replace with"));
    this.replacement = element(doc, "textarea", "viewer-form-text");
    this.replacement.rows = 3;
    this.replacement.spellcheck = false;
    this.replacement.setAttribute("aria-label", "Replacement");
    this.replacementGiven = options.replacement !== undefined;
    this.replacement.value = options.replacement ?? (this.kind === "change" ? options.quote ?? "" : "");
    if (this.kind === "change") this.replacementGiven = true;
    this.deletionNote = element(doc, "p", "viewer-form-deletion", DELETION_NOTE);
    this.replacementBox.append(this.replacement, this.deletionNote);
    this.replacement.addEventListener("input", () => {
      this.replacementGiven = true;
      this.showTitle();
    });

    const remarkBox = element(doc, "div", "viewer-form-field");
    this.remarkLabel = element(doc, "div", "viewer-form-label");
    this.remark = element(doc, "textarea", "viewer-form-text");
    this.remark.rows = 3;
    this.remark.value = options.body ?? "";
    remarkBox.append(this.remarkLabel, this.remark);
    this.element.append(this.replacementBox, remarkBox);

    this.problem = element(doc, "p", "viewer-form-problem");
    this.problem.setAttribute("role", "alert");
    this.problem.hidden = true;

    const actions = element(doc, "div", "viewer-form-actions");
    const hint = element(doc, "span", "viewer-form-hint", `${isMac ? "⌘" : "Ctrl+"}Enter saves, Esc cancels`);
    const cancel = element(doc, "button", "viewer-form-button", "Cancel");
    cancel.type = "button";
    cancel.addEventListener("click", () => this.options.cancel());
    const save = element(doc, "button", "viewer-form-button viewer-form-save", "Save");
    save.type = "button";
    save.addEventListener("click", () => this.save());
    actions.append(hint, cancel, save);
    this.element.append(this.problem, actions);

    this.element.addEventListener("keydown", (event) => {
      if (event.isComposing) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        this.options.cancel();
      } else if (event.key === "Enter" && (isMac ? event.metaKey : event.ctrlKey)) {
        event.preventDefault();
        event.stopPropagation();
        this.save();
      }
    });
    // The text fields' own menu, not the pane's.
    this.element.addEventListener("contextmenu", (event) => event.stopPropagation());
    this.setKind(this.kind);
  }

  /** Give the keyboard to the field the kind is written in. */
  focus(): void {
    const field = this.kind === "change" ? this.replacement : this.remark;
    field.focus({ preventScroll: true });
    field.setSelectionRange(field.value.length, field.value.length);
  }

  /** What the form holds now. */
  draft(): FormDraft {
    return { kind: this.kind, body: this.remark.value, replacement: this.replacement.value };
  }

  private setKind(kind: AnnotationKind): void {
    this.kind = kind;
    for (const [each, button] of this.kindButtons) {
      button.setAttribute("aria-checked", String(each === kind));
      button.classList.toggle("selected", each === kind);
    }
    // A change starts with the passage the first time it is chosen.
    if (kind === "change" && !this.replacementGiven) {
      this.replacement.value = this.options.quote ?? "";
      this.replacementGiven = true;
    }
    this.remarkLabel.textContent = REMARK_LABELS[kind];
    this.remark.setAttribute("aria-label", REMARK_LABELS[kind]);
    this.replacementBox.hidden = kind !== "change";
    this.problem.hidden = true;
    this.showTitle();
  }

  /** The title names what the form makes: a comment, a suggested change, or a suggested deletion. */
  private showTitle(): void {
    const verb = this.options.editing ? "Edit" : "New";
    const deletion = this.kind === "change" && this.replacement.value === "";
    const what = this.kind === "comment" ? "Comment" : deletion ? "Suggested Deletion" : "Suggested Change";
    this.title.textContent = `${verb} ${what}`;
    this.deletionNote.hidden = !deletion;
    this.element.setAttribute("aria-label", this.title.textContent);
  }

  private save(): void {
    const problem = this.options.save(this.draft());
    this.problem.textContent = problem ?? "";
    this.problem.hidden = !problem;
  }
}
