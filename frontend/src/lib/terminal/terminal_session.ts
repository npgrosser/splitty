/**
 * A terminal session shown in a pane, bound to the session client of the
 * connection it belongs to: input, resize, `close` and `detach` go to that
 * server, and only that server's events reach it.
 *
 * Mounting a session into the DOM (creating its emulator, `create` or
 * `attach`) is `terminal_mount.ts`; this module has no DOM dependency.
 */

import type { RestoreMode } from "../protocol/types.ts";
import { AttachTransferredError, type AttachedSession, type ViewHandle } from "./attach_registry.ts";
import type { PtySize, SessionClient, SessionTitle } from "./session_client.ts";
import type { TerminalEmulator } from "./terminal_emulator.ts";

export { AttachTransferredError };

export interface CreateSessionOptions {
  command?: string;
  dir?: string;
  root?: string;
  restoreMode?: RestoreMode;
  paneId?: string;
  /** The screen's id on the session's server. */
  screenId?: string;
  shellProfileId?: string;
  /** Whether the pane shows the session now: its panel visible on the active screen. */
  shown?: () => boolean;
}

export interface Grid {
  cols: number;
  rows: number;
}

/** What a pane needs to show a PTY size that differs from its own. */
export interface SizeView {
  /** The grid that fits the pane. */
  pane: Grid | null;
  /** The session's PTY size, once its server told it (`pty-size-v1`). */
  pty: PtySize | null;
}

function sameGrid(a: Grid, b: Grid): boolean {
  return a.cols === b.cols && a.rows === b.rows;
}

export class TerminalSession {
  /** The command used to create this session (if any). */
  command?: string;
  /** How this command should be restored. */
  restoreMode?: RestoreMode;
  /** The working directory used to create this session (if any). */
  dir?: string;
  /** Stable backend profile used by this session. */
  shellProfileId?: string;

  /** Last known foreground process name from the backend. */
  lastTitle = "";
  /** Last known working directory from the backend. */
  lastCwd = "";
  /** Last known full command line from the backend. */
  lastCmdline = "";
  /** Whether the foreground process is the session's original shell. */
  lastIsBaseShell = false;
  /** Last known OSC application title (set by the process via OSC 2). */
  lastOscTitle = "";
  private titleCallback: ((title: string, cwd: string) => void) | null = null;
  private endCallback: ((exitCode: number) => void) | null = null;
  private cleanups: (() => void)[] = [];
  private detached = false;
  private view: ViewHandle | null = null;

  // With a server that decides the PTY size (`pty-size-v1`), the pane's size
  // is reported and the emulator always has the PTY's size.
  private readonly isShown: () => boolean;
  private shown = true;
  private paneSize: Grid | null = null;
  private pty: PtySize | null = null;
  /** The next report follows an attach and asks the server for a redraw. */
  private redrawPending = false;
  private readonly sizeListeners = new Set<(view: SizeView) => void>();

  constructor(
    readonly client: SessionClient,
    readonly id: string,
    readonly emulator: TerminalEmulator,
    options?: CreateSessionOptions,
  ) {
    this.command = options?.command;
    this.restoreMode = options?.restoreMode;
    this.dir = options?.dir;
    this.shellProfileId = options?.shellProfileId;
    this.isShown = options?.shown ?? (() => true);
  }

  /** Register a callback for title changes from the backend. Returns unsubscribe. */
  onTitleChange(cb: (title: string, cwd: string) => void): () => void {
    this.titleCallback = cb;
    return () => { this.titleCallback = null; };
  }

  /** Register a callback for when the PTY process exits. Returns unsubscribe. */
  onEnd(cb: (exitCode: number) => void): () => void {
    this.endCallback = cb;
    return () => { this.endCallback = null; };
  }

  /** Called with the pane's and the PTY's size whenever either changes. Returns unsubscribe. */
  onSizeView(cb: (view: SizeView) => void): () => void {
    this.sizeListeners.add(cb);
    cb(this.sizeView());
    return () => this.sizeListeners.delete(cb);
  }

  sizeView(): SizeView {
    return { pane: this.paneSize, pty: this.pty };
  }

  /** Dispose the session: close the backend PTY and clean up all resources. */
  dispose(): void {
    if (this.detached) return;
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
    this.sizeListeners.clear();
    if (this.view) this.client.attachments.drop(this.view);
    this.view = null;
    this.client.socket.send({ type: "close", id: this.id });
    this.emulator.dispose();
    this.client.unregister(this);
  }

  /**
   * Clean up local resources without killing the backend PTY. The server stops
   * streaming the session to this connection only when no other view holds or
   * waits for it.
   */
  detach(): void {
    this.detached = true;
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
    this.sizeListeners.clear();
    if (this.view) this.client.attachments.detach(this.view);
    this.view = null;
    this.emulator.dispose();
    // A newer view of the same session may already be registered.
    this.client.unregister(this);
  }

