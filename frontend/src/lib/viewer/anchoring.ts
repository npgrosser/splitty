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
 * paragraph.
 *
 * When that fails as well, the window looks for the passage the quote most
 * likely became (D7 of review-threads): spans that start within 50 lines of
 * the recorded start line and are between half and twice as long as the
 * quote, scored by the Dice coefficient of their character trigrams with the
 * quote's, whitespace collapsed and case folded. The best span that scores at
 * least 0.6 anchors the annotation as changed. Spans start and end at word
 * boundaries, or at any character of a script written without spaces. A quote
 * of more than 4,000 characters skips this step, at most 65,536 characters
 * around the recorded lines are searched, and the work per annotation and
 * per version of the text is bounded, so that a long line or a large
 * document cannot stall the window. Otherwise the annotation is
 * stale: it keeps its recorded quote and lines and anchors again once a later
 * version contains the quote.
 *
 * Pure functions over one string. The same code serves the source text and
 * the text of the rendered view; a line map names the source lines of a
 * passage in either (`sourceLines`, `segmentLines`).
 */

/** Characters of context kept on each side of a quote. */
export const CONTEXT_CHARS = 32;
/** The most occurrences of one quote that are compared, so that a very common word cannot stall anchoring. */
export const MAX_OCCURRENCES = 100_000;
/** How far from the recorded start line a changed passage may start. */
export const SIMILAR_LINES = 50;
/** The lowest score of a changed passage. */
export const SIMILAR_SCORE = 0.6;
/** A longer quote, in characters, is not looked for by similarity. */
export const SIMILAR_MAX_QUOTE = 4_000;
/** The most characters of the text in which a changed passage may start. */
export const SIMILAR_MAX_REGION = 65_536;
/** The most span ends one annotation's search for a changed passage compares: some milliseconds. */
export const SIMILAR_STEPS_PER_QUOTE = 1_000_000;
/** The most span ends the searches against one version of a text compare together. */
export const SIMILAR_MAX_STEPS = 4_000_000;

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

/**
 * Where an annotation's passage is: anchored by its quote, at the passage it
 * most likely became (`changed`, with the passage's score), or not found.
 */
export type Anchoring =
  | { readonly state: "anchored"; readonly anchor: Anchor }
  | { readonly state: "changed"; readonly anchor: Anchor; readonly score: number }
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

// ── Similarity (D7 of review-threads) ──

/** The lower case of each UTF-16 unit outside ASCII, where it is one unit; the unit itself otherwise. */
const folded = new Map<number, number>();

function fold(code: number): number {
  if (code < 0x80) return code >= 65 && code <= 90 ? code + 32 : code;
  let lower = folded.get(code);
  if (lower === undefined) {
    const text = String.fromCharCode(code).toLowerCase();
    lower = text.length === 1 ? text.charCodeAt(0) : code;
    folded.set(code, lower);
  }
  return lower;
}

/** A script written without spaces between words: a span may start and end at any of its characters. */
const UNSPACED = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Thai}\p{Script=Lao}\p{Script=Khmer}\p{Script=Myanmar}]/u;

function isUnspaced(code: number): boolean {
  return code >= 0x0e00 && UNSPACED.test(String.fromCharCode(code));
}

/** A run of text with whitespace collapsed to one space and case folded, and the original offset of each unit. */
interface Normalized {
  readonly codes: Uint16Array;
  readonly length: number;
  /** `origin[i]` is the offset in the original text of unit `i`. */
  readonly origin: Int32Array;
}

/**
 * `text` from `from` on, normalized, until the original offset reaches
 * `until` and `extra` more units follow; or to its end.
 */
function normalized(text: string, from: number, until: number, extra: number): Normalized {
  const capacity = Math.min(text.length - from, until - from + extra + 1);
  const codes = new Uint16Array(Math.max(0, capacity));
  const origin = new Int32Array(Math.max(0, capacity));
  let length = 0;
  let afterUntil = 0;
  let inSpace = false;
  for (let i = from; i < text.length && length < capacity; i++) {
    const code = text.charCodeAt(i);
    if (isSpace(code)) {
      if (inSpace) continue;
      inSpace = true;
      codes[length] = 32;
    } else {
      inSpace = false;
      codes[length] = fold(code);
    }
    origin[length++] = i;
    if (i >= until && ++afterUntil > extra) break;
  }
  return { codes, length, origin };
}

