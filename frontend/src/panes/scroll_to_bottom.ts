import type { TerminalEmulator } from "../lib/terminal/terminal_emulator.ts";
import { getSettings } from "../lib/settings.ts";

const SVG_ARROW =
  '<svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">' +
  '<path d="M1.5 3.5L5 7L8.5 3.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>' +
  "</svg>";

/**
 * Imperatively creates and manages a "scroll to bottom" pill for a terminal pane.
 * Shows a floating button with line count when the viewport is scrolled up.
 * Returns a cleanup function.
 */
export function setupScrollToBottom(
  container: HTMLElement,
  emulator: TerminalEmulator,
): () => void {
  let visible = false;
  let exiting = false;
  let linesBehind = 0;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;

  // Create the button element
  const btn = document.createElement("button");
  btn.className = "scroll-to-bottom";
  btn.title = "Scroll to bottom";

  const countSpan = document.createElement("span");
  countSpan.className = "scroll-to-bottom-count";

  btn.innerHTML = SVG_ARROW;
  btn.appendChild(countSpan);

  btn.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    emulator.scrollToBottom();
    emulator.focus();
  });

  btn.addEventListener("mousedown", (e) => {
    // Prevent terminal from losing focus / dockview reacting
    e.preventDefault();
    e.stopPropagation();
  });

  function formatLabel(lines: number): string {
    return lines >= 1000 ? `${Math.floor(lines / 1000)}k` : `${lines}`;
  }

  function show() {
    if (!container.contains(btn)) {
      container.appendChild(btn);
    }
    btn.classList.remove("scroll-to-bottom-exit");
    btn.setAttribute("aria-label", `Scroll to bottom (${linesBehind} lines behind)`);
    countSpan.textContent = formatLabel(linesBehind);
    visible = true;
    exiting = false;
  }

  function hide() {
    if (!visible) return;
    exiting = true;
    btn.classList.add("scroll-to-bottom-exit");
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      visible = false;
      exiting = false;
      if (container.contains(btn)) {
        container.removeChild(btn);
      }
    }, 200);
  }

  function update() {
    if (!getSettings().terminal.scrollToBottom) {
      if (visible) hide();
      return;
    }

    const gap = emulator.getScrollGap();
    if (gap > 0) {
      clearTimeout(hideTimer);
      linesBehind = gap;
      show();
    } else if (visible && !exiting) {
      hide();
    }
  }

  const unsubScroll = emulator.onScroll(update);
  const unsubWrite = emulator.onWriteParsed(update);

  // Initial check
  update();

  return () => {
    clearTimeout(hideTimer);
    unsubScroll();
    unsubWrite();
    if (container.contains(btn)) {
      container.removeChild(btn);
    }
  };
}
