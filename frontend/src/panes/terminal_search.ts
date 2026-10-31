import type { TerminalEmulator } from "../lib/terminal/terminal_emulator.ts";
import { isMac } from "../lib/shortcuts.ts";

const SVG_UP =
  '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">' +
  '<path d="M2.5 7.5L6 4L9.5 7.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>' +
  "</svg>";

const SVG_DOWN =
  '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">' +
  '<path d="M2.5 4.5L6 8L9.5 4.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>' +
  "</svg>";

const SVG_CLOSE =
  '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">' +
  '<path d="M3 3L9 9M9 3L3 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>' +
  "</svg>";

const SVG_REGEX =
  '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">' +
  '<text x="1" y="11" font-size="10" font-family="monospace" fill="currentColor">.*</text>' +
  "</svg>";

const SVG_CASE =
  '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">' +
  '<text x="0" y="11" font-size="9" font-weight="bold" font-family="monospace" fill="currentColor">Aa</text>' +
  "</svg>";

const SVG_WORD =
  '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">' +
  '<text x="-0.5" y="11" font-size="9" font-weight="bold" font-family="monospace" fill="currentColor">ab</text>' +
  '<rect x="0" y="12" width="13" height="1.2" rx="0.5" fill="currentColor"/>' +
  "</svg>";

/**
 * Imperatively creates and manages a search bar overlay for a terminal pane.
 * Returns an object with show/hide/toggle + cleanup.
 */
export function setupTerminalSearch(
  container: HTMLElement,
  emulator: TerminalEmulator,
) {
  let visible = false;
  let resultIndex = -1;
  let resultCount = 0;
  let unsubResults: (() => void) | null = null;

  // Options state
  let useRegex = false;
  let caseSensitive = false;
  let wholeWord = false;

  // ── Build DOM ──

  const bar = document.createElement("div");
  bar.className = "terminal-search";

  const input = document.createElement("input");
  input.type = "text";
  input.className = "terminal-search-input";
  input.placeholder = "Search…";
  input.spellcheck = false;
  input.autocomplete = "off";

  const toggleGroup = document.createElement("div");
  toggleGroup.className = "terminal-search-toggles";

  function makeToggle(svg: string, title: string, getter: () => boolean, setter: (v: boolean) => void): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.className = "terminal-search-toggle";
    btn.innerHTML = svg;
    btn.title = title;
    btn.tabIndex = -1;
    btn.addEventListener("mousedown", (e) => { e.preventDefault(); e.stopPropagation(); });
    btn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      setter(!getter());
      btn.classList.toggle("active", getter());
      triggerSearch();
    });
    return btn;
  }

  const regexBtn = makeToggle(SVG_REGEX, "Regex", () => useRegex, (v) => { useRegex = v; });
  const caseBtn = makeToggle(SVG_CASE, "Case sensitive", () => caseSensitive, (v) => { caseSensitive = v; });
  const wordBtn = makeToggle(SVG_WORD, "Whole word", () => wholeWord, (v) => { wholeWord = v; });
  toggleGroup.append(regexBtn, caseBtn, wordBtn);

  const countSpan = document.createElement("span");
  countSpan.className = "terminal-search-count";

  const prevBtn = document.createElement("button");
  prevBtn.className = "terminal-search-btn";
  prevBtn.innerHTML = SVG_UP;
  prevBtn.title = "Previous (Shift+Enter)";
  prevBtn.tabIndex = -1;

  const nextBtn = document.createElement("button");
  nextBtn.className = "terminal-search-btn";
  nextBtn.innerHTML = SVG_DOWN;
  nextBtn.title = "Next (Enter)";
  nextBtn.tabIndex = -1;

  const closeBtn = document.createElement("button");
  closeBtn.className = "terminal-search-btn terminal-search-close";
  closeBtn.innerHTML = SVG_CLOSE;
  closeBtn.title = "Close (Escape)";
  closeBtn.tabIndex = -1;

  bar.append(input, toggleGroup, countSpan, prevBtn, nextBtn, closeBtn);

  // ── Search Logic ──

  function updateCount() {
    if (!input.value) {
      countSpan.textContent = "";
      return;
    }
    if (resultCount === 0) {
      countSpan.textContent = "No results";
      countSpan.classList.add("no-results");
    } else {
      countSpan.textContent = resultIndex >= 0 ? `${resultIndex + 1} of ${resultCount}` : `${resultCount} found`;
      countSpan.classList.remove("no-results");
    }
  }

  function triggerSearch() {
    const term = input.value;
    if (!term) {
      emulator.clearSearch?.();
      resultIndex = -1;
      resultCount = 0;
      updateCount();
      return;
    }
    emulator.findNext?.(term, { regex: useRegex, caseSensitive, wholeWord, incremental: true });
  }

  function findNext() {
    const term = input.value;
    if (term) emulator.findNext?.(term, { regex: useRegex, caseSensitive, wholeWord });
  }

  function findPrevious() {
    const term = input.value;
    if (term) emulator.findPrevious?.(term, { regex: useRegex, caseSensitive, wholeWord });
  }

  // ── Event Handlers ──

  input.addEventListener("input", () => triggerSearch());

  input.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      hide();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      if (e.shiftKey) {
        findPrevious();
      } else {
        findNext();
      }
    }
  });

  // Prevent clicks inside search bar from affecting terminal/dockview
  bar.addEventListener("mousedown", (e) => {
    e.stopPropagation();
  });

  prevBtn.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    findPrevious();
    input.focus();
  });

  nextBtn.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    findNext();
    input.focus();
  });

  closeBtn.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    hide();
  });

  // ── Show / Hide ──

  function subscribeResults() {
    unsubResults?.();
    unsubResults = emulator.onSearchResults?.((idx, count) => {
      resultIndex = idx;
      resultCount = count;
      updateCount();
    }) ?? null;
  }

  function show() {
    if (visible) {
      input.focus();
      input.select();
      return;
    }
    visible = true;
    container.appendChild(bar);
    subscribeResults();
    // Slight delay so the bar is in the DOM before focusing
    requestAnimationFrame(() => {
      input.focus();
      // If there's selected text in the terminal, use it as initial search
      const sel = emulator.getSelection();
      if (sel && !input.value) {
        input.value = sel;
        triggerSearch();
      }
      input.select();
    });
  }

  function hide() {
    if (!visible) return;
    visible = false;
    emulator.clearSearch?.();
    unsubResults?.();
    unsubResults = null;
    resultIndex = -1;
    resultCount = 0;
    updateCount();
    if (container.contains(bar)) {
      container.removeChild(bar);
    }
    emulator.focus();
  }

  function toggle() {
    if (visible) hide();
    else show();
  }

  // ── Keyboard shortcut listener on container ──

  function onKeyDown(e: KeyboardEvent) {
    const isFindKey = (isMac ? e.metaKey : e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "f";
    if (isFindKey) {
      e.preventDefault();
      e.stopPropagation();
      show();
    }
  }

  container.addEventListener("keydown", onKeyDown, true);

  function dispose() {
    hide();
    container.removeEventListener("keydown", onKeyDown, true);
  }

  return { show, hide, toggle, dispose };
}
