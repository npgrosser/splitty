/**
 * Anchoring viewer annotations by quote (`viewer-annotations`, Anchoring By
 * Quote).
 *
 * An annotation keeps the text it was made on, a little context on both sides
 * and the source lines of the passage. When the document changes, the passage
 * is found again by that quote. Where the quote occurs more than once, the
 * occurrence whose context matches best wins, and among equal ones the one
 * nearest the recorded lines. When the quote does not occur, the search runs
 * again with runs of whitespace treated as one space, which finds a reflowed
 * paragraph. If the quote is still not found, the annotation is stale: it
 * keeps its recorded quote and lines and anchors again once a later version
 * contains the quote.
 *
 * Pure functions over one string. The same code serves the source text and
 * the text of the rendered view; a line map names the source lines of a
 * passage in either (`sourceLines`, `segmentLines`).
 */

/** Characters of context kept on each side of a quote. */
export const CONTEXT_CHARS = 32;
/** The most occurrences of one quote that are compared, so that a very common word cannot stall anchoring. */
export const MAX_OCCURRENCES = 100_000;

/** What an annotation remembers of its passage. */
export interface QuoteTarget {
  readonly quote: string;
  readonly prefix: string;
  readonly suffix: string;
  /** 1-based source lines of the passage when it was annotated. */
  readonly startLine: number;
  readonly endLine: number;
}

/** Where a passage is in the current text. */
export interface Anchor {
  /** UTF-16 offsets of the passage in the text, end exclusive. */
  readonly start: number;
  readonly end: number;
  /** 1-based source lines of the passage. */
  readonly startLine: number;
  readonly endLine: number;
}

export type Anchoring =
  | { readonly state: "anchored"; readonly anchor: Anchor }
  | { readonly state: "stale" };

/** The 1-based source lines of the passage `[start, end)` of a text. */
export type LineRange = (start: number, end: number) => { startLine: number; endLine: number };

const STALE: Anchoring = { state: "stale" };

/** The offset at which each line of `text` starts. */
export function lineStarts(text: string): number[] {
  const starts = [0];
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1);
  return starts;
}

/** The 1-based line that holds `offset`. */
export function lineAt(starts: readonly number[], offset: number): number {
  let low = 0;
  let high = starts.length - 1;
  while (low < high) {
    const middle = (low + high + 1) >> 1;
    if (starts[middle] <= offset) low = middle;
    else high = middle - 1;
  }
  return low + 1;
}

/** Line numbers of a source text: a passage ends on the line of its last character. */
export function sourceLines(text: string): LineRange {
  const starts = lineStarts(text);
  return (start, end) => ({
    startLine: lineAt(starts, start),
    endLine: lineAt(starts, Math.max(start, end - 1)),
  });
}

/**
 * The text that source annotations anchor in, as the source view shows it:
 * every line break as `\n`, as CodeMirror keeps them.
 */
export function sourceText(text: string): string {
  return text.includes("\r") ? text.replace(/\r\n?/g, "\n") : text;
}

/** The number of a source text's last line; a final line break starts none. */
export function lastLine(text: string): number {
  let lines = 1;
  for (let i = text.indexOf("\n"); i !== -1 && i + 1 < text.length; i = text.indexOf("\n", i + 1)) lines++;
  return lines;
}

/** A run of a rendered view's text and the source lines of the block it is in. */
export interface LineSegment {
  /** Where the run starts in the text; it lasts until the next run starts. */
  readonly start: number;
  /** The block's source lines, 1-based and inclusive; null where no block names them. */
  readonly lines: { readonly start: number; readonly end: number } | null;
}

/**
 * Line numbers of a rendered view from its runs, in text order: a passage
 * starts on the first line of the run it starts in and ends on the last line
 * of the run it ends in. A run without lines, such as the line break between
 * two blocks, takes those of the next run at a passage's start and of the
 * previous one at its end. Without any lines, as for an HTML page, every
 * passage spans lines 1 to `last`; lines past `last` become `last`.
 */