/** The normalized quote: whitespace collapsed, trimmed and case folded. */
function normalizedQuote(quote: string): Uint16Array {
  const collapsed = collapseString(quote).trim();
  const codes = new Uint16Array(collapsed.length);
  for (let i = 0; i < collapsed.length; i++) codes[i] = fold(collapsed.charCodeAt(i));
  return codes;
}

function trigram(codes: Uint16Array, at: number): number {
  return (codes[at] * 65536 + codes[at + 1]) * 65536 + codes[at + 2];
}

/** How many characters `text` holds, counted as code points, up to `stop`. */
function charCount(text: string, stop: number): number {
  let count = 0;
  for (const _ of text) if (++count > stop) break;
  return count;
}

interface Similar {
  readonly start: number;
  readonly end: number;
  readonly score: number;
}

/** What is left of an anchorer's work for changed passages, in span ends compared. */
interface Budget {
  steps: number;
}

/**
 * The span of `text` that `quote` most likely became, near `startLine`, or
 * null when none scores at least `SIMILAR_SCORE`. Offsets are the original
 * text's, the end exclusive.
 *
 * Every start's spans share at most as many trigrams with the quote as the
 * longest of them, which bounds their score; the starts are compared in the
 * order of that bound, so that the search ends once no start left can beat
 * the best span. Ties go to the start nearest the recorded line. The search
 * also ends when its share of `budget` is spent, with the best span so far.
 */
function similarPassage(
  text: string,
  quote: string,
  startLine: number,
  lineOf: (offset: number) => number,
  budget: Budget,
): Similar | null {
  if (budget.steps <= 0 || text.length === 0 || charCount(quote, SIMILAR_MAX_QUOTE) > SIMILAR_MAX_QUOTE) return null;
  const q = normalizedQuote(quote);
  const quoteTrigrams = q.length - 2;
  if (quoteTrigrams < 1) return null;

  // The distinct trigrams of the quote and how often each occurs.
  const index = new Map<number, number>();
  const wanted: number[] = [];
  for (let i = 0; i < quoteTrigrams; i++) {
    const code = trigram(q, i);
    let slot = index.get(code);
    if (slot === undefined) {
      slot = wanted.length;
      index.set(code, slot);
      wanted.push(0);
    }
    wanted[slot]++;
  }

  // Where spans may start: within the lines around the recorded start line, bounded.
  const firstAtLine = (line: number) => {
    let low = 0;
    let high = text.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (lineOf(middle) >= line) high = middle;
      else low = middle + 1;
    }
    return low;
  };
  let regionStart = firstAtLine(startLine - SIMILAR_LINES);
  let regionEnd = firstAtLine(startLine + SIMILAR_LINES + 1);
  if (regionEnd - regionStart > SIMILAR_MAX_REGION) {
    const centre = firstAtLine(startLine);
    regionStart = Math.max(regionStart, centre - SIMILAR_MAX_REGION / 2);
    regionEnd = Math.min(regionEnd, regionStart + SIMILAR_MAX_REGION);
  }
  if (regionStart >= regionEnd) return null;

  const minLength = Math.max(3, Math.ceil(q.length / 2));
  const maxLength = q.length * 2;
  const { codes, length, origin } = normalized(text, regionStart, regionEnd, maxLength);
  if (length < minLength) return null;

  // Per position, the slot of the quote's trigram that starts there, or -1.
  const positions = Math.max(0, length - 2);
  const slots = new Int32Array(positions);
  for (let i = 0; i < positions; i++) slots[i] = index.get(trigram(codes, i)) ?? -1;
  const boundary = new Uint8Array(length + 1);
  for (let i = 0; i < length; i++) {
    const space = codes[i] === 32;
    const previousSpace = i === 0 || codes[i - 1] === 32;
    const unspaced = !space && isUnspaced(codes[i]);
    // 1: a span may start here; 2: a span may end here.
    if (!space && (previousSpace || unspaced)) boundary[i] |= 1;
    if (!space && (i + 1 === length || codes[i + 1] === 32 || unspaced)) boundary[i + 1] |= 2;
  }

  // The bound of each start, from the trigrams the quote shares with its
  // longest span, kept in a window that slides along the starts.
  const minTrigrams = minLength - 2;
  const counts = new Int32Array(wanted.length);
  const starts: number[] = [];
  const bounds: number[] = [];
  let shared = 0;
  let high = 0;
  for (let s = 0; s + minLength <= length && origin[s] < regionEnd; s++) {
    const top = Math.min(positions, s + maxLength - 2);
    if (high < s) high = s;
    for (; high < top; high++) {
      const slot = slots[high];
      if (slot >= 0 && counts[slot]++ < wanted[slot]) shared++;
    }
    if (boundary[s] & 1) {
      const bound = (2 * shared) / (quoteTrigrams + Math.max(shared, minTrigrams));
      if (bound >= SIMILAR_SCORE) {
        starts.push(s);
        bounds.push(bound);
      }
    }
    if (s < high) {
      const slot = slots[s];
      if (slot >= 0 && --counts[slot] < wanted[slot]) shared--;
    }
  }
  if (starts.length === 0) return null;
  const distances = starts.map((s) => Math.abs(lineOf(origin[s]) - startLine));
  const order = starts.map((_, i) => i).sort((a, b) => bounds[b] - bounds[a] || distances[a] - distances[b] || starts[a] - starts[b]);

  counts.fill(0);
  const touched: number[] = [];
  let best: Similar | null = null;
  let bestDistance = Infinity;
  const allowed = Math.min(budget.steps, SIMILAR_STEPS_PER_QUOTE);
  let steps = allowed;
  for (const i of order) {
    if (steps <= 0 || (best && bounds[i] < best.score)) break;
    const s = starts[i];
    const distance = distances[i];
    let common = 0;
    const last = Math.min(length, s + maxLength);
    for (let e = s + 3; e <= last; e++) {
      steps--;
      const slot = slots[e - 3];
      if (slot >= 0) {
        if (counts[slot] === 0) touched.push(slot);
        if (counts[slot]++ < wanted[slot]) common++;
      }
      const spanTrigrams = e - s - 2;
      // A longer span cannot do better than the best so far.
      if ((2 * quoteTrigrams) / (quoteTrigrams + spanTrigrams) < (best?.score ?? SIMILAR_SCORE)) break;
      if (e - s < minLength || !(boundary[e] & 2)) continue;
      const score = (2 * common) / (quoteTrigrams + spanTrigrams);
      if (score < SIMILAR_SCORE) continue;
      if (best && (score < best.score || (score === best.score && distance >= bestDistance))) continue;
      best = { start: origin[s], end: origin[e - 1] + 1, score };
      bestDistance = distance;
    }
    for (const slot of touched) counts[slot] = 0;
    touched.length = 0;
  }
  budget.steps -= allowed - steps;
  return best;
}

