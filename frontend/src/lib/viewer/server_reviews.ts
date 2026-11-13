/**
 * The anchors and the review of a viewer pane's annotations, which the pane's
 * server works out (`mux-protocol`, Review Texts And Anchors; SBR-1 of
 * server-built-reviews): `viewer.anchor` and `viewer.review` over the pane's
 * connection, answered to this window only with `viewer.anchored`,
 * `viewer.reviewed` or `viewer.failed`.
 *
 * The server anchors every annotation in the document's source text and
 * counts its ranges in UTF-8 bytes of that text; `utf16Offsets` turns them
 * into offsets of the window's source text, which holds the same text. An
 * annotation taken from the rendered view is anchored by the window in its
 * rendered text, and the window hands what it found to the review as a
 * passage (SBR-5).
 */

import type { ConnSocket } from "../protocol/socket.ts";
import type { IncomingMessage, ReviewOutcome, SentEntry } from "../protocol/types.ts";
import { MAX_TEXT, plainText } from "../protocol/untrusted.ts";

/** The server anchors annotations and writes reviews (`mux.welcome`). */
export const REVIEWS_CAPABILITY = "viewer-reviews-v1";
/** How long a request waits for its answer. */
export const REVIEW_REQUEST_TIMEOUT_MS = 15_000;
/** The most passages one review request carries, as the server takes them. */
export const PASSAGE_LIMIT = 100;

/** Where the server found an annotation's passage in the source text. */
export interface ServerAnchor {
  readonly id: string;
  readonly state: "anchored" | "changed" | "lost" | "general";
  /** UTF-8 bytes of the source text; with `anchored` and `changed` only. */
  readonly start?: number;
  readonly end?: number;
  readonly startLine?: number;
  readonly endLine?: number;
}

/** The anchors of a pane's annotations in one version of its document, in the order of its list. */
export interface Anchored {
  readonly paneId: string;
  readonly version: string;
  readonly anchors: readonly ServerAnchor[];
}

/** What the window found in its rendered view for an annotation taken from it. */
export interface RenderedPassage {
  readonly id: string;
  readonly state: "anchored" | "changed" | "lost";
  readonly startLine?: number;
  readonly endLine?: number;
  readonly offset?: number;
  /** A page that names no source lines. */
  readonly unlined?: true;
}

export interface ReviewRequest {
  readonly paneId: string;
  readonly verdict?: ReviewOutcome;
  readonly changedDuringWait?: boolean;
  readonly passages?: readonly RenderedPassage[];
}

/** The review the server wrote. */
export interface Reviewed {
  readonly paneId: string;
  readonly version: string;
  readonly text: string;
  /** How many annotations it holds. */
  readonly count: number;
  /** The user's entries it holds that were not handed over, as `mark_sent` names them. */
  readonly entries: readonly SentEntry[];
}

/** The anchors and reviews a pane's connection answers. */
export interface PaneReviews {
  anchor(paneId: string): Promise<Anchored>;
  review(request: ReviewRequest): Promise<Reviewed>;
}

export class ViewerReviewError extends Error {
  constructor(message: string, readonly code?: string) {
    super(message);
    this.name = "ViewerReviewError";
  }
}

interface Pending {
  readonly kind: "anchor" | "review";
  settle(answer: Record<string, unknown> | Error): void;
}

const ANCHOR_STATES = new Set(["anchored", "changed", "lost", "general"]);

