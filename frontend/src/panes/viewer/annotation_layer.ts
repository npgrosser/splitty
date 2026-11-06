/**
 * Annotations in a viewer pane (`viewer-annotations`): anchoring, marks, the
 * selection toolbar, the form, the list column and the hand-over of the
 * review, for the document the pane's view shows (`viewer_view.ts`).
 *
 * Each annotation anchors by its quote in the text of its own view: the
 * source, or the rendered view's text, which is rendered for its text alone
 * while the source is shown. A rendered passage of a document that is shown
 * only as source anchors in the source. Anchoring runs again whenever the
 * annotations, the document's version or the view change. The passage is
 * marked in its own view, and its lines in the other one: a bar beside the
 * source lines, a mark on the covering blocks of a rendered Markdown
 * document. A rendered HTML page names no lines, so its passages show only
 * in its rendered view.
 *
 * Annotations are offered while the pane's server keeps them and the
 * document is text; an image or a binary file shows the list only while it
 * holds annotations.
 */

import type { ContextMenuItem } from "../../stores/ui.svelte.ts";
import type { AnnotationKind, AnnotationTarget, ViewerAnnotation } from "../../lib/protocol/types.ts";
import { anchorer, sourceLines, targetAt, type Anchor, type Anchoring, type LineRange, type QuoteTarget } from "../../lib/viewer/anchoring.ts";
import {
  ANNOTATION_LINE_LIMIT, ANNOTATION_TEXT_LIMIT, MIN_QUOTE_CHARS, newAnnotationId, registerAnnotationList, type ViewerAnnotations,
} from "../../lib/viewer/annotations.ts";
import { utf8Length } from "../../lib/viewer/limits.ts";
import { scratchpadProblem, sendReview, type HandoverNotices, type Review, type ReviewTarget, type SendForm } from "../../lib/viewer/handover.ts";
import type { BlockMark, BoundaryRange, RenderedDocument, RenderedMark, RenderedText } from "../../lib/viewer/render.ts";
import { compareListPlaces, reviewFileName, reviewMarkdown, type ListPlace, type ReviewDocument, type ReviewEntry } from "../../lib/viewer/review.ts";
import type { SourceMark, SourceView } from "../../lib/viewer/source_view.ts";
import { AnnotationForm, type FormDraft } from "./annotation_form.ts";
import { AnnotationList, type ListEntry } from "./annotation_list.ts";
import { SelectionToolbar, type SelectionState } from "./selection_toolbar.ts";

/** What a viewer pane shows, as annotations need it. */
export interface AnnotatedContent {
  /** The version of the document shown; undefined while none is. */
  readonly version: string | undefined;
  /** The view on the screen: the rendered document, its source, or neither. */
  readonly shown: "rendered" | "source" | null;
  /** The format of a document that renders. */
  readonly format: "markdown" | "html" | undefined;
  /** The text source annotations anchor in, for a text document; line breaks as `\n`. */
  readonly sourceText: string | undefined;
  /** The rendered view's text: the shown document's, or one rendered for its text alone. */
  renderedText(): RenderedText | undefined;
  readonly source: SourceView | null;
  readonly rendered: RenderedDocument | null;
}

/** The terminals a review can go to. */
export interface ReviewHandover {
  /** The viewer's origin, while it exists as a terminal pane. */
  origin(): ReviewTarget | undefined;
  /** The terminal panes of the viewer's screen. */
  terminals(): ReviewTarget[];
}

export interface LayerNotices extends HandoverNotices {
  /** A change the user can take back for a while, such as a deletion. */
  undoable(message: string, undo: () => void): void;
}

export interface AnnotationLayerOptions {
  readonly store: ViewerAnnotations;
  readonly paneId: string;
  /** The id of the pane's screen on its server, now. */
  screenId(): string;
  /** Whether the pane's server keeps annotations. */
  available(): boolean;
  /** The document as the review names it. */
  readonly document: ReviewDocument;
  readonly handover: ReviewHandover;
  readonly notices: LayerNotices;
  showMenu(items: ContextMenuItem[], x: number, y: number): void;
  writeClipboard(text: string): Promise<void>;
  download(name: string, blob: Blob): void;
}

