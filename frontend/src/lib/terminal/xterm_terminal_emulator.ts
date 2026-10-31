import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { SearchAddon, ISearchOptions } from "@xterm/addon-search";
import { cellSize, fit as customFit, measure } from "./fit.ts";
import { getXtermTheme } from "../theme.ts";
import { getConfig, type TerminalConfig } from "../terminal_config.ts";
import { getSettings, type SplittySettings } from "../settings.ts";
import { zoomedFontSize } from "../zoom.ts";
import { isMac } from "../shortcuts.ts";
import type { TerminalEmulator } from "./terminal_emulator.ts";
import { windowAPI } from "../window.ts";
import { openHyperlink } from "./hyperlinks.ts";
import {
  ensureFont,
  isSplittyShortcut,
  shouldUseNativeClipboardShortcut,
  isCopyShortcut,
  writeClipboardText,
  wheelDeltaToLines,
  appReportsWheel,
  waitForLayout,
  BSU,
  ESU,
  MAX_WRITE_BUF,
} from "./terminal_shared.ts";

// ── WebGL Addon Loader ──

async function loadWebgl(terminal: Terminal, retries = 2): Promise<void> {
  try {
    const { WebglAddon } = await import("@xterm/addon-webgl");
    const addon = new WebglAddon();
    addon.onContextLoss(() => {
      addon.dispose();
      if (retries > 0) {
        loadWebgl(terminal, retries - 1);
      } else {
        console.warn("WebGL context lost too many times, falling back to canvas");
      }
    });
    terminal.loadAddon(addon);
  } catch (e) {
    console.warn("WebGL addon failed to load, falling back to canvas:", e);
  }
}

// ── Addon Loading ──

async function loadXtermAddons(
  terminal: Terminal,
  onSearch?: (addon: SearchAddon) => void,
  openLink?: (url: string) => void,
): Promise<void> {
  if (getSettings().terminal.xterm.gpuAcceleration === "on") {
    loadWebgl(terminal);
  }

  const [
    { WebLinksAddon },
    { SearchAddon },
    { ImageAddon },
    { ClipboardAddon },
    { UnicodeGraphemesAddon },
  ] = await Promise.all([
    import("@xterm/addon-web-links"),
    import("@xterm/addon-search"),
    import("@xterm/addon-image"),
    import("@xterm/addon-clipboard"),
    import("@xterm/addon-unicode-graphemes"),
  ]);

  // Terminal output is attacker-influenced: any process can print a link, and a
  // click on it used to be routable into an in-app iframe pane via the
  // `linkTarget` setting. That made terminal output an entry point into the
  // path described in security review SR-01/SR-02. Links now
  // always go to the system browser, and openExternalTarget rejects any scheme
  // other than http/https. A host's pane hands its links to `openLink`, which
  // opens a link to the host's loopback at its forward (lib/remote/port_forwarding.ts).
  terminal.loadAddon(new WebLinksAddon((_event, url) => {
    openHyperlink(url, openLink);
  }));

  const searchAddon = new SearchAddon();
  terminal.loadAddon(searchAddon);
  onSearch?.(searchAddon);

  const imgAddon = new ImageAddon();
  terminal.loadAddon(imgAddon);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (terminal as any)._splittyImageAddon = imgAddon;

  // Custom provider, not the stock one (security review SR-20).
  // See clipboard_policy.ts: OSC 52 reads are refused outright, writes are
  // enabled by default, size-capped and announced.
  const { Base64 } = await import("@xterm/addon-clipboard");
  const { SplittyClipboardProvider } = await import("./clipboard_policy.ts");
  terminal.loadAddon(new ClipboardAddon(new Base64(), new SplittyClipboardProvider()));
  terminal.loadAddon(new UnicodeGraphemesAddon());
  terminal.unicode.activeVersion = "15";

  if (getSettings().terminal.ligatures) {
    const { LigaturesAddon } = await import("@xterm/addon-ligatures");
    terminal.loadAddon(new LigaturesAddon());
  }
}

// ── Pre-warm ──

let warmupDone = false;

/**
 * Pre-load xterm.js + font so the first terminal opens instantly.
 * Call early (e.g. on app mount).
 */
export function warmup(): void {
  if (warmupDone) return;
  warmupDone = true;
  const cfg = getConfig();
  ensureFont(cfg);
}


// ══════════════════════════════════════════════════════════════════════════════
// XtermTerminalEmulator
// ══════════════════════════════════════════════════════════════════════════════

