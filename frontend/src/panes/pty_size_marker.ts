import type { SizeView, TerminalSession } from "../lib/terminal/terminal_session.ts";

/**
 * Shows that a terminal's PTY size is set by another client (terminal-sessions,
 * Client Handling of the PTY Size): the area right of and below a smaller
 * terminal is hatched with a dashed edge, a taller one moves up as far as its
 * cursor needs (as tmux does), and a badge names the size and, unless the
 * policy is `smallest`, takes it over on click.
 */

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The pane's container in CSS pixels, with the padding the fit put around the grid. */
export interface MarkerBox {
  width: number;
  height: number;
  padLeft: number;
  padTop: number;
  padRight: number;
}

export interface MarkerLayout {
  /** The hatched area right of a narrower terminal. */
  right: Rect | null;
  /** The hatched area below a shorter terminal. */
  bottom: Rect | null;
  /** How far a taller terminal moves up, so that its cursor's row stays in view. */
  shift: number;
  /** The width of a narrower terminal's element, so that its scrollbar stays at its edge. */
  width: number | null;
  label: string;
  title: string;
  takeover: boolean;
}

/**
 * Where the marker goes, for a terminal whose cursor is on `cursorRow`; null
 * while the PTY has the pane's size or is unknown.
 */
export function markerLayout(
  view: SizeView,
  cell: { width: number; height: number },
  box: MarkerBox,
  cursorRow: number,
): MarkerLayout | null {
  const { pane, pty } = view;
  if (!pane || !pty || (pane.cols === pty.cols && pane.rows === pty.rows)) return null;
  // What the pane's width holds besides its grid: a scrollbar that does not overlay it.
  const extra = Math.max(0, box.width - box.padLeft - box.padRight - pane.cols * cell.width);
  const gridRight = box.padLeft + pty.cols * cell.width + extra;
  const gridBottom = box.padTop + pty.rows * cell.height;
  const narrower = pty.cols < pane.cols && gridRight < box.width;
  const shorter = pty.rows < pane.rows && gridBottom < box.height;
  const label = `${pty.cols}×${pty.rows}`;
  const takeover = pty.policy !== "smallest";
  return {
    right: narrower ? { left: gridRight, top: 0, width: box.width - gridRight, height: box.height } : null,
    bottom: shorter
      ? { left: 0, top: gridBottom, width: narrower ? gridRight : box.width, height: box.height - gridBottom }
      : null,
    shift: Math.min(Math.max(cursorRow + 1 - pane.rows, 0), Math.max(pty.rows - pane.rows, 0)) * cell.height,
    width: narrower ? pty.cols * cell.width + extra : null,
    label,
    title: takeover
      ? `Another client set this terminal to ${label}. Click to use this window's size.`
      : `Another client shows this terminal at ${label}.`,
    takeover,
  };
}

/** Draw the marker of `session` in the pane `element`, around `container`. Returns cleanup. */
export function setupPtySizeMarker(element: HTMLElement, container: HTMLElement, session: TerminalSession): () => void {
  const overlay = document.createElement("div");
  overlay.className = "pty-size-marker";
  overlay.hidden = true;
  const right = document.createElement("div");
  right.className = "pty-size-marker-area pty-size-marker-right";
  const bottom = document.createElement("div");
  bottom.className = "pty-size-marker-area pty-size-marker-bottom";
  const badge = document.createElement("button");
  badge.className = "pty-size-marker-badge";
  overlay.append(right, bottom, badge);
  element.appendChild(overlay);

  let takeover = false;
  badge.addEventListener("mousedown", (e) => {
    // Keep the terminal's focus and dockview out of it.
    e.preventDefault();
    e.stopPropagation();
  });
  badge.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (!takeover) return;
    session.takeOver();
    session.emulator.focus();
  });

  const place = (area: HTMLElement, rect: Rect | null) => {
    area.hidden = !rect;
    if (!rect) return;
    area.style.left = `${rect.left}px`;
    area.style.top = `${rect.top}px`;
    area.style.width = `${rect.width}px`;
    area.style.height = `${rect.height}px`;
  };

  const render = (view: SizeView = session.sizeView()) => {
    const cell = session.emulator.cellSize();
    const style = getComputedStyle(container);
    const layout = cell && markerLayout(view, cell, {
      width: container.clientWidth,
      height: container.clientHeight,
      padLeft: parseFloat(style.paddingLeft) || 0,
      padTop: parseFloat(style.paddingTop) || 0,
      padRight: parseFloat(style.paddingRight) || 0,
    }, session.emulator.cursorRow);
    const terminal = session.emulator.element;
    if (terminal) {
      terminal.style.transform = layout?.shift ? `translateY(-${layout.shift}px)` : "";
      terminal.style.width = layout?.width ? `${layout.width}px` : "";
    }
    overlay.hidden = !layout;
    if (!layout) return;
    place(right, layout.right);
    place(bottom, layout.bottom);
    takeover = layout.takeover;
    badge.textContent = layout.label;
    badge.title = layout.title;
    badge.setAttribute("aria-label", layout.title);
    badge.classList.toggle("pty-size-marker-badge-static", !takeover);
  };

  const unsubscribe = session.onSizeView(render);
  // The areas follow the pane between grid changes, too.
  const observer = new ResizeObserver(() => render());
  observer.observe(container);
  // A taller terminal follows its cursor, once per frame at most.
  let frame = 0;
  const unsubOutput = session.emulator.onWriteParsed(() => {
    if (frame || overlay.hidden) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      render();
    });
  });
  return () => {
    unsubscribe();
    unsubOutput();
    cancelAnimationFrame(frame);
    observer.disconnect();
    overlay.remove();
    const terminal = session.emulator.element;
    if (terminal) {
      terminal.style.transform = "";
      terminal.style.width = "";
    }
  };
}