/** The parts of the pane the layer places its own in. */
export interface LayerHost {
  /** The pane's body, which the list column joins on the right. */
  readonly body: HTMLElement;
  /** The document's area, positioned: the toolbar and the form float in it. */
  readonly main: HTMLElement;
  /** The document's view, whose pointer, keys and scrolling show and hide the toolbar. */
  readonly documentBox: HTMLElement;
  /** The bar, whose end takes the list's button. */
  readonly bar: HTMLElement;
}

/** What the context menu offers for annotations. */
export interface AnnotationMenu {
  /** A selected passage, one too short or too long to annotate, or none. */
  readonly selection: SelectionState | "none";
  annotate(kind: AnnotationKind): void;
  addComment(): void;
  readonly listShown: boolean;
  toggleList(): void;
}

/** Where the list column stands by the user's choice; without one it is open while the pane holds annotations. */
export type ListChoice = "open" | "closed";

interface Placed {
  readonly annotation: ViewerAnnotation;
  readonly place: ListPlace;
  /** The text the passage was found in. */
  readonly view?: "source" | "rendered";
  readonly anchor?: Anchor;
}

interface Anchorers {
  readonly version: string | undefined;
  readonly shown: AnnotatedContent["shown"];
  readonly sourceText: string | undefined;
  readonly source?: (target: QuoteTarget) => Anchoring;
  readonly sourceLines?: LineRange;
  rendered?: ((target: QuoteTarget) => Anchoring) | null;
}

interface Selected {
  readonly state: SelectionState;
  readonly target: AnnotationTarget;
  /** The selection's end and start on the screen, where the engine tells. */
  readonly rect?: { readonly left: number; readonly bottom: number; readonly top: number };
}

const STALE: Anchoring = { state: "stale" };

function linesLabel(start: number, end: number): string {
  return start === end ? `Line ${start}` : `Lines ${start}-${end}`;
}

/** Whether `text` holds at least `count` characters, counted as code points. */
function hasChars(text: string, count: number): boolean {
  let seen = 0;
  for (const _ of text) if (++seen >= count) return true;
  return false;
}

function clampLine(line: number): number {
  return Math.max(1, Math.min(ANNOTATION_LINE_LIMIT, line));
}

/** The rectangles of a selection on the screen: a live range of it, where the engine lays ranges out. */
function selectionRect(doc: Document, range: BoundaryRange): Selected["rect"] {
  let live: Range;
  try {
    live = doc.createRange();
    live.setStart(range.startContainer, range.startOffset);
    live.setEnd(range.endContainer, range.endOffset);
  } catch {
    return undefined;
  }
  if (typeof live.getClientRects !== "function") return undefined;
  const rects = [...live.getClientRects()].filter((rect) => rect.width > 0 || rect.height > 0);
  if (rects.length === 0) return undefined;
  const last = rects[rects.length - 1];
  return { left: last.right, bottom: last.bottom, top: rects[0].top };
}

export class AnnotationLayer {
  private readonly toolbar: SelectionToolbar;
  private readonly list: AnnotationList;
  private readonly listButton: HTMLButtonElement;
  private placed: Placed[] = [];
  /** The store's list the annotations were placed from; the same array while it is unchanged. */
  private listed: readonly ViewerAnnotation[] | undefined;
  private anchorers: Anchorers | undefined;
  private active: string | undefined;
  private choice: ListChoice | undefined;
  private form: { readonly form: AnnotationForm; readonly id?: string } | null = null;
  private selected: Selected | undefined;
  private readonly cleanups: (() => void)[] = [];
  private toolbarTimer: ReturnType<typeof setTimeout> | undefined;
  private destroyed = false;

