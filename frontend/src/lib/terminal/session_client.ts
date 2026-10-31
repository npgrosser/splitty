/**
 * One server connection's side of the session protocol.
 *
 * Holds what used to be module state: the `create` queue, the sessions shown
 * from this server, which pane shows which session and on which screen, and
 * the server's home directory. Session events (`closed`, `title`, `bell`) are
 * routed from this connection's socket to this connection's sessions only, so a
 * session id of one server never reaches a session of another.
 */

import type { ConnSocket } from "../protocol/socket.ts";
import type { IncomingMessage, OutgoingMessage, RestoreMode, SizePolicy } from "../protocol/types.ts";
import { MAX_TITLE, MAX_VALUE, optionalBounded, plainText } from "../protocol/untrusted.ts";
import { toBase64 } from "../protocol/base64.ts";
import { deleteIfCurrent, type AttachedSession, type AttachRegistry } from "./attach_registry.ts";
import type { TerminalSession } from "./terminal_session.ts";

export interface SessionRequest {
  cols: number;
  rows: number;
  command?: string;
  dir?: string;
  root?: string;
  restoreMode?: RestoreMode;
  paneId?: string;
  /** The screen's id on this server. */
  screenId?: string;
  shellProfileId?: string;
}

export interface SessionCreation {
  sessionId: string;
  shellProfileId?: string;
}

/** A `title` event, checked and capped. */
export interface SessionTitle {
  title: string;
  cwd: string;
  cmdline: string;
  baseShell: boolean;
  oscTitle: string;
}

/** A session's PTY size, as its server's size policy set it. */
export interface PtySize {
  cols: number;
  rows: number;
  policy: SizePolicy;
}

/** The capability of servers that decide PTY sizes from their clients' reports. */
export const SIZE_CAPABILITY = "pty-size-v1";
/**
 * The largest PTY size taken from a server. A host must not make a terminal
 * allocate a huge buffer; a larger size is ignored.
 */
export const MAX_PTY_COLS = 1000;
export const MAX_PTY_ROWS = 500;

const POLICIES: readonly SizePolicy[] = ["active", "latest", "smallest"];

/** The size a `size` event carries, or null for one out of bounds or malformed. */
export function ptySize(msg: { cols?: unknown; rows?: unknown; policy?: unknown }): PtySize | null {
  const { cols, rows, policy } = msg;
  if (!Number.isInteger(cols) || !Number.isInteger(rows)) return null;
  if ((cols as number) < 1 || (cols as number) > MAX_PTY_COLS) return null;
  if ((rows as number) < 1 || (rows as number) > MAX_PTY_ROWS) return null;
  if (!POLICIES.includes(policy as SizePolicy)) return null;
  return { cols: cols as number, rows: rows as number, policy: policy as SizePolicy };
}

/** How long a `save_image` may take before its paste is given up. */
export const SAVE_IMAGE_TIMEOUT_MS = 30_000;
/** The answer of a server older than `save_image`. */
const UNKNOWN_SAVE_IMAGE = "unknown message type: save_image";
/** Longest path a server's `image_saved` may name. */
const MAX_IMAGE_PATH = 4096;

/** How much of a file one `upload` message carries. */
export const UPLOAD_CHUNK_BYTES = 1024 * 1024;
/** The largest file an upload sends; the server refuses larger ones. */
export const MAX_UPLOAD_BYTES = 512 * 1024 * 1024;
/** How long the answer to one `upload` chunk may take before the upload is given up. */
export const UPLOAD_CHUNK_TIMEOUT_MS = 30_000;
/** The answer of a server older than `upload`. */
const UNKNOWN_UPLOAD = "unknown message type: upload";

/**
 * Why an image or a file was not saved; `needsUpdate` for a server that does
 * not know `save_image` or `upload`.
 */
export class ImageSaveError extends Error {
  constructor(message: string, readonly needsUpdate = false) {
    super(message);
  }
}

