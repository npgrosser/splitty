/**
 * A session's scratchpad in its terminal pane (`session-scratchpad`,
 * Scratchpad In The Pane): a collapsible area below the terminal.
 *
 * Whether the area is open, and how much of the pane it takes, belongs to
 * this window and is kept per session, so it follows a pane into another tab
 * stack, a floating group, a dock or another screen. After a reload every
 * area starts closed; the tab's marker shows where text waits.
 */

import {
  SCRATCHPAD_LIMIT, byteLength, onScratchpadFontSize, registerScratchpad, scratchpadFontSize, setScratchpadFontSize,
  steppedFontSize, type ScratchpadControl, type Scratchpads,
} from "../lib/scratchpad.ts";
import { shortcutKey } from "../lib/shortcuts.ts";
import { attachImageInsert } from "../lib/terminal/image_paste.ts";

/** A note page, for the pane controls and the tab's marker. */
export function scratchpadIcon(size: number): string {
  return `<svg width="${size}" height="${size}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><rect x="3" y="1.5" width="10" height="13" rx="1.5"/><line x1="5.5" y1="5.5" x2="10.5" y2="5.5"/><line x1="5.5" y1="8.5" x2="10.5" y2="8.5"/><line x1="5.5" y1="11.5" x2="8.5" y2="11.5"/></svg>`;
}

interface Placement {
  open: boolean;
  /** The area's share of the pane's height. */
  share: number;
}

const DEFAULT_SHARE = 1 / 3;
const MIN_SHARE = 0.1;
const MAX_SHARE = 0.8;

/** Per session, in this window only. */
const placements = new Map<string, Placement>();

export interface ScratchpadAreaOptions {
  paneId: string;
  sessionId: string;
  store: Scratchpads;
  /** The id of the screen that holds the pane now, on its server. */
  screenId: () => string;
  /** Store an image (base64) on the session's server; resolves with its path. */
  saveImage: (data: string) => Promise<string>;
  /** Store a file on the session's server in chunks; resolves with its path. */
  uploadFile: (file: File, onProgress?: (received: number) => void) => Promise<string>;
  /** Give the keyboard back to the terminal. */
  focusTerminal: () => void;
}

export class ScratchpadArea implements ScratchpadControl {
  readonly element: HTMLElement;
  private readonly textarea: HTMLTextAreaElement;
  private readonly note: HTMLElement;
  private readonly placement: Placement;
  private readonly cleanups: (() => void)[] = [];

  constructor(private readonly options: ScratchpadAreaOptions) {
    const { sessionId, store } = options;
    this.placement = placements.get(sessionId) ?? { open: false, share: DEFAULT_SHARE };
    placements.set(sessionId, this.placement);

    this.element = document.createElement("div");
    this.element.className = "splitty-scratchpad";
    Object.assign(this.element.style, {
      position: "relative",
      flexDirection: "column",
      minHeight: "56px",
      borderTop: "1px solid var(--splitty-border-default)",
      background: "var(--splitty-surface-raised)",
    });

    const handle = document.createElement("div");
    handle.title = "Drag to resize";
    Object.assign(handle.style, {
      position: "absolute",
      top: "-3px",
      left: "0",
      right: "0",
      height: "6px",
      cursor: "row-resize",
      zIndex: "1",
    });
    this.element.appendChild(handle);

    this.textarea = document.createElement("textarea");
    this.textarea.spellcheck = false;
    this.textarea.setAttribute("aria-label", "Scratchpad");
    this.textarea.placeholder = `Scratchpad for this session: notes and prompts for later. Esc returns to the terminal, ${shortcutKey("scratchpad")} closes.`;
    Object.assign(this.textarea.style, {
      flex: "1 1 auto",
      minHeight: "0",
      width: "100%",
      boxSizing: "border-box",
      resize: "none",
      border: "0",
      outline: "none",
      padding: "6px 10px",
      background: "transparent",
      color: "var(--splitty-text-primary)",
      font: `${scratchpadFontSize()}px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`,
    });
    this.textarea.value = store.text(sessionId);
    this.element.appendChild(this.textarea);

    this.note = document.createElement("div");
    this.note.textContent = `The scratchpad is full: it keeps at most ${SCRATCHPAD_LIMIT / 1024} KiB.`;
    Object.assign(this.note.style, {
      display: "none",
      padding: "2px 10px 4px",
      fontSize: "11px",
      color: "var(--splitty-warning)",
    });
    this.element.appendChild(this.note);

    this.listen(this.textarea, "input", () => {
      const kept = store.edit(options.screenId(), sessionId, this.textarea.value);
      if (kept !== this.textarea.value) this.textarea.value = kept;
      this.note.style.display = byteLength(kept) >= SCRATCHPAD_LIMIT ? "block" : "none";
    });
    this.listen(this.textarea, "blur", () => store.flush(sessionId));
    this.listen(this.textarea, "keydown", (event) => {
      const e = event as KeyboardEvent;
      if (e.key === "Escape" && !e.isComposing) {
        e.preventDefault();
        e.stopPropagation();
        options.focusTerminal();
      }
    });
    this.cleanups.push(attachImageInsert(this.textarea, options, (text) => this.insert(text)));
    this.listen(this.textarea, "wheel", (event) => {
      const e = event as WheelEvent;
      if (!e.ctrlKey) return;
      e.preventDefault();
      setScratchpadFontSize(steppedFontSize(scratchpadFontSize(), e.deltaY));
    });
    this.cleanups.push(onScratchpadFontSize(() => {
      this.textarea.style.fontSize = `${scratchpadFontSize()}px`;
    }));
    // The text field's own menu, not the pane's.
    this.listen(this.element, "contextmenu", (event) => event.stopPropagation());
    this.cleanups.push(store.subscribe(() => this.adopt()));
    this.cleanups.push(this.resizable(handle));

    this.cleanups.push(registerScratchpad(options.paneId, this));
    this.apply();
  }

