/**
 * The review of a viewer pane (`viewer-annotations`, The Review; D8 of
 * review-threads): annotations as one Markdown text that an agent reads
 * without explanation.
 *
 * A heading names the document, a file also by its absolute path. The
 * annotations on passages follow in the order of the pane's list, each under
 * its number: its lines, its kind, its passage as a block quote, the remark,
 * for a suggested change the replacement in a fence the replacement cannot
 * close, and its replies with their authors. A change with an empty
 * replacement is a suggested deletion. General comments come after them, and
 * a last line names the command with which an agent replies in a thread: the
 * file's path, or the viewer pane's ID for a text. Nothing else is added.
 *
 * The review goes into a terminal as a paste. Control characters of a quote,
 * remark or reply, which could end a bracketed paste early or reorder what the
 * terminal shows, become U+FFFD; tabs and line breaks stay.
 */

import type { AnnotationKind } from "../protocol/types.ts";
import { quoteUploadPath } from "../terminal/image_paste.ts";

export type { AnnotationKind } from "../protocol/types.ts";

export type ReviewDocument =
  | { readonly kind: "file"; readonly path: string }
  | { readonly kind: "text"; readonly title: string };

/** A reply as the review names it: the user's, or an agent's with the title of its pane. */
export interface ReviewReply {
  readonly author: "user" | "agent";
  /** The title of an agent's pane. */
  readonly title?: string;
  readonly body: string;
}

/** One annotation as the review shows it, already anchored by the caller. */
export interface ReviewEntry {
  /** The number the server gave it; absent until the window knows it. */
  readonly number?: number;
  readonly kind: AnnotationKind;
  readonly body?: string;
  readonly replacement?: string;
  /** The annotated passage; absent for a general comment. */
  readonly passage?: {
    readonly quote: string;
    /** The lines to name, or null for a passage of a rendered HTML page, which has no source lines. */
    readonly lines: { readonly start: number; readonly end: number } | null;
    /**
     * Found by its quote, found as the passage it most likely became, or not
     * found in the current version; `lines` are the recorded ones unless it
     * was found by its quote.
     */
    readonly state: "anchored" | "changed" | "stale";
  };
  readonly replies?: readonly ReviewReply[];
}

/** The last segment of a path, for either separator. */
export function documentName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return (cut === -1 ? trimmed : trimmed.slice(cut + 1)) || path;
}

