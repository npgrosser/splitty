/**
 * The annotations of one server's viewer panes (`viewer-annotations`): what
 * a window shows for each pane, from that server's snapshots and from the
 * window's own operations, and the rules the server applies to them.
 *
 * The window adds, changes and removes annotations with `put_annotations` and
 * `remove_annotations`, shows each change at once and sends it without
 * waiting. A snapshot replaces a pane's list, except for the annotations this
 * window put or removed during the last two seconds: their state stays until
 * a snapshot shows it or the time has passed (Concurrent Annotation Edits).
 * The server sends the window nothing for its own operation, so an older
 * snapshot that still contradicts such a state asks for the screen once more
 * when the time has passed; the window then shows what the server holds.
 * A rejected operation drops the window's own state of the pane; the server's
 * annotations are shown again with the next snapshot.
 *
 * What the window sends passes the server's rules first, and where a rule
 * leaves room the window is stricter, never looser: a remark of nothing but
 * U+FEFF is blank here, not on the server. What arrives is checked by the
 * server's rules exactly, so that nothing the server keeps is dropped, and
 * malformed entries are.
 */

import type { AnnotationKind, AnnotationTarget, MuxOperation, ScreenState, ViewerAnnotation } from "../protocol/types.ts";
import { SETTLE_MS } from "../scratchpad.ts";
import { utf8Length, wellFormed } from "./limits.ts";

/** The capability of a server that keeps annotations (`viewer-annotations`, Annotations Capability). */
export const ANNOTATIONS_CAPABILITY = "viewer-annotations-v1";
/** The most annotations a pane holds, and the most one operation carries. */
export const ANNOTATION_COUNT_LIMIT = 100;
/** The longest remark, replacement and quote, in bytes of UTF-8. */
export const ANNOTATION_TEXT_LIMIT = 16_384;
/** The longest context on either side of a quote, in bytes of UTF-8. */
export const ANNOTATION_CONTEXT_LIMIT = 256;
/** The most text of all annotations of a pane, in bytes of UTF-8. */
export const ANNOTATION_PANE_TEXT_LIMIT = 262_144;
/** The highest line an annotation names. */
export const ANNOTATION_LINE_LIMIT = 10_000_000;
/** A shorter quote occurs too often to find its passage again; the window asks for a longer selection. */
export const MIN_QUOTE_CHARS = 3;
/** How long the window's own change outlasts snapshots the server made before it. */
export const ANNOTATION_SETTLE_MS = SETTLE_MS;

const KINDS: ReadonlySet<string> = new Set<AnnotationKind>(["comment", "question", "change", "delete"]);
const ID = /^ann-[0-9a-f]{16}$/;

type Raw = Record<string, unknown>;

