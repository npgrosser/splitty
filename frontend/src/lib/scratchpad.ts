/**
 * The scratchpads of one server's sessions (`session-scratchpad`): the text a
 * window shows for each, from the server's snapshots and from its own edits.
 *
 * An edit shows at once and is saved with `set_scratchpad` half a second after
 * typing pauses, or at once on `flush`. A text from the server replaces the
 * shown one only while no edit of this window waits; a waiting edit is saved
 * and wins (last write wins). Right after a save, a snapshot the server made
 * before it may still arrive; for a moment the saved text stays.
 */

import type { ScreenState } from "./protocol/types.ts";

/** The longest scratchpad, in bytes of UTF-8; the server refuses more. */
export const SCRATCHPAD_LIMIT = 64 * 1024;
/** How long typing pauses before an edit is saved. */
export const SAVE_DELAY_MS = 500;
/** How long a saved text outlasts snapshots the server made before the save. */
export const SETTLE_MS = 2000;

const encoder = new TextEncoder();

export function byteLength(text: string): number {
  return encoder.encode(text).length;
}

/** `text` cut to the limit at a character boundary. */
export function withinLimit(text: string): string {
  if (text.length * 3 <= SCRATCHPAD_LIMIT || byteLength(text) <= SCRATCHPAD_LIMIT) return text;
  let bytes = 0;
  let end = 0;
  for (const ch of text) {
    const point = ch.codePointAt(0) ?? 0;
    const size = point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
    if (bytes + size > SCRATCHPAD_LIMIT) break;
    bytes += size;
    end += ch.length;
  }
  return text.slice(0, end);
}

export const FONT_SIZE_DEFAULT = 12;
export const FONT_SIZE_MIN = 9;
export const FONT_SIZE_MAX = 28;
const FONT_SIZE_KEY = "splitty.scratchpad.fontSize";

/** `size` one step up (wheel up) or down, within the limits. */
export function steppedFontSize(size: number, wheelDeltaY: number): number {
  const next = size + (wheelDeltaY < 0 ? 1 : -1);
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, next));
}

/** The scratchpad font size of this window, shared by all its scratchpads. */
let fontSize = FONT_SIZE_DEFAULT;
try {
  const stored = Number(localStorage.getItem(FONT_SIZE_KEY));
  if (Number.isInteger(stored) && stored >= FONT_SIZE_MIN && stored <= FONT_SIZE_MAX) fontSize = stored;
} catch {
  // No storage: the default holds until the window closes.
}
const fontSizeListeners = new Set<() => void>();

export function scratchpadFontSize(): number {
  return fontSize;
}

export function setScratchpadFontSize(size: number): void {
  if (size === fontSize) return;
  fontSize = size;
  try {
    localStorage.setItem(FONT_SIZE_KEY, String(size));
  } catch {
    // Kept in memory only.
  }
  for (const listener of [...fontSizeListeners]) listener();
}

/** Called when the font size changes; returns the removal. */
export function onScratchpadFontSize(listener: () => void): () => void {
  fontSizeListeners.add(listener);
  return () => fontSizeListeners.delete(listener);
}

interface Waiting {
  screenId: string;
  text: string;
  timer: ReturnType<typeof setTimeout>;
}

export class Scratchpads {
  private readonly texts = new Map<string, string>();
  private readonly waiting = new Map<string, Waiting>();
  /** Per session, until when a saved text outlasts older snapshots. */
  private readonly settling = new Map<string, number>();
  private readonly panes = new Map<string, string>();
  private readonly screens = new Map<string, Set<string>>();
  private readonly listeners = new Set<() => void>();

  constructor(private readonly save: (screenId: string, sessionId: string, text: string) => void) {}