  constructor(
    private readonly doc: Document,
    private readonly host: LayerHost,
    private readonly content: () => AnnotatedContent,
    private readonly options: AnnotationLayerOptions,
    choice: ListChoice | undefined,
    private readonly focusDocument: () => void,
  ) {
    this.choice = choice;
    this.toolbar = new SelectionToolbar(doc, (kind) => this.annotateSelection(kind));
    host.main.append(this.toolbar.element);
    this.list = new AnnotationList(doc, {
      select: (id) => this.select(id),
      edit: (id) => this.edit(id),
      remove: (id) => this.remove(id),
      addComment: () => this.openForm({ kind: "comment" }),
      send: (anchor) => this.sendToDefault(anchor),
      more: (anchor) => this.showMenuAt(anchor, this.handoverItems()),
      close: () => this.setChoice("closed"),
    });
    host.body.append(this.list.element);
    this.list.element.addEventListener("pointerenter", () => this.renderList());

    this.listButton = doc.createElement("button");
    this.listButton.type = "button";
    this.listButton.className = "viewer-bar-button viewer-bar-annotations";
    this.listButton.addEventListener("click", () => this.toggleList());
    host.bar.append(this.listButton);

    const box = host.documentBox;
    const later = () => this.scheduleToolbar();
    const hide = (event: Event) => {
      if (!this.toolbar.element.contains(event.target as Node)) this.hideToolbar();
    };
    this.listen(box, "pointerup", later);
    this.listen(box, "keyup", later);
    this.listen(box, "pointerdown", hide);
    this.listen(box, "scroll", () => this.hideToolbar(), { capture: true, passive: true });
    this.listen(doc, "selectionchange", () => {
      if (this.toolbar.shown && !this.readSelection()) this.hideToolbar();
    });

    const { store, paneId } = options;
    this.cleanups.push(
      // The store tells of every pane's changes; this pane's list keeps its array while it has none.
      store.subscribe(() => {
        if (store.list(paneId) !== this.listed) this.refresh();
      }),
      store.onRejected((rejected, error) => {
        if (rejected === paneId) options.notices.error("The annotations could not be saved", error);
      }),
      registerAnnotationList(paneId, () => this.setChoice("open")),
    );
    this.refresh();
  }

  /** The list column's state as the user chose it, for the pane's next view. */
  get listChoice(): ListChoice | undefined {
    return this.choice;
  }

  /** The document, its version or its view changed: anchor and mark again. */
  contentChanged(): void {
    this.hideToolbar();
    this.refresh();
  }

  /** The selection changed; a toolbar shown for an earlier one goes once it is empty. */
  selectionChanged(): void {
    if (this.toolbar.shown && !this.readSelection()) this.hideToolbar();
  }

  /** What the context menu offers, for the selection as it is now. */
  menu(): AnnotationMenu | undefined {
    if (!this.writable()) return undefined;
    const selected = this.readSelection();
    return {
      selection: selected?.state ?? "none",
      annotate: (kind) => {
        if (selected?.state === "passage") this.openForm({ kind, target: selected.target, rect: selected.rect });
      },
      addComment: () => this.openForm({ kind: "comment" }),
      listShown: this.listShown(),
      toggleList: () => this.toggleList(),
    };
  }

  /** The review of the pane's annotations as they are anchored now, and the annotations it holds. */
  review(): { review: Review; sent: ViewerAnnotation[] } {
    this.anchorAll();
    const content = this.content();
    const entries: ReviewEntry[] = this.placed.map(({ annotation, view, anchor }) => {
      const { kind, body, replacement, target } = annotation;
      if (!target) return { kind, body };
      const page = (view ?? target.view) === "rendered" && content.format === "html";
      const lines = page ? null
        : anchor ? { start: anchor.startLine, end: anchor.endLine }
        : { start: target.startLine, end: target.endLine };
      return { kind, body, replacement, passage: { quote: target.quote, lines, stale: !anchor } };
    });
    return {
      review: { text: reviewMarkdown(this.options.document, entries), fileName: reviewFileName(this.options.document) },
      sent: this.placed.map((placed) => placed.annotation),
    };
  }

  destroy(): void {
    this.destroyed = true;
    clearTimeout(this.toolbarTimer);
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups.length = 0;
    this.closeForm();
    this.toolbar.element.remove();
    this.list.element.remove();
    this.listButton.remove();
  }

