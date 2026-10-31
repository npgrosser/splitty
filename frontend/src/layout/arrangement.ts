/**
 * The arrangement of a screen's grid, as `arrange_panes` sends it
 * (mux-protocol, Window Layout Changes Reach the Server).
 *
 * Dockview's grid and the server's layout tree can show the same picture with
 * different trees: the server nests splits of the same axis where dockview
 * keeps one row, and dockview counts sizes in pixels. Both are brought into
 * one form here (fractions, no single-child splits, no nested splits of the
 * same axis) before they are compared.
 */

import type { Arrangement, LayoutNode } from "../lib/protocol/types.ts";

/** Two sizes closer than this are the same; dockview rounds to pixels. */
const SIZE_TOLERANCE = 0.005;

type Axis = "horizontal" | "vertical";

/** The part of dockview's `toJSON().grid` read here. */
export interface SerializedGrid {
  root: GridObject;
  orientation: string;
}

interface GridObject {
  type: "leaf" | "branch";
  data: unknown;
  size?: number;
}

/** The empty grid: every pane floats, or there is none. */
const EMPTY: Arrangement = { type: "slot", paneIds: [] };

/**
 * The grid of dockview's `toJSON()`. Its root lays out its children along the
 * grid's orientation, and every branch below along the other one. Floating
 * groups are not part of the grid.
 */
export function gridArrangement(grid: SerializedGrid): Arrangement {
  return fromGrid(grid.root, grid.orientation === "HORIZONTAL" ? "horizontal" : "vertical") ?? EMPTY;
}

/** A snapshot's root layout in the same form. */
export function layoutArrangement(root: LayoutNode): Arrangement {
  return fromLayout(root) ?? EMPTY;
}

/**
 * Whether two arrangements show the same grid: the same rows, columns and
 * tabs in the same order, sizes within a tolerance. An active tab counts
 * only when both name one; the server keeps none for a slot no one chose.
 */
export function sameArrangement(a: Arrangement, b: Arrangement): boolean {
  if (a.type === "slot" || b.type === "slot") {
    if (a.type !== "slot" || b.type !== "slot") return false;
    return a.paneIds.length === b.paneIds.length
      && a.paneIds.every((id, i) => id === b.paneIds[i])
      && (!a.activePaneId || !b.activePaneId || a.activePaneId === b.activePaneId);
  }
  return a.axis === b.axis
    && a.children.length === b.children.length
    && a.sizes.every((size, i) => Math.abs(size - b.sizes[i]) <= SIZE_TOLERANCE)
    && a.children.every((child, i) => sameArrangement(child, b.children[i]));
}

/** The slots of an arrangement, depth first. */
export function arrangementSlots(root: Arrangement): Extract<Arrangement, { type: "slot" }>[] {
  return root.type === "slot" ? [root] : root.children.flatMap(arrangementSlots);
}

/** The arrangement with `paneId` as the active tab of the slot that holds it. */
export function withActivePane(root: Arrangement, paneId: string): Arrangement {
  if (root.type === "slot") return root.paneIds.includes(paneId) ? { ...root, activePaneId: paneId } : root;
  return { ...root, children: root.children.map(child => withActivePane(child, paneId)) };
}

function fromGrid(node: GridObject, axis: Axis): Arrangement | null {
  if (node.type === "leaf") {
    const group = (node.data ?? {}) as { views?: unknown; activeView?: unknown };
    const paneIds = Array.isArray(group.views) ? group.views.filter((id): id is string => typeof id === "string") : [];
    return slot(paneIds, typeof group.activeView === "string" ? group.activeView : undefined);
  }
  const children = Array.isArray(node.data) ? (node.data as GridObject[]) : [];
  const other: Axis = axis === "horizontal" ? "vertical" : "horizontal";
  return split(axis, children.map(child => ({ node: fromGrid(child, other), size: child.size ?? 0 })));
}

function fromLayout(node: LayoutNode): Arrangement | null {
  if (node.type === "slot") return slot(node.paneIds ?? [], node.activePaneId);
  const children = node.children ?? [];
  const sizes = node.sizes?.length === children.length ? node.sizes : children.map(() => 1);
  return split(node.axis ?? "horizontal", children.map((child, i) => ({ node: fromLayout(child), size: sizes[i] })));
}

function slot(paneIds: string[], active: string | undefined): Arrangement | null {
  if (paneIds.length === 0) return null;
  return active && paneIds.includes(active)
    ? { type: "slot", paneIds, activePaneId: active }
    : { type: "slot", paneIds };
}

/**
 * A split of the children that hold panes, sizes as fractions; a child split
 * of the same axis gives its children to this one, with their share of its size.
 */
function split(axis: Axis, parts: { node: Arrangement | null; size: number }[]): Arrangement | null {
  const kept = parts.filter((part): part is { node: Arrangement; size: number } => part.node !== null);
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0].node;
  const total = kept.reduce((sum, part) => sum + Math.max(part.size, 0), 0);
  const children: Arrangement[] = [];
  const sizes: number[] = [];
  for (const { node, size } of kept) {
    const share = total > 0 ? Math.max(size, 0) / total : 1 / kept.length;
    if (node.type === "split" && node.axis === axis) {
      children.push(...node.children);
      sizes.push(...node.sizes.map(inner => inner * share));
    } else {
      children.push(node);
      sizes.push(share);
    }
  }
  return { type: "split", axis, children, sizes };
}
