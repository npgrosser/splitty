/**
 * The review of a viewer pane (`viewer-annotations`, The Review): its
 * annotations as one Markdown text that an agent reads without explanation.
 *
 * A heading names the document, a file also by its absolute path. The
 * annotations on passages follow, numbered in the order of the pane's list:
 * each one's lines, its kind, its passage as a block quote, the remark, and
 * for a suggested change the replacement in a fence the replacement cannot
 * close. General comments come last. Nothing else is added; a general comment
 * is the place for an instruction to the agent.
 *
 * The review goes into a terminal as a paste. Control characters of a quote
 * or remark, which could end a bracketed paste early or reorder what the
 * terminal shows, become U+FFFD; tabs and line breaks stay.
 */

import type { AnnotationKind } from "../protocol/types.ts";

export type { AnnotationKind } from "../protocol/types.ts";

export type ReviewDocument =
  | { readonly kind: "file"; readonly path: string }
  | { readonly kind: "text"; readonly title: string };

/** One annotation as the review shows it, already anchored by the caller. */
export interface ReviewEntry {
  readonly kind: AnnotationKind;
  readonly body?: string;
  readonly replacement?: string;
  /** The annotated passage; absent for a general comment. */
  readonly passage?: {
    readonly quote: string;
    /** The lines to name, or null for a passage of a rendered HTML page, which has no source lines. */
    readonly lines: { readonly start: number; readonly end: number } | null;
    /** Whether the passage was not found in the current version; `lines` are then the recorded ones. */
    readonly stale: boolean;
  };
}

const KIND_LABELS: Record<AnnotationKind, string> = {
  comment: "comment",
  question: "question",
  change: "suggested change",
  delete: "delete",
};

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

// C0 controls but tab and line feed, DEL, C1 controls, and the bidirectional
// embeddings, overrides and isolates.
// eslint-disable-next-line no-control-regex
const UNSAFE = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/g;

/** `text` with line breaks as `\n` and every other control character as U+FFFD. */
function normalizeNewlines(text: string): string {
  return text.replace(/\r\n?/g, "\n").replace(UNSAFE, "\ufffd");
}

function blockQuote(text: string): string {
  return normalizeNewlines(text)
    .split("\n")
    .map((line) => (line.trimEnd() ? `> ${line.trimEnd()}` : ">"))
    .join("\n");
}

function linesLabel(lines: { start: number; end: number }): string {
  return lines.start === lines.end ? `Line ${lines.start}` : `Lines ${lines.start}-${lines.end}`;
}

function remark(text: string | undefined): string {
  return normalizeNewlines(text ?? "").trim();
}

function passageSection(number: number, entry: ReviewEntry): string {
  const passage = entry.passage!;
  const where = passage.lines ? linesLabel(passage.lines) : "Passage";
  const note = passage.stale ? " (passage not found in the current version)" : "";
  const parts = [`## ${number}. ${where}, ${KIND_LABELS[entry.kind]}${note}\n${blockQuote(passage.quote)}`];
  const body = remark(entry.body);
  if (body) parts.push(body);
  if (entry.kind === "change") {
    const replacement = normalizeNewlines(entry.replacement ?? "");
    const fence = fenceFor(replacement);
    parts.push(`Replace with:\n${fence}\n${replacement.replace(/\n$/, "")}\n${fence}`);
  }
  return parts.join("\n\n");
}

function generalItem(entry: ReviewEntry): string {
  const body = remark(entry.body);
  const text = entry.kind === "question" ? `Question: ${body}` : body;
  return `- ${text.split("\n").map((line, i) => (i === 0 || !line ? line : `  ${line}`)).join("\n")}`;
}

/** The review of `entries`, given in the order of the pane's list. */
export function reviewMarkdown(document: ReviewDocument, entries: readonly ReviewEntry[]): string {
  const sections: string[] = [];
  if (document.kind === "file") {
    const path = normalizeNewlines(document.path);
    sections.push(`# Review of ${documentName(path)}`, `Document: ${path}`);
  } else {
    sections.push(`# Review of ${normalizeNewlines(document.title)}`);
  }

  let number = 0;
  for (const entry of entries) {
    if (entry.passage) sections.push(passageSection(++number, entry));
  }

  const general = entries.filter((entry) => !entry.passage && remark(entry.body));
  if (general.length > 0) {
    sections.push("## General comments", general.map(generalItem).join("\n"));
  }
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
  /** Anchored in the current version, not found in it, or a general comment. */
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
