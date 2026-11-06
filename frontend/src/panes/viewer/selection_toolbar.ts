/**
 * The toolbar beside a selection in a viewer pane (`viewer-annotations`,
 * D6): the four kinds of annotation for the selected passage. A selection
 * too short to be found again, or longer than a quote may be, says so in
 * place of the kinds.
 */

import type { AnnotationKind } from "../../lib/protocol/types.ts";

/** What a selection allows: a passage to annotate, or why not. */
export type SelectionState = "passage" | "short" | "long";

/** The kinds as the toolbar offers them. */
export const KIND_ACTIONS: readonly { readonly kind: AnnotationKind; readonly label: string; readonly title: string }[] = [
  { kind: "comment", label: "Comment", title: "Comment on the selection" },
  { kind: "question", label: "Ask", title: "Ask about the selection" },
  { kind: "change", label: "Suggest Change", title: "Suggest a replacement for the selection" },
  { kind: "delete", label: "Suggest Deletion", title: "Suggest deleting the selection" },
];

export const SHORT_SELECTION = "Select a longer passage to annotate it.";
export const LONG_SELECTION = "The selection is too long to annotate: a passage may hold at most 16 KB.";

/** Where to show the toolbar: below the selection's end, or above its start where there is no room below. */
export interface ToolbarPlace {
  readonly left: number;
  readonly top: number;
  readonly above: number;
}

const GAP = 6;
const MARGIN = 8;

export class SelectionToolbar {
  readonly element: HTMLElement;
  private readonly kinds: HTMLElement;
  private readonly note: HTMLElement;

  constructor(doc: Document, private readonly choose: (kind: AnnotationKind) => void) {
    this.element = doc.createElement("div");
    this.element.className = "viewer-selection-toolbar";
    this.element.setAttribute("role", "toolbar");
    this.element.setAttribute("aria-label", "Annotate the selection");
    this.element.hidden = true;
    this.kinds = doc.createElement("div");
    this.kinds.className = "viewer-selection-kinds";
    for (const action of KIND_ACTIONS) {
      const button = doc.createElement("button");
      button.type = "button";
      button.className = "viewer-selection-kind";
      button.textContent = action.label;
      button.title = action.title;
      button.dataset.kind = action.kind;
      button.addEventListener("click", () => this.choose(action.kind));
      this.kinds.append(button);
    }
    this.note = doc.createElement("p");
    this.note.className = "viewer-selection-note";
    this.element.append(this.kinds, this.note);
    // Pressing a button must not end the selection it acts on.
    this.element.addEventListener("pointerdown", (event) => event.preventDefault());
    this.element.addEventListener("mousedown", (event) => event.preventDefault());
  }

  get shown(): boolean {
    return !this.element.hidden;
  }

  /** Show the toolbar for a selection in `state`, at `place` inside a box of `width` by `height`. */
  show(state: SelectionState, place: ToolbarPlace, width: number, height: number): void {
    this.kinds.hidden = state !== "passage";
    this.note.hidden = state === "passage";
    this.note.textContent = state === "short" ? SHORT_SELECTION : state === "long" ? LONG_SELECTION : "";
    this.element.hidden = false;
    // Measured at the box's left edge: where it stood before, near the right
    // edge, it would be squeezed and measure narrower than it is.
    this.element.style.left = "0px";
    this.element.style.top = "0px";
    const ownWidth = this.element.offsetWidth;
    const ownHeight = this.element.offsetHeight;
    const left = Math.max(MARGIN, Math.min(place.left, width - ownWidth - MARGIN));
    const below = place.top + GAP;
    const top = below + ownHeight + MARGIN <= height || place.above - GAP - ownHeight < MARGIN
      ? below
      : place.above - GAP - ownHeight;
    this.element.style.left = `${Math.round(left)}px`;
    this.element.style.top = `${Math.round(Math.max(MARGIN, top))}px`;
  }

  hide(): void {
    this.element.hidden = true;
  }
}
