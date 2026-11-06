/**
 * The annotations of one server's viewer panes (`viewer-annotations`): what
 * a window shows for each pane, from that server's snapshots and from the
 * window's own operations, and the rules the server applies to them.
 *
 * Every annotation opens a thread. The window writes the annotation itself
 * with `put_annotations`, which carries no number and no replies, and removes
 * it with `remove_annotations`; it writes the user's replies with
 * `put_replies`, removes replies of either author with `remove_replies`, and
 * marks entries as handed over to an agent with `mark_sent`. An agent's
 * replies come from the server alone, which also numbers the annotations.
 * Each change shows at once and is sent without waiting. A snapshot replaces
 * a pane's list, except for the entries this window changed during the last
 * two seconds: their state stays until a snapshot shows it or the time has
 * passed (Concurrent Annotation Edits). Replacing an annotation keeps the
 * number and the replies the server holds, so a reply an agent adds while the
 * user edits stays.
 *
 * The server sends the window nothing for its own operation, so an older
 * snapshot that still contradicts such a state asks for the screen once more
 * when the time has passed; the window then shows what the server holds. A
 * new annotation asks for the screen at once, since only the server knows its
 * number. A rejected operation drops the window's own state of the pane; the
 * server's annotations are shown again with the next snapshot.
 *
 * What the window sends passes the server's rules first, and where a rule
 * leaves room the window is stricter, never looser: a remark of nothing but
 * U+FEFF is blank here, not on the server. What arrives is checked by the
 * server's rules exactly, so that nothing the server keeps is dropped, and
 * malformed entries are.
 *
 * An entry of the user, the annotation itself or a reply, is pending until it
 * has been handed over (D4 of review-threads): the review holds the open
 * annotations with a pending entry, or every open one when none is pending
 * (`reviewSelection`).
 */

import type {
  AnnotationKind, AnnotationReply, AnnotationTarget, MuxOperation, ScreenState, SentEntry, UserReply, ViewerAnnotation,
} from "../protocol/types.ts";
import { plainText } from "../protocol/untrusted.ts";
import { SETTLE_MS } from "../scratchpad.ts";
import { utf8Length, wellFormed } from "./limits.ts";

/** The capability of a server that keeps annotations (`viewer-annotations`, Annotations Capability). */
export const ANNOTATIONS_CAPABILITY = "viewer-annotations-v1";
/** The most annotations a pane holds, and the most one operation carries. */
export const ANNOTATION_COUNT_LIMIT = 100;
/** The longest remark, replacement, quote and reply, in bytes of UTF-8. */
export const ANNOTATION_TEXT_LIMIT = 16_384;
/** The longest context on either side of a quote, in bytes of UTF-8. */
export const ANNOTATION_CONTEXT_LIMIT = 256;
/** The most text of all annotations of a pane and their replies, in bytes of UTF-8. */
export const ANNOTATION_PANE_TEXT_LIMIT = 262_144;
/** The highest line an annotation names. */
export const ANNOTATION_LINE_LIMIT = 10_000_000;
/** The most replies a thread holds, and the most one reply operation carries. */
export const REPLY_LIMIT = 50;
/** The longest title of an agent's reply, in bytes of UTF-8. */
export const REPLY_TITLE_LIMIT = 256;
/** The most entries one `mark_sent` carries: 100 threads of 51 entries. */
export const MARK_SENT_LIMIT = 5_100;
/** A shorter quote occurs too often to find its passage again; the window asks for a longer selection. */
export const MIN_QUOTE_CHARS = 3;
/** How long the window's own change outlasts snapshots the server made before it. */
export const ANNOTATION_SETTLE_MS = SETTLE_MS;

/** The operations of the window on annotations, whose rejection names the viewer pane. */
export const ANNOTATION_OPS: ReadonlySet<string> = new Set([
  "put_annotations", "remove_annotations", "put_replies", "remove_replies", "mark_sent",
]);

const KINDS: ReadonlySet<string> = new Set<AnnotationKind>(["comment", "change"]);
const ID = /^ann-[0-9a-f]{16}$/;
const REPLY_ID = /^rep-[0-9a-f]{16}$/;
const WAIT_ID = /^wait-[0-9a-f]{16}$/;
/** The longest pane ID taken from a server. */
const MAX_PANE_ID = 256;

type Raw = Record<string, unknown>;

