import { getSettings } from "../settings.ts";
import type { Terminal } from "@xterm/xterm";

/**
 * Custom fit logic for xterm.js that replaces @xterm/addon-fit.
 *
 * - scrollbarOverlay: scrollbar width is NOT subtracted from available width.
 * - windowPaddingBalance: leftover pixels are distributed symmetrically.
 */

export interface FitPadding {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

export interface FitGrid {
  cols: number;
  rows: number;
  padding: FitPadding;
}

/** The size of one cell in CSS pixels, once the terminal has rendered. */
export function cellSize(terminal: Terminal): { width: number; height: number } | undefined {
  // Access internal render dimensions — no public API for cell metrics.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const dims = (terminal as any)._core._renderService.dimensions;
  const width: number = dims.css.cell.width;
  const height: number = dims.css.cell.height;
  return width === 0 || height === 0 ? undefined : { width, height };
}

/** Fit the terminal to its container: {@link measure}, then resize the grid. */
export function fit(terminal: Terminal): FitPadding | undefined {
  const grid = measure(terminal);
  if (!grid) return undefined;
  if (terminal.rows !== grid.rows || terminal.cols !== grid.cols) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (terminal as any)._core._renderService.clear();
    terminal.resize(grid.cols, grid.rows);
  }
  return grid.padding;
}

/**
 * The grid that fits the terminal's container. Applies the container padding
 * for that grid, but leaves the grid's size alone.
 */
export function measure(terminal: Terminal): FitGrid | undefined {
  const parent = terminal.element?.parentElement as HTMLElement | undefined;
  if (!parent) return undefined;

  const cell = cellSize(terminal);
  if (!cell) return undefined;
  const { width: cellWidth, height: cellHeight } = cell;

  const settings = getSettings().terminal;
  const xtermSettings = settings.xterm;

  const scrollbarWidth = xtermSettings.scrollbarOverlay
    ? 0
    : xtermSettings.scrollbarWidth;

  // Use offsetWidth/offsetHeight — stable regardless of padding we set on parent.
  const totalWidth = parent.offsetWidth;
  const totalHeight = parent.offsetHeight;

  // Guard: skip fit when parent has no layout (e.g. display: none from dockview
  // hiding an inactive tab).  Resizing the terminal to near-zero dimensions
  // would reflow the buffer into tiny widths, overflowing the scrollback limit
  // and permanently destroying content.
  if (totalWidth === 0 || totalHeight === 0) return undefined;

  const cols = Math.max(2, Math.floor((totalWidth - scrollbarWidth) / cellWidth));
  const rows = Math.max(1, Math.floor(totalHeight / cellHeight));

  const usedWidth = cols * cellWidth + scrollbarWidth;
  const usedHeight = rows * cellHeight;
  const extraX = totalWidth - usedWidth;
  const extraY = totalHeight - usedHeight;

  let padding: FitPadding;
  if (settings.windowPaddingBalance) {
    padding = {
      left: Math.max(0, Math.floor(extraX / 2)),
      right: Math.max(0, extraX - Math.floor(extraX / 2)),
      top: Math.max(0, Math.floor(extraY / 2)),
      bottom: Math.max(0, extraY - Math.floor(extraY / 2)),
    };
  } else {
    padding = { top: 0, right: 0, bottom: 0, left: 0 };
  }

  // Apply padding to parent so terminal.element shrinks to exactly the used size.
  parent.style.padding = `${padding.top}px ${padding.right}px ${padding.bottom}px ${padding.left}px`;

  return { cols, rows, padding };
}