function isRecord(value: unknown): value is Raw {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Whether `id` has the form of an annotation ID: `ann-` and 16 lowercase hex characters. */
export function isAnnotationId(id: unknown): id is string {
  return typeof id === "string" && ID.test(id);
}

/** A new annotation ID; 64 random bits make a clash within a pane negligible. */
export function newAnnotationId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return `ann-${[...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

/** Unicode White_Space: what Rust's `str::trim` takes away, and so the server's notion of blank. */
function isServerSpace(code: number): boolean {
  if (code <= 0x20) return code === 0x20 || (code >= 9 && code <= 13);
  return code === 0x85 || code === 0xa0 || code === 0x1680 || (code >= 0x2000 && code <= 0x200a)
    || code === 0x2028 || code === 0x2029 || code === 0x202f || code === 0x205f || code === 0x3000;
}

/**
 * Whether `text` holds nothing but whitespace: the server's White_Space, and
 * for what the window sends (`strict`) also U+FEFF, which JavaScript's `trim`
 * takes away as well.
 */
export function isBlank(text: string, strict = true): boolean {
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (!isServerSpace(code) && !(strict && code === 0xfeff)) return false;
  }
  return true;
}

/** The text of an annotation that counts toward the pane's limit. */
export function annotationBytes(annotation: ViewerAnnotation): number {
  const target = annotation.target;
  return utf8Length(annotation.body ?? "") + utf8Length(annotation.replacement ?? "")
    + (target ? utf8Length(target.quote) + utf8Length(target.prefix) + utf8Length(target.suffix) : 0);
}

const KIND_NAMES: Record<AnnotationKind, string> = {
  comment: "A comment",
  question: "A question",
  change: "A suggested change",
  delete: "A suggested deletion",
};

/**
 * Why the server would refuse `annotation` on its own, as the window says it;
 * undefined when it would not. `strict` applies the window's notion of blank.
 */
export function annotationProblem(annotation: ViewerAnnotation, strict = true): string | undefined {
  if (!isAnnotationId(annotation.id)) return "The annotation has no valid ID.";
  if (!KINDS.has(annotation.kind)) return "The annotation has an unknown kind.";
  const { body, replacement, target } = annotation;
  if (body !== undefined && utf8Length(body) > ANNOTATION_TEXT_LIMIT) return "The remark is too long: it may hold at most 16 KB.";
  if (replacement !== undefined && utf8Length(replacement) > ANNOTATION_TEXT_LIMIT) return "The replacement is too long: it may hold at most 16 KB.";
  if (target) {
    if (utf8Length(target.quote) > ANNOTATION_TEXT_LIMIT) return "The passage is too long to annotate: it may hold at most 16 KB.";
    if (utf8Length(target.prefix) > ANNOTATION_CONTEXT_LIMIT || utf8Length(target.suffix) > ANNOTATION_CONTEXT_LIMIT) {
      return "The context of the passage is too long.";
    }
  }
  const kind = annotation.kind;
  if ((kind === "comment" || kind === "question") && (body === undefined || isBlank(body, strict))) {
    return `${KIND_NAMES[kind]} needs a remark.`;
  }
  if (kind === "change" && !replacement) return "A suggested change needs a replacement.";
  if ((kind === "change" || kind === "delete") && !target) return `${KIND_NAMES[kind]} needs a passage.`;
  if (target) {
    if (!target.quote) return "The passage is empty.";
    const { startLine, endLine } = target;
    if (!Number.isSafeInteger(startLine) || !Number.isSafeInteger(endLine)
      || startLine < 1 || startLine > endLine || endLine > ANNOTATION_LINE_LIMIT) {
      return "The lines of the passage are invalid.";
    }
  }
  return undefined;
}

/** Why a pane may not hold `annotations`; undefined when it may. */
export function paneProblem(annotations: readonly ViewerAnnotation[]): string | undefined {
  if (annotations.length > ANNOTATION_COUNT_LIMIT) return `A pane holds at most ${ANNOTATION_COUNT_LIMIT} annotations.`;
  let bytes = 0;
  for (const annotation of annotations) bytes += annotationBytes(annotation);
  if (bytes > ANNOTATION_PANE_TEXT_LIMIT) return "The annotations of this pane would hold more than 256 KB of text.";
  return undefined;
}

function optionalString(value: unknown): string | undefined | null {
  if (value === undefined || value === null) return undefined;
  return typeof value === "string" ? value : null;
}

function receivedTarget(value: unknown): AnnotationTarget | null {
  if (!isRecord(value)) return null;
  const { view, quote, prefix, suffix, startLine, endLine } = value;
  if ((view !== "source" && view !== "rendered") || typeof quote !== "string") return null;
  if (typeof prefix !== "string" || typeof suffix !== "string") return null;
  if (typeof startLine !== "number" || typeof endLine !== "number") return null;
  return { view, quote, prefix, suffix, startLine, endLine };
}

/** One annotation of a snapshot, or null when the server could not have kept it. */
function receivedAnnotation(value: unknown): ViewerAnnotation | null {
  if (!isRecord(value) || !isAnnotationId(value.id) || typeof value.kind !== "string" || !KINDS.has(value.kind)) return null;
  const body = optionalString(value.body);
  const replacement = optionalString(value.replacement);
  if (body === null || replacement === null) return null;
  let target: AnnotationTarget | undefined;
  if (value.target !== undefined && value.target !== null) {
    const parsed = receivedTarget(value.target);
    if (!parsed) return null;
    target = parsed;
  }
  const annotation: ViewerAnnotation = {
    id: value.id,
    kind: value.kind as AnnotationKind,
    ...(body !== undefined ? { body } : {}),
    ...(replacement !== undefined ? { replacement } : {}),
    ...(target ? { target } : {}),
  };
  return annotationProblem(annotation, false) ? null : annotation;
}

/**
 * The annotations of a viewer record as a snapshot brings them, by the
 * server's rules: malformed entries and repeated IDs are dropped, and the
 * list stops at the pane's limits. Only the listed fields are kept.
 */
export function receivedAnnotations(value: unknown): ViewerAnnotation[] {
  if (!Array.isArray(value)) return [];
  const list: ViewerAnnotation[] = [];
  const ids = new Set<string>();
  let bytes = 0;
  for (const entry of value) {
    if (list.length >= ANNOTATION_COUNT_LIMIT) break;
    const annotation = receivedAnnotation(entry);
    if (!annotation || ids.has(annotation.id)) continue;
    const size = annotationBytes(annotation);
    if (bytes + size > ANNOTATION_PANE_TEXT_LIMIT) continue;
    ids.add(annotation.id);
    bytes += size;
    list.push(annotation);
  }
  return list;
}

function sameTarget(a: AnnotationTarget | undefined, b: AnnotationTarget | undefined): boolean {
  if (!a || !b) return a === b;
  return a.view === b.view && a.quote === b.quote && a.prefix === b.prefix && a.suffix === b.suffix
    && a.startLine === b.startLine && a.endLine === b.endLine;
}

export function sameAnnotation(a: ViewerAnnotation, b: ViewerAnnotation): boolean {
  return a.id === b.id && a.kind === b.kind && a.body === b.body && a.replacement === b.replacement && sameTarget(a.target, b.target);
}

function sameList(a: readonly ViewerAnnotation[], b: readonly ViewerAnnotation[]): boolean {
  return a.length === b.length && a.every((annotation, i) => sameAnnotation(annotation, b[i]));
}

/** An annotation as it is sent: well-formed text, no blank remark, and only the fields its kind uses. */
function outgoing(annotation: ViewerAnnotation): ViewerAnnotation {
  const { id, kind, body, replacement, target } = annotation;
  return {
    id,
    kind,
    ...(body !== undefined && !isBlank(body) ? { body: wellFormed(body) } : {}),
    ...(kind === "change" && replacement !== undefined ? { replacement: wellFormed(replacement) } : {}),
    ...(target ? {
      target: {
        view: target.view,
        quote: wellFormed(target.quote),
        prefix: wellFormed(target.prefix),
        suffix: wellFormed(target.suffix),
        startLine: target.startLine,
        endLine: target.endLine,
      },
    } : {}),
  };
}

/** A rejected annotation operation of this window, as `mux.op_failed` reports it. */
export interface AnnotationFailure {
  readonly op: string;
  readonly screenId?: string;
  readonly paneId?: string;
  readonly error: string;
}

interface Hold {
  /** What this window put, or null for what it removed. */
  readonly annotation: ViewerAnnotation | null;
  /** The screen the operation went to, on the server. */
  readonly screenId: string;
  readonly until: number;
  /** A snapshot showed something else while the hold lasted. */
  contradicted: boolean;
}

export interface AnnotationStoreOptions {
  /** Send an operation for a screen, by its id on the server. */
  send(screenId: string, op: MuxOperation): void;
  /** Ask for a screen's snapshot again. */
  resync(screenId: string): void;
  now?(): number;
}

const EMPTY: readonly ViewerAnnotation[] = Object.freeze([]);

export class ViewerAnnotations {
  /** Per pane, the list of its last snapshot. */
  private readonly server = new Map<string, readonly ViewerAnnotation[]>();
  /** Per pane, this window's own changes that outrank older snapshots. */
  private readonly holds = new Map<string, Map<string, Hold>>();
  /** Per pane, what the window shows; the same array while nothing changes. */
  private readonly shown = new Map<string, readonly ViewerAnnotation[]>();
  /** Per screen, the viewer panes its last snapshot listed. */
  private readonly screens = new Map<string, Set<string>>();
  /** Viewer panes the window shows, and how many views of each. */
  private readonly mounted = new Map<string, number>();
  /** Per screen, the resync after holds that a snapshot contradicted. */
  private readonly checks = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly listeners = new Set<() => void>();
  private readonly rejections = new Set<(paneId: string, error: string) => void>();
  private readonly now: () => number;

  constructor(private readonly options: AnnotationStoreOptions) {
    this.now = options.now ?? (() => Date.now());
  }

  /** A screen's snapshot: its viewer panes' annotations (absent is none). */
  snapshot(screen: ScreenState): void {
    let changed = false;
    const listed = new Set<string>();
    for (const [paneId, pane] of Object.entries(screen.panes)) {
      if (pane.role !== "viewer") continue;
      listed.add(paneId);
      changed = this.adopt(screen.id, paneId, pane.viewer?.annotations ?? EMPTY) || changed;
    }
    // A pane gone from its screen is closed, or listed by another screen's snapshot.
    for (const paneId of this.screens.get(screen.id) ?? []) {
      if (listed.has(paneId) || this.listedElsewhere(screen.id, paneId)) continue;
      changed = this.forget(paneId) || changed;
    }
    this.screens.set(screen.id, listed);
    if (changed) this.emit();
  }

  /** The annotations the window shows for a pane, in creation order. */
  list(paneId: string): readonly ViewerAnnotation[] {
    return this.shown.get(paneId) ?? EMPTY;
  }

  count(paneId: string): number {
    return this.list(paneId).length;
  }

  /**
   * Add `annotations` to a pane, or replace those with the same IDs in their
   * place: shown at once and sent. Returns why the server would refuse it
   * instead, and then changes nothing. `restore` puts back annotations the
   * server held before, as they were.
   */
  put(
    screenId: string,
    paneId: string,
    annotations: readonly ViewerAnnotation[],
    options: { readonly restore?: boolean } = {},
  ): string | undefined {
    if (annotations.length === 0) return undefined;
    if (annotations.length > ANNOTATION_COUNT_LIMIT) return `At most ${ANNOTATION_COUNT_LIMIT} annotations can be saved at once.`;
    // What is put back was kept by the server before: its own rules apply.
    const sent = options.restore ? [...annotations] : annotations.map(outgoing);
    const ids = new Set<string>();
    for (const annotation of sent) {
      const problem = annotationProblem(annotation, !options.restore);
      if (problem) return problem;
      if (ids.has(annotation.id)) return "The same annotation is saved twice.";
      ids.add(annotation.id);
    }
    const after = [...this.list(paneId).filter((held) => !ids.has(held.id)), ...sent];
    const problem = paneProblem(after);
    if (problem) return problem;

    const holds = this.holdsOf(paneId);
    const until = this.now() + ANNOTATION_SETTLE_MS;
    for (const annotation of sent) holds.set(annotation.id, { annotation, screenId, until, contradicted: false });
    this.refresh(paneId);
    this.emit();
    this.options.send(screenId, { type: "put_annotations", paneId, annotations: sent });
    return undefined;
  }

  /** Remove annotations of a pane by ID: gone at once, and sent. */
  remove(screenId: string, paneId: string, ids: readonly string[]): void {
    const unique = [...new Set(ids.filter(isAnnotationId))];
    if (unique.length === 0) return;
    const holds = this.holdsOf(paneId);
    const until = this.now() + ANNOTATION_SETTLE_MS;
    for (const id of unique) holds.set(id, { annotation: null, screenId, until, contradicted: false });
    this.refresh(paneId);
    this.emit();
    for (let i = 0; i < unique.length; i += ANNOTATION_COUNT_LIMIT) {
      this.options.send(screenId, { type: "remove_annotations", paneId, ids: unique.slice(i, i + ANNOTATION_COUNT_LIMIT) });
    }
  }

  /**
   * The server rejected an annotation operation of this window: the pane's
   * own changes go, and its next snapshot shows what the server holds.
   */
  failed(failure: AnnotationFailure): void {
    if (failure.op !== "put_annotations" && failure.op !== "remove_annotations") return;
    const paneId = failure.paneId;
    if (!paneId) return;
    if (this.holds.delete(paneId) && this.refresh(paneId)) this.emit();
    for (const listener of [...this.rejections]) listener(paneId, failure.error);
  }

  /** Called with a pane and the server's reason when an operation of this window on it was rejected. */
  onRejected(listener: (paneId: string, error: string) => void): () => void {
    this.rejections.add(listener);
    return () => this.rejections.delete(listener);
  }

  /** A view of the pane is shown in this window; returns its end. */
  mount(paneId: string): () => void {
    this.mounted.set(paneId, (this.mounted.get(paneId) ?? 0) + 1);
    let mounted = true;
    return () => {
      if (!mounted) return;
      mounted = false;
      const views = (this.mounted.get(paneId) ?? 1) - 1;
      if (views > 0) this.mounted.set(paneId, views);
      else this.mounted.delete(paneId);
    };
  }

  /** The annotations of the panes the window shows, which an ending server would end with them. */
  shownCount(): number {
    let count = 0;
    for (const paneId of this.mounted.keys()) count += this.count(paneId);
    return count;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  dispose(): void {
    for (const timer of this.checks.values()) clearTimeout(timer);
    this.checks.clear();
    this.listeners.clear();
    this.rejections.clear();
  }

  /** A pane's list from a snapshot, under this window's own changes; true when what is shown changed. */
  private adopt(screenId: string, paneId: string, list: readonly ViewerAnnotation[]): boolean {
    const holds = this.holds.get(paneId);
    if (holds) {
      const now = this.now();
      for (const [id, hold] of holds) {
        const held = list.find((annotation) => annotation.id === id);
        const shows = hold.annotation ? held !== undefined && sameAnnotation(held, hold.annotation) : held === undefined;
        if (shows || now >= hold.until) {
          holds.delete(id);
        } else if (!hold.contradicted) {
          hold.contradicted = true;
          this.check(hold.screenId, hold.until - now);
        }
      }
      if (holds.size === 0) this.holds.delete(paneId);
    }
    this.server.set(paneId, list);
    // A pane listed again on another screen belongs there now.
    for (const [other, ids] of this.screens) if (other !== screenId) ids.delete(paneId);
    return this.refresh(paneId);
  }

  /** Ask for a screen again once the holds a snapshot contradicted have passed. */
  private check(screenId: string, delay: number): void {
    const earlier = this.checks.get(screenId);
    if (earlier) clearTimeout(earlier);
    this.checks.set(screenId, setTimeout(() => {
      this.checks.delete(screenId);
      this.options.resync(screenId);
    }, Math.max(0, delay) + 50));
  }

  private holdsOf(paneId: string): Map<string, Hold> {
    let holds = this.holds.get(paneId);
    if (!holds) {
      holds = new Map();
      this.holds.set(paneId, holds);
    }
    return holds;
  }

  /** Compute what the window shows for a pane; true when it changed. */
  private refresh(paneId: string): boolean {
    const server = this.server.get(paneId) ?? EMPTY;
    const holds = this.holds.get(paneId);
    let next: readonly ViewerAnnotation[] = server;
    if (holds && holds.size > 0) {
      const listed = new Set(server.map((annotation) => annotation.id));
      const list: ViewerAnnotation[] = [];
      for (const annotation of server) {
        const hold = holds.get(annotation.id);
        if (!hold) list.push(annotation);
        else if (hold.annotation) list.push(hold.annotation);
      }
      for (const [id, hold] of holds) if (hold.annotation && !listed.has(id)) list.push(hold.annotation);
      next = list;
    }
    const before = this.shown.get(paneId) ?? EMPTY;
    if (sameList(before, next)) return false;
    if (next.length > 0) this.shown.set(paneId, Object.freeze([...next]));
    else this.shown.delete(paneId);
    return true;
  }

  private forget(paneId: string): boolean {
    this.server.delete(paneId);
    this.holds.delete(paneId);
    return this.shown.delete(paneId);
  }

  private listedElsewhere(screenId: string, paneId: string): boolean {
    for (const [other, ids] of this.screens) {
      if (other !== screenId && ids.has(paneId)) return true;
    }
    return false;
  }

  private emit(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

/** The list column of each mounted viewer pane, for the tab's marker. */
const lists = new Map<string, () => void>();

/** A pane's list column is mounted; `show` opens it. Returns the removal. */
export function registerAnnotationList(paneId: string, show: () => void): () => void {
  lists.set(paneId, show);
  return () => {
    if (lists.get(paneId) === show) lists.delete(paneId);
  };
}

/** Open a pane's list column; false when the pane has none mounted. */
export function showAnnotationList(paneId: string): boolean {
  const show = lists.get(paneId);
  show?.();
  return show !== undefined;
}
