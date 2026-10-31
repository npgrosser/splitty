import { init, Terminal, FitAddon, OSC8LinkProvider, UrlRegexProvider, type ILink, type ILinkProvider } from "ghostty-web";
import { getXtermTheme } from "../theme.ts";
import { getConfig, type TerminalConfig } from "../terminal_config.ts";
import { getSettings, type SplittySettings } from "../settings.ts";
import { zoomedFontSize } from "../zoom.ts";
import { isMac } from "../shortcuts.ts";
import type { TerminalEmulator } from "./terminal_emulator.ts";
import { windowAPI } from "../window.ts";
import { openInBrowser } from "./hyperlinks.ts";
import {
  ensureFont,
  isSplittyShortcut,
  shouldUseNativeClipboardShortcut,
  isCopyShortcut,
  writeClipboardText,
  wheelDeltaToLines,
  waitForLayout,
  BSU,
  ESU,
  MAX_WRITE_BUF,
} from "./terminal_shared.ts";

// ── Pre-warm ──

let initDone = false;
let warmupDone = false;

async function ensureInit(): Promise<void> {
  if (initDone) return;
  await init();
  initDone = true;
}

export function warmup(): void {
  if (warmupDone) return;
  warmupDone = true;
  const cfg = getConfig();
  ensureFont(cfg);
  // Pre-load WASM in background
  ensureInit().catch(() => {});
}

// ── Links ──

/**
 * ghostty-web's own link providers (OSC 8 hyperlinks, detected URLs), with a
 * Ctrl/Cmd-click handed to `open`. Registered after the built-in ones, its
 * links take their places in the link cache, which keys a link by its
 * hyperlink id or its range.
 */
function linksThrough(terminal: Terminal, open: (url: string) => void): ILinkProvider {
  const providers: ILinkProvider[] = [new OSC8LinkProvider(terminal), new UrlRegexProvider(terminal)];
  return {
    provideLinks(y, callback) {
      const links: ILink[] = [];
      let waiting = providers.length;
      for (const provider of providers) {
        provider.provideLinks(y, (found) => {
          for (const link of found ?? []) {
            links.push({
              ...link,
              activate: (event) => {
                if (event.ctrlKey || event.metaKey) open(link.text);
              },
            });
          }
          if (--waiting === 0) callback(links.length > 0 ? links : undefined);
        });
      }
    },
    dispose() {
      for (const provider of providers) provider.dispose?.();
    },
  };
}

// ══════════════════════════════════════════════════════════════════════════════
// GhosttyTerminalEmulator
// ══════════════════════════════════════════════════════════════════════════════

export class GhosttyTerminalEmulator implements TerminalEmulator {
  private terminal: Terminal;
  private fitAddon: FitAddon;
  private container: HTMLElement | null = null;
  private cleanups: (() => void)[] = [];

  // Write batching state (rAF + DEC 2026 sync mode)
  private writeBuf: Uint8Array[] = [];
  private writeBufSize = 0;
  private writeScheduled = false;

  private constructor(terminal: Terminal, fitAddon: FitAddon, private readonly openLink?: (url: string) => void) {
    this.terminal = terminal;
    this.fitAddon = fitAddon;
  }

  /**
   * With `openLink` (a host's pane), a Ctrl/Cmd-click on a link goes there
   * instead of ghostty-web's bare `window.open`; see {@link linksThrough}.
   */
  static async create(openLink?: (url: string) => void): Promise<GhosttyTerminalEmulator> {
    await ensureInit();

    const cfg = getConfig();
    await ensureFont(cfg);

    const xterm = getSettings().terminal.xterm;
    const terminal = new Terminal({
      fontSize: zoomedFontSize(cfg.fontSize, getSettings().appearance.zoom, getSettings().appearance.zoomScope),
      fontFamily: cfg.fontFamily,
      cursorStyle: cfg.cursorStyle,
      cursorBlink: xterm.cursorBlink,
      scrollback: 2000,
      allowTransparency: xterm.allowTransparency,
      theme: getXtermTheme(),
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);

    return new GhosttyTerminalEmulator(terminal, fitAddon, openLink);
  }

  // ── TerminalEmulator interface ──

  open(container: HTMLElement): void {
    this.container = container;
    this.terminal.open(container);
    // A local pane's links go to the system browser, a host's to its forward.
    this.terminal.registerLinkProvider(linksThrough(this.terminal, this.openLink ?? openInBrowser));
    this.attachKeyEventHandler();
    this.attachCopySelectionHandler(container);
    this.attachShiftEnterHandler(container);
    this.attachOptionArrowHandler(container);
    this.attachWheelHandler();
  }

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
    this.terminal.resize(cols, rows);
  }