function isRecord(value: unknown): value is Raw {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function randomHex(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** Whether `id` has the form of an annotation ID: `ann-` and 16 lowercase hex characters. */
export function isAnnotationId(id: unknown): id is string {
  return typeof id === "string" && ID.test(id);
}

/** Whether `id` has the form of a reply ID: `rep-` and 16 lowercase hex characters. */
export function isReplyId(id: unknown): id is string {
  return typeof id === "string" && REPLY_ID.test(id);
}

/** Whether `id` has the form of a wait's ID: `wait-` and 16 lowercase hex characters. */
export function isWaitId(id: unknown): id is string {
  return typeof id === "string" && WAIT_ID.test(id);
}

/** A new annotation ID; 64 random bits make a clash within a pane negligible. */
export function newAnnotationId(): string {
  return `ann-${randomHex()}`;
}

/** A new reply ID. */
export function newReplyId(): string {
  return `rep-${randomHex()}`;
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

/** Whether an annotation is open: not resolved. */
export function isOpen(annotation: ViewerAnnotation): boolean {
  return annotation.status !== "resolved";
}

/** Whether a change suggests deleting its passage: its replacement is empty. */
export function isDeletion(annotation: Pick<ViewerAnnotation, "kind" | "replacement">): boolean {
  return annotation.kind === "change" && !annotation.replacement;
}

/**
 * The text of an annotation that counts toward the pane's limit: its own
 * fields and its replies' texts, for what the window sends (`strict`) also
 * the titles of agents' replies.
 */
export function annotationBytes(annotation: ViewerAnnotation, strict = false): number {
  const target = annotation.target;
  let bytes = utf8Length(annotation.body ?? "") + utf8Length(annotation.replacement ?? "")
    + (target ? utf8Length(target.quote) + utf8Length(target.prefix) + utf8Length(target.suffix) : 0);
  for (const reply of annotation.replies ?? []) {
    bytes += utf8Length(reply.body) + (strict && reply.from ? utf8Length(reply.from.title) : 0);
  }
  return bytes;
}

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
  if (annotation.kind === "comment" && (body === undefined || isBlank(body, strict))) return "A comment needs a remark.";
  if (annotation.kind === "change") {
    if (replacement === undefined) return "A suggested change needs a replacement.";
    if (!target) return "A suggested change needs a passage.";
  }
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

/** Why the server would refuse a reply of the user; undefined when it would not. */
export function replyProblem(reply: Pick<AnnotationReply, "id" | "body">, strict = true): string | undefined {
  if (!isReplyId(reply.id)) return "The reply has no valid ID.";
  if (isBlank(reply.body, strict)) return "A reply needs a text.";
  if (utf8Length(reply.body) > ANNOTATION_TEXT_LIMIT) return "The reply is too long: it may hold at most 16 KB.";
  return undefined;
}

/** Why a pane may not hold `annotations`; undefined when it may. */
export function paneProblem(annotations: readonly ViewerAnnotation[]): string | undefined {
  if (annotations.length > ANNOTATION_COUNT_LIMIT) return `A pane holds at most ${ANNOTATION_COUNT_LIMIT} annotations.`;
  let bytes = 0;
  for (const annotation of annotations) bytes += annotationBytes(annotation, true);
  if (bytes > ANNOTATION_PANE_TEXT_LIMIT) return "The annotations of this pane would hold more than 256 KB of text.";
  return undefined;
}

/**
 * The IDs of the user's entries of `annotation` that were not handed over:
 * the annotation itself and its replies of the user, in their order.
 */
export function pendingEntries(annotation: ViewerAnnotation): string[] {
  const ids = annotation.sent ? [] : [annotation.id];
  for (const reply of annotation.replies ?? []) {
    if (reply.author === "user" && !reply.sent) ids.push(reply.id);
  }
  return ids;
}

/** Whether an annotation goes into the next review for its own sake: open, with an entry of the user not handed over. */
export function isPending(annotation: ViewerAnnotation): boolean {
  return isOpen(annotation) && pendingEntries(annotation).length > 0;
}

/**
 * The annotations of a review, from `list` in the pane's order (D4 of
 * review-threads): the open ones with a pending entry; when none is
 * pending, every open one, which is a review sent again.
 */
export function reviewSelection(list: readonly ViewerAnnotation[]): { readonly annotations: ViewerAnnotation[]; readonly again: boolean } {
  const pending = list.filter(isPending);
  if (pending.length > 0) return { annotations: pending, again: false };
  return { annotations: list.filter(isOpen), again: true };
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

/**
 * One reply of a snapshot, or null when the server could not have kept it.
 * An agent's title is untrusted text: controls and bidirectional formatting
 * characters go, and it is cut to its limit.
 */
function receivedReply(value: unknown): AnnotationReply | null {
  if (!isRecord(value) || !isReplyId(value.id) || typeof value.body !== "string") return null;
  const { id, author, body } = value;
  if (author !== "user" && author !== "agent") return null;
  if (replyProblem({ id, body }, false)) return null;
  if (author === "user") return { id, author, body, ...(value.sent === true ? { sent: true as const } : {}) };
  const from = value.from;
  if (!isRecord(from)) return { id, author, body };
  const paneId = typeof from.paneId === "string" && from.paneId.length <= MAX_PANE_ID ? from.paneId : "";
  let title = plainText(from.title, REPLY_TITLE_LIMIT);
  while (utf8Length(title) > REPLY_TITLE_LIMIT) title = [...title].slice(0, -1).join("");
  return { id, author, body, from: { paneId, title } };
}

/**
 * The kind of an annotation as the server reads it: `question` from builds
 * before threads is a comment, `delete` a change with an empty replacement.
 */
function receivedKind(kind: unknown): { kind: AnnotationKind; deletion: boolean } | null {
  if (kind === "comment" || kind === "question") return { kind: "comment", deletion: false };
  if (kind === "change") return { kind, deletion: false };
  if (kind === "delete") return { kind: "change", deletion: true };
  return null;
}

/**
 * One annotation of a snapshot, or null when the server could not have kept
 * it. A reply whose ID `taken` holds, or the thread holds already, is dropped.
 */
function receivedAnnotation(value: unknown, taken: ReadonlySet<string>): ViewerAnnotation | null {
  if (!isRecord(value) || !isAnnotationId(value.id)) return null;
  const kind = receivedKind(value.kind);
  if (!kind) return null;
  const body = optionalString(value.body);
  const given = optionalString(value.replacement);
  if (body === null || given === null) return null;
  const replacement = kind.kind === "change" ? (kind.deletion ? "" : given) : given;
  let target: AnnotationTarget | undefined;
  if (value.target !== undefined && value.target !== null) {
    const parsed = receivedTarget(value.target);
    if (!parsed) return null;
    target = parsed;
  }
  const number = value.number;
  if (number !== undefined && number !== null && (typeof number !== "number" || !Number.isSafeInteger(number) || number < 1)) return null;
  const status = value.status;
  if (status !== undefined && status !== null && status !== "resolved" && status !== "open") return null;
  const replies: AnnotationReply[] = [];
  if (Array.isArray(value.replies)) {
    const ids = new Set<string>();
    for (const entry of value.replies) {
      if (replies.length >= REPLY_LIMIT) break;
      const reply = receivedReply(entry);
      if (!reply || taken.has(reply.id) || ids.has(reply.id)) continue;
      ids.add(reply.id);
      replies.push(reply);
    }
  }
  const annotation: ViewerAnnotation = {
    id: value.id,
    ...(typeof number === "number" ? { number } : {}),
    kind: kind.kind,
    ...(body !== undefined ? { body } : {}),
    ...(replacement !== undefined ? { replacement } : {}),
    ...(target ? { target } : {}),
    ...(status === "resolved" ? { status } : {}),
    ...(value.sent === true ? { sent: true as const } : {}),
    ...(replies.length > 0 ? { replies } : {}),
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
  const replyIds = new Set<string>();
  let bytes = 0;
  for (const entry of value) {
    if (list.length >= ANNOTATION_COUNT_LIMIT) break;
    const annotation = receivedAnnotation(entry, replyIds);
    if (!annotation || ids.has(annotation.id)) continue;
    const size = annotationBytes(annotation);
    if (bytes + size > ANNOTATION_PANE_TEXT_LIMIT) continue;
    ids.add(annotation.id);
    for (const reply of annotation.replies ?? []) replyIds.add(reply.id);
    bytes += size;
    list.push(annotation);
  }
  return list;
}

/** What a window puts of an annotation: all but the server's number and thread. */
export type OwnAnnotation = Omit<ViewerAnnotation, "number" | "replies">;

/** An annotation without the fields the server keeps for it. */
export function ownOf(annotation: ViewerAnnotation): OwnAnnotation {
  const { number: _number, replies: _replies, ...own } = annotation;
  return own;
}

function sameTarget(a: AnnotationTarget | undefined, b: AnnotationTarget | undefined): boolean {
  if (!a || !b) return a === b;
  return a.view === b.view && a.quote === b.quote && a.prefix === b.prefix && a.suffix === b.suffix
    && a.startLine === b.startLine && a.endLine === b.endLine;
}

/** Whether two annotations say the same, apart from the server's number, thread and the hand-over. */
export function sameContent(a: OwnAnnotation, b: OwnAnnotation): boolean {
  return a.id === b.id && a.kind === b.kind && a.body === b.body && a.replacement === b.replacement && sameTarget(a.target, b.target);
}

/** Whether the window put the same of both: content, status and hand-over. */
function sameOwn(a: OwnAnnotation, b: OwnAnnotation): boolean {
  return sameContent(a, b) && a.status === b.status && !!a.sent === !!b.sent;
}

function sameReply(a: AnnotationReply, b: AnnotationReply): boolean {
  return a.id === b.id && a.author === b.author && a.body === b.body && !!a.sent === !!b.sent
    && a.from?.paneId === b.from?.paneId && a.from?.title === b.from?.title;
}

export function sameAnnotation(a: ViewerAnnotation, b: ViewerAnnotation): boolean {
  const left = a.replies ?? [];
  const right = b.replies ?? [];
  return sameOwn(a, b) && a.number === b.number && left.length === right.length
    && left.every((reply, i) => sameReply(reply, right[i]));
}

function sameList(a: readonly ViewerAnnotation[], b: readonly ViewerAnnotation[]): boolean {
  return a.length === b.length && a.every((annotation, i) => sameAnnotation(annotation, b[i]));
}

/** An annotation as it is sent: well-formed text, no blank remark, and only the fields its kind uses. */
function outgoing(annotation: ViewerAnnotation): OwnAnnotation {
  const { id, kind, body, replacement, target, status, sent } = annotation;
  return {
    id,
    kind,
    ...(body !== undefined && !isBlank(body) ? { body: wellFormed(body) } : {}),
    ...(kind === "change" ? { replacement: wellFormed(replacement ?? "") } : {}),
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
    ...(status === "resolved" ? { status } : {}),
    ...(sent ? { sent: true as const } : {}),
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
  /** The screen the operation went to, on the server. */
  readonly screenId: string;
  until: number;
  /** A snapshot showed something else while the hold lasted. */
  contradicted: boolean;
}

interface AnnotationHold extends Hold {
  /** What this window put of the annotation, or null for what it removed. */
  own: OwnAnnotation | null;
  /**
   * Put again after this window removed it: the server adds it anew, with a
   * new number and no thread, so the last snapshot's number and thread are
   * not its own.
   */
  revived?: boolean;
}

interface ReplyHold extends Hold {
  readonly annotationId: string;
  /** What this window put of the user's reply, or null for a reply it removed. */
  reply: AnnotationReply | null;
}

/** The window's own changes of a pane that outrank older snapshots. */
interface PaneHolds {
  readonly annotations: Map<string, AnnotationHold>;
  readonly replies: Map<string, ReplyHold>;
  /** Entries marked as handed over that no other hold covers. */
  readonly sent: Map<string, Hold>;
}

export interface AnnotationStoreOptions {
  /** Send an operation for a screen, by its id on the server. */
  send(screenId: string, op: MuxOperation): void;
  /** Ask for a screen's snapshot again. */
  resync(screenId: string): void;
  now?(): number;
}

const EMPTY: readonly ViewerAnnotation[] = Object.freeze([]);

function holdCount(holds: PaneHolds): number {
  return holds.annotations.size + holds.replies.size + holds.sent.size;
}

export class ViewerAnnotations {
  /** Per pane, the list of its last snapshot. */
  private readonly server = new Map<string, readonly ViewerAnnotation[]>();
  /** Per pane, this window's own changes that outrank older snapshots. */
  private readonly holds = new Map<string, PaneHolds>();
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

  /** The annotations the window shows for a pane, in creation order, each with its thread. */
  list(paneId: string): readonly ViewerAnnotation[] {
    return this.shown.get(paneId) ?? EMPTY;
  }

  /** One annotation the window shows. */
  get(paneId: string, id: string): ViewerAnnotation | undefined {
    return this.list(paneId).find((annotation) => annotation.id === id);
  }

  /** All annotations of a pane, open and resolved: what closing the pane ends. */
  count(paneId: string): number {
    return this.list(paneId).length;
  }

  /** The open annotations of a pane, which its tab's marker counts. */
  openCount(paneId: string): number {
    let open = 0;
    for (const annotation of this.list(paneId)) if (isOpen(annotation)) open++;
    return open;
  }

  /**
   * Add `annotations` to a pane, or replace those with the same IDs in their
   * place, keeping their numbers and threads: shown at once and sent. Returns
   * why the server would refuse it instead, and then changes nothing.
   * `restore` puts back annotations the server held before, as they were.
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
    const sent = annotations.map((annotation) => (options.restore ? ownOf(annotation) : outgoing(annotation)));
    const ids = new Set<string>();
    for (const annotation of sent) {
      const problem = annotationProblem(annotation, !options.restore);
      if (problem) return problem;
      if (ids.has(annotation.id)) return "The same annotation is saved twice.";
      ids.add(annotation.id);
    }
    const current = this.list(paneId);
    const known = new Map(current.map((annotation) => [annotation.id, annotation]));
    const after = [
      ...current.filter((held) => !ids.has(held.id)),
      ...sent.map((annotation) => ({ ...annotation, replies: known.get(annotation.id)?.replies })),
    ];
    const problem = paneProblem(after);
    if (problem) return problem;

    const holds = this.holdsOf(paneId);
    const until = this.now() + ANNOTATION_SETTLE_MS;
    // New to the server: not in its last snapshot, or removed by this window since.
    const listed = new Set((this.server.get(paneId) ?? EMPTY).map((annotation) => annotation.id));
    let fresh = false;
    for (const own of sent) {
      const before = holds.annotations.get(own.id);
      const revived = before !== undefined && (before.own === null || !!before.revived);
      if (revived || !listed.has(own.id)) fresh = true;
      holds.annotations.set(own.id, { own, screenId, until, contradicted: false, ...(revived ? { revived: true } : {}) });
      // The put says whether it was handed over.
      holds.sent.delete(own.id);
    }
    this.refresh(paneId);
    this.emit();
    this.options.send(screenId, { type: "put_annotations", paneId, annotations: sent });
    // Only the server knows the number of a new annotation, and it sends this window nothing for its op.
    if (fresh) this.options.resync(screenId);
    return undefined;
  }

  /** Remove annotations of a pane by ID, with their threads: gone at once, and sent. */
  remove(screenId: string, paneId: string, ids: readonly string[]): void {
    const unique = [...new Set(ids.filter(isAnnotationId))];
    if (unique.length === 0) return;
    const holds = this.holdsOf(paneId);
    const until = this.now() + ANNOTATION_SETTLE_MS;
    const gone = new Set(unique);
    for (const id of unique) {
      holds.annotations.set(id, { own: null, screenId, until, contradicted: false });
      holds.sent.delete(id);
    }
    // Their replies go with them.
    for (const [id, hold] of holds.replies) if (gone.has(hold.annotationId)) holds.replies.delete(id);
    this.refresh(paneId);
    this.emit();
    for (let i = 0; i < unique.length; i += ANNOTATION_COUNT_LIMIT) {
      this.options.send(screenId, { type: "remove_annotations", paneId, ids: unique.slice(i, i + ANNOTATION_COUNT_LIMIT) });
    }
  }

  /**
   * Add replies of the user to an annotation's thread, or replace the user's
   * replies with the same IDs in their place: shown at once and sent. Returns
   * why the server would refuse it instead, and then changes nothing.
   * `restore` puts back replies the server held before, as they were.
   */
  putReplies(
    screenId: string,
    paneId: string,
    annotationId: string,
    replies: readonly Pick<UserReply, "id" | "body" | "sent">[],
    options: { readonly restore?: boolean } = {},
  ): string | undefined {
    if (replies.length === 0) return undefined;
    if (replies.length > REPLY_LIMIT) return `At most ${REPLY_LIMIT} replies can be saved at once.`;
    const annotation = this.get(paneId, annotationId);
    if (!annotation) return "The annotation is no longer there.";
    const sent: UserReply[] = replies.map((reply) => ({
      id: reply.id,
      author: "user",
      body: options.restore ? reply.body : wellFormed(reply.body),
      ...(reply.sent ? { sent: true as const } : {}),
    }));
    const held = annotation.replies ?? [];
    const ids = new Set<string>();
    for (const reply of sent) {
      const problem = replyProblem(reply, !options.restore);
      if (problem) return problem;
      if (ids.has(reply.id)) return "The same reply is saved twice.";
      ids.add(reply.id);
      if (held.some((each) => each.id === reply.id && each.author !== "user")) return "An agent's reply cannot be changed.";
    }
    const kept = held.filter((each) => !ids.has(each.id)).length;
    if (kept + sent.length > REPLY_LIMIT) return `A thread holds at most ${REPLY_LIMIT} replies.`;
    const thread = [...held.map((each) => sent.find((reply) => reply.id === each.id) ?? each),
      ...sent.filter((reply) => !held.some((each) => each.id === reply.id))];
    const after = this.list(paneId).map((each) => (each.id === annotationId ? { ...each, replies: thread } : each));
    const problem = paneProblem(after);
    if (problem) return problem;

    const holds = this.holdsOf(paneId);
    const until = this.now() + ANNOTATION_SETTLE_MS;
    for (const reply of sent) {
      holds.replies.set(reply.id, { annotationId, reply, screenId, until, contradicted: false });
      holds.sent.delete(reply.id);
    }
    this.refresh(paneId);
    this.emit();
    this.options.send(screenId, { type: "put_replies", paneId, annotationId, replies: sent });
    return undefined;
  }

  /** Remove replies of either author from an annotation's thread: gone at once, and sent. */
  removeReplies(screenId: string, paneId: string, annotationId: string, ids: readonly string[]): void {
    if (!isAnnotationId(annotationId)) return;
    const unique = [...new Set(ids.filter(isReplyId))];
    if (unique.length === 0) return;
    const holds = this.holdsOf(paneId);
    const until = this.now() + ANNOTATION_SETTLE_MS;
    for (const id of unique) {
      holds.replies.set(id, { annotationId, reply: null, screenId, until, contradicted: false });
      holds.sent.delete(id);
    }
    this.refresh(paneId);
    this.emit();
    for (let i = 0; i < unique.length; i += REPLY_LIMIT) {
      this.options.send(screenId, { type: "remove_replies", paneId, annotationId, ids: unique.slice(i, i + REPLY_LIMIT) });
    }
  }

  /**
   * Mark annotations and replies of the user as handed over to an agent,
   * each with the content the review held: shown at once and sent; the
   * server marks only the entries that still read so. Agents' replies and
   * entries the window does not show are left out.
   */
  markSent(screenId: string, paneId: string, entries: readonly SentEntry[]): void {
    const own = new Set<string>();
    for (const annotation of this.list(paneId)) {
      own.add(annotation.id);
      for (const reply of annotation.replies ?? []) if (reply.author === "user") own.add(reply.id);
    }
    const unique = [...new Map(entries.map((entry) => [entry.id, entry])).values()].filter((entry) => own.has(entry.id));
    if (unique.length === 0) return;
    const holds = this.holdsOf(paneId);
    const until = this.now() + ANNOTATION_SETTLE_MS;
    for (const { id } of unique) {
      const annotation = holds.annotations.get(id);
      const reply = holds.replies.get(id);
      if (annotation?.own) {
        annotation.own = { ...annotation.own, sent: true };
        annotation.until = until;
        annotation.contradicted = false;
      } else if (reply?.reply) {
        reply.reply = { ...reply.reply, sent: true };
        reply.until = until;
        reply.contradicted = false;
      } else {
        holds.sent.set(id, { screenId, until, contradicted: false });
      }
    }
    this.refresh(paneId);
    this.emit();
    for (let i = 0; i < unique.length; i += MARK_SENT_LIMIT) {
      this.options.send(screenId, { type: "mark_sent", paneId, entries: unique.slice(i, i + MARK_SENT_LIMIT) });
    }
  }

  /**
   * Resolves once every annotation of `ids` that the pane still shows has the
   * number the server gives it, or after `timeoutMs`.
   */
  whenNumbered(paneId: string, ids: readonly string[], timeoutMs = ANNOTATION_SETTLE_MS): Promise<void> {
    const wanted = new Set(ids);
    const numbered = () => this.list(paneId).every((annotation) => !wanted.has(annotation.id) || annotation.number !== undefined);
    if (numbered()) return Promise.resolve();
    return new Promise((resolve) => {
      const done = () => {
        clearTimeout(timer);
        unsubscribe();
        resolve();
      };
      const timer = setTimeout(done, timeoutMs);
      const unsubscribe = this.subscribe(() => {
        if (numbered()) done();
      });
    });
  }

  /**
   * The server rejected an annotation operation of this window: the pane's
   * own changes go, and its next snapshot shows what the server holds.
   */
  failed(failure: AnnotationFailure): void {
    if (!ANNOTATION_OPS.has(failure.op)) return;
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
      const byId = new Map(list.map((annotation) => [annotation.id, annotation]));
      const replies = new Map<string, { readonly annotationId: string; readonly reply: AnnotationReply }>();
      for (const annotation of list) {
        for (const reply of annotation.replies ?? []) replies.set(reply.id, { annotationId: annotation.id, reply });
      }
      const settle = <H extends Hold>(map: Map<string, H>, shows: (id: string, hold: H) => boolean) => {
        for (const [id, hold] of map) {
          if (shows(id, hold) || now >= hold.until) {
            map.delete(id);
          } else if (!hold.contradicted) {
            hold.contradicted = true;
            this.check(hold.screenId, hold.until - now);
          }
        }
      };
      settle(holds.annotations, (id, hold) => {
        const held = byId.get(id);
        return hold.own ? held !== undefined && sameOwn(ownOf(held), hold.own) : held === undefined;
      });
      settle(holds.replies, (id, hold) => {
        const held = replies.get(id);
        if (!hold.reply) return held === undefined || held.annotationId !== hold.annotationId;
        return held !== undefined && held.annotationId === hold.annotationId && sameReply(held.reply, hold.reply);
      });
      // A mark is shown once the entry is marked, and moot once it is gone.
      settle(holds.sent, (id) => {
        const entry = byId.get(id) ?? replies.get(id)?.reply;
        return entry === undefined || !!entry.sent;
      });
      if (holdCount(holds) === 0) this.holds.delete(paneId);
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

  private holdsOf(paneId: string): PaneHolds {
    let holds = this.holds.get(paneId);
    if (!holds) {
      holds = { annotations: new Map(), replies: new Map(), sent: new Map() };
      this.holds.set(paneId, holds);
    }
    return holds;
  }

  /** A thread from the server's replies under this window's own changes. */
  private thread(holds: PaneHolds, annotationId: string, replies: readonly AnnotationReply[]): AnnotationReply[] {
    const thread: AnnotationReply[] = [];
    const listed = new Set<string>();
    for (const reply of replies) {
      listed.add(reply.id);
      const hold = holds.replies.get(reply.id);
      if (hold && hold.annotationId === annotationId) {
        if (hold.reply) thread.push(hold.reply);
      } else {
        thread.push(reply.author === "user" && holds.sent.has(reply.id) ? { ...reply, sent: true } : reply);
      }
    }
    for (const [id, hold] of holds.replies) {
      if (hold.reply && hold.annotationId === annotationId && !listed.has(id)) thread.push(hold.reply);
    }
    return thread;
  }

  private compose(holds: PaneHolds, own: OwnAnnotation, server: ViewerAnnotation | undefined): ViewerAnnotation {
    const replies = this.thread(holds, own.id, server?.replies ?? []);
    return {
      ...own,
      ...(server?.number !== undefined ? { number: server.number } : {}),
      ...(replies.length > 0 ? { replies } : {}),
    };
  }

  /** Compute what the window shows for a pane; true when it changed. */
  private refresh(paneId: string): boolean {
    const server = this.server.get(paneId) ?? EMPTY;
    const holds = this.holds.get(paneId);
    let next: readonly ViewerAnnotation[] = server;
    if (holds && holdCount(holds) > 0) {
      const listed = new Set<string>();
      const list: ViewerAnnotation[] = [];
      for (const annotation of server) {
        listed.add(annotation.id);
        const hold = holds.annotations.get(annotation.id);
        if (hold && !hold.own) continue;
        let own = hold?.own ?? ownOf(annotation);
        if (!hold && holds.sent.has(annotation.id)) own = { ...own, sent: true };
        list.push(this.compose(holds, own, hold?.revived ? undefined : annotation));
      }
      for (const [id, hold] of holds.annotations) {
        if (hold.own && !listed.has(id)) list.push(this.compose(holds, hold.own, undefined));
      }
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
