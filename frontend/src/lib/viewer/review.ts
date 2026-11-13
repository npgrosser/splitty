/**
 * What the window keeps of a viewer pane's review (`viewer-annotations`, The
 * Review): the file name a review is stored and saved under, the order of the
 * pane's list, and the text the window shows of annotations. The review's
 * text itself is written by the pane's server (`server_reviews.ts`; SBR-2 of
 * server-built-reviews).
 */

export type ReviewDocument =
  | { readonly kind: "file"; readonly path: string }
  | { readonly kind: "text"; readonly title: string };

/** The last segment of a path, for either separator. */
export function documentName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return (cut === -1 ? trimmed : trimmed.slice(cut + 1)) || path;
}

// C0 controls but tab and line feed, DEL, C1 controls, the bidirectional
// marks, embeddings, overrides and isolates, and the invisible format
// characters: zero-width characters, the line and paragraph separators, word
// joiners and invisible operators, the byte order mark, interlinear
// annotation marks and tag characters. A terminal acts on them, shows text in
// another order than it was written, or shows nothing at all.
// eslint-disable-next-line no-control-regex, no-misleading-character-class
const UNSAFE = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u061c\u200b-\u200f\u2028-\u202e\u2060-\u206f\ufeff\ufff9-\ufffb\u{e0000}-\u{e007f}]/gu;

/** `text` with line breaks as `\n` and every other control or invisible format character as U+FFFD, as the window shows text of an annotation. */
export function safeText(text: string): string {
  return text.replace(/\r\n?/g, "\n").replace(UNSAFE, "\ufffd");
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

const STATE_RANK = { general: 0, anchored: 1, stale: 2 } as const;
const VIEW_RANK = { source: 0, rendered: 1 } as const;

/**
 * The order of the pane's list and its review: the general comments first
 * (D5 of review-verdict), then the anchored annotations in document order (by
 * line, and within one view by position; on one line the source view's
 * first), then those not found by their recorded lines, each group in
 * creation order where nothing else decides.
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