function newRequestId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return `vq-${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

const isCount = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;

/** A server's anchor, or undefined for one that does not keep to the protocol. */
function anchorOf(value: unknown): ServerAnchor | undefined {
  if (!value || typeof value !== "object") return undefined;
  const { id, state, start, end, startLine, endLine } = value as Record<string, unknown>;
  if (typeof id !== "string" || typeof state !== "string" || !ANCHOR_STATES.has(state)) return undefined;
  if (state === "anchored" || state === "changed") {
    if (!isCount(start) || !isCount(end) || start > end || !isCount(startLine) || !isCount(endLine) || startLine < 1 || startLine > endLine) return undefined;
    return { id, state, start, end, startLine, endLine };
  }
  return { id, state: state as ServerAnchor["state"] };
}

/** A server's entry of a review, as `mark_sent` takes it back. */
function entryOf(value: unknown): SentEntry | undefined {
  if (!value || typeof value !== "object" || typeof (value as { id?: unknown }).id !== "string") return undefined;
  return value as SentEntry;
}

/**
 * The anchor and review requests of one connection. A request fails when the
 * server answers `viewer.failed`, the connection ends or no answer comes
 * within `REVIEW_REQUEST_TIMEOUT_MS`.
 */
export class ViewerReviews implements PaneReviews {
  private readonly pending = new Map<string, Pending>();
  private readonly unsubscribes: (() => void)[];

  constructor(private readonly socket: ConnSocket, private readonly timeoutMs = REVIEW_REQUEST_TIMEOUT_MS) {
    this.unsubscribes = [
      socket.onMessage((msg) => {
        if (msg.type !== "viewer.anchored" && msg.type !== "viewer.reviewed" && msg.type !== "viewer.failed") return;
        const requestId = (msg as { requestId?: unknown }).requestId;
        const pending = typeof requestId === "string" ? this.pending.get(requestId) : undefined;
        if (!pending) return;
        if (msg.type === "viewer.failed") {
          const { code, error } = msg as { code?: unknown; error?: unknown };
          pending.settle(new ViewerReviewError(
            typeof error === "string" && error ? plainText(error, MAX_TEXT) : "The server could not answer.",
            typeof code === "string" ? code : undefined,
          ));
        } else if ((msg.type === "viewer.anchored") === (pending.kind === "anchor")) {
          pending.settle(msg as unknown as Record<string, unknown>);
        } else {
          pending.settle(new ViewerReviewError("The server's answer does not fit the request."));
        }
      }),
      socket.onDisconnect(() => {
        for (const pending of [...this.pending.values()]) pending.settle(new ViewerReviewError("The connection to the server ended."));
      }),
    ];
  }

  async anchor(paneId: string): Promise<Anchored> {
    const answer = await this.request("anchor", { type: "viewer.anchor", paneId });
    const { version, anchors } = answer;
    const parsed = Array.isArray(anchors) ? anchors.map(anchorOf) : undefined;
    if (typeof version !== "string" || !parsed || parsed.some((each) => !each)) {
      throw new ViewerReviewError("The server's anchors could not be read.");
    }
    return { paneId, version, anchors: parsed as ServerAnchor[] };
  }

  async review(request: ReviewRequest): Promise<Reviewed> {
    const answer = await this.request("review", {
      type: "viewer.review",
      paneId: request.paneId,
      ...(request.verdict ? { verdict: request.verdict } : {}),
      ...(request.changedDuringWait ? { changedDuringWait: true } : {}),
      ...(request.passages && request.passages.length > 0 ? { passages: request.passages.slice(0, PASSAGE_LIMIT).map((passage) => ({ ...passage })) } : {}),
    });
    const { version, text, count, entries } = answer;
    const parsed = Array.isArray(entries) ? entries.map(entryOf) : undefined;
    if (typeof version !== "string" || typeof text !== "string" || !isCount(count) || !parsed || parsed.some((each) => !each)) {
      throw new ViewerReviewError("The server's review could not be read.");
    }
    return { paneId: request.paneId, version, text, count, entries: parsed as SentEntry[] };
  }

  dispose(): void {
    for (const unsubscribe of this.unsubscribes) unsubscribe();
    this.unsubscribes.length = 0;
    for (const pending of [...this.pending.values()]) pending.settle(new ViewerReviewError("The connection was closed."));
  }

  private request(
    kind: Pending["kind"],
    message: Omit<Extract<IncomingMessage, { type: "viewer.anchor" | "viewer.review" }>, "requestId">,
  ): Promise<Record<string, unknown>> {
    const requestId = newRequestId();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => settle(new ViewerReviewError("The server did not answer in time.")), this.timeoutMs);
      const settle = (answer: Record<string, unknown> | Error) => {
        clearTimeout(timer);
        this.pending.delete(requestId);
        if (answer instanceof Error) reject(answer);
        else resolve(answer);
      };
      this.pending.set(requestId, { kind, settle });
      this.socket.send({ ...message, requestId } as IncomingMessage);
    });
  }
}

/**
 * The UTF-16 offsets of `text` at the UTF-8 byte offsets `bytes`, in their
 * order; undefined for an offset past the text or inside a character.
 */
export function utf16Offsets(text: string, bytes: readonly number[]): (number | undefined)[] {
  const order = bytes.map((_, index) => index).sort((a, b) => bytes[a] - bytes[b]);
  const found: (number | undefined)[] = bytes.map(() => undefined);
  let byte = 0;
  let unit = 0;
  let next = 0;
  while (next < order.length) {
    const wanted = bytes[order[next]];
    if (wanted < byte) {
      next++;
      continue;
    }
    if (wanted === byte) {
      found[order[next++]] = unit;
      continue;
    }
    if (unit >= text.length) break;
    const point = text.codePointAt(unit)!;
    byte += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
    unit += point >= 0x10000 ? 2 : 1;
  }
  return found;
}
