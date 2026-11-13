/**
 * Annotations in a viewer pane (`viewer-annotations`): anchoring, marks, the
 * selection toolbar, the form, the list column and the hand-over of the
 * review, for the document the pane's view shows (`viewer_view.ts`).
 *
 * The pane's server anchors every annotation in the document's source
 * (`server_reviews.ts`): the window asks for the anchors whenever the
 * document's version or the annotations' targets change, and an annotation
 * the answer for the version shown does not place yet is listed at its
 * recorded lines without a mark or a note (SBR-9 of server-built-reviews).
 * An annotation taken from the rendered view anchors in the rendered view's
 * text, which only the window has, while the document renders: the window
 * anchors it there, rendered for its text alone while the source is shown,
 * and hands what it found to the review (SBR-5). The passage is marked in
 * its own view, and its lines in the other one: a bar beside the source
 * lines, a mark on the covering blocks of a rendered Markdown document. A
 * rendered HTML page names no lines, so its passages show only in its
 * rendered view.
 *
 * A passage that changed is anchored where it most likely is now and marked
 * in its own style; resolved annotations are marked only while the user
 * shows them, and the user can hide every mark of the pane with the eye in
 * the pane's bar; then only the passage of the annotation chosen since is
 * marked. Both choices are the window's own and stay with the pane's views
 * (`viewer_view_state.ts`).
 *
 * A mark leads to its thread (D1 of viewer-clickable-marks): a click on a
 * marked passage or on its bar opens the list, chooses the annotation and
 * gives its entry the keyboard, and the pointer over a mark shows which
 * annotation it is. Where marks overlap, the shortest passage comes first and
 * further clicks go through the others (`mark_hits.ts`). A click that ends a
 * selection, a double click and a click on a link do what they did.
 *
 * The server writes the review when the user sends, copies, saves or answers
 * with it. Sending the review marks what it held as handed over and keeps it
 * (D4 of review-threads); a process waiting for the review is answered from
 * the pane's banner (`review_banner.ts`).
 *
 * Annotations are offered while the pane's server keeps, anchors and reviews
 * them and the document is text; an image or a binary file shows the list
 * only while it holds annotations. A server that keeps annotations but writes
 * no reviews needs an update, and the list says so (SBR-8).
 */

import type { ContextMenuItem } from "../../stores/ui.svelte.ts";
import type { AnnotationKind, AnnotationTarget, ReviewOutcome, ReviewWait, SentEntry, ViewerAnnotation } from "../../lib/protocol/types.ts";
import { anchorer, sourceLines, targetAt, type Anchor, type Anchoring, type LineRange, type QuoteTarget } from "../../lib/viewer/anchoring.ts";
import {
  ANNOTATION_LINE_LIMIT, ANNOTATION_TEXT_LIMIT, MIN_QUOTE_CHARS, isBlank, isDeletion, isOpen, newAnnotationId, newReplyId, ownOf,
  registerAnnotationList, reviewSelection, sameContent, type ViewerAnnotations,
} from "../../lib/viewer/annotations.ts";
import { utf8Length } from "../../lib/viewer/limits.ts";
import { pickMark, type MarkHit, type MarkPick } from "../../lib/viewer/mark_hits.ts";
import {
  REVIEW_ANSWER_LIMIT, scratchpadProblem, sendReview, type HandoverNotices, type Review, type ReviewAnswerRequest, type ReviewTarget,
  type SendForm,
} from "../../lib/viewer/handover.ts";
import type { BlockMark, BoundaryRange, RenderedDocument, RenderedMark, RenderedText } from "../../lib/viewer/render.ts";
import { compareListPlaces, reviewFileName, type ListPlace, type ReviewDocument } from "../../lib/viewer/review.ts";
import { utf16Offsets, type Anchored, type PaneReviews, type RenderedPassage } from "../../lib/viewer/server_reviews.ts";
import type { SourceMark, SourceView } from "../../lib/viewer/source_view.ts";
import { AnnotationForm, type FormDraft } from "./annotation_form.ts";
import { AnnotationList, type ListEntry, type ListHeader } from "./annotation_list.ts";
import { ReviewBanner } from "./review_banner.ts";
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
  /** Whether the pane's server keeps annotations, anchors them and writes their review. */
  available(): boolean;
  /** Whether the pane's server keeps annotations but needs an update to anchor them and write their review. */
  outdated?(): boolean;
  /** The anchors and reviews the pane's server works out. */
  readonly reviews: PaneReviews;
  /** The document as the review names it. */
  readonly document: ReviewDocument;
  readonly handover: ReviewHandover;
  readonly notices: LayerNotices;
  showMenu(items: ContextMenuItem[], x: number, y: number): void;
  writeClipboard(text: string): Promise<void>;
  download(name: string, blob: Blob): void;
  /**
   * Answer a process that waits for a review of the pane; resolves with
   * whether the server took the answer (`handover.ts`, `ReviewAnswers`).
   */
  answer?(request: ReviewAnswerRequest): Promise<boolean>;
}

/** The parts of the pane the layer places its own in. */
export interface LayerHost {
  /** The pane, whose banner goes above its body. */
  readonly pane: HTMLElement;
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
  readonly marksHidden: boolean;
  toggleMarks(): void;
  readonly resolvedShown: boolean;
  toggleResolved(): void;
}

/** Where the list column stands by the user's choice; without one it is open while the pane holds annotations. */
export type ListChoice = "open" | "closed";

/** The user's choices of a pane's annotations that its next view starts from. */
export interface LayerState {
  readonly list?: ListChoice;
  /** No marks in the document. */
  readonly hideMarks?: boolean;
  /** The passages of resolved annotations are marked as well. */
  readonly showResolved?: boolean;
}

interface Placed {
  readonly annotation: ViewerAnnotation;
  readonly place: ListPlace;
  /** The text the passage was found in. */
  readonly view?: "source" | "rendered";
  readonly anchor?: Anchor;
  /** Anchored at the passage its quote most likely became. */
  readonly changed?: boolean;
  /** Anchored by the window in its rendered view's text, found or not. */
  readonly byWindow?: boolean;
  /** Not anchored yet in the version shown. */
  readonly pending?: boolean;
}