  // ── Anchoring and Marks ──

  private refresh(): void {
    if (this.destroyed) return;
    this.anchorAll();
    this.applyMarks();
    this.renderList();
  }

  private anchorersFor(content: AnnotatedContent): Anchorers {
    const known = this.anchorers;
    if (known && known.version === content.version && known.shown === content.shown && known.sourceText === content.sourceText) return known;
    const text = content.sourceText;
    const lines = text !== undefined ? sourceLines(text) : undefined;
    this.anchorers = {
      version: content.version,
      shown: content.shown,
      sourceText: text,
      ...(text !== undefined && lines ? { source: anchorer(text, lines), sourceLines: lines } : {}),
    };
    return this.anchorers;
  }

  /** The anchorer of the rendered view's text, built when a rendered annotation needs it. */
  private renderedAnchorer(content: AnnotatedContent, anchorers: Anchorers): ((target: QuoteTarget) => Anchoring) | null {
    if (anchorers.rendered === undefined) {
      const model = content.format ? content.renderedText() : undefined;
      anchorers.rendered = model ? anchorer(model.text, model.lines) : null;
    }
    return anchorers.rendered;
  }

  private anchorAll(): void {
    const content = this.content();
    const anchorers = this.anchorersFor(content);
    this.listed = this.options.store.list(this.options.paneId);
    const placed = this.listed.map((annotation, index): Placed => {
      const target = annotation.target;
      if (!target) return { annotation, place: { state: "general", index } };
      let view: "source" | "rendered" | undefined;
      let anchoring = STALE;
      const rendered = target.view === "rendered" ? this.renderedAnchorer(content, anchorers) : null;
      if (rendered) {
        view = "rendered";
        anchoring = rendered(target);
      } else if (anchorers.source) {
        view = "source";
        anchoring = anchorers.source(target);
      }
      if (anchoring.state === "anchored") {
        const anchor = anchoring.anchor;
        return {
          annotation, view, anchor,
          place: { state: "anchored", view, offset: anchor.start, startLine: anchor.startLine, endLine: anchor.endLine, index },
        };
      }
      return { annotation, place: { state: "stale", startLine: target.startLine, endLine: target.endLine, index } };
    });
    placed.sort((a, b) => compareListPlaces(a.place, b.place));
    this.placed = placed;
  }

  private applyMarks(): void {
    const content = this.content();
    const lineMarks = content.format === "markdown";
    if (content.shown === "source" && content.source) {
      const marks: SourceMark[] = [];
      for (const { annotation, view, anchor } of this.placed) {
        if (!anchor) continue;
        const active = annotation.id === this.active;
        if (view === "source") marks.push({ from: anchor.start, to: anchor.end, startLine: anchor.startLine, endLine: anchor.endLine, active });
        else if (lineMarks) marks.push({ startLine: anchor.startLine, endLine: anchor.endLine, active });
      }
      content.source.setMarks(marks);
    } else if (content.shown === "rendered" && content.rendered) {
      const marks: RenderedMark[] = [];
      const blocks: BlockMark[] = [];
      for (const { annotation, view, anchor } of this.placed) {
        if (!anchor) continue;
        const active = annotation.id === this.active;
        if (view === "rendered") marks.push({ start: anchor.start, end: anchor.end, active });
        else if (lineMarks) blocks.push({ startLine: anchor.startLine, endLine: anchor.endLine, active });
      }
      content.rendered.setMarks(marks, blocks);
    }
  }

  // ── The List ──

  private listShown(): boolean {
    const count = this.options.store.count(this.options.paneId);
    if (!this.options.available() && count === 0) return false;
    return this.choice === "open" || (this.choice !== "closed" && count > 0);
  }

  private toggleList(): void {
    this.setChoice(this.listShown() ? "closed" : "open");
  }

  private setChoice(choice: ListChoice): void {
    this.choice = choice;
    this.renderList();
    if (choice === "closed" && this.list.element.contains(this.doc.activeElement)) this.focusDocument();
  }