/** The most results an anchorer keeps. */
const MAX_REMEMBERED = 1024;

/**
 * An anchorer for one version of a text: it anchors any number of
 * annotations against `text`, whose lines `lines` names (by default those of
 * a source text). The whitespace-collapsed copy is built only when a quote
 * needs it, and each target's result is kept, so that anchoring again
 * against the same version costs nothing.
 */
export function anchorer(text: string, lines: LineRange = sourceLines(text)): (target: QuoteTarget) => Anchoring {
  const lineOf = (offset: number) => lines(offset, offset + 1).startLine;
  let collapsed: Collapsed | null = null;
  let collapsedLineOf: ((offset: number) => number) | null = null;
  const remembered = new Map<string, Anchoring>();
  const budget: Budget = { steps: SIMILAR_MAX_STEPS };

  const anchored = (start: number, end: number): Anchoring => {
    const range = lines(start, end);
    return { state: "anchored", anchor: { start, end, startLine: range.startLine, endLine: range.endLine } };
  };

  const find = (target: QuoteTarget): Anchoring => {
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
    // The collapsed quote starts and ends with a character that is not
    // whitespace, so both ends map to that very character of the original.
    if (found) return anchored(collapsed.origin[found.start], collapsed.origin[found.end - 1] + 1);

    const similar = similarPassage(text, target.quote, target.startLine, lineOf, budget);
    if (!similar) return STALE;
    const range = lines(similar.start, similar.end);
    return {
      state: "changed",
      anchor: { start: similar.start, end: similar.end, startLine: range.startLine, endLine: range.endLine },
      score: similar.score,
    };
  };

  return (target) => {
    const key = `${target.startLine}\u0000${target.prefix}\u0000${target.suffix}\u0000${target.quote}`;
    let result = remembered.get(key);
    if (!result) {
      result = find(target);
      if (remembered.size >= MAX_REMEMBERED) remembered.clear();
      remembered.set(key, result);
    }
    return result;
  };
}

/** One annotation anchored against `text`; for many, use `anchorer`. */
export function anchor(text: string, target: QuoteTarget, lines?: LineRange): Anchoring {
  return anchorer(text, lines)(target);
}