/** A review ready to go, and what it held of each of the user's pending entries. */
interface Prepared {
  readonly review: Review;
  /** The annotations it holds. */
  readonly count: number;
  /** Per pending entry of the user it holds, the entry as it read then; an entry changed since stays pending. */
  readonly entries: ReadonlyMap<string, SentEntry>;
}

interface Anchorers {
  readonly version: string | undefined;
  readonly shown: AnnotatedContent["shown"];
  readonly sourceText: string | undefined;
  readonly sourceLines?: LineRange;
  rendered?: ((target: QuoteTarget) => Anchoring) | null;
}

/** The server's anchors for a version of the source text, by annotation, with the target each was made for. */
interface Served {
  readonly version: string;
  readonly text: string;
  readonly byId: ReadonlyMap<string, { readonly target: AnnotationTarget; readonly anchor?: Anchor; readonly changed?: boolean }>;
}

interface Selected {
  readonly state: SelectionState;
  readonly target: AnnotationTarget;
  /** The selection's end and start on the screen, where the engine tells. */
  readonly rect?: { readonly left: number; readonly bottom: number; readonly top: number };
}

const STALE: Anchoring = { state: "stale" };

/** Where the pointer is over the document: the element it moved over, at a point of the window. */
interface PointerAt {
  readonly target: EventTarget | null;
  readonly x: number;
  readonly y: number;
}

/** How far the pointer may move between pressing and releasing for a click on a mark, in pixels. */
const CLICK_SLOP = 4;

/** At most this many annotations are named for the marks under the pointer. */
const HOVER_NAMES = 3;

const EYE_PATH = `<path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8s-2.4 4.5-6.5 4.5S1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/>`;
const EYE_SVG = `<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${EYE_PATH}</svg>`;
const EYE_OFF_SVG = `<svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${EYE_PATH}<line x1="2.5" y1="2.5" x2="13.5" y2="13.5"/></svg>`;

/** How the pointer over a mark names its annotation: its number and kind. */
function hoverName(annotation: ViewerAnnotation): string {
  const kind = annotation.kind === "comment" ? "Comment" : isDeletion(annotation) ? "Deletion" : "Change";
  return annotation.number !== undefined ? `#${annotation.number} · ${kind}` : kind;
}

function sameTarget(a: AnnotationTarget, b: AnnotationTarget): boolean {
  return a.view === b.view && a.quote === b.quote && a.prefix === b.prefix && a.suffix === b.suffix
    && a.startLine === b.startLine && a.endLine === b.endLine;
}

/**
 * The server's anchors for `targets` as offsets of the window's source text,
 * which holds the text the server anchored in; a range that falls inside a
 * character, which no server sends, leaves its annotation unanchored.
 */
function servedAnchors(answer: Anchored, text: string, targets: ReadonlyMap<string, AnnotationTarget>): Served {
  const found = answer.anchors.filter((each) => targets.has(each.id) && (each.state === "anchored" || each.state === "changed"));
  const offsets = utf16Offsets(text, found.flatMap((each) => [each.start!, each.end!]));
  const byId = new Map<string, { target: AnnotationTarget; anchor?: Anchor; changed?: boolean }>();
  found.forEach((each, index) => {
    const [start, end] = [offsets[2 * index], offsets[2 * index + 1]];
    if (start === undefined || end === undefined) return;
    byId.set(each.id, { target: targets.get(each.id)!, anchor: { start, end, startLine: each.startLine!, endLine: each.endLine! }, changed: each.state === "changed" });
  });
  for (const each of answer.anchors) {
    if (each.state === "lost" && targets.has(each.id)) byId.set(each.id, { target: targets.get(each.id)! });
  }
  return { version: answer.version, text, byId };
}

function linesLabel(start: number, end: number): string {
  return start === end ? `Line ${start}` : `Lines ${start}-${end}`;
}

/**
 * One entry of the user as it reads now, as `mark_sent` names it: the
 * annotation `id` itself with its kind, remark, replacement and passage, or a
 * reply of the user in its thread with its text.
 */
function entryOf(annotation: ViewerAnnotation, id: string): SentEntry | undefined {
  if (id === annotation.id) {
    const { kind, body, replacement, target } = annotation;
    return {
      id,
      kind,
      ...(body === undefined ? {} : { body }),
      ...(replacement === undefined ? {} : { replacement }),
      ...(target ? { target: { view: target.view, quote: target.quote, prefix: target.prefix, suffix: target.suffix, startLine: target.startLine, endLine: target.endLine } } : {}),
    };
  }
  const reply = annotation.replies?.find((each) => each.id === id && each.author === "user");
  return reply ? { id, body: reply.body } : undefined;
}