  private where(placed: Placed): string {
    const { annotation, view, anchor } = placed;
    const target = annotation.target;
    if (!target) return "General";
    if ((view ?? target.view) === "rendered" && this.content().format === "html") return "Passage";
    return anchor ? linesLabel(anchor.startLine, anchor.endLine) : linesLabel(target.startLine, target.endLine);
  }

  private renderList(): void {
    if (this.destroyed) return;
    const count = this.placed.length;
    const available = this.options.available();
    const shown = this.listShown();
    this.list.setShown(shown);
    this.listButton.hidden = !available && count === 0;
    this.listButton.textContent = count === 0 ? "Annotations" : count === 1 ? "1 Annotation" : `${count} Annotations`;
    this.listButton.title = shown ? "Hide the annotations" : "Show the annotations";
    this.listButton.setAttribute("aria-pressed", String(shown));
    if (!shown) return;
    const origin = this.options.handover.origin();
    const entries: ListEntry[] = this.placed.map((placed) => ({
      id: placed.annotation.id,
      kind: placed.annotation.kind,
      where: this.where(placed),
      stale: placed.place.state === "stale",
      quote: placed.annotation.target?.quote,
      body: placed.annotation.body,
      replacement: placed.annotation.replacement,
    }));
    this.list.render({
      count,
      send: origin ? `Send to ${origin.title}` : "Send to Terminal…",
      writable: this.writable(),
    }, entries, this.active);
  }

  private select(id: string): void {
    const placed = this.placed.find((each) => each.annotation.id === id);
    if (!placed) return;
    this.active = id;
    this.applyMarks();
    this.list.reveal(id);
    const content = this.content();
    const { view, anchor } = placed;
    if (!anchor) return;
    if (content.shown === "source" && content.source) {
      if (view === "source") content.source.scrollToPassage(anchor.start, anchor.end);
      else if (content.source) content.source.scrollToLine(anchor.startLine);
    } else if (content.shown === "rendered" && content.rendered) {
      if (view === "rendered") content.rendered.scrollToPassage(anchor.start, anchor.end);
      else content.rendered.scrollToLines(anchor.startLine, anchor.endLine);
    }
  }

  private edit(id: string): void {
    const annotation = this.options.store.list(this.options.paneId).find((each) => each.id === id);
    if (!annotation) return;
    const placed = this.placed.find((each) => each.annotation.id === id);
    this.openForm({ kind: annotation.kind, target: annotation.target, existing: annotation, where: placed ? this.where(placed) : undefined });
  }

  private remove(id: string): void {
    const annotation = this.options.store.list(this.options.paneId).find((each) => each.id === id);
    if (!annotation) return;
    if (this.active === id) this.active = undefined;
    this.options.store.remove(this.options.screenId(), this.options.paneId, [id]);
    this.options.notices.undoable("Annotation deleted", () => this.putBack([annotation]));
  }

  private clear(): void {
    const all = [...this.options.store.list(this.options.paneId)];
    if (all.length === 0) return;
    this.active = undefined;
    this.options.store.remove(this.options.screenId(), this.options.paneId, all.map((annotation) => annotation.id));
    this.options.notices.undoable(all.length === 1 ? "Annotation cleared" : `${all.length} annotations cleared`, () => this.putBack(all));
  }

  /** Put annotations back as they were, by the server's rules: they were kept before. */
  private putBack(annotations: readonly ViewerAnnotation[]): void {
    const problem = this.options.store.put(this.options.screenId(), this.options.paneId, annotations, { restore: true });
    if (problem) this.options.notices.error("The annotations could not be put back", problem);
  }

  // ── The Review ──

  private targets(): ReviewTarget[] {
    const origin = this.options.handover.origin();
    const terminals = this.options.handover.terminals();
    return origin ? [origin, ...terminals.filter((target) => target.paneId !== origin.paneId)] : terminals;
  }

  private showMenuAt(anchor: HTMLElement, items: ContextMenuItem[]): void {
    const rect = anchor.getBoundingClientRect();
    this.options.showMenu(items, rect.left, rect.bottom);
  }

