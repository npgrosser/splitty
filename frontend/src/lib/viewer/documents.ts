/**
 * The documents of one server's viewer panes (`viewer-panes`, Reading A
 * Document and Keeping A Shown Document Current), read over that server's
 * connection only (`access-control`, Viewer Documents Stay With Their
 * Connection): a pane id is resolved only against this connection's server.
 *
 * A read starts at offset 0 and asks for every next chunk with the version of
 * the first answer; a document that changes in between fails with `changed`,
 * and the read starts over. Every request waits a bounded time for its answer,
 * since a socket drops what it cannot send and no answer would ever come.
 *
 * At most one read per pane runs at a time. While a file's pane is shown and
 * the window is visible, it is read again every two seconds with `ifVersion`,
 * which costs the server a `stat`; a pane that is not shown, and every pane of
 * a hidden window, reads nothing. A snapshot that raises a pane's revision
 * reads it at once. The last content of each pane stays in a bounded cache, so
 * a pane rebuilt from a snapshot or moved to a popout window shows it at once.
 *
 * Everything a server answers is checked: sizes, offsets, versions and chunk
 * lengths are bounded, and a malformed answer fails the read.
 */

import type { ConnSocket } from "../protocol/socket.ts";
import type { OutgoingMessage, ScreenState, ViewerFailureCode } from "../protocol/types.ts";
import { fromBase64 } from "../protocol/base64.ts";
import { MAX_TEXT, plainText } from "../protocol/untrusted.ts";
import { formatBytes } from "../terminal/image_paste.ts";
import { CHUNK_BYTES, MAX_FILE_BYTES, VIEWER_CAPABILITY } from "./limits.ts";

/** How often a shown file is checked for a new version. */
export const POLL_MS = 2000;
/** How long one request may wait for its answer. */
export const READ_TIMEOUT_MS = 15_000;
/** How often a read starts over because the file changed under it, before it gives up. */
export const MAX_RESTARTS = 3;
/** Contents kept for panes nobody shows, at most this many and this many bytes. */
export const CACHE_ENTRIES = 32;
export const CACHE_BYTES = 32 * 1024 * 1024;
/** Revisions remembered from snapshots. */
const MAX_REVISIONS = 4096;
/** Longest version string taken from a server; it is opaque and only goes back. */
const MAX_VERSION = 256;
/** Longest base64 text of one chunk. */
const MAX_CHUNK_TEXT = Math.ceil(CHUNK_BYTES / 3) * 4;

export interface ViewerDocument {
  readonly bytes: Uint8Array;
  readonly version: string;
}

export type ReadFailureCode = ViewerFailureCode | "timeout" | "disconnected" | "malformed" | "unsupported";

export interface ReadFailure {
  readonly code: ReadFailureCode;
  /** What the pane says in place of the content, plain text. */
  readonly message: string;
}

export interface DocumentState {
  /** The last content read. */
  readonly document?: ViewerDocument;
  /** Why the last read failed; the pane says so in place of the content and offers to read again. */
  readonly failure?: ReadFailure;
  /** A read runs. */
  readonly reading: boolean;
}

export interface WatchOptions {
  /** Read again while shown, every two seconds: file documents. A text does not change. */
  poll: boolean;
  /** The most the document may hold. */
  limit: number;
  onChange(state: DocumentState): void;
}

export interface DocumentWatch {
  /** The state now, the cached content included. */
  readonly state: DocumentState;
  /** Whether the pane is shown: its tab visible on the active screen. */
  setShown(shown: boolean): void;
  /** Read the whole document again now. */
  reload(): void;
  close(): void;
}

/** Whether the window is visible, and when that changes. */
export interface Visibility {
  visible(): boolean;
  subscribe(listener: () => void): () => void;
}

export interface ReaderOptions {
  visibility?: Visibility;
  timeoutMs?: number;
  pollMs?: number;
}

const FAILURE_CODES: readonly ViewerFailureCode[] = [
  "unknown_pane", "not_found", "not_a_file", "too_large", "unreadable", "changed", "busy", "invalid",
];

interface Watcher {
  shown: boolean;
  readonly options: WatchOptions;
}

interface Read {
  /** No `ifVersion`: the whole document is read, also when it has the cached version. */
  readonly full: boolean;
  ifVersion?: string;
  limit: number;
  restarts: number;
  /** From the first chunk. */
  version?: string;
  size?: number;
  readonly chunks: Uint8Array[];
  received: number;
}

interface Entry {
  readonly paneId: string;
  readonly watchers: Set<Watcher>;
  document?: ViewerDocument;
  failure?: ReadFailure;
  read?: Read;
  /** Read again once the current read ends (`full` for a reload). */
  again?: { full: boolean };
  /** No more polls after this failure until something asks again. */
  stopped: boolean;
  timer?: ReturnType<typeof setTimeout>;
  lastUsed: number;
}

