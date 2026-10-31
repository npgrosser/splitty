import type { SplittySettings } from "../settings.ts";
import type { TerminalConfig } from "../terminal_config.ts";

/**
 * Abstract interface for a terminal emulator.
 *
 * Encapsulates all rendering and user-input concerns.
 * Knows nothing about sessions, WebSocket, or pane IDs.
 */
export interface TerminalEmulator {
  /** Mount the terminal into a DOM container. */
  open(container: HTMLElement): void;

  /** Open the terminal after the container has layout. Returns initial grid dimensions. */
  openDeferred(container: HTMLElement): Promise<{ cols: number; rows: number }>;

  /** Write raw PTY output to the terminal. */
  write(data: Uint8Array): void;

  /** Resize the terminal grid to the given dimensions. */
  resize(cols: number, rows: number): void;

  /** Fit the terminal to its container, returning the current grid size. */
  fit(): { cols: number; rows: number } | undefined;

  /**
   * The grid that fits the container, without resizing the terminal; undefined
   * while the container has no layout. For a terminal shown at a size its
   * server decides (`pty-size-v1`).
   */
  measure(): { cols: number; rows: number } | undefined;

  /** The size of one cell in CSS pixels, once the terminal has rendered. */
  cellSize(): { width: number; height: number } | undefined;

  /** The terminal's element in its container, once opened. */
  readonly element: HTMLElement | undefined;

  /** The cursor's row on the screen, from 0. */
  readonly cursorRow: number;

  /** Focus the terminal. */
  focus(): void;

  /** Blur the terminal. */
  blur(): void;

  /** Dispose all resources. */
  dispose(): void;

  /** Register a callback for user keyboard input. Returns unsubscribe. */
  onData(cb: (data: string) => void): () => void;

  /** Register a callback for binary input (e.g. file paste). Returns unsubscribe. */
  onBinary(cb: (data: string) => void): () => void;

  /** Register a callback for terminal resize events. Returns unsubscribe. */
  onResize(cb: (cols: number, rows: number) => void): () => void;

  /** Get the current text selection, or empty string if none. */
  getSelection(): string;

  /** Whether the terminal has an active text selection. */
  hasSelection(): boolean;

  /** Write text to the terminal as if pasted. */
  paste(text: string): void;

  /** Clear the terminal scrollback and screen. */
  clear(): void;

  /** Scroll the terminal viewport to the bottom. */
  scrollToBottom(): void;

  /** Get the number of lines the viewport is scrolled up from the bottom. */
  getScrollGap(): number;

  /** Register a callback for scroll events. Returns unsubscribe. */
  onScroll(cb: () => void): () => void;

  /** Register a callback for when new output is parsed. Returns unsubscribe. */
  onWriteParsed(cb: () => void): () => void;

  /** Apply a color theme. */
  applyTheme(theme: Record<string, string>): void;

  /** Apply xterm-specific settings and re-fit. */
  applySettings(settings: SplittySettings, config: TerminalConfig): void;

  /** Re-apply scrollbar overlay/fade settings. */
  applyScrollbar(): void;

  /** Toggle cursor blinking (used to disable blink on inactive panes). */
  setCursorBlink?(enabled: boolean): void;

  /** Current column count. */
  readonly cols: number;

  /** Current row count. */
  readonly rows: number;

  /** Whether the terminal is currently showing the alternate screen buffer. */
  readonly isAlternateScreen: boolean;

  // ── Search ──

  /** Search forward for the next match. Returns true if found. */
  findNext?(term: string, options?: { regex?: boolean; caseSensitive?: boolean; wholeWord?: boolean; incremental?: boolean }): boolean;

  /** Search backward for the previous match. Returns true if found. */
  findPrevious?(term: string, options?: { regex?: boolean; caseSensitive?: boolean; wholeWord?: boolean }): boolean;

  /** Clear search highlights and selection. */
  clearSearch?(): void;

  /** Register a callback for search result count changes. Returns unsubscribe. */
  onSearchResults?(cb: (index: number, count: number) => void): () => void;
}