function requestId(prefix = "img"): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  return `${prefix}-${[...bytes].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
}

/** An absolute path without hidden characters, as a pane may receive it; the server is untrusted. */
function savedPath(path: unknown): string | null {
  if (typeof path !== "string" || !path.startsWith("/") || path.length > MAX_IMAGE_PATH) return null;
  return plainText(path, MAX_IMAGE_PATH) === path ? path : null;
}

export class SessionClient {
  /** Home directory reported by this server, used for ~ substitution in paths. */
  homeDir = "";
  /** This server decides PTY sizes (`pty-size-v1` in its latest welcome). */
  sizing = false;

  private readonly sessions = new Map<string, TerminalSession>();
  private readonly paneToSession = new Map<string, string>();
  /** Pane → the key of its screen in the window. */
  private readonly paneToScreen = new Map<string, string>();
  private createQueue: Promise<unknown> = Promise.resolve();
  private readonly metadataListeners = new Set<() => void>();
  private metadataDirty = false;
  /** The latest PTY size of each session, also before its view is wired. */
  private readonly sizes = new Map<string, PtySize>();
  private unsubscribe: () => void;

  constructor(
    readonly socket: ConnSocket,
    readonly attachments: AttachRegistry,
    private readonly createTimeoutMs = 5000,
  ) {
    const unsubMessage = socket.onMessage((msg) => this.route(msg));
    // Sizes of a lost connection are stale; an attach on the next one gets its own.
    const unsubDisconnect = socket.onDisconnect(() => this.sizes.clear());
    this.unsubscribe = () => {
      unsubMessage();
      unsubDisconnect();
    };
  }

  // ── Create Queue ──
  // Serialize "create" requests so each one gets its own "created" response.

  requestSession(req: SessionRequest): Promise<SessionCreation> {
    const next = this.createQueue.catch(() => undefined).then(
      () =>
        new Promise<SessionCreation>((resolve, reject) => {
          const timeout = setTimeout(() => {
            unsub();
            reject(new Error("Timeout waiting for session creation"));
          }, this.createTimeoutMs);

          const unsub = this.socket.onMessage((msg: OutgoingMessage) => {
            if (msg.type === "created") {
              clearTimeout(timeout);
              unsub();
              if (typeof msg.homeDir === "string" && msg.homeDir) this.homeDir = msg.homeDir;
              resolve({ sessionId: msg.sessionId, shellProfileId: msg.shellProfileId });
            } else if (msg.type === "error" && !msg.sessionId && msg.error !== UNKNOWN_SAVE_IMAGE && msg.error !== UNKNOWN_UPLOAD) {
              clearTimeout(timeout);
              unsub();
              reject(new Error(String(msg.error)));
            }
          });

          this.socket.send({ type: "create", ...req });
        }),
    );
    this.createQueue = next;
    return next;
  }

  // ── Pasted Images ──

  /**
   * Store an image (base64) on this server for a pane's paste; resolves with
   * the absolute path of the file the server chose.
   */
  saveImage(data: string, timeoutMs = SAVE_IMAGE_TIMEOUT_MS): Promise<string> {
    const id = requestId();
    return new Promise<string>((resolve, reject) => {
      const finish = (settle: () => void) => {
        clearTimeout(timeout);
        unsub();
        settle();
      };
      const timeout = setTimeout(() => finish(() => reject(new ImageSaveError("The server did not answer."))), timeoutMs);
      const unsub = this.socket.onMessage((msg: OutgoingMessage) => {
        if (msg.type === "image_saved" && msg.requestId === id) {
          const path = savedPath(msg.path);
          finish(() => (path ? resolve(path) : reject(new ImageSaveError("The server named no usable file."))));
        } else if (msg.type === "image_failed" && msg.requestId === id) {
          finish(() => reject(new ImageSaveError(plainText(msg.error, MAX_VALUE) || "The server refused the image.")));
        } else if (msg.type === "error" && !msg.sessionId && msg.error === UNKNOWN_SAVE_IMAGE) {
          finish(() => reject(new ImageSaveError("This server's Splitty is too old to take images.", true)));
        }
      });
      this.socket.send({ type: "save_image", requestId: id, data });
    });
  }

  // ── Uploaded Files ──

  /**
   * Store `file` on this server for a pane's drop or paste, in chunks of
   * {@link UPLOAD_CHUNK_BYTES}, each sent only once the server answered the
   * last; resolves with the absolute path of the file the server stored.
   * `onProgress` gets the bytes the server has received.
   */
  async uploadFile(
    file: Blob & { readonly name: string },
    onProgress?: (received: number) => void,
    timeoutMs = UPLOAD_CHUNK_TIMEOUT_MS,
  ): Promise<string> {
    if (file.size > MAX_UPLOAD_BYTES) throw new ImageSaveError(`${file.name || "The file"} is larger than 512 MB.`);
    const id = requestId("up");
    let offset = 0;
    for (;;) {
      const bytes = new Uint8Array(await file.slice(offset, offset + UPLOAD_CHUNK_BYTES).arrayBuffer());
      const end = offset + bytes.length;
      const chunk: Extract<IncomingMessage, { type: "upload" }> = { type: "upload", requestId: id, offset, data: toBase64(bytes) };
      if (offset === 0) Object.assign(chunk, { name: file.name, size: file.size });
      const answer = await this.uploadChunk(chunk, timeoutMs);
      if (answer.saved !== undefined) {
        if (end !== file.size) throw new ImageSaveError("The server stored the file before all of it was sent.");
        onProgress?.(file.size);
        return answer.saved;
      }
      if (answer.received !== end || end >= file.size) throw new ImageSaveError("The server lost track of the upload.");
      onProgress?.(end);
      offset = end;
    }
  }

  /** Send one `upload` chunk and wait for the server's answer to it. */
  private uploadChunk(
    chunk: Extract<IncomingMessage, { type: "upload" }>,
    timeoutMs: number,
  ): Promise<{ received?: number; saved?: string }> {
    return new Promise((resolve, reject) => {
      const finish = (settle: () => void) => {
        clearTimeout(timeout);
        unsubMessage();
        unsubDisconnect();
        settle();
      };
      const timeout = setTimeout(() => finish(() => reject(new ImageSaveError("The server did not answer."))), timeoutMs);
      const unsubDisconnect = this.socket.onDisconnect(() => finish(() => reject(new ImageSaveError("The connection was lost during the upload."))));
      const unsubMessage = this.socket.onMessage((msg: OutgoingMessage) => {
        if (msg.type === "upload_progress" && msg.requestId === chunk.requestId) {
          const received = msg.received;
          finish(() => (Number.isSafeInteger(received) ? resolve({ received }) : reject(new ImageSaveError("The server lost track of the upload."))));
        } else if (msg.type === "upload_saved" && msg.requestId === chunk.requestId) {
          const path = savedPath(msg.path);
          finish(() => (path ? resolve({ saved: path }) : reject(new ImageSaveError("The server named no usable file."))));
        } else if (msg.type === "upload_failed" && msg.requestId === chunk.requestId) {
          finish(() => reject(new ImageSaveError(plainText(msg.error, MAX_VALUE) || "The server refused the file.")));
        } else if (msg.type === "error" && !msg.sessionId && msg.error === UNKNOWN_UPLOAD) {
          finish(() => reject(new ImageSaveError("This server's Splitty is too old to take files.", true)));
        }
      });
      this.socket.send(chunk);
    });
  }

  /** Attach to a session of this server; see `AttachRegistry.attach`. */
  async attach(sessionId: string, cols: number, rows: number): Promise<AttachedSession> {
    const attached = await this.attachments.attach(sessionId, cols, rows);
    if (attached.homeDir) this.homeDir = attached.homeDir;
    return attached;
  }

  // ── Sessions and Panes ──

  register(session: TerminalSession): void {
    this.sessions.set(session.id, session);
  }

  /** Forget `session`, unless a newer view of the same session replaced it. */
  unregister(session: TerminalSession): void {
    deleteIfCurrent(this.sessions, session.id, session);
  }

  get(sessionId: string): TerminalSession | undefined {
    return this.sessions.get(sessionId);
  }

  /** The session's PTY size from its server's latest `size` event. */
  sizeOf(sessionId: string): PtySize | undefined {
    return this.sizes.get(sessionId);
  }

  all(): IterableIterator<TerminalSession> {
    return this.sessions.values();
  }

  /** `paneId` shows `sessionId`, on the screen with the window key `screenKey`. */
  linkPane(paneId: string, sessionId: string, screenKey: string): void {
    this.paneToSession.set(paneId, sessionId);
    this.paneToScreen.set(paneId, screenKey);
  }

  unlinkPane(paneId: string): void {
    this.paneToSession.delete(paneId);
    this.paneToScreen.delete(paneId);
  }

  forPane(paneId: string): TerminalSession | undefined {
    const sid = this.paneToSession.get(paneId);
    return sid ? this.sessions.get(sid) : undefined;
  }

  screenForPane(paneId: string): string | undefined {
    return this.paneToScreen.get(paneId);
  }

  paneForSession(sessionId: string): string | undefined {
    for (const [paneId, sid] of this.paneToSession) {
      if (sid === sessionId) return paneId;
    }
    return undefined;
  }

  // ── Session Metadata Change Subscription ──
  // Notifies listeners when any session's title/cwd/process changes,
  // so the drawer can re-render with up-to-date info.

  subscribeMetadata(cb: () => void): () => void {
    this.metadataListeners.add(cb);
    return () => this.metadataListeners.delete(cb);
  }

  private metadataChanged(): void {
    if (this.metadataDirty) return;
    this.metadataDirty = true;
    queueMicrotask(() => {
      this.metadataDirty = false;
      for (const cb of this.metadataListeners) cb();
    });
  }

  /** Stop routing; the sessions are left to their views. */
  dispose(): void {
    this.unsubscribe();
    this.metadataListeners.clear();
  }

  // ── Routing ──

  private route(msg: OutgoingMessage): void {
    switch (msg.type) {
      case "mux.welcome":
        this.sizing = Array.isArray(msg.capabilities) && msg.capabilities.includes(SIZE_CAPABILITY);
        break;
      case "size": {
        const size = ptySize(msg);
        if (!size || typeof msg.sessionId !== "string") break;
        this.sizes.set(msg.sessionId, size);
        this.sessions.get(msg.sessionId)?.handleSize(size);
        break;
      }
      case "closed": {
        const exitCode = typeof msg.exitCode === "number" ? msg.exitCode : -1;
        this.sizes.delete(msg.sessionId);
        this.sessions.get(msg.sessionId)?.handleClosed(exitCode);
        break;
      }
      case "title": {
        const session = this.sessions.get(msg.sessionId);
        if (!session) break;
        session.handleTitle({
          title: plainText(msg.title, MAX_TITLE),
          cwd: optionalBounded(msg.cwd, MAX_VALUE) ?? "",
          cmdline: optionalBounded(msg.cmdline, MAX_VALUE) ?? "",
          baseShell: msg.baseShell === true,
          oscTitle: plainText(msg.oscTitle, MAX_TITLE),
        });
        this.metadataChanged();
        break;
      }
      case "bell":
        // Logged only; the UI indicator is not designed yet.
        if (this.sessions.has(msg.sessionId)) console.debug("[bell] session", msg.sessionId);
        break;
    }
  }
}