interface Pending {
  readonly entry: Entry;
  readonly read: Read;
  readonly offset: number;
  readonly timer: ReturnType<typeof setTimeout>;
}

function documentVisibility(): Visibility {
  if (typeof document === "undefined") return { visible: () => true, subscribe: () => () => {} };
  return {
    visible: () => document.visibilityState === "visible",
    subscribe: (listener) => {
      document.addEventListener("visibilitychange", listener);
      return () => document.removeEventListener("visibilitychange", listener);
    },
  };
}

function newRequestId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return `vr-${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

function failureMessage(code: ReadFailureCode, limit: number): string {
  switch (code) {
    case "unknown_pane": return "The server does not know this pane.";
    case "not_found": return "The file does not exist.";
    case "not_a_file": return "This is not a regular file.";
    case "too_large": return `The document is larger than ${formatBytes(limit)}.`;
    case "unreadable": return "The file cannot be read.";
    case "changed": return "The file kept changing while it was read.";
    case "busy": return "The server is busy reading other documents.";
    case "invalid": return "The server did not understand the request.";
    case "timeout": return "The server did not answer.";
    case "disconnected": return "The connection was lost.";
    case "unsupported": return "This server's Splitty does not show viewer panes. Update it to show this pane.";
    case "malformed": return "The server sent a malformed document.";
  }
}

export class ViewerDocuments {
  private readonly entries = new Map<string, Entry>();
  private readonly pending = new Map<string, Pending>();
  private readonly revisions = new Map<string, number>();
  /** The connection's latest welcome announced viewer panes. */
  private enabled = false;
  /** A welcome came, and no disconnect since. */
  private connected = false;
  private readonly visibility: Visibility;
  private readonly timeoutMs: number;
  private readonly pollMs: number;
  private unsubscribes: (() => void)[];

  constructor(private readonly socket: ConnSocket, options: ReaderOptions = {}) {
    this.visibility = options.visibility ?? documentVisibility();
    this.timeoutMs = options.timeoutMs ?? READ_TIMEOUT_MS;
    this.pollMs = options.pollMs ?? POLL_MS;
    this.unsubscribes = [
      socket.onMessage((msg) => this.handle(msg)),
      socket.onDisconnect(() => this.disconnected()),
      this.visibility.subscribe(() => this.visibilityChanged()),
    ];
  }

  /**
   * Follow the document of `paneId`. Nothing is read before the pane is
   * shown; its cached content is there at once.
   */
  watch(paneId: string, options: WatchOptions): DocumentWatch {
    let entry = this.entries.get(paneId);
    if (!entry) {
      entry = { paneId, watchers: new Set(), stopped: false, lastUsed: 0 };
      this.entries.set(paneId, entry);
    }
    const watcher: Watcher = { shown: false, options };
    entry.watchers.add(watcher);
    const current = entry;
    let closed = false;
    return {
      get state() { return stateOf(current); },
      setShown: (shown) => {
        if (closed || watcher.shown === shown) return;
        watcher.shown = shown;
        if (shown) {
          current.stopped = false;
          this.request(current, false);
        } else {
          this.schedule(current);
        }
      },
      reload: () => {
        if (closed) return;
        current.stopped = false;
        this.request(current, true);
      },
      close: () => {
        if (closed) return;
        closed = true;
        current.watchers.delete(watcher);
        if (current.watchers.size === 0) {
          clearTimeout(current.timer);
          current.timer = undefined;
          current.lastUsed = Date.now();
          this.evict();
        } else {
          this.schedule(current);
        }
      },
    };
  }

  /**
   * A snapshot of one of this server's screens: a viewer pane whose revision
   * rose is read again at once, if it is shown.
   */
  snapshot(screen: ScreenState): void {
    for (const pane of Object.values(screen.panes)) {
      const revision = pane.viewer?.revision;
      if (revision === undefined) continue;
      const previous = this.revisions.get(pane.id);
      this.revisions.delete(pane.id);
      this.revisions.set(pane.id, revision);
      if (previous === undefined || revision <= previous) continue;
      const entry = this.entries.get(pane.id);
      if (entry && shown(entry)) {
        entry.stopped = false;
        this.request(entry, false);
      }
    }
    while (this.revisions.size > MAX_REVISIONS) this.revisions.delete(this.revisions.keys().next().value!);
  }

  dispose(): void {
    for (const unsubscribe of this.unsubscribes) unsubscribe();
    this.unsubscribes = [];
    for (const pending of this.pending.values()) clearTimeout(pending.timer);
    this.pending.clear();
    for (const entry of this.entries.values()) clearTimeout(entry.timer);
    this.entries.clear();
  }

  // ── Reads ──

  /**
   * Read `entry` now, or once the read that runs ends, the connection is back
   * or the window is visible again.
   */
  private request(entry: Entry, wanted: boolean): void {
    clearTimeout(entry.timer);
    entry.timer = undefined;
    // A reload that waited stays a reload.
    const full = wanted || !!entry.again?.full;
    if (entry.read || !this.enabled || !this.connected || !this.visibility.visible()) {
      entry.again = { full };
      if (this.connected && !this.enabled) this.fail(entry, "unsupported");
      return;
    }
    entry.again = undefined;
    const ifVersion = full ? undefined : entry.document?.version;
    entry.read = {
      full, ifVersion, limit: limitOf(entry), restarts: 0, chunks: [], received: 0,
    };
    this.notify(entry);
    this.send(entry, entry.read, 0);
  }

  private send(entry: Entry, read: Read, offset: number): void {
    const requestId = newRequestId();
    const timer = setTimeout(() => {
      if (!this.pending.delete(requestId)) return;
      this.finish(entry, read, { failure: "timeout" });
    }, this.timeoutMs);
    this.pending.set(requestId, { entry, read, offset, timer });
    this.socket.send({
      type: "viewer.read",
      requestId,
      paneId: entry.paneId,
      offset,
      ...(offset > 0 && read.version ? { version: read.version } : {}),
      ...(offset === 0 && read.ifVersion ? { ifVersion: read.ifVersion } : {}),
    });
  }

  private handle(msg: OutgoingMessage): void {
    switch (msg.type) {
      case "mux.welcome": {
        this.enabled = Array.isArray(msg.capabilities) && msg.capabilities.includes(VIEWER_CAPABILITY);
        this.connected = true;
        // Every shown pane: what it shows may have changed while the
        // connection was gone. A pane that is not shown reads nothing.
        for (const entry of this.entries.values()) {
          if (!entry.read && shown(entry)) this.request(entry, false);
        }
        return;
      }
      case "viewer.chunk":
      case "viewer.unchanged":
      case "viewer.failed": {
        const pending = typeof msg.requestId === "string" ? this.pending.get(msg.requestId) : undefined;
        if (!pending) return;
        this.pending.delete(msg.requestId);
        clearTimeout(pending.timer);
        if (pending.entry.read !== pending.read) return;
        if (msg.type === "viewer.chunk") this.chunk(pending, msg);
        else if (msg.type === "viewer.unchanged") this.unchanged(pending, msg.version);
        else this.failed(pending, msg.code, msg.error);
        return;
      }
    }
  }

  private chunk(pending: Pending, msg: Extract<OutgoingMessage, { type: "viewer.chunk" }>): void {
    const { entry, read, offset } = pending;
    const { size, version, data, eof } = msg;
    const bytes = typeof data === "string" && data.length <= MAX_CHUNK_TEXT ? fromBase64(data) : null;
    if (
      bytes === null || bytes.length > CHUNK_BYTES || msg.offset !== offset
      || !Number.isSafeInteger(size) || size < 0
      || typeof version !== "string" || version.length === 0 || version.length > MAX_VERSION
      || typeof eof !== "boolean"
    ) {
      this.finish(entry, read, { failure: "malformed" });
      return;
    }
    if (offset === 0 && size > read.limit) {
      this.finish(entry, read, { failure: "too_large" });
      return;
    }
    // `size` is the whole document's: every chunk names the same size and
    // version, and the last one ends at that size. A chunk that does not fit
    // comes from a file that changed while it was read, so the read starts over.
    const consistent = (offset === 0 || (size === read.size && version === read.version))
      && read.received + bytes.length <= size
      && eof === (read.received + bytes.length === size)
      && (eof || bytes.length > 0);
    if (!consistent) {
      this.restart(entry, read);
      return;
    }
    if (offset === 0) {
      read.size = size;
      read.version = version;
    }
    read.chunks.push(bytes);
    read.received += bytes.length;
    if (!eof) {
      this.send(entry, read, read.received);
      return;
    }
    const whole = new Uint8Array(read.received);
    let at = 0;
    for (const chunk of read.chunks) {
      whole.set(chunk, at);
      at += chunk.length;
    }
    this.finish(entry, read, { document: { bytes: whole, version } });
  }

  private unchanged(pending: Pending, version: unknown): void {
    const { entry, read } = pending;
    if (pending.offset !== 0 || !read.ifVersion || version !== read.ifVersion || entry.document?.version !== version) {
      // An answer that does not fit the question: read the whole document instead.
      this.restart(entry, read);
      return;
    }
    this.finish(entry, read, {});
  }

  private failed(pending: Pending, code: unknown, error: unknown): void {
    const { entry, read } = pending;
    if (code === "changed") {
      this.restart(entry, read);
      return;
    }
    const known = FAILURE_CODES.find((c) => c === code);
    // The server's own words, as plain text, where it names no known reason.
    this.finish(entry, read, { failure: known ?? "malformed", detail: known ? undefined : plainText(error, MAX_TEXT) });
  }

  /** Start the read over at offset 0, as a whole read. */
  private restart(entry: Entry, read: Read): void {
    if (read.restarts >= MAX_RESTARTS) {
      this.finish(entry, read, { failure: "changed" });
      return;
    }
    read.restarts++;
    read.ifVersion = undefined;
    read.version = undefined;
    read.size = undefined;
    read.chunks.length = 0;
    read.received = 0;
    this.send(entry, read, 0);
  }

  private finish(entry: Entry, read: Read, outcome: { document?: ViewerDocument; failure?: ReadFailureCode; detail?: string }): void {
    if (entry.read !== read) return;
    entry.read = undefined;
    const { failure } = outcome;
    if (outcome.document) {
      entry.document = outcome.document;
      entry.failure = undefined;
    } else if (failure === "busy" || (failure === "disconnected" && entry.document)) {
      // Passing: the content stays, and the pane asks again.
      entry.again ??= { full: read.full };
    } else if (failure) {
      if (failure === "unknown_pane" || failure === "invalid") entry.stopped = true;
      entry.failure = { code: failure, message: outcome.detail || failureMessage(failure, read.limit) };
    } else {
      entry.failure = undefined;
    }
    this.notify(entry);
    if (entry.again && failure !== "busy" && this.connected && shown(entry)) {
      this.request(entry, false);
      return;
    }
    this.schedule(entry, failure === "busy");
  }

  private fail(entry: Entry, code: ReadFailureCode): void {
    entry.failure = { code, message: failureMessage(code, limitOf(entry)) };
    this.notify(entry);
  }

  /** The next poll of a shown file, or the retry of a read the server was too busy for. */
  private schedule(entry: Entry, retry = false): void {
    clearTimeout(entry.timer);
    entry.timer = undefined;
    if (entry.read || entry.stopped || !this.enabled || !this.connected || !this.visibility.visible()) return;
    const polled = [...entry.watchers].some((w) => w.shown && w.options.poll);
    if (!polled && !(retry && shown(entry))) return;
    entry.timer = setTimeout(() => {
      entry.timer = undefined;
      if (shown(entry)) this.request(entry, false);
    }, this.pollMs);
  }

  /** A hidden window reads nothing; a window visible again reads what it shows. */
  private visibilityChanged(): void {
    const visible = this.visibility.visible();
    for (const entry of this.entries.values()) {
      if (!visible) {
        clearTimeout(entry.timer);
        entry.timer = undefined;
      } else if (shown(entry) && !entry.read && (!entry.stopped || entry.again)) {
        this.request(entry, false);
      }
    }
  }

  private disconnected(): void {
    this.connected = false;
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const { entry, read, timer } of pending) {
      clearTimeout(timer);
      this.finish(entry, read, { failure: "disconnected" });
    }
    for (const entry of this.entries.values()) {
      clearTimeout(entry.timer);
      entry.timer = undefined;
    }
  }

  private notify(entry: Entry): void {
    const state = stateOf(entry);
    for (const watcher of [...entry.watchers]) watcher.options.onChange(state);
  }

  /** Forget the oldest contents nobody shows beyond the cache's bounds. */
  private evict(): void {
    const idle = [...this.entries.values()]
      .filter((entry) => entry.watchers.size === 0)
      .sort((a, b) => a.lastUsed - b.lastUsed);
    let bytes = idle.reduce((sum, entry) => sum + (entry.document?.bytes.length ?? 0), 0);
    let count = idle.length;
    for (const entry of idle) {
      if (count <= CACHE_ENTRIES && bytes <= CACHE_BYTES) break;
      this.drop(entry);
      count--;
      bytes -= entry.document?.bytes.length ?? 0;
    }
  }

  /** Forget `entry`; an answer to its read is ignored. */
  private drop(entry: Entry): void {
    this.entries.delete(entry.paneId);
    clearTimeout(entry.timer);
    entry.timer = undefined;
    entry.read = undefined;
    for (const [requestId, pending] of this.pending) {
      if (pending.entry !== entry) continue;
      clearTimeout(pending.timer);
      this.pending.delete(requestId);
    }
  }
}

function stateOf(entry: Entry): DocumentState {
  return { document: entry.document, failure: entry.failure, reading: !!entry.read };
}

function shown(entry: Entry): boolean {
  for (const watcher of entry.watchers) if (watcher.shown) return true;
  return false;
}

function limitOf(entry: Entry): number {
  let limit = 0;
  for (const watcher of entry.watchers) limit = Math.max(limit, watcher.options.limit);
  return Math.min(limit || MAX_FILE_BYTES, MAX_FILE_BYTES);
}