/** Whether `entry` reads as `held`, the way the server compares them for `mark_sent`. */
function readsAs(entry: SentEntry | undefined, held: SentEntry): boolean {
  if (!entry || entry.id !== held.id || entry.kind !== held.kind || entry.body !== held.body || entry.replacement !== held.replacement) return false;
  const [a, b] = [entry.target, held.target];
  if (!a || !b) return a === b;
  return sameTarget(a, b);
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

/**
 * Per viewer pane, the text of an overall comment that no answer took yet
 * (D3 of review-verdict), kept for the window's lifetime so that a rebuilt
 * pane and the next wait offer it again.
 */
const overallDrafts = new Map<string, string>();

export class AnnotationLayer {
  private readonly toolbar: SelectionToolbar;
  private readonly list: AnnotationList;
  private readonly listButton: HTMLButtonElement;
  private readonly marksButton: HTMLButtonElement;
  private readonly banner: ReviewBanner;
  private placed: Placed[] = [];
  /** The store's list the annotations were placed from; the same array while it is unchanged. */
  private listed: readonly ViewerAnnotation[] | undefined;
  private anchorers: Anchorers | undefined;
  /** The server's last anchors, and what anchors were asked for last: a version and the annotations' targets. */
  private served: Served | undefined;
  private asked: string | undefined;
  private anchorRequest = 0;
  private active: string | undefined;
  /** While the marks are hidden, the annotation chosen since, whose passage is marked. */
  private revealed: string | undefined;
  /** The annotation whose mark is under the pointer, and the last click on marks. */
  private hovered: string | undefined;
  private lastPick: MarkPick | undefined;
  /** Where the pointer is over the document, and whether a look at it is due. */
  private pointer: PointerAt | undefined;
  private hoverDue = false;
  /** Where the primary button went down in the document. */
  private pressed: { readonly x: number; readonly y: number } | undefined;
  private choice: ListChoice | undefined;
  private marksHidden: boolean;
  private resolvedShown: boolean;
  private form: { readonly form: AnnotationForm; readonly id?: string } | null = null;
  private selected: Selected | undefined;
  /** The wait for a review the pane's record names, and the one this window answered. */
  private wait: ReviewWait | undefined;
  private answered: string | undefined;
  private answering = false;
  /**
   * The version of a file under a wait (D6 of review-verdict): taken from the
   * first read that starts and settles after the wait appeared, so that a
   * read of an older revision does not count as a change.
   */
  private waitVersion: { phase: "start" | "end" | "set"; version?: string } | undefined;
  /** The window saw the file change while the process waits. */
  private waitChanged = false;
  private readonly cleanups: (() => void)[] = [];
  private toolbarTimer: ReturnType<typeof setTimeout> | undefined;
  private destroyed = false;

  constructor(
    private readonly doc: Document,
    private readonly host: LayerHost,
    private readonly content: () => AnnotatedContent,
    private readonly options: AnnotationLayerOptions,
    state: LayerState | undefined,
    private readonly focusDocument: () => void,
  ) {
    this.choice = state?.list;
    this.marksHidden = !!state?.hideMarks;
    this.resolvedShown = !!state?.showResolved;
    this.toolbar = new SelectionToolbar(doc, (kind) => this.annotateSelection(kind));
    host.main.append(this.toolbar.element);
    this.list = new AnnotationList(doc, {
      select: (id) => this.select(id),
      edit: (id) => this.edit(id),
      remove: (id) => this.remove(id),
      setResolved: (id, resolved) => this.setResolved(id, resolved),
      move: (id) => this.move(id),
      reply: (id, body, replyId) => this.reply(id, body, replyId),
      removeReply: (id, replyId) => this.removeReply(id, replyId),
      addComment: () => this.openForm({ kind: "comment" }),
      copy: () => void this.copy(),
      send: (anchor) => this.sendToDefault(anchor),
      more: (anchor) => this.showMenuAt(anchor, this.handoverItems()),
      close: () => this.setChoice("closed"),
      toggleResolved: () => this.toggleResolved(),
    });
    host.body.append(this.list.element);
    this.list.element.addEventListener("pointerenter", () => this.renderList());

    this.listButton = doc.createElement("button");
    this.listButton.type = "button";
    this.listButton.className = "viewer-bar-button viewer-bar-annotations";
    this.listButton.addEventListener("click", () => this.toggleList());
    this.marksButton = doc.createElement("button");
    this.marksButton.type = "button";
    this.marksButton.className = "viewer-bar-button viewer-bar-marks";
    this.marksButton.setAttribute("aria-label", "Hide marks");
    this.marksButton.addEventListener("click", () => this.toggleMarks());
    host.bar.append(this.marksButton, this.listButton);

    this.banner = new ReviewBanner(doc, {
      approve: () => void this.answerWait("approved"),
      requestChanges: () => void this.answerWait("changes_requested"),
      overallChanged: (text) => {
        if (text.trim()) overallDrafts.set(options.paneId, text);
        else overallDrafts.delete(options.paneId);
      },
    }, overallDrafts.get(options.paneId));
    host.pane.insertBefore(this.banner.element, host.body);

    const box = host.documentBox;
    const later = () => this.scheduleToolbar();
    const hide = (event: Event) => {
      if (!this.toolbar.element.contains(event.target as Node)) this.hideToolbar();
    };
    this.listen(box, "pointerup", later);
    this.listen(box, "keyup", later);
    this.listen(box, "pointerdown", hide);
    this.listen(box, "pointerdown", (event) => {
      const pointer = event as PointerEvent;
      this.pressed = pointer.button === 0 ? { x: pointer.clientX, y: pointer.clientY } : undefined;
    });
    this.listen(box, "click", (event) => this.markClicked(event as MouseEvent));
    // Mouse events, which engines also send after the document scrolled under a resting pointer.
    this.listen(box, "mousemove", (event) => this.pointerMoved(event as MouseEvent), { passive: true });
    this.listen(box, "mouseleave", () => {
      this.pointer = undefined;
      this.showHover();
    });
    this.listen(box, "scroll", () => this.hideToolbar(), { capture: true, passive: true });
    this.listen(doc, "selectionchange", () => this.selectionChanged());

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

  /** The user's choices for the pane's next view: the list column, the marks, the resolved ones. */
  viewState(): LayerState {
    return {
      ...(this.choice ? { list: this.choice } : {}),
      ...(this.marksHidden ? { hideMarks: true } : {}),
      ...(this.resolvedShown ? { showResolved: true } : {}),
    };
  }

  /** The document, its version or its view changed: anchor and mark again. */
  contentChanged(): void {
    this.hideToolbar();
    this.refresh();
  }

  /** The selection changed; a toolbar shown for an earlier one goes once it is empty. */
  selectionChanged(): void {
    if (this.destroyed) return;
    const selected = this.toolbar.shown || this.list.shown ? this.readSelection() : undefined;
    if (this.toolbar.shown && !selected) this.hideToolbar();
    if (this.list.shown) this.list.setMovable(selected?.state === "passage");
  }

  /** The pane's record names a wait for a review, or none. */
  setWait(wait: ReviewWait | undefined): void {
    if (this.wait?.id !== wait?.id) {
      this.answered = undefined;
      this.waitVersion = wait && this.options.document.kind === "file" ? { phase: "start" } : undefined;
      this.waitChanged = false;
    }
    this.wait = wait;
    this.renderBanner();
  }

  /**
   * A read of the document started, or settled with `version`: under a wait,
   * the first read after the wait gives the version the review is about,
   * and a later one with another version is a change (D6 of review-verdict).
   */
  documentRead(reading: boolean, version: string | undefined): void {
    const watched = this.waitVersion;
    if (!watched) return;
    if (reading) {
      if (watched.phase === "start") watched.phase = "end";
      return;
    }
    if (version === undefined) return;
    if (watched.phase === "end") {
      watched.phase = "set";
      watched.version = version;
    } else if (watched.phase === "set" && version !== watched.version && !this.waitChanged) {
      this.waitChanged = true;
      this.renderBanner();
    }
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
      marksHidden: this.marksHidden,
      toggleMarks: () => this.toggleMarks(),
      resolvedShown: this.resolvedShown,
      toggleResolved: () => this.toggleResolved(),
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
    this.marksButton.remove();
    this.banner.element.remove();
  }

  // ── Anchoring and Marks ──

  private refresh(): void {
    if (this.destroyed) return;
    this.anchorAll();
    this.applyMarks();
    this.renderList();
    this.renderBanner();
    this.askAnchors();
  }

  /**
   * Ask the server for the anchors of the version shown when it or the
   * annotations' targets changed since the last request; an edit of a remark
   * or a reply asks for nothing. An answer about another version than the
   * one shown waits for the window's own read of that version.
   */
  private askAnchors(): void {
    if (!this.options.available()) return;
    const { version, sourceText } = this.content();
    if (version === undefined || sourceText === undefined) return;
    const targets = new Map<string, AnnotationTarget>();
    for (const annotation of this.listed ?? []) if (annotation.target) targets.set(annotation.id, annotation.target);
    if (targets.size === 0) return;
    const key = JSON.stringify([version, [...targets]]);
    if (key === this.asked) return;
    this.asked = key;
    const request = ++this.anchorRequest;
    // The store tells of a change before it sends it: the request follows it on the connection.
    queueMicrotask(() => {
      if (this.destroyed || request !== this.anchorRequest) return;
      this.options.reviews.anchor(this.options.paneId).then((answer) => {
        if (this.destroyed || request !== this.anchorRequest) return;
        const now = this.content();
        if (answer.version !== now.version || now.sourceText === undefined) return;
        this.served = servedAnchors(answer, now.sourceText, targets);
        this.refresh();
      }, () => {
        // The annotations stay where they were recorded until the next change asks again.
      });
    });
  }

  private anchorersFor(content: AnnotatedContent): Anchorers {
    const known = this.anchorers;
    if (known && known.version === content.version && known.shown === content.shown && known.sourceText === content.sourceText) return known;
    const text = content.sourceText;
    this.anchorers = {
      version: content.version,
      shown: content.shown,
      sourceText: text,
      ...(text !== undefined ? { sourceLines: sourceLines(text) } : {}),
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
    const available = this.options.available();
    const served = this.served && this.served.version === content.version && this.served.text === content.sourceText ? this.served : undefined;
    const placed = this.listed.map((annotation, index): Placed => {
      const target = annotation.target;
      if (!target) return { annotation, place: { state: "general", index } };
      const recorded = { state: "stale" as const, startLine: target.startLine, endLine: target.endLine, index };
      let view: "source" | "rendered" | undefined;
      let anchoring = STALE;
      let byWindow = false;
      const rendered = available && target.view === "rendered" ? this.renderedAnchorer(content, anchorers) : null;
      if (rendered) {
        view = "rendered";
        byWindow = true;
        anchoring = rendered(target);
      } else if (content.sourceText !== undefined) {
        const found = served?.byId.get(annotation.id);
        if (!found || !sameTarget(found.target, target)) return { annotation, place: recorded, pending: true };
        view = "source";
        if (found.anchor) anchoring = found.changed ? { state: "changed", anchor: found.anchor, score: 0 } : { state: "anchored", anchor: found.anchor };
      }
      if (anchoring.state !== "stale") {
        const anchor = anchoring.anchor;
        return {
          annotation, view, anchor, changed: anchoring.state === "changed", byWindow,
          place: { state: "anchored", view, offset: anchor.start, startLine: anchor.startLine, endLine: anchor.endLine, index },
        };
      }
      return { annotation, place: recorded, byWindow };
    });
    placed.sort((a, b) => compareListPlaces(a.place, b.place));
    this.placed = placed;
  }

  /**
   * The passages to mark: a resolved one only while resolved ones are shown,
   * and while the marks are hidden, only the one chosen since.
   */
  private marked(): Placed[] {
    return this.placed.filter((placed) => placed.anchor && (this.resolvedShown || isOpen(placed.annotation))
      && (!this.marksHidden || (placed.annotation.id === this.revealed && placed.annotation.id === this.active)));
  }

  private applyMarks(): void {
    const content = this.content();
    const lineMarks = content.format === "markdown";
    const style = (placed: Placed) => (!isOpen(placed.annotation) ? "resolved" as const : placed.changed ? "changed" as const : undefined);
    if (content.shown === "source" && content.source) {
      const marks: SourceMark[] = [];
      for (const placed of this.marked()) {
        const { view, anchor } = placed;
        const extra = this.markOf(placed, style(placed));
        if (view === "source") marks.push({ from: anchor!.start, to: anchor!.end, startLine: anchor!.startLine, endLine: anchor!.endLine, ...extra });
        else if (lineMarks) marks.push({ startLine: anchor!.startLine, endLine: anchor!.endLine, ...extra });
      }
      content.source.setMarks(marks);
    } else if (content.shown === "rendered" && content.rendered) {
      const marks: RenderedMark[] = [];
      const blocks: BlockMark[] = [];
      for (const placed of this.marked()) {
        const { view, anchor } = placed;
        const extra = this.markOf(placed, style(placed));
        if (view === "rendered") marks.push({ start: anchor!.start, end: anchor!.end, ...extra });
        else if (lineMarks) blocks.push({ startLine: anchor!.startLine, endLine: anchor!.endLine, ...extra });
      }
      content.rendered.setMarks(marks, blocks);
    }
  }

  /** What every mark of an annotation carries: its ID, and whether it is the chosen one or under the pointer. */
  private markOf(placed: Placed, style: "changed" | "resolved" | undefined) {
    const { id } = placed.annotation;
    return { id, active: id === this.active, ...(id === this.hovered ? { hover: true } : {}), ...(style ? { style } : {}) };
  }

  private toggleMarks(): void {
    this.marksHidden = !this.marksHidden;
    this.revealed = undefined;
    this.lastPick = undefined;
    this.applyMarks();
    this.renderList();
  }

  // ── From A Mark To Its Thread ──

  /** The marks of the shown view at `target`, the element at the point `x`, `y`. */
  private hitsAt(target: EventTarget | null, x: number, y: number): MarkHit[] {
    const content = this.content();
    const ids = content.shown === "source" && content.source ? content.source.marksAt(target, y)
      : content.shown === "rendered" && content.rendered ? content.rendered.marksAt(target, x, y)
      : [];
    const hits: MarkHit[] = [];
    for (const id of ids) {
      const placed = this.placed.find((each) => each.annotation.id === id);
      if (!placed?.anchor) continue;
      const { anchor } = placed;
      const own = placed.view === content.shown;
      hits.push(own
        ? { id, lines: false, length: anchor.end - anchor.start, start: anchor.start }
        : { id, lines: true, length: anchor.endLine - anchor.startLine + 1, start: anchor.startLine });
    }
    return hits;
  }

  /** Whether a passage of the shown view is selected. */
  private documentSelected(): boolean {
    const content = this.content();
    if (content.shown === "source") return !!content.source?.selection();
    if (content.shown === "rendered") return !!content.rendered?.selectedPassage();
    return false;
  }

  /** A click in the document: on a mark, its thread; anything else is the document's. */
  private markClicked(event: MouseEvent): void {
    const pressed = this.pressed;
    this.pressed = undefined;
    if (event.button !== 0 || event.detail > 1 || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) return;
    if (pressed && Math.abs(event.clientX - pressed.x) + Math.abs(event.clientY - pressed.y) > CLICK_SLOP) return;
    if (this.documentSelected()) return;
    const pick = pickMark(this.hitsAt(event.composedPath()[0] ?? event.target, event.clientX, event.clientY), this.lastPick);
    if (!pick) return;
    this.lastPick = pick;
    this.openThread(pick.chosen);
    this.showHover();
  }

  /** Open the list on an annotation's thread, chosen and with the keyboard; its passage stays where it is. */
  private openThread(id: string): void {
    if (!this.listShown()) this.setChoice("open");
    this.select(id, false);
    this.list.focusEntry(id);
  }

  private pointerMoved(event: MouseEvent): void {
    // While a button is down the pointer selects; it points at nothing.
    this.pointer = event.buttons === 0 ? { target: event.composedPath()[0] ?? event.target, x: event.clientX, y: event.clientY } : undefined;
    if (this.hoverDue) return;
    this.hoverDue = true;
    const view = this.doc.defaultView;
    const look = () => {
      this.hoverDue = false;
      this.showHover();
    };
    if (view?.requestAnimationFrame) view.requestAnimationFrame(look);
    else setTimeout(look, 16);
  }

  /**
   * The element under the pointer: the one it last moved over, or, where a
   * change of the marks replaced that one, the one at its place now.
   */
  private pointerTarget(at: PointerAt): EventTarget | null {
    const node = at.target as Node | null;
    if (!node || node.isConnected) return node;
    let element = typeof this.doc.elementFromPoint === "function" ? this.doc.elementFromPoint(at.x, at.y) : null;
    for (let inner = element?.shadowRoot?.elementFromPoint(at.x, at.y); inner && inner !== element; inner = inner.shadowRoot?.elementFromPoint(at.x, at.y)) {
      element = inner;
    }
    return element;
  }

  /**
   * Show which marks are under the pointer: the one a click chooses in its
   * stronger style, and the numbers and kinds of all of them as the
   * document's tooltip.
   */
  private showHover(): void {
    if (this.destroyed) return;
    const at = this.pointer;
    const pick = at ? pickMark(this.hitsAt(this.pointerTarget(at), at.x, at.y), this.lastPick) : undefined;
    const box = this.host.documentBox;
    box.classList.toggle("viewer-mark-hover", !!pick);
    if (pick) {
      const names = pick.ids.slice(0, HOVER_NAMES).flatMap((id) => {
        const annotation = this.placed.find((each) => each.annotation.id === id)?.annotation;
        return annotation ? [hoverName(annotation)] : [];
      });
      box.title = pick.ids.length > HOVER_NAMES ? `${names.join(", ")}, …` : names.join(", ");
    } else {
      box.removeAttribute("title");
    }
    const hovered = pick?.chosen;
    if (hovered === this.hovered) return;
    this.hovered = hovered;
    this.applyMarks();
  }

  private toggleResolved(): void {
    this.resolvedShown = !this.resolvedShown;
    this.applyMarks();
    this.renderList();
  }

  // ── The List ──

  /** The open annotations as they were placed last. */
  private openCount(): number {
    let open = 0;
    for (const placed of this.placed) if (isOpen(placed.annotation)) open++;
    return open;
  }

  private listShown(): boolean {
    const count = this.options.store.count(this.options.paneId);
    if (!this.options.available() && !this.outdated() && count === 0) return false;
    return this.choice === "open" || (this.choice !== "closed" && count > 0);
  }

  /** The pane's server keeps annotations but needs an update for them. */
  private outdated(): boolean {
    return !this.options.available() && !!this.options.outdated?.();
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

  private entryOf(placed: Placed): ListEntry {
    const { annotation, anchor, changed } = placed;
    return {
      id: annotation.id,
      ...(annotation.number !== undefined ? { number: annotation.number } : {}),
      kind: annotation.kind,
      deletion: isDeletion(annotation),
      where: this.where(placed),
      state: !annotation.target ? "general" : placed.pending ? "pending" : !anchor ? "stale" : changed ? "changed" : "anchored",
      resolved: !isOpen(annotation),
      sent: !!annotation.sent,
      quote: annotation.target?.quote,
      body: annotation.body,
      replacement: annotation.replacement,
      replies: (annotation.replies ?? []).map((reply) => ({
        id: reply.id,
        author: reply.author,
        ...(reply.from?.title ? { title: reply.from.title } : {}),
        body: reply.body,
        sent: !!reply.sent,
      })),
    };
  }

  /**
   * The hand-over action: to the origin, named in its title, or to a
   * terminal the user chooses; again when nothing is pending.
   */
  private sendLabel(again: boolean): { send: string; sendTitle: string } {
    const origin = this.options.handover.origin();
    const verb = again ? "Send Again" : "Send";
    const what = again ? "the review of all open annotations" : "the review of the annotations not sent yet";
    return {
      send: origin ? verb : `${verb}…`,
      sendTitle: origin
        ? `Paste ${what} into ${origin.title}, without submitting it`
        : `Paste ${what} into a terminal of this screen, without submitting it`,
    };
  }

  private renderList(): void {
    if (this.destroyed) return;
    const total = this.placed.length;
    const open = this.openCount();
    const available = this.options.available();
    const shown = this.listShown();
    this.list.setShown(shown);
    const outdated = this.outdated();
    this.listButton.hidden = !available && !outdated && total === 0;
    this.listButton.textContent = open === 0 ? "Annotations" : open === 1 ? "1 Annotation" : `${open} Annotations`;
    this.listButton.title = shown ? "Hide the annotations" : "Show the annotations";
    this.listButton.setAttribute("aria-pressed", String(shown));
    this.renderMarksButton();
    if (!shown) return;
    const { again } = reviewSelection(this.placed.filter((placed) => isOpen(placed.annotation)).map((placed) => placed.annotation));
    const writable = this.writable();
    const header: ListHeader = {
      open,
      resolved: total - open,
      ...this.sendLabel(again && open > 0),
      sendable: available && open > 0,
      writable,
      movable: writable && this.readSelection()?.state === "passage",
      resolvedShown: this.resolvedShown,
      ...(outdated ? { notice: "The Splitty of this pane's server does not anchor annotations or write their review. Update Splitty there to annotate this document." } : {}),
    };
    this.list.render(header, this.placed.map((placed) => this.entryOf(placed)), this.active);
  }

  /** The eye in the pane's bar, beside the list's button: whether the document shows its marks. */
  private renderMarksButton(): void {
    const button = this.marksButton;
    button.hidden = this.listButton.hidden;
    button.setAttribute("aria-pressed", String(this.marksHidden));
    button.title = this.marksHidden ? "Show the marks in the document" : "Hide the marks in the document";
    const icon = this.marksHidden ? "hidden" : "shown";
    if (button.dataset.icon === icon) return;
    button.dataset.icon = icon;
    // A constant of this module, never text of a document.
    button.innerHTML = this.marksHidden ? EYE_OFF_SVG : EYE_SVG;
  }

  /** Choose an annotation: mark its passage as the chosen one and, unless the user pointed at it, bring it into view. */
  private select(id: string, scroll = true): void {
    const placed = this.placed.find((each) => each.annotation.id === id);
    if (!placed) return;
    this.active = id;
    this.revealed = id;
    this.applyMarks();
    this.list.reveal(id);
    const content = this.content();
    const { view, anchor } = placed;
    if (!anchor || !scroll) return;
    if (content.shown === "source" && content.source) {
      if (view === "source") content.source.scrollToPassage(anchor.start, anchor.end);
      else if (content.source) content.source.scrollToLine(anchor.startLine);
    } else if (content.shown === "rendered" && content.rendered) {
      if (view === "rendered") content.rendered.scrollToPassage(anchor.start, anchor.end);
      else content.rendered.scrollToLines(anchor.startLine, anchor.endLine);
    }
  }

  private edit(id: string): void {
    const annotation = this.options.store.get(this.options.paneId, id);
    if (!annotation) return;
    const placed = this.placed.find((each) => each.annotation.id === id);
    this.openForm({ kind: annotation.kind, target: annotation.target, existing: annotation, where: placed ? this.where(placed) : undefined });
  }

  private remove(id: string): void {
    const annotation = this.options.store.get(this.options.paneId, id);
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

  /**
   * Put annotations back as they were, by the server's rules: they were kept
   * before. The server numbers them anew; the user's replies come back with
   * them, an agent's cannot.
   */
  private putBack(annotations: readonly ViewerAnnotation[]): void {
    const { store, paneId } = this.options;
    const screenId = this.options.screenId();
    const problem = store.put(screenId, paneId, annotations, { restore: true });
    if (problem) {
      this.options.notices.error("The annotations could not be put back", problem);
      return;
    }
    for (const annotation of annotations) {
      const replies = (annotation.replies ?? []).filter((reply) => reply.author === "user");
      const failed = replies.length > 0 ? store.putReplies(screenId, paneId, annotation.id, replies, { restore: true }) : undefined;
      if (failed) this.options.notices.error("The replies could not be put back", failed);
    }
  }

  /** Resolve an open annotation or reopen a resolved one; its number, thread and hand-over mark stay. */
  private setResolved(id: string, resolved: boolean): void {
    const { store, paneId } = this.options;
    const annotation = store.get(paneId, id);
    if (!annotation) return;
    const { status: _status, ...rest } = annotation;
    const problem = store.put(this.options.screenId(), paneId, [resolved ? { ...rest, status: "resolved" } : rest]);
    if (problem) {
      this.options.notices.error(resolved ? "The annotation could not be resolved" : "The annotation could not be reopened", problem);
      return;
    }
    this.refresh();
    this.list.focusEntry(id);
  }

  /**
   * Give an annotation the passage selected in the pane as its target (D7 of
   * review-threads). Its passage changed, so it is pending again.
   */
  private move(id: string): void {
    const { store, paneId } = this.options;
    const annotation = store.get(paneId, id);
    if (!annotation) return;
    const selected = this.readSelection();
    if (!selected || selected.state !== "passage") {
      this.options.notices.info("Select a passage in the document to move the annotation to it");
      return;
    }
    const { sent: _sent, ...rest } = annotation;
    const problem = store.put(this.options.screenId(), paneId, [{ ...rest, target: selected.target }]);
    if (problem) {
      this.options.notices.error("The annotation could not be moved", problem);
      return;
    }
    this.active = id;
    this.refresh();
    this.select(id);
  }

  /** Save a reply of the user; an edit that changes its text is pending again. */
  private reply(id: string, body: string, replyId?: string): string | undefined {
    const { store, paneId } = this.options;
    const annotation = store.get(paneId, id);
    if (!annotation) return "The annotation is no longer there.";
    const existing = replyId ? annotation.replies?.find((reply) => reply.id === replyId && reply.author === "user") : undefined;
    const sent = existing?.sent && existing.body === body;
    return store.putReplies(this.options.screenId(), paneId, id, [{ id: existing?.id ?? newReplyId(), body, ...(sent ? { sent: true } : {}) }]);
  }

  /** Remove a reply; the user's own can be put back for a while, an agent's cannot. */
  private removeReply(id: string, replyId: string): void {
    const { store, paneId } = this.options;
    const reply = store.get(paneId, id)?.replies?.find((each) => each.id === replyId);
    if (!reply) return;
    const screenId = this.options.screenId();
    store.removeReplies(screenId, paneId, id, [replyId]);
    if (reply.author !== "user") {
      this.options.notices.info("The agent's reply was removed");
      return;
    }
    this.options.notices.undoable("Reply removed", () => {
      const problem = store.putReplies(this.options.screenId(), paneId, id, [reply], { restore: true });
      if (problem) this.options.notices.error("The reply could not be put back", problem);
    });
  }

  // ── The Review ──

  /**
   * What the window found in its rendered view for the annotations taken
   * from it, which the server cannot anchor (SBR-5): their state, current
   * lines and offset, and for a page without source lines no lines in the
   * review.
   */
  private renderedPassages(): RenderedPassage[] {
    const unlined = this.content().format === "html";
    return this.placed.flatMap((placed): RenderedPassage[] => {
      if (!placed.byWindow) return [];
      const { anchor, changed, annotation } = placed;
      const passage: RenderedPassage = anchor
        ? { id: annotation.id, state: changed ? "changed" : "anchored", startLine: clampLine(anchor.startLine), endLine: Math.max(clampLine(anchor.startLine), clampLine(anchor.endLine)), offset: anchor.start }
        : { id: annotation.id, state: "lost" };
      return [unlined ? { ...passage, unlined: true } : passage];
    });
  }

  /**
   * The review the server writes of the pane's annotations (D4 of
   * review-threads; SBR-2 of server-built-reviews): the open ones with an
   * entry of the user not handed over, or every open one when none is; an
   * answer to a waiting process states its verdict, and an approval holds
   * only the pending ones. The server reads the annotations after this
   * window's earlier changes, so a comment saved just before is in it.
   */
  private async prepareReview(verdict?: ReviewOutcome): Promise<Prepared> {
    const { document, paneId } = this.options;
    this.refresh();
    const reviewed = await this.options.reviews.review({
      paneId,
      ...(verdict ? { verdict, ...(this.waitChanged ? { changedDuringWait: true } : {}) } : {}),
      passages: this.renderedPassages(),
    });
    return {
      review: { text: reviewed.text, fileName: reviewFileName(document) },
      count: reviewed.count,
      entries: new Map(reviewed.entries.map((entry) => [entry.id, entry])),
    };
  }

  /**
   * Mark what the review held as handed over: the entries of the user that
   * read as they did when it was made, each named with that content, so that
   * the server, too, leaves an entry another window changed meanwhile pending.
   */
  private markHandedOver(prepared: Prepared): void {
    const { store, paneId } = this.options;
    const list = store.list(paneId);
    const held: SentEntry[] = [];
    for (const [id, entry] of prepared.entries) {
      const owner = list.find((annotation) => annotation.id === id || annotation.replies?.some((reply) => reply.id === id));
      if (owner && readsAs(entryOf(owner, id), entry)) held.push(entry);
    }
    store.markSent(this.options.screenId(), paneId, held);
  }

  private targets(): ReviewTarget[] {
    const origin = this.options.handover.origin();
    const terminals = this.options.handover.terminals();
    return origin ? [origin, ...terminals.filter((target) => target.paneId !== origin.paneId)] : terminals;
  }

  private showMenuAt(anchor: HTMLElement, items: ContextMenuItem[]): void {
    const rect = anchor.getBoundingClientRect();
    this.options.showMenu(items, rect.left, rect.bottom);
  }

  private targetItems(form: SendForm): ContextMenuItem[] {
    const targets = this.targets();
    if (targets.length === 0) return [{ label: "No terminal pane on this screen", action: () => {}, disabled: true }];
    return targets.map((target) => {
      // Whether the review fits the scratchpad is known once the server wrote it.
      const problem = form === "scratchpad" && !target.scratchpad ? scratchpadProblem({ text: "", fileName: "" }, target) : undefined;
      return {
        label: target.title,
        action: () => void this.send(target, form),
        ...(problem ? { disabled: true, detail: problem } : {}),
      };
    });
  }

  private handoverItems(): ContextMenuItem[] {
    return [
      { label: "Send To", action: () => {}, children: this.targetItems("paste") },
      { label: "Send as File To", action: () => {}, children: this.targetItems("file") },
      { label: "Append to Scratchpad Of", action: () => {}, children: this.targetItems("scratchpad") },
      { label: "", action: () => {}, separator: true },
      { label: "Copy as Markdown", action: () => void this.copy() },
      { label: "Save as File", action: () => void this.download() },
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

  /** The server's review, or undefined after telling the user why there is none. */
  private async writtenReview(): Promise<Prepared | undefined> {
    try {
      return await this.prepareReview();
    } catch (error) {
      if (!this.destroyed) this.options.notices.error("The review could not be written", error instanceof Error ? error.message : undefined);
      return undefined;
    }
  }

  private async send(target: ReviewTarget, form: SendForm): Promise<void> {
    const prepared = await this.writtenReview();
    if (!prepared || prepared.count === 0 || this.destroyed) return;
    if (await sendReview(prepared.review, target, form, this.options.notices)) this.markHandedOver(prepared);
  }

  private async copy(): Promise<void> {
    const prepared = await this.writtenReview();
    if (!prepared) return;
    try {
      await this.options.writeClipboard(prepared.review.text);
      this.options.notices.info("The review is in the clipboard");
    } catch (error) {
      this.options.notices.error("The review could not be copied", error instanceof Error ? error.message : undefined);
    }
  }

  private async download(): Promise<void> {
    const prepared = await this.writtenReview();
    if (!prepared) return;
    this.options.download(prepared.review.fileName, new Blob([prepared.review.text], { type: "text/markdown" }));
  }

  // ── Waiting For A Review ──

  /** The wait the banner offers to answer: while the server keeps annotations, and until this window answered it. */
  private openWait(): ReviewWait | undefined {
    const wait = this.wait;
    if (!wait || !this.options.answer || !this.options.available() || wait.id === this.answered) return undefined;
    return wait;
  }

  private renderBanner(): void {
    if (this.destroyed) return;
    this.banner.show({ wait: this.openWait(), answering: this.answering, changed: this.waitChanged });
  }

  /**
   * Answer the waiting process with a verdict (D1 and D3 of review-verdict):
   * the overall comment first becomes a general comment; an approval carries
   * the pending annotations as notes, or no review without any; a request for
   * changes carries the review, which may hold no annotation. The entries of
   * a review are marked once the server took the answer. Only the first
   * answer to a wait counts; once the server acknowledged one from this
   * window, taken or not, the banner goes.
   */
  private async answerWait(outcome: ReviewOutcome): Promise<void> {
    const wait = this.openWait();
    const answer = this.options.answer;
    if (!wait || !answer || this.answering) return;
    const { notices, paneId, store } = this.options;
    const waiting = wait.title || "the waiting command";
    this.answering = true;
    this.renderBanner();
    try {
      const overall = this.banner.overall;
      if (!isBlank(overall)) {
        const problem = store.put(this.options.screenId(), paneId, [{ id: newAnnotationId(), kind: "comment", body: overall }]);
        if (problem) {
          notices.error("The overall comment was not saved", problem);
          return;
        }
        this.banner.clearOverall();
        overallDrafts.delete(paneId);
      }
      const prepared = await this.prepareReview(outcome);
      const review = outcome === "changes_requested" || prepared.count > 0 ? prepared.review.text : undefined;
      if (review !== undefined && utf8Length(review) > REVIEW_ANSWER_LIMIT) {
        notices.error("The review was not sent", "It is longer than the 1 MiB a waiting command takes.");
        return;
      }
      const accepted = await answer({
        screenId: this.options.screenId(),
        paneId,
        waitId: wait.id,
        outcome,
        ...(review !== undefined ? { review } : {}),
      });
      this.answered = wait.id;
      if (!accepted) {
        notices.info(`${outcome === "approved" ? "The approval" : "The request for changes"} was not taken: the command no longer waits for it`);
        return;
      }
      if (review !== undefined && prepared.entries.size > 0) this.markHandedOver(prepared);
      if (outcome === "changes_requested") notices.sent(`Changes requested from ${waiting}`);
      else if (review !== undefined) notices.sent(`Approved for ${waiting}, with the new comments as notes`);
      else notices.info(`Approved for ${waiting}`);
    } catch (error) {
      notices.error(`The answer did not reach ${waiting}`, error instanceof Error ? error.message : undefined);
    } finally {
      this.answering = false;
      this.renderBanner();
    }
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
    if (this.list.shown) this.list.setMovable(selected?.state === "passage");
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
      ...(existing?.kind === "change" ? { replacement: existing.replacement ?? "" } : {}),
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

  /**
   * Save the form: a new annotation, or the annotation edited, which keeps
   * its status; an edit that changes it is pending again, one that changes
   * nothing keeps its hand-over mark.
   */
  private save(draft: FormDraft, target: AnnotationTarget | undefined, existing: ViewerAnnotation | undefined): string | undefined {
    // A general comment is a comment.
    const kind = target ? draft.kind : "comment";
    const edited: ViewerAnnotation = {
      id: existing?.id ?? newAnnotationId(),
      kind,
      ...(isBlank(draft.body) ? {} : { body: draft.body }),
      ...(kind === "change" ? { replacement: draft.replacement } : {}),
      ...(target ? { target } : {}),
      ...(existing && !isOpen(existing) ? { status: "resolved" as const } : {}),
    };
    const unchanged = !!existing?.sent && sameContent(ownOf(existing), edited);
    const annotation: ViewerAnnotation = unchanged ? { ...edited, sent: true } : edited;
    const problem = this.options.store.put(this.options.screenId(), this.options.paneId, [annotation]);
    if (problem) return problem;
    this.active = annotation.id;
    this.revealed = annotation.id;
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