export function segmentLines(segments: readonly LineSegment[], last: number): LineRange {
  const count = segments.length;
  // The nearest run with lines at or after, and at or before, each run.
  const next = new Int32Array(count + 1).fill(-1);
  const previous = new Int32Array(count).fill(-1);
  for (let i = count - 1; i >= 0; i--) next[i] = segments[i].lines ? i : next[i + 1];
  for (let i = 0; i < count; i++) previous[i] = segments[i].lines ? i : i > 0 ? previous[i - 1] : -1;
  const runAt = (offset: number) => {
    let low = 0;
    let high = count - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (segments[middle].start <= offset) low = middle;
      else high = middle - 1;
    }
    return low;
  };
  const clamp = (line: number) => Math.max(1, Math.min(last, line));
  const whole = { startLine: 1, endLine: Math.max(1, last) };
  return (start, end) => {
    if (count === 0) return whole;
    const first = runAt(start);
    const final = runAt(Math.max(start, end - 1));
    const opening = next[first] !== -1 ? next[first] : previous[first];
    const closing = previous[final] !== -1 ? previous[final] : next[final];
    if (opening === -1 || closing === -1) return whole;
    const startLine = clamp(segments[opening].lines!.start);
    return { startLine, endLine: Math.max(startLine, clamp(segments[closing].lines!.end)) };
  };
}