  /** A screen's snapshot: its sessions' texts (absent is empty) and which pane shows which. */
  snapshot(screen: ScreenState): void {
    let changed = false;
    const listed = new Set(Object.keys(screen.sessions));
    for (const [id, session] of Object.entries(screen.sessions)) {
      if (this.holds(id)) continue;
      changed = this.store(id, session.scratchpad ?? "") || changed;
    }
    // A session gone from its screen is closed, or listed by another screen's snapshot.
    for (const id of this.screens.get(screen.id) ?? []) {
      if (listed.has(id) || this.waiting.has(id) || this.listedElsewhere(screen.id, id)) continue;
      changed = this.texts.delete(id) || changed;
    }
    this.screens.set(screen.id, listed);
    for (const [paneId, pane] of Object.entries(screen.panes)) {
      if (pane.sessionId && this.panes.get(paneId) !== pane.sessionId) {
        this.panes.set(paneId, pane.sessionId);
        changed = true;
      }
    }
    if (changed) this.emit();
  }

  /** The text this window shows for a session. */
  text(sessionId: string): string {
    return this.texts.get(sessionId) ?? "";
  }

  /** The session a pane shows, from snapshots or from the pane itself. */
  sessionOf(paneId: string): string | undefined {
    return this.panes.get(paneId);
  }

  /** A pane attached to its session, possibly before a snapshot lists it. */
  link(paneId: string, sessionId: string): void {
    if (this.panes.get(paneId) === sessionId) return;
    this.panes.set(paneId, sessionId);
    this.emit();
  }

  /** This window's edit: shown at once, saved once typing pauses. Returns the text kept. */
  edit(screenId: string, sessionId: string, text: string): string {
    const kept = withinLimit(text);
    this.store(sessionId, kept);
    const earlier = this.waiting.get(sessionId);
    if (earlier) clearTimeout(earlier.timer);
    this.waiting.set(sessionId, {
      screenId,
      text: kept,
      timer: setTimeout(() => this.flush(sessionId), SAVE_DELAY_MS),
    });
    this.emit();
    return kept;
  }

  /** Save a waiting edit now: the text lost focus, its area closes, its pane goes. */
  flush(sessionId: string): void {
    const edit = this.waiting.get(sessionId);
    if (!edit) return;
    clearTimeout(edit.timer);
    this.waiting.delete(sessionId);
    this.settling.set(sessionId, Date.now() + SETTLE_MS);
    this.save(edit.screenId, sessionId, edit.text);
  }

  /** Whether an edit of this window waits to be saved. */
  unsaved(sessionId: string): boolean {
    return this.waiting.has(sessionId);
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Whether this window's own text outranks a snapshot's for the session. */
  private holds(sessionId: string): boolean {
    if (this.waiting.has(sessionId)) return true;
    const until = this.settling.get(sessionId);
    if (until === undefined) return false;
    if (Date.now() < until) return true;
    this.settling.delete(sessionId);
    return false;
  }

  private listedElsewhere(screenId: string, sessionId: string): boolean {
    for (const [other, ids] of this.screens) {
      if (other !== screenId && ids.has(sessionId)) return true;
    }
    return false;
  }

  /** Set a session's text; true when it changed. */
  private store(sessionId: string, text: string): boolean {
    if (this.text(sessionId) === text) return false;
    if (text) this.texts.set(sessionId, text);
    else this.texts.delete(sessionId);
    return true;
  }

  private emit(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

/** What the shortcut and the pane controls do with a pane's scratchpad area. */
export interface ScratchpadControl {
  toggle(): void;
  isOpen(): boolean;
}

/** The scratchpad area of each mounted pane. */
const controls = new Map<string, ScratchpadControl>();

/** A pane's area is mounted; returns its removal. */
export function registerScratchpad(paneId: string, control: ScratchpadControl): () => void {
  controls.set(paneId, control);
  return () => {
    if (controls.get(paneId) === control) controls.delete(paneId);
  };
}

/** Toggle a pane's scratchpad; false when the pane has none. */
export function toggleScratchpad(paneId: string): boolean {
  const control = controls.get(paneId);
  if (!control) return false;
  control.toggle();
  return true;
}

/** Whether a pane has a scratchpad, and whether its area is open. */
export function scratchpadOf(paneId: string): "open" | "closed" | undefined {
  const control = controls.get(paneId);
  if (!control) return undefined;
  return control.isOpen() ? "open" : "closed";
}