  private targetItems(form: SendForm, review?: Review): ContextMenuItem[] {
    const targets = this.targets();
    if (targets.length === 0) return [{ label: "No terminal pane on this screen", action: () => {}, disabled: true }];
    return targets.map((target) => {
      const problem = form === "scratchpad" && review ? scratchpadProblem(review, target) : undefined;
      return {
        label: target.title,
        action: () => void this.send(target, form),
        ...(problem ? { disabled: true, detail: problem } : {}),
      };
    });
  }

  private handoverItems(): ContextMenuItem[] {
    const { review } = this.review();
    return [
      { label: "Send To", action: () => {}, children: this.targetItems("paste") },
      { label: "Send as File To", action: () => {}, children: this.targetItems("file") },
      { label: "Append to Scratchpad Of", action: () => {}, children: this.targetItems("scratchpad", review) },
      { label: "", action: () => {}, separator: true },
      { label: "Copy as Markdown", action: () => void this.copy() },
      { label: "Save as File", action: () => this.download() },
      { label: "", action: () => {}, separator: true },
      { label: "Clear Annotations", action: () => this.clear(), danger: true },
    ];
  }

  /** The main action: the origin, or a choice among the screen's terminals when it is gone. */
  private sendToDefault(anchor: HTMLElement): void {
    const origin = this.options.handover.origin();
    if (origin) void this.send(origin, "paste");
    else this.showMenuAt(anchor, this.targetItems("paste"));
  }

  private async send(target: ReviewTarget, form: SendForm): Promise<void> {
    const { review, sent } = this.review();
    if (sent.length === 0) return;
    const { store, paneId } = this.options;
    const screenId = this.options.screenId();
    const ids = sent.map((annotation) => annotation.id);
    await sendReview(review, target, form, {
      end: () => {
        if (this.active && ids.includes(this.active)) this.active = undefined;
        store.remove(screenId, paneId, ids);
      },
      restore: () => this.putBack(sent),
    }, this.options.notices);
  }

  private async copy(): Promise<void> {
    const { review } = this.review();
    try {
      await this.options.writeClipboard(review.text);
      this.options.notices.info("The review is in the clipboard");
    } catch (error) {
      this.options.notices.error("The review could not be copied", error instanceof Error ? error.message : undefined);
    }
  }

  private download(): void {
    const { review } = this.review();
    this.options.download(review.fileName, new Blob([review.text], { type: "text/markdown" }));
  }

  // ── Making Annotations ──

  private writable(): boolean {
    return this.options.available() && this.content().sourceText !== undefined;
  }

  /** The selection as an annotation's target, or undefined without one. */
  private readSelection(): Selected | undefined {
    if (!this.writable()) return undefined;
    const content = this.content();
    let target: QuoteTarget | null = null;
    let view: AnnotationTarget["view"];
    let rect: Selected["rect"];
    if (content.shown === "source" && content.source && content.sourceText !== undefined) {
      const selection = content.source.selection();
      if (!selection) return undefined;
      const lines = this.anchorersFor(content).sourceLines ?? sourceLines(content.sourceText);
      target = targetAt(content.sourceText, selection.from, selection.to, lines);
      view = "source";
      const head = content.source.selectionHead();
      if (head) rect = { left: head.left, bottom: head.bottom, top: head.top };
    } else if (content.shown === "rendered" && content.rendered) {
      const passage = content.rendered.selectedPassage();
      const model = content.rendered.textModel();
      if (!passage || !model) return undefined;
      target = targetAt(model.text, passage.start, passage.end, model.lines);
      view = "rendered";
      rect = selectionRect(this.doc, passage.range);
    } else {
      return undefined;
    }
    if (!target) return undefined;
    const state: SelectionState = utf8Length(target.quote) > ANNOTATION_TEXT_LIMIT ? "long"
      : hasChars(target.quote, MIN_QUOTE_CHARS) ? "passage" : "short";
    const startLine = clampLine(target.startLine);
    return {
      state,
      target: { view, quote: target.quote, prefix: target.prefix, suffix: target.suffix, startLine, endLine: Math.max(startLine, clampLine(target.endLine)) },
      rect,
    };
  }