  isOpen(): boolean {
    return this.placement.open;
  }

  toggle(): void {
    if (this.isOpen()) this.close();
    else this.open();
  }

  open(): void {
    this.placement.open = true;
    this.apply();
    this.textarea.focus();
  }

  close(): void {
    this.options.store.flush(this.options.sessionId);
    this.placement.open = false;
    this.apply();
    this.options.focusTerminal();
  }

  /** Whether `node` lies inside the area. */
  contains(node: Node | null): boolean {
    return node !== null && this.element.contains(node);
  }

  dispose(): void {
    this.options.store.flush(this.options.sessionId);
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups.length = 0;
    this.element.remove();
  }

  /** Put `text` at the cursor, or over the selection, as an edit of the user. */
  private insert(text: string): void {
    const { selectionStart, selectionEnd, value } = this.textarea;
    this.textarea.value = value.slice(0, selectionStart) + text + value.slice(selectionEnd);
    const cursor = selectionStart + text.length;
    this.textarea.setSelectionRange(cursor, cursor);
    this.textarea.dispatchEvent(new Event("input"));
  }

  /** A newer text from the server, unless this window's edit waits. */
  private adopt(): void {
    const { store, sessionId } = this.options;
    if (store.unsaved(sessionId)) return;
    const text = store.text(sessionId);
    if (this.textarea.value === text) return;
    const { selectionStart, selectionEnd } = this.textarea;
    this.textarea.value = text;
    this.textarea.setSelectionRange(Math.min(selectionStart, text.length), Math.min(selectionEnd, text.length));
  }

  private apply(): void {
    this.element.style.display = this.placement.open ? "flex" : "none";
    this.element.style.flex = `0 0 ${(this.placement.share * 100).toFixed(1)}%`;
  }

  private resizable(handle: HTMLElement): () => void {
    const onDown = (event: PointerEvent) => {
      const pane = this.element.parentElement;
      if (!pane) return;
      event.preventDefault();
      handle.setPointerCapture(event.pointerId);
      const bounds = pane.getBoundingClientRect();
      const onMove = (move: PointerEvent) => {
        const share = (bounds.bottom - move.clientY) / bounds.height;
        this.placement.share = Math.min(MAX_SHARE, Math.max(MIN_SHARE, share));
        this.apply();
      };
      const onUp = () => {
        handle.removeEventListener("pointermove", onMove);
        handle.removeEventListener("pointerup", onUp);
      };
      handle.addEventListener("pointermove", onMove);
      handle.addEventListener("pointerup", onUp);
    };
    handle.addEventListener("pointerdown", onDown);
    return () => handle.removeEventListener("pointerdown", onDown);
  }

  private listen(target: EventTarget, type: string, handler: (event: Event) => void): void {
    target.addEventListener(type, handler);
    this.cleanups.push(() => target.removeEventListener(type, handler));
  }
}