export class XtermTerminalEmulator implements TerminalEmulator {
  private terminal: Terminal;
  private fitAddon: FitAddon;
  private container: HTMLElement | null = null;
  private cleanups: (() => void)[] = [];

  // Write batching state (rAF + DEC 2026 sync mode)
  private writeBuf: Uint8Array[] = [];
  private writeBufSize = 0;
  private writeScheduled = false;

  // Scrollbar fade state
  private scrollCleanup: (() => void) | undefined;

  // Search addon
  private _searchAddon: SearchAddon | null = null;

  private constructor(terminal: Terminal, fitAddon: FitAddon, private readonly openLink?: (url: string) => void) {
    this.terminal = terminal;
    this.fitAddon = fitAddon;
  }

  /**
   * Create a new XtermTerminalEmulator.
   * Ensures font is loaded before creating the Terminal instance.
   *
   * Detected links and OSC 8 hyperlinks open directly through `openLink`
   * (a host's pane) or the system browser.
   */
  static async create(openLink?: (url: string) => void): Promise<XtermTerminalEmulator> {
    const cfg = getConfig();
    await ensureFont(cfg);

    const xterm = getSettings().terminal.xterm;
    const terminal = new Terminal({
      fontSize: zoomedFontSize(cfg.fontSize, getSettings().appearance.zoom, getSettings().appearance.zoomScope),
      fontFamily: cfg.fontFamily,
      cursorStyle: cfg.cursorStyle,
      cursorBlink: xterm.cursorBlink,
      scrollback: 2000,
      scrollSensitivity: xterm.scrollSensitivity,
      fastScrollSensitivity: xterm.fastScrollSensitivity,
      lineHeight: xterm.lineHeight,
      letterSpacing: xterm.letterSpacing,
      fontWeight: xterm.fontWeight,
      fontWeightBold: xterm.fontWeightBold,
      drawBoldTextInBrightColors: xterm.drawBoldTextInBrightColors,
      minimumContrastRatio: xterm.minimumContrastRatio,
      customGlyphs: xterm.customGlyphs,
      rescaleOverlappingGlyphs: xterm.rescaleOverlappingGlyphs,
      allowTransparency: xterm.allowTransparency,
      allowProposedApi: true,
      cursorInactiveStyle: "none",
      theme: getXtermTheme(),
      // Use the same opener as detected URLs, bypassing xterm's default dialog.
      linkHandler: {
        activate: (_event, uri) => openHyperlink(uri, openLink),
      },
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);

    return new XtermTerminalEmulator(terminal, fitAddon, openLink);
  }

  // ── TerminalEmulator interface ──

  open(container: HTMLElement): void {
    this.container = container;
    this.terminal.open(container);
    loadXtermAddons(this.terminal, (addon) => { this._searchAddon = addon; }, this.openLink);
    this.attachKeyEventHandler();
    this.attachCopySelectionHandler(container);
    this.attachShiftEnterHandler(container);
    this.attachOptionArrowHandler(container);
    this.attachWheelHandler();
    this.attachAltScreenStateReset();
    this.applyScrollbar();
  }

  /**
   * Open the terminal, waiting for the container to have layout if needed.
   * Returns the initial grid dimensions for the PTY request.
   */
  async openDeferred(container: HTMLElement): Promise<{ cols: number; rows: number }> {
    if (!(container.offsetWidth > 0 && container.offsetHeight > 0)) {
      await waitForLayout(container);
    }
    this.open(container);
    const dims = this.fit();
    return dims ?? { cols: this.cols, rows: this.rows };
  }

  write(data: Uint8Array): void {
    this.writeBuf.push(data);
    this.writeBufSize += data.length;
    if (!this.writeScheduled) {
      this.writeScheduled = true;
      requestAnimationFrame(() => this.flushWrites());
    }
    if (this.writeBufSize > MAX_WRITE_BUF) {
      this.flushWrites();
    }
  }

  resize(cols: number, rows: number): void {
    if (cols === this.terminal.cols && rows === this.terminal.rows) return;
    // As in fit(): drop the old frame, or its cells linger at the new size.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (this.terminal as any)._core._renderService.clear();
    this.terminal.resize(cols, rows);
  }

  fit(): { cols: number; rows: number } | undefined {
    const padding = customFit(this.terminal);
    if (!padding) return undefined;
    return { cols: this.terminal.cols, rows: this.terminal.rows };
  }

  measure(): { cols: number; rows: number } | undefined {
    const grid = measure(this.terminal);
    return grid && { cols: grid.cols, rows: grid.rows };
  }

  cellSize(): { width: number; height: number } | undefined {
    return cellSize(this.terminal);
  }

  get element(): HTMLElement | undefined {
    return this.terminal.element;
  }

  get cursorRow(): number {
    return this.terminal.buffer.active.cursorY;
  }

  focus(): void {
    this.terminal.focus();
  }

  blur(): void {
    this.terminal.blur();
  }

  dispose(): void {
    this.scrollCleanup?.();
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
    this.terminal.dispose();
    this.container = null;
  }

  onData(cb: (data: string) => void): () => void {
    const disposable = this.terminal.onData(cb);
    return () => disposable.dispose();
  }

  onBinary(cb: (data: string) => void): () => void {
    const disposable = this.terminal.onBinary(cb);
    return () => disposable.dispose();
  }

  onResize(cb: (cols: number, rows: number) => void): () => void {
    const disposable = this.terminal.onResize(({ cols, rows }) => cb(cols, rows));
    return () => disposable.dispose();
  }

  getSelection(): string {
    return this.terminal.getSelection();
  }

  hasSelection(): boolean {
    return this.terminal.hasSelection();
  }

  paste(text: string): void {
    this.terminal.paste(text);
  }

  clear(): void {
    this.terminal.clear();
  }

  scrollToBottom(): void {
    this.terminal.scrollToBottom();
  }

  getScrollGap(): number {
    const buf = this.terminal.buffer.active;
    return buf.baseY - buf.viewportY;
  }

  onScroll(cb: () => void): () => void {
    const d = this.terminal.onScroll(() => cb());
    return () => d.dispose();
  }

  onWriteParsed(cb: () => void): () => void {
    const d = this.terminal.onWriteParsed(() => cb());
    return () => d.dispose();
  }

  applyTheme(theme: Record<string, string>): void {
    this.terminal.options.theme = theme;
  }

  applySettings(settings: SplittySettings, config: TerminalConfig): void {
    const xt = settings.terminal.xterm;
    const t = this.terminal;
    t.options.fontSize = zoomedFontSize(config.fontSize, settings.appearance.zoom, settings.appearance.zoomScope);
    t.options.fontFamily = config.fontFamily;
    t.options.cursorBlink = xt.cursorBlink;
    t.options.lineHeight = xt.lineHeight;
    t.options.letterSpacing = xt.letterSpacing;
    t.options.fontWeight = xt.fontWeight;
    t.options.fontWeightBold = xt.fontWeightBold;
    t.options.drawBoldTextInBrightColors = xt.drawBoldTextInBrightColors;
    t.options.minimumContrastRatio = xt.minimumContrastRatio;
    t.options.customGlyphs = xt.customGlyphs;
    t.options.rescaleOverlappingGlyphs = xt.rescaleOverlappingGlyphs;
    t.options.allowTransparency = xt.allowTransparency;
    t.options.scrollSensitivity = xt.scrollSensitivity;
    t.options.fastScrollSensitivity = xt.fastScrollSensitivity;
    this.applyScrollbar();
    this.fit();
  }

  applyScrollbar(): void {
    this.scrollCleanup?.();
    this.scrollCleanup = undefined;

    const el = this.terminal.element;
    if (!el || !this.container) return;

    const xt = getSettings().terminal.xterm;

    el.classList.remove("xterm-scrollbar-overlay", "xterm-scrollbar-fade", "xterm-scrollbar-active");

    if (xt.scrollbarOverlay) {
      el.classList.add("xterm-scrollbar-overlay");
    }

    if (xt.scrollbarFade) {
      el.classList.add("xterm-scrollbar-fade");

      let hideTimer: number | undefined;
      const showScrollbar = () => {
        el.classList.add("xterm-scrollbar-active");
        clearTimeout(hideTimer);
        hideTimer = window.setTimeout(() => {
          el.classList.remove("xterm-scrollbar-active");
        }, 800);
      };

      const onScroll = this.terminal.onScroll(() => showScrollbar());
      const container = this.container;
      const onMouseMove = (e: MouseEvent) => {
        const rect = el.getBoundingClientRect();
        if (e.clientX >= rect.right - 30) {
          showScrollbar();
        }
      };
      const onMouseLeave = () => {
        clearTimeout(hideTimer);
        el.classList.remove("xterm-scrollbar-active");
      };

      container.addEventListener("mousemove", onMouseMove);
      container.addEventListener("mouseleave", onMouseLeave);

      this.scrollCleanup = () => {
        clearTimeout(hideTimer);
        onScroll.dispose();
        container.removeEventListener("mousemove", onMouseMove);
        container.removeEventListener("mouseleave", onMouseLeave);
      };
    }
  }

  get cols(): number {
    return this.terminal.cols;
  }

  get rows(): number {
    return this.terminal.rows;
  }

  get isAlternateScreen(): boolean {
    return this.terminal.buffer.active.type === "alternate";
  }

  setCursorBlink(enabled: boolean): void {
    this.terminal.options.cursorBlink = enabled;
  }

  // ── Search ──

  private searchDecorations(): ISearchOptions["decorations"] {
    return {
      matchBackground: "#e5c07b40",
      matchBorder: "#e5c07b80",
      matchOverviewRuler: "#e5c07b80",
      activeMatchBackground: "#63a0ff60",
      activeMatchBorder: "#63a0ff",
      activeMatchColorOverviewRuler: "#63a0ff",
    };
  }

  findNext(term: string, options?: { regex?: boolean; caseSensitive?: boolean; wholeWord?: boolean; incremental?: boolean }): boolean {
    if (!this._searchAddon) return false;
    return this._searchAddon.findNext(term, { ...options, decorations: this.searchDecorations() });
  }

  findPrevious(term: string, options?: { regex?: boolean; caseSensitive?: boolean; wholeWord?: boolean }): boolean {
    if (!this._searchAddon) return false;
    return this._searchAddon.findPrevious(term, { ...options, decorations: this.searchDecorations() });
  }

  clearSearch(): void {
    this._searchAddon?.clearDecorations();
  }

  onSearchResults(cb: (index: number, count: number) => void): () => void {
    if (!this._searchAddon) return () => {};
    const disposable = this._searchAddon.onDidChangeResults(({ resultIndex, resultCount }) => {
      cb(resultIndex, resultCount);
    });
    return () => disposable.dispose();
  }

  // ── Internal ──

  private flushWrites(): void {
    if (this.writeBuf.length === 0) return;
    const chunks = this.writeBuf;
    this.writeBuf = [];
    this.writeBufSize = 0;
    this.writeScheduled = false;

    let total = BSU.length;
    for (const c of chunks) total += c.length;
    total += ESU.length;

    const merged = new Uint8Array(total);
    merged.set(BSU, 0);
    let offset = BSU.length;
    for (const c of chunks) {
      merged.set(c, offset);
      offset += c.length;
    }
    merged.set(ESU, offset);
    this.terminal.write(merged);
  }

  private attachKeyEventHandler(): void {
    this.terminal.attachCustomKeyEventHandler((e: KeyboardEvent) => {
      if (shouldUseNativeClipboardShortcut(e) || isSplittyShortcut(e)) return false;
      return true;
    });
  }

  private attachCopySelectionHandler(container: HTMLElement): void {
    const handler = (e: KeyboardEvent) => {
      if (!isCopyShortcut(e)) return;
      if (!this.terminal.hasSelection()) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      void writeClipboardText(this.terminal.getSelection());
    };
    container.addEventListener("keydown", handler, true);
    this.cleanups.push(() => container.removeEventListener("keydown", handler, true));
  }

  /**
   * Shift+Enter fix: both xterm.js and ghostty-web send \r for Shift+Enter,
   * identical to plain Enter. Apps using the kitty keyboard protocol (e.g.
   * Claude Code) expect \x1b[13;2u to distinguish "newline" from "submit".
   */
  private attachShiftEnterHandler(container: HTMLElement): void {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Enter" && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
        e.preventDefault();
        e.stopImmediatePropagation();
        // Emit the kitty protocol sequence via onData listeners.
        // Must use input() instead of paste() — paste() wraps with bracket
        // paste markers (\x1b[200~…\x1b[201~), causing apps in bracket paste
        // mode to see literal "[13;2u" text instead of a key event.
        this.terminal.input("\x1b[13;2u");
      }
    };
    container.addEventListener("keydown", handler, true);
    this.cleanups.push(() => container.removeEventListener("keydown", handler, true));
  }

  /**
   * Option+Arrow word navigation (macOS): xterm.js treats Option as a compose
   * key by default, so Option+Left/Right produce dead keys instead of the
   * escape sequences shells expect for word jumping.
   */
  private attachOptionArrowHandler(container: HTMLElement): void {
    if (!isMac) return;
    const handler = (e: KeyboardEvent) => {
      if (!e.altKey || e.metaKey || e.ctrlKey || e.shiftKey) return;
      let seq: string | null = null;
      if (e.key === "ArrowLeft") seq = "\x1bb";
      if (e.key === "ArrowRight") seq = "\x1bf";
      if (e.key === "ArrowUp") seq = "\x1b[1;3A";
      if (e.key === "ArrowDown") seq = "\x1b[1;3B";
      if (seq) {
        e.preventDefault();
        e.stopImmediatePropagation();
        // Must use input() instead of paste() to avoid bracket paste wrapping
        this.terminal.input(seq);
      }
    };
    container.addEventListener("keydown", handler, true);
    this.cleanups.push(() => container.removeEventListener("keydown", handler, true));
  }

  /**
   * Undo terminal state a fullscreen app left behind when it exits.
   *
   * A TUI is supposed to restore what it changed. opencode restores bracketed
   * paste (`?2004l`), the alt screen (`?1049l`), the title and the cursor style
   * (`CSI 0 SP q`), but leaks two things:
   *
   * - All four mouse modes stay armed, so `?1003` (any-motion) turns every
   *   later mouse move into an SGR report written into the shell, showing up as
   *   literal `35;86;9M` garbage.
   * - `OSC 12` set the cursor color to `#eeeeee` for its own dark UI and no
   *   matching `OSC 112` follows, so the cursor keeps that color and becomes
   *   invisible against a light theme.
   *
   * Leaving the alt screen means that fullscreen app is gone, so state still
   * set at that point is a leak. Well-behaved apps restore it themselves around
   * the transition (vim disarms the mouse across a `:sh` shell-out and re-arms
   * on return), so this stays a no-op for them.
   *
   * Written into the emulator, not the PTY: it corrects our own state without
   * sending anything to the app.
   */
  private attachAltScreenStateReset(): void {
    if (!this.terminal.buffer) return;
    const disposable = this.terminal.buffer.onBufferChange(buffer => {
      if (buffer.type !== "normal") return;
      let seq = "";
      if (this.terminal.modes.mouseTrackingMode !== "none") {
        seq += "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l";
      }
      // OSC 112 restores the themed cursor color. There is no getter for the
      // current color, so this is unconditional — it is a no-op when the app
      // never touched it.
      seq += "\x1b]112\x1b\\";
      this.terminal.write(seq);
    });
    this.cleanups.push(() => disposable.dispose());
  }

  /**
   * Alternate-screen wheel acceleration: in fullscreen apps that do *not* track
   * the mouse (`less`, `bat`, `vim` without `set mouse`), xterm.js turns the
   * wheel into cursor keys so the app scrolls its own viewport — but only one
   * keypress per wheel event, which feels sluggish. We scale one gesture into
   * up to 5 cursor keys instead.
   *
   * Apps that *do* enable mouse reporting are left to xterm.js, which sends
   * real SGR wheel events. Returning false here would swallow those and feed
   * the app cursor keys it never asked for.
   *
   * The cursor key prefix depends on the terminal mode: with application cursor
   * keys enabled, Up/Down must be SS3 (`ESC O A/B`) rather than CSI
   * (`ESC [ A/B`), otherwise apps render literal `Esc([` fragments.
   */
  private attachWheelHandler(): void {
    if (!this.terminal.buffer) return;
    let wheelRemainder = 0;
    this.terminal.attachCustomWheelEventHandler((e: WheelEvent) => {
      if (this.terminal.buffer.active.type !== "alternate") return true;
      if (appReportsWheel(this.terminal.modes.mouseTrackingMode)) {
        wheelRemainder = 0;
        return true;
      }
      const rawLines = wheelDeltaToLines(e.deltaY, e.deltaMode, this.terminal.rows) + wheelRemainder;
      const wholeLines = rawLines > 0 ? Math.floor(rawLines) : Math.ceil(rawLines);
      wheelRemainder = rawLines - wholeLines;
      const lines = Math.min(Math.abs(wholeLines), 5);
      if (lines === 0) {
        // Sub-line delta is banked in wheelRemainder. Falling through would let
        // xterm.js count it a second time with its own accumulator, so swallow
        // the event ourselves.
        e.preventDefault();
        return false;
      }
      const prefix = this.terminal.modes.applicationCursorKeysMode ? "\x1bO" : "\x1b[";
      const seq = wholeLines < 0 ? `${prefix}A` : `${prefix}B`;
      for (let i = 0; i < lines; i++) {
        this.terminal.input(seq);
      }
      return false;
    });
  }
}
