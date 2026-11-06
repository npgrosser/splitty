/**
 * Which annotation a click on the document's marks chooses
 * (`viewer-annotations`, Annotations In The Pane; D1 of
 * viewer-clickable-marks): of the marks under the pointer, a passage marked
 * in the view itself before the lines a passage of the other view covers,
 * and the shortest first. A click on the same marks as the click before goes
 * on to the next of them, so that every one of overlapping passages can be
 * reached.
 */

/** A mark under the pointer. */
export interface MarkHit {
  readonly id: string;
  /** Marked by its lines only: a passage of the other view, or a bar beside its lines. */
  readonly lines: boolean;
  /** How long the passage is: characters of a passage, lines of one marked by its lines. */
  readonly length: number;
  /** Where it starts, in the same unit, which orders equally long ones. */
  readonly start: number;
}

/** The marks a click chose from, in their order, and the one it chose. */
export interface MarkPick {
  readonly ids: readonly string[];
  readonly chosen: string;
}

/** The IDs of `hits` in the order clicks go through them. */
export function hitOrder(hits: readonly MarkHit[]): string[] {
  const unique = new Map<string, MarkHit>();
  for (const hit of hits) if (!unique.has(hit.id)) unique.set(hit.id, hit);
  return [...unique.values()]
    .sort((a, b) => Number(a.lines) - Number(b.lines) || a.length - b.length || a.start - b.start || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
    .map((hit) => hit.id);
}

function sameIds(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false;
  const known = new Set(a);
  return b.every((id) => known.has(id));
}

/**
 * The click on `hits`: the first of them, or after `last` on the same marks
 * the one that follows its choice; undefined without a mark.
 */
export function pickMark(hits: readonly MarkHit[], last?: MarkPick): MarkPick | undefined {
  const ids = hitOrder(hits);
  if (ids.length === 0) return undefined;
  const at = last && sameIds(last.ids, ids) ? ids.indexOf(last.chosen) : -1;
  return { ids, chosen: ids[at >= 0 ? (at + 1) % ids.length : 0] };
}