  private scheduleToolbar(): void {
    clearTimeout(this.toolbarTimer);
    this.toolbarTimer = setTimeout(() => this.showToolbar(), 0);
  }

  private showToolbar(): void {
    if (this.destroyed || this.form) return;
    const selected = this.readSelection();
    this.selected = selected;
    if (!selected) {
      this.hideToolbar();
      return;
    }
    const box = this.host.main.getBoundingClientRect();
    const rect = selected.rect;
    const place = rect
      ? { left: rect.left - box.left, top: rect.bottom - box.top, above: rect.top - box.top }
      : { left: box.width, top: 0, above: 0 };
    this.toolbar.show(selected.state, place, box.width, box.height);
  }

  private hideToolbar(): void {
    clearTimeout(this.toolbarTimer);
    this.toolbar.hide();
  }

  private annotateSelection(kind: AnnotationKind): void {
    const selected = this.selected ?? this.readSelection();
    if (!selected || selected.state !== "passage") return;
    this.openForm({ kind, target: selected.target, rect: selected.rect });
  }

  private openForm(request: {
    kind: AnnotationKind;
    target?: AnnotationTarget;
    existing?: ViewerAnnotation;
    rect?: Selected["rect"];
    where?: string;
  }): void {
    if (!this.writable()) return;
    this.closeForm();
    this.hideToolbar();
    const { kind, target, existing } = request;
    const where = request.where ?? (target ? linesLabel(target.startLine, target.endLine) : undefined);
    const form = new AnnotationForm(this.doc, {
      kind,
      ...(target ? { quote: target.quote, where } : {}),
      body: existing?.body,
      replacement: existing?.replacement,
      editing: !!existing,
      save: (draft) => this.save(draft, target, existing),
      cancel: () => {
        this.closeForm();
        this.focusDocument();
      },
    });
    this.form = { form, id: existing?.id };
    this.host.main.append(form.element);
    this.placeForm(form.element, request.rect);
    form.focus();
  }

  private placeForm(element: HTMLElement, rect: Selected["rect"]): void {
    const box = this.host.main.getBoundingClientRect();
    const width = element.offsetWidth;
    const height = element.offsetHeight;
    const margin = 8;
    let left = box.width - width - margin;
    let top = margin;
    if (rect) {
      left = rect.left - box.left - width / 2;
      top = rect.bottom - box.top + margin;
      if (top + height + margin > box.height) top = rect.top - box.top - height - margin;
    }
    element.style.left = `${Math.round(Math.max(margin, Math.min(left, box.width - width - margin)))}px`;
    element.style.top = `${Math.round(Math.max(margin, Math.min(top, box.height - height - margin)))}px`;
  }

  private save(draft: FormDraft, target: AnnotationTarget | undefined, existing: ViewerAnnotation | undefined): string | undefined {
    const general = !target;
    const kind = general && draft.kind !== "comment" && draft.kind !== "question" ? "comment" : draft.kind;
    const annotation: ViewerAnnotation = {
      id: existing?.id ?? newAnnotationId(),
      kind,
      ...(draft.body ? { body: draft.body } : {}),
      ...(kind === "change" ? { replacement: draft.replacement } : {}),
      ...(target ? { target } : {}),
    };
    const problem = this.options.store.put(this.options.screenId(), this.options.paneId, [annotation]);
    if (problem) return problem;
    this.active = annotation.id;
    this.closeForm();
    this.refresh();
    this.list.reveal(annotation.id);
    this.focusDocument();
    return undefined;
  }

  private closeForm(): void {
    this.form?.form.element.remove();
    this.form = null;
  }

  private listen(target: EventTarget, type: string, handler: (event: Event) => void, options?: AddEventListenerOptions): void {
    target.addEventListener(type, handler, options);
    this.cleanups.push(() => target.removeEventListener(type, handler, options));
  }
}