function isHighSurrogate(code: number): boolean {
  return code >= 0xd800 && code <= 0xdbff;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

/** `offset` moved forward if it would split a surrogate pair. */
function boundaryAfter(text: string, offset: number): number {
  return offset > 0 && offset < text.length && isLowSurrogate(text.charCodeAt(offset)) && isHighSurrogate(text.charCodeAt(offset - 1))
    ? offset + 1
    : offset;
}

/** `offset` moved back if it would split a surrogate pair. */
function boundaryBefore(text: string, offset: number): number {
  return offset > 0 && offset < text.length && isLowSurrogate(text.charCodeAt(offset)) && isHighSurrogate(text.charCodeAt(offset - 1))
    ? offset - 1
    : offset;
}

/** Whether the UTF-16 unit `code` is whitespace in the sense of `\s`. */
function isSpace(code: number): boolean {
  if (code <= 32) return code === 32 || (code >= 9 && code <= 13);
  if (code < 0xa0) return false;
  return code === 0xa0 || code === 0x1680 || (code >= 0x2000 && code <= 0x200a) || code === 0x2028 || code === 0x2029
    || code === 0x202f || code === 0x205f || code === 0x3000 || code === 0xfeff;
}

/**
 * The target of a selection `[start, end)` of `text`, without the whitespace
 * at its ends, or null when the selection holds only whitespace.
 */
export function targetAt(text: string, start: number, end: number, lines: LineRange): QuoteTarget | null {
  let from = Math.max(0, Math.min(start, end));
  let to = Math.min(text.length, Math.max(start, end));
  while (from < to && isSpace(text.charCodeAt(from))) from++;
  while (to > from && isSpace(text.charCodeAt(to - 1))) to--;
  if (from === to) return null;
  const { startLine, endLine } = lines(from, to);
  return {
    quote: text.slice(from, to),
    prefix: text.slice(boundaryAfter(text, Math.max(0, from - CONTEXT_CHARS)), from),
    suffix: text.slice(to, boundaryBefore(text, Math.min(text.length, to + CONTEXT_CHARS))),
    startLine,
    endLine,
  };
}

/** How many characters of context around `[start, end)` match, counted outward from the passage. */
function contextScore(text: string, start: number, end: number, prefix: string, suffix: string): number {
  let score = 0;
  for (let i = 1; i <= prefix.length && start - i >= 0; i++) {
    if (text.charCodeAt(start - i) !== prefix.charCodeAt(prefix.length - i)) break;
    score++;
  }
  for (let i = 0; i < suffix.length && end + i < text.length; i++) {
    if (text.charCodeAt(end + i) !== suffix.charCodeAt(i)) break;
    score++;
  }
  return score;
}

interface Candidate {
  readonly start: number;
  readonly end: number;
}

/**
 * The best occurrence of `quote` in `text`: the most matching context, then
 * the nearest line to `startLine`, then the first.
 */
function bestOccurrence(
  text: string,
  quote: string,
  prefix: string,
  suffix: string,
  startLine: number,
  lineOf: (offset: number) => number,
): Candidate | null {
  let best: Candidate | null = null;
  let bestScore = -1;
  let bestDistance = Infinity;
  let seen = 0;
  for (let at = text.indexOf(quote); at !== -1 && seen < MAX_OCCURRENCES; at = text.indexOf(quote, at + 1), seen++) {
    const score = contextScore(text, at, at + quote.length, prefix, suffix);
    if (score < bestScore) continue;
    const distance = Math.abs(lineOf(at) - startLine);
    if (score > bestScore || distance < bestDistance) {
      best = { start: at, end: at + quote.length };
      bestScore = score;
      bestDistance = distance;
    }
  }
  return best;
}

/** `text` with every run of whitespace as one space, and the original offset of each character. */
interface Collapsed {
  readonly text: string;
  /** `origin[i]` is the offset in the original text of character `i`; one more entry holds the original length. */
  readonly origin: Uint32Array;
}

function collapse(text: string): Collapsed {
  const origin = new Uint32Array(text.length + 1);
  const chunks: string[] = [];
  let length = 0;
  let inSpace = false;
  let runStart = 0;
  for (let i = 0; i < text.length; i++) {
    if (isSpace(text.charCodeAt(i))) {
      if (!inSpace) {
        chunks.push(text.slice(runStart, i), " ");
        origin[length++] = i;
        inSpace = true;
      }
      runStart = i + 1;
    } else {
      inSpace = false;
      origin[length++] = i;
    }
  }
  chunks.push(text.slice(runStart));
  origin[length] = text.length;
  return { text: chunks.join(""), origin: origin.subarray(0, length + 1) };
}

function collapseString(text: string): string {
  return text.replace(/\s+/g, " ");
}

/**
 * An anchorer for one version of a text: it anchors any number of
 * annotations against `text`, whose lines `lines` names (by default those of
 * a source text). The whitespace-collapsed copy is built only when a quote
 * needs it.
 */
export function anchorer(text: string, lines: LineRange = sourceLines(text)): (target: QuoteTarget) => Anchoring {
  const lineOf = (offset: number) => lines(offset, offset + 1).startLine;
  let collapsed: Collapsed | null = null;
  let collapsedLineOf: ((offset: number) => number) | null = null;

  const anchored = (start: number, end: number): Anchoring => {
    const range = lines(start, end);
    return { state: "anchored", anchor: { start, end, startLine: range.startLine, endLine: range.endLine } };
  };

  return (target) => {
    if (!target.quote) return STALE;
    const exact = bestOccurrence(text, target.quote, target.prefix, target.suffix, target.startLine, lineOf);
    if (exact) return anchored(exact.start, exact.end);

    const quote = collapseString(target.quote).trim();
    if (!quote) return STALE;
    if (!collapsed) {
      collapsed = collapse(text);
      const origin = collapsed.origin;
      collapsedLineOf = (offset) => lineOf(origin[offset]);
    }
    const found = bestOccurrence(
      collapsed.text,
      quote,
      collapseString(target.prefix),
      collapseString(target.suffix),
      target.startLine,
      collapsedLineOf!,
    );
    if (!found) return STALE;
    // The collapsed quote starts and ends with a character that is not
    // whitespace, so both ends map to that very character of the original.
    return anchored(collapsed.origin[found.start], collapsed.origin[found.end - 1] + 1);
  };
}

/** One annotation anchored against `text`; for many, use `anchorer`. */
export function anchor(text: string, target: QuoteTarget, lines?: LineRange): Anchoring {
  return anchorer(text, lines)(target);
}
