/**
 * The form in which an annotation is written (`viewer-annotations`, D6): its
 * kind, the passage exactly as it is stored, the remark and, for a suggested
 * change, the replacement, prefilled with the passage. Ctrl+Enter or
 * Cmd+Enter saves, Escape cancels.
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
  /** The passage as it is stored; absent for a general comment, which is a comment or a question. */
  readonly quote?: string;
  /** Where the passage is, as the list names it. */
  readonly where?: string;
  readonly body?: string;
  readonly replacement?: string;
  /** The annotation exists already. */
  readonly editing: boolean;
  /** Save the draft; returns why it cannot be saved, or undefined once it is saved. */
  save(draft: FormDraft): string | undefined;
  cancel(): void;
}

const KIND_LABELS: Record<AnnotationKind, string> = {
  comment: "Comment",
  question: "Question",
  change: "Change",
  delete: "Deletion",
};

const REMARK_LABELS: Record<AnnotationKind, string> = {
  comment: "Comment",
  question: "Question",
  change: "Reason (optional)",
  delete: "Reason (optional)",
};

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
  private readonly problem: HTMLElement;

  constructor(doc: Document, private readonly options: AnnotationFormOptions) {
    this.kind = options.kind;
    this.element = element(doc, "div", "viewer-annotation-form");
    this.element.setAttribute("role", "dialog");

    const header = element(doc, "div", "viewer-form-header");
    this.title = element(doc, "span", "viewer-form-title");
    const kinds = element(doc, "div", "viewer-form-kinds");
    kinds.setAttribute("role", "radiogroup");
    kinds.setAttribute("aria-label", "Kind");
    const offered: AnnotationKind[] = options.quote === undefined ? ["comment", "question"] : ["comment", "question", "change", "delete"];
    for (const kind of offered) {
      const button = element(doc, "button", "viewer-form-kind", KIND_LABELS[kind]);
      button.type = "button";
      button.setAttribute("role", "radio");
      button.addEventListener("click", () => this.setKind(kind));
      this.kindButtons.set(kind, button);
      kinds.append(button);
    }
    header.append(this.title, kinds);
    this.element.append(header);

    if (options.quote !== undefined) {
      const label = element(doc, "div", "viewer-form-label", options.where ? `Passage, ${options.where}` : "Passage");
      const quote = element(doc, "pre", "viewer-form-quote", options.quote);
      // Read with the keyboard as well: it scrolls when it is long.
      quote.tabIndex = 0;
      quote.setAttribute("aria-label", "The annotated passage as it is stored");
      this.element.append(label, quote);
    }

    this.replacementBox = element(doc, "div", "viewer-form-field");
    this.replacementBox.append(element(doc, "div", "viewer-form-label", "Replace with"));
    this.replacement = element(doc, "textarea", "viewer-form-text");
    this.replacement.rows = 3;
    this.replacement.spellcheck = false;
    this.replacement.setAttribute("aria-label", "Replacement");
    this.replacement.value = options.replacement ?? (options.kind === "change" ? options.quote ?? "" : "");
    this.replacementBox.append(this.replacement);

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
    const verb = this.options.editing ? "Edit" : "New";
    this.title.textContent = `${verb} ${KIND_LABELS[kind]}`;
    this.remarkLabel.textContent = REMARK_LABELS[kind];
    this.remark.setAttribute("aria-label", REMARK_LABELS[kind]);
    this.replacementBox.hidden = kind !== "change";
    if (kind === "change" && !this.replacement.value) this.replacement.value = this.options.quote ?? "";
    this.problem.hidden = true;
  }

  private save(): void {
    const problem = this.options.save(this.draft());
    this.problem.textContent = problem ?? "";
    this.problem.hidden = !problem;
  }
}