/** A code fence longer than any run of backticks in `text`, at least three. */
export function fenceFor(text: string): string {
  let longest = 0;
  for (const run of text.match(/`+/g) ?? []) longest = Math.max(longest, run.length);
  return "`".repeat(Math.max(3, longest + 1));
}

// C0 controls but tab and line feed, DEL, C1 controls, the bidirectional
// marks, embeddings, overrides and isolates, and the invisible format
// characters: zero-width characters, the line and paragraph separators, word
// joiners and invisible operators, the byte order mark, interlinear
// annotation marks and tag characters. A terminal acts on them, shows text in
// another order than it was written, or shows nothing at all.
// eslint-disable-next-line no-control-regex, no-misleading-character-class
const UNSAFE = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u061c\u200b-\u200f\u2028-\u202e\u2060-\u206f\ufeff\ufff9-\ufffb\u{e0000}-\u{e007f}]/gu;

/** `text` with line breaks as `\n` and every other control or invisible format character as U+FFFD. */
export function safeText(text: string): string {
  return text.replace(/\r\n?/g, "\n").replace(UNSAFE, "\ufffd");
}

function blockQuote(text: string): string {
  return safeText(text)
    .split("\n")
    .map((line) => (line.trimEnd() ? `> ${line.trimEnd()}` : ">"))
    .join("\n");
}

function linesLabel(lines: { start: number; end: number }): string {
  return lines.start === lines.end ? `Line ${lines.start}` : `Lines ${lines.start}-${lines.end}`;
}

function remark(text: string | undefined): string {
  return safeText(text ?? "").trim();
}

/** How the review names an annotation's kind. */
export function kindLabel(entry: Pick<ReviewEntry, "kind" | "replacement">): string {
  if (entry.kind === "comment") return "comment";
  return entry.replacement ? "suggested change" : "suggested deletion";
}

/** How the review names the author of a reply: `User`, or `Agent (<title>)`. */
export function authorLabel(reply: Pick<ReviewReply, "author" | "title">): string {
  if (reply.author === "user") return "User";
  const title = safeText(reply.title ?? "").replace(/\s+/g, " ").trim();
  return title ? `Agent (${title})` : "Agent";
}

/** A reply under its author, its further lines indented, so that none of them can pass for another reply or a section. */
function replyText(reply: ReviewReply): string {
  return `${authorLabel(reply)}: ${indented(remark(reply.body))}`;
}

const NOTES = {
  anchored: "",
  changed: " (passage changed since)",
  stale: " (passage not found in the current version)",
} as const;

function numbered(entry: ReviewEntry): string {
  return entry.number !== undefined ? `#${entry.number} ` : "";
}

function passageSection(entry: ReviewEntry): string {
  const passage = entry.passage!;
  const where = passage.lines ? linesLabel(passage.lines) : "Passage";
  const parts = [`## ${numbered(entry)}${where}, ${kindLabel(entry)}${NOTES[passage.state]}\n${blockQuote(passage.quote)}`];
  const body = remark(entry.body);
  if (body) parts.push(body);
  if (entry.kind === "change" && entry.replacement) {
    const replacement = safeText(entry.replacement);
    const fence = fenceFor(replacement);
    parts.push(`Replace with:\n${fence}\n${replacement.replace(/\n$/, "")}\n${fence}`);
  }
  for (const reply of entry.replies ?? []) parts.push(replyText(reply));
  return parts.join("\n\n");
}

/** Every line but the first indented, so that it stays in its list item. */
function indented(text: string): string {
  return text.split("\n").map((line, i) => (i === 0 || !line ? line : `  ${line}`)).join("\n");
}

function generalItem(entry: ReviewEntry): string {
  const parts = [`- ${indented(`${numbered(entry)}${remark(entry.body)}`)}`];
  for (const reply of entry.replies ?? []) parts.push(`  ${indented(replyText(reply))}`);
  return parts.join("\n\n");
}

/** How the CLI names a review's document: by its viewer pane's ID. */
export interface ReviewOptions {
  /** The viewer pane, which names the document for `splitty comments`. */
  readonly paneId: string;
}

/**
 * The last line: the command with which an agent replies in a thread of the
 * document. It names the viewer pane, so that a reply to an older review of
 * a file shown again in another pane fails instead of landing in a thread of
 * the same number there; the text goes in single quotes, in which a shell
 * expands nothing.
 */
export function replyLine(_document: ReviewDocument, paneId: string): string {
  return `Reply in a thread: splitty comments reply ${quoteUploadPath(safeText(paneId))} <number> '<text>'`;
}

/** The review of `entries`, given in the order of the pane's list. */
export function reviewMarkdown(document: ReviewDocument, entries: readonly ReviewEntry[], options: ReviewOptions): string {
  const sections: string[] = [];
  if (document.kind === "file") {
    const path = safeText(document.path);
    sections.push(`# Review of ${documentName(path)}`, `Document: ${path}`);
  } else {
    sections.push(`# Review of ${safeText(document.title)}`);
  }

  for (const entry of entries) {
    if (entry.passage) sections.push(passageSection(entry));
  }

  const general = entries.filter((entry) => !entry.passage && remark(entry.body));
  if (general.length > 0) {
    sections.push("## General comments", general.map(generalItem).join("\n"));
  }
  sections.push(replyLine(document, options.paneId));
  return `${sections.join("\n\n")}\n`;
}

const MARKDOWN_EXTENSION = /\.(?:md|markdown|mdown)$/i;

/**
 * The file name a review is stored and saved under: `review-` and the
 * document's name, a Markdown file's without its extension, in letters,
 * digits, `.`, `_` and `-` only, so that its path needs no quoting at a prompt.
 */
export function reviewFileName(document: ReviewDocument): string {
  const name = document.kind === "file" ? documentName(document.path) : document.title;
  const plain = name.replace(MARKDOWN_EXTENSION, "").replace(/[^\p{L}\p{N}._-]+/gu, "-").replace(/^[-.]+|-+$/g, "").slice(0, 96);
  return `review-${plain || "document"}.md`;
}

/** Where an annotation stands in the pane's list (`viewer-annotations`, Annotations In The Pane). */
export interface ListPlace {
  /** Anchored in the current version (also as a changed passage), not found in it, or a general comment. */
  readonly state: "anchored" | "stale" | "general";
  /** The view whose text `offset` is in. */
  readonly view?: "source" | "rendered";
  /** Where the passage starts in its view's text, for an anchored annotation. */
  readonly offset?: number;
  /** Its current lines, or the recorded ones for a passage not found. */
  readonly startLine?: number;
  readonly endLine?: number;
  /** Its place in creation order. */
  readonly index: number;
}

const STATE_RANK = { anchored: 0, stale: 1, general: 2 } as const;
const VIEW_RANK = { source: 0, rendered: 1 } as const;

/**
 * The order of the pane's list and its review: the anchored annotations in
 * document order (by line, and within one view by position; on one line the
 * source view's first), then those not found by their recorded lines, then
 * the general comments, each group in creation order where nothing else
 * decides.
 */
export function compareListPlaces(a: ListPlace, b: ListPlace): number {
  const rank = STATE_RANK[a.state] - STATE_RANK[b.state];
  if (rank !== 0) return rank;
  if (a.state !== "general") {
    const line = (a.startLine ?? 0) - (b.startLine ?? 0);
    if (line !== 0) return line;
    if (a.state === "anchored") {
      const view = VIEW_RANK[a.view ?? "source"] - VIEW_RANK[b.view ?? "source"];
      if (view !== 0) return view;
      const offset = (a.offset ?? 0) - (b.offset ?? 0);
      if (offset !== 0) return offset;
    }
    const end = (a.endLine ?? 0) - (b.endLine ?? 0);
    if (end !== 0) return end;
  }
  return a.index - b.index;
}