  fit(): { cols: number; rows: number } | undefined {
    const dims = this.fitAddon.proposeDimensions();
    if (!dims) return undefined;
    this.terminal.resize(dims.cols, dims.rows);
    return { cols: dims.cols, rows: dims.rows };
  }

  measure(): { cols: number; rows: number } | undefined {
    const dims = this.fitAddon.proposeDimensions();
    return dims && { cols: dims.cols, rows: dims.rows };
  }

  cellSize(): { width: number; height: number } | undefined {
    const metrics = this.terminal.renderer?.getMetrics();
    return metrics && metrics.width > 0 && metrics.height > 0
      ? { width: metrics.width, height: metrics.height }
      : undefined;
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
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
    this.terminal.dispose();
    this.container = null;
  }

  onData(cb: (data: string) => void): () => void {
    const disposable = this.terminal.onData(cb);
    return () => disposable.dispose();
  }

  onBinary(_cb: (data: string) => void): () => void {
    // ghostty-web doesn't have onBinary — no-op
    return () => {};
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
    const disposable = this.terminal.onScroll(() => cb());
    return () => disposable.dispose();
  }

  onWriteParsed(cb: () => void): () => void {
    // ghostty-web doesn't have onWriteParsed — use onRender as equivalent
    const disposable = this.terminal.onRender(() => cb());
    return () => disposable.dispose();
  }

  applyTheme(theme: Record<string, string>): void {
    this.terminal.options.theme = theme;
  }

  applySettings(settings: SplittySettings, config: TerminalConfig): void {
    const t = this.terminal;
    t.options.fontSize = zoomedFontSize(config.fontSize, settings.appearance.zoom, settings.appearance.zoomScope);
    t.options.fontFamily = config.fontFamily;
    t.options.cursorBlink = settings.terminal.xterm.cursorBlink;
    t.options.allowTransparency = settings.terminal.xterm.allowTransparency;
    this.fit();
  }

  applyScrollbar(): void {
    // ghostty-web has built-in scrollbar handling — no custom logic needed
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

  // Search not available in ghostty-web — optional methods left undefined

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
    // NOTE: ghostty-web has INVERTED convention vs xterm.js:
    //   return true  = "I handled it, skip terminal processing"
    //   return false = "let the terminal handle it"
    this.terminal.attachCustomKeyEventHandler((e: KeyboardEvent) => {
      if (shouldUseNativeClipboardShortcut(e) || isSplittyShortcut(e)) return true;
      return false;
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

  private attachShiftEnterHandler(container: HTMLElement): void {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Enter" && e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
        e.preventDefault();
        e.stopImmediatePropagation();
        this.terminal.input("\x1b[13;2u");
      }
    };
    container.addEventListener("keydown", handler, true);
    this.cleanups.push(() => container.removeEventListener("keydown", handler, true));
  }

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
        this.terminal.input(seq);
      }
    };
    container.addEventListener("keydown", handler, true);
    this.cleanups.push(() => container.removeEventListener("keydown", handler, true));
  }

  /**
   * Alternate-screen wheel acceleration — see the xterm.js emulator for the
   * rationale. Two ghostty-web specifics:
   *
   * - The handler contract is inverted relative to xterm.js: here `true` means
   *   "handled, skip default", `false` means "fall through to ghostty-web".
   * - ghostty-web has no mouse reporting at all, so mouse-tracking apps get the
   *   cursor-key fallback either way. Accelerating it is still an improvement
   *   over ghostty-web's own single-keypress path, and we add the application
   *   cursor keys (DECCKM) prefix it is missing.
   */
  private attachWheelHandler(): void {
    if (!this.terminal.buffer) return;
    let wheelRemainder = 0;
    this.terminal.attachCustomWheelEventHandler((e: WheelEvent) => {
      if (this.terminal.buffer.active.type !== "alternate") return false;
      const rawLines = wheelDeltaToLines(e.deltaY, e.deltaMode, this.terminal.rows) + wheelRemainder;
      const wholeLines = rawLines > 0 ? Math.floor(rawLines) : Math.ceil(rawLines);
      wheelRemainder = rawLines - wholeLines;
      const lines = Math.min(Math.abs(wholeLines), 5);
      if (lines === 0) return true;
      const prefix = this.terminal.getMode(1) ? "\x1bO" : "\x1b[";
      const seq = wholeLines < 0 ? `${prefix}A` : `${prefix}B`;
      for (let i = 0; i < lines; i++) {
        this.terminal.input(seq);
      }
      return true;
    });
  }
}