  /**
   * Wire output, input and lifecycle messages. `attached` hands over the
   * buffered output of an attach; without it (a `create`) output is routed
   * from now on. Throws when the attach was taken over, before anything is
   * registered.
   */
  wire(attached?: AttachedSession): void {
    const sizing = this.client.sizing;
    if (sizing) {
      // The replay was written at the PTY's size; so is the emulator, first.
      this.pty = this.client.sizeOf(this.id) ?? null;
      if (this.pty) this.emulator.resize(this.pty.cols, this.pty.rows);
    }

    // Wire output: backend binary frames → emulator
    const sink = (data: Uint8Array) => this.emulator.write(data);
    this.view = attached ? attached.wire(sink) : this.client.attachments.addView(this.id, sink);

    // Lifecycle and title events arrive through the client (handleClosed, handleTitle).
    this.client.register(this);

    // Wire input: emulator → backend
    const unsubData = this.emulator.onData((data: string) => {
      this.client.socket.send({ type: "input", id: this.id, data });
    });
    this.cleanups.push(unsubData);

    // Wire resize: emulator → backend. With a server that decides the size,
    // the pane's measurement is reported instead: reporting the emulator's
    // size, which is the PTY's, would keep the PTY from ever growing again.
    const unsubResize = this.emulator.onResize((cols: number, rows: number) => {
      if (!this.client.sizing) {
        this.client.socket.send({ type: "resize", id: this.id, cols, rows });
        return;
      }
      // Something else resized the emulator (a settings change refits it).
      const target = this.pty ?? this.paneSize;
      if (target && !sameGrid(target, { cols, rows })) this.fit();
    });
    this.cleanups.push(unsubResize);

    if (sizing) {
      this.shown = this.isShown();
      this.paneSize = this.emulator.measure() ?? { cols: this.emulator.cols, rows: this.emulator.rows };
      // The server redraws after the attach (`redraw`) instead of the nudge below.
      this.redrawPending = attached !== undefined;
      if (this.shown) this.report();
      else this.client.socket.send({ type: "hide", id: this.id });
      this.apply();
    } else if (attached) {
      // Layout may have changed while the attach (or its takeover) was in
      // flight. Older servers ignore the dimensions on `attach`, and even
      // an unchanged PTY size needs a real resize to make a TUI redraw after
      // replay. Nudge only the backend, leaving the emulator's buffer intact.
      const { cols, rows } = this.emulator.fit() ?? this.emulator;
      const nudgeRows = rows < 65535 ? rows + 1 : rows - 1;
      this.client.socket.send({ type: "resize", id: this.id, cols, rows: nudgeRows });
      this.client.socket.send({ type: "resize", id: this.id, cols, rows });
    }
  }

  /** Run `cleanup` when the session is disposed or detached. */
  own(cleanup: () => void): void {
    this.cleanups.push(cleanup);
  }

  // ── PTY Size ──

  /**
   * Fit to the pane. With a server that decides the PTY size, the pane's size
   * is measured and reported, and the emulator keeps the PTY's.
   */
  fit(): void {
    if (!this.client.sizing) {
      this.emulator.fit();
      return;
    }
    if (this.measurePane() && this.shown) this.report();
    this.apply();
  }

  /**
   * The pane is shown (its panel visible on the active screen) or not. A pane
   * that is not shown takes no part in the session's PTY size.
   */
  setShown(shown: boolean): void {
    if (shown === this.shown) return;
    this.shown = shown;
    if (!this.client.sizing || this.detached) return;
    if (shown) {
      this.measurePane();
      this.report();
      this.apply();
    } else {
      this.client.socket.send({ type: "hide", id: this.id });
    }
  }

  /** Ask the server to use this connection's sizes now. */
  takeOver(): void {
    this.client.socket.send({ type: "activity", takeover: true });
  }

  /** The session's PTY size, from a `size` event on its connection. */
  handleSize(size: PtySize): void {
    if (this.detached) return;
    this.pty = size;
    this.apply();
  }

  /** Measure the pane; true when its size changed. */
  private measurePane(): boolean {
    const grid = this.emulator.measure();
    if (!grid) return false;
    const previous = this.paneSize;
    if (previous && sameGrid(previous, grid)) return false;
    // While the pane's own size decides the PTY, show the new one right away;
    // the server's answer to the report corrects it if another client decides.
    if (this.pty && previous && sameGrid(this.pty, previous)) {
      this.pty = { ...this.pty, cols: grid.cols, rows: grid.rows };
    }
    this.paneSize = grid;
    return true;
  }

  private report(): void {
    if (!this.paneSize) return;
    const { cols, rows } = this.paneSize;
    const redraw = this.redrawPending;
    this.redrawPending = false;
    this.client.socket.send(redraw
      ? { type: "resize", id: this.id, cols, rows, redraw: true }
      : { type: "resize", id: this.id, cols, rows });
  }

  /** Give the emulator the PTY's size (the pane's until the server tells) and tell the pane. */
  private apply(): void {
    const target = this.pty ?? this.paneSize;
    if (target && !sameGrid(target, { cols: this.emulator.cols, rows: this.emulator.rows })) {
      this.emulator.resize(target.cols, target.rows);
    }
    const view = this.sizeView();
    for (const cb of [...this.sizeListeners]) cb(view);
  }

  /** The session's process ended (`closed` on its connection). */
  handleClosed(exitCode: number): void {
    if (this.endCallback) {
      this.endCallback(exitCode);
    } else {
      const encoder = new TextEncoder();
      this.emulator.write(encoder.encode("\r\n\x1b[90m[session ended]\x1b[0m\r\n"));
    }
  }

  /** A `title` event for this session arrived on its connection. */
  handleTitle(event: SessionTitle): void {
    this.lastTitle = event.title;
    this.lastCwd = event.cwd;
    this.lastCmdline = event.cmdline;
    this.lastIsBaseShell = event.baseShell;
    this.lastOscTitle = event.oscTitle;
    if (this.titleCallback) {
      const title = (!event.baseShell && event.oscTitle) ? event.oscTitle : event.title;
      this.titleCallback(title, event.cwd);
    }
  }
}
