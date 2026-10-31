/**
 * The socket to a host's daemon through the local server's hub (`/ws/remote`,
 * `remote-screens`).
 *
 * A hub socket has two phases. Until `remote.ready` the hub speaks for itself:
 * `remote.progress` per step, then `remote.ready` or `remote.error`; anything
 * else ends the attempt. After `remote.ready` every message is the host
 * daemon's own and reaches the connection's handlers, except `remote.*`: only
 * a host could send one then, to fake the hub's progress or errors.
 *
 * Messages sent while the socket does not relay wait in a queue and go out at
 * the next `remote.ready`. Typed input never waits, so nothing typed reaches a
 * host late.
 *
 * A connection that drops after `remote.ready` (the hub's close code 4001, or
 * any other close) reconnects on its own: a fresh ticket per attempt, a delay
 * growing to 10 seconds, until an error that retrying cannot fix (`failed`) or
 * until the host stayed unreachable for two minutes (`offline`). A first
 * connect that fails is not retried; the user sees the error and retries. A
 * failed ticket request is a failed attempt like any other, never the local
 * login's failure (`stores/auth.svelte.ts` follows the local socket only).
 *
 * A retry the user confirmed as an update opens the hub with `update=1`
 * (`remote-screens`, Host Update), which may stop the host's daemon. Only that
 * one attempt carries it; later ones, automatic or not, never do.
 */

import type { IncomingMessage, OutgoingMessage } from "../protocol/types.ts";
import { belongsToConnection, SocketHandlers, type ConnSocket } from "../protocol/socket.ts";
import { MAX_NAME, MAX_TEXT, MAX_TITLE, plainLines, plainText } from "../protocol/untrusted.ts";

/** The hub's error codes (`docs/protocol.ts`, RemoteErrorCode). */
export type RemoteErrorCode =
  | "invalid_target" | "host_key" | "auth" | "unreachable" | "unsupported_host"
  | "install" | "daemon" | "incompatible" | "bridge" | "internal";

/**
 * Why an attempt failed: the hub's code, or the client's own when the hub
 * could not say (`ticket`: no ticket; `closed`: the socket closed before
 * `remote.ready` without an error; `protocol`: the hub sent something else
 * than its own messages before `remote.ready`).
 */
export type RemoteFailureCode = RemoteErrorCode | "ticket" | "closed" | "protocol";

export interface RemoteFailure {
  code: RemoteFailureCode;
  /** The hub's message; may carry `ssh`'s error output, as plain text. */
  message: string;
  hint?: string;
  /** For `incompatible`: the host daemon's version and protocol. */
  version?: string;
  protocol?: number;
  /** For `incompatible`: whether the host daemon's sessions outlive it, so an update ends none. */
  keepsSessions?: boolean;
  /** For `incompatible`: the daemon's screens, which an update would end. */
  screens?: HostScreen[];
}

export interface RemoteProgressStep {
  /** `install`, `daemon`, `started` or `bridge`. */
  step: string;
  message: string;
}

/** A screen of the host as the handshake listed it. Untrusted host data. */
export interface HostScreen {
  id: string;
  name: string;
  /** How many panes it has; unknown counts as 0. */
  panes: number;
}

export interface RemoteReadyInfo {
  /** The target as the hub validated it. */
  target: string;
  /** `keepsSessions`: its sessions outlive it, so an update ends none (false when the hub does not say). */
  daemon: { version: string; protocol: number; keepsSessions: boolean; started: boolean };
  screens: HostScreen[];
}

/**
 * - `connecting`: the first connect is under way;
 * - `ready`: relaying the host daemon's messages;
 * - `reconnecting`: the connection dropped, attempts are running;
 * - `failed`: an error that needs the user (`retry`);
 * - `offline`: the host stayed unreachable (`retry`);
 * - `closed`: the socket was closed for good.
 */
export type RemotePhase = "connecting" | "ready" | "reconnecting" | "failed" | "offline" | "closed";

export interface RemoteState {
  readonly phase: RemotePhase;
  /** The steps of the attempt in flight (or of the failed first connect), oldest first. */
  readonly progress: readonly RemoteProgressStep[];
  /** Why the last attempt failed, until one succeeds. */
  readonly error?: RemoteFailure;
  /** From the last `remote.ready`. */
  readonly ready?: RemoteReadyInfo;
  /** Reconnect attempts since the connection was lost. */
  readonly attempt: number;
}

/** The part of a browser WebSocket the hub socket uses. */
export interface HubWebSocket {
  binaryType: BinaryType;
  readonly readyState: number;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: { code: number; reason: string }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

export interface RemoteSocketDeps {
  /** A single-use ticket from the local server, or null. */
  fetchTicket(): Promise<string | null>;
  /** Open `/ws/remote` for `target` with `ticket`, with `update=1` when `update` is set. */
  open(target: string, ticket: string, options: { update: boolean }): HubWebSocket;
  now?(): number;
  random?(): number;
}

/** The hub's close code of a relaying socket whose host side ended. */
export const REMOTE_HOST_LOST = 4001;
export const MAX_RECONNECT_DELAY = 10_000;
/** How long reconnect attempts run before the host counts as offline (provisional, tbd). */
export const GIVE_UP_AFTER = 120_000;
/** Messages kept while the socket does not relay; the oldest go first. */
export const MAX_QUEUE = 256;

const OPEN = 1;

const HUB_CODES: readonly RemoteErrorCode[] = [
  "invalid_target", "host_key", "auth", "unreachable", "unsupported_host",
  "install", "daemon", "incompatible", "bridge", "internal",
];

/** Whether another attempt can help: the hub's transient codes and the client's own. */
export function retryable(code: RemoteFailureCode): boolean {
  switch (code) {
    case "unreachable":
    case "daemon":
    case "bridge":
    case "internal":
    case "ticket":
    case "closed":
    case "protocol":
      return true;
    default:
      return false;
  }
}

export class RemoteSocket extends SocketHandlers implements ConnSocket {
  private ws: HubWebSocket | null = null;
  /** Counts attempts; a ticket that arrives for an older one is dropped. */
  private attemptId = 0;
  private fetching = false;
  /** The next attempt opens the hub with `update=1`. */
  private updateNext = false;
  /** The current socket got `remote.ready`. */
  private relaying = false;
  /** When a ready connection dropped; undefined before the first `remote.ready`. */
  private droppedAt: number | undefined;
  private queue: IncomingMessage[] = [];
  private timer: ReturnType<typeof setTimeout> | undefined;
  private current: RemoteState = { phase: "connecting", progress: [], attempt: 0 };
  private readonly stateListeners = new Set<(state: RemoteState) => void>();
  private readonly readyListeners = new Set<(info: RemoteReadyInfo) => void>();

  constructor(
    /** The target as the user gave it. */
    readonly target: string,
    private readonly deps: RemoteSocketDeps,
  ) {
    super();
  }

  get state(): RemoteState {
    return this.current;
  }

  /** Called with every new state. Returns unsubscribe. */
  onState(cb: (state: RemoteState) => void): () => void {
    this.stateListeners.add(cb);
    return () => this.stateListeners.delete(cb);
  }

  /** Called on every `remote.ready`, before the host daemon's first message. Returns unsubscribe. */
  onReady(cb: (info: RemoteReadyInfo) => void): () => void {
    this.readyListeners.add(cb);
    return () => this.readyListeners.delete(cb);
  }

  /** Start the first connect; does nothing while an attempt runs or the socket relays. */
  connect(): void {
    if (this.current.phase === "closed" || this.ws || this.fetching) return;
    void this.attempt();
  }

  /**
   * Try again now: after `failed` or `offline`, or to skip the wait between
   * reconnect attempts. A connection that was ready starts a new two minutes.
   * With `update`, this attempt asks the hub to update the host's daemon; only
   * after the user confirmed what that ends. A connected socket then lets its
   * connection go, as a drop would, and makes that attempt at once.
   */
  retry(options?: { update?: boolean }): void {
    if (this.current.phase === "closed" || this.fetching) return;
    if (this.ws) {
      // A connected host of another version is updated by a new attempt:
      // the hub replaces its daemon only for a socket opened with `update=1`.
      if (!options?.update || !this.relaying) return;
      const ws = this.ws;
      this.ws = null;
      detach(ws).close(1000);
      this.relaying = false;
      this.queue = [];
      this.droppedAt = this.now();
      this.disconnected();
    }
    this.updateNext = options?.update === true;
    clearTimeout(this.timer);
    this.timer = undefined;
    if (this.droppedAt !== undefined) this.droppedAt = this.now();
    this.update({ attempt: 0 });
    void this.attempt();
  }

  send(msg: IncomingMessage): void {
    if (this.current.phase === "closed") return;
    if (this.relaying && this.ws?.readyState === OPEN) {
      this.ws.send(JSON.stringify(msg));
      return;
    }
    // Input, images, uploads, file offers and editor messages belong to this connection; never replay them.
    if (msg.type === "input" || belongsToConnection(msg)) return;
    if (this.queue.length >= MAX_QUEUE) this.queue.shift();
    this.queue.push(msg);
  }

  /** End the socket for good; the hub then ends its `ssh`, and the host's daemon keeps running. */
  close(): void {
    if (this.current.phase === "closed") return;
    this.attemptId++;
    this.fetching = false;
    this.updateNext = false;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.queue = [];
    this.relaying = false;
    const ws = this.ws;
    this.ws = null;
    if (ws) detach(ws).close(1000);
    this.update({ phase: "closed", progress: [] });
  }

  dispose(): void {
    this.close();
    this.clearHandlers();
    this.stateListeners.clear();
    this.readyListeners.clear();
  }

  /** After `remote.ready`, a `remote.*` message can only come from the host. */
  protected override admit(msg: OutgoingMessage): boolean {
    return !msg.type.startsWith("remote.");
  }

  // ── Attempts ──

  private async attempt(): Promise<void> {
    const id = ++this.attemptId;
    const update = this.updateNext;
    this.updateNext = false;
    this.fetching = true;
    this.update({ phase: this.droppedAt === undefined ? "connecting" : "reconnecting", progress: [] });

    let ticket: string | null;
    try {
      ticket = await this.deps.fetchTicket();
    } catch {
      ticket = null;
    }
    if (id !== this.attemptId) return;
    this.fetching = false;
    if (!ticket) {
      this.failed({ code: "ticket", message: "The local Splitty server gave no ticket for the connection." });
      return;
    }

    let ws: HubWebSocket;
    try {
      ws = this.deps.open(this.target, ticket, { update });
    } catch (err) {
      this.failed({ code: "closed", message: `The connection could not be opened: ${String(err)}` });
      return;
    }
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    this.relaying = false;
    ws.onmessage = (ev) => { if (this.ws === ws) this.receive(ws, ev.data); };
    ws.onerror = () => {};
    ws.onclose = () => { if (this.ws === ws) this.closed(); };
  }

  private receive(ws: HubWebSocket, data: unknown): void {
    if (this.relaying) {
      // Binary frames are terminal output, text frames JSON messages; both
      // reach the handlers through `deliver`, `remote.*` excepted.
      this.deliver(data);
      return;
    }

    // Before `remote.ready`: the hub's own text messages only.
    const msg = typeof data === "string" ? parseObject(data) : undefined;
    switch (msg?.type) {
      case "remote.progress":
        this.update({
          progress: [...this.current.progress, {
            step: plainText(msg.step, MAX_NAME),
            message: plainText(msg.message, MAX_TEXT),
          }],
        });
        return;
      case "remote.ready": {
        const info = readyInfo(msg);
        if (!info) {
          this.abandon(ws, { code: "protocol", message: "The local Splitty server sent a malformed ready message." });
          return;
        }
        this.relaying = true;
        this.update({ phase: "ready", progress: [], error: undefined, ready: info, attempt: 0 });
        for (const cb of [...this.readyListeners]) cb(info);
        for (const queued of this.queue.splice(0)) ws.send(JSON.stringify(queued));
        return;
      }
      case "remote.error":
        this.abandon(ws, failure(msg));
        return;
      default:
        this.abandon(ws, {
          code: "protocol",
          message: "The local Splitty server sent an unexpected message before the host was ready.",
        });
    }
  }

  /** The current socket closed. */
  private closed(): void {
    this.ws = null;
    if (!this.relaying) {
      this.failed({ code: "closed", message: "The connection to the local Splitty server closed before the host was ready." });
      return;
    }
    // A ready connection was lost: the host's close code 4001, or the local
    // server went away. Either way the connection's views wait for a new one.
    this.relaying = false;
    this.queue = [];
    this.droppedAt = this.now();
    this.update({ phase: "reconnecting", progress: [], attempt: 0 });
    this.disconnected();
    this.schedule();
  }

  /** End the current socket because of `error`. */
  private abandon(ws: HubWebSocket, error: RemoteFailure): void {
    this.ws = null;
    detach(ws).close(1000);
    this.failed(error);
  }

  private failed(error: RemoteFailure): void {
    this.fetching = false;
    if (this.droppedAt === undefined) {
      // The first connect: the user decides. The progress shows how far it got.
      this.queue = [];
      this.update({ phase: "failed", error });
    } else if (!retryable(error.code)) {
      this.update({ phase: "failed", progress: [], error });
    } else if (this.now() - this.droppedAt >= GIVE_UP_AFTER) {
      this.update({ phase: "offline", progress: [], error });
    } else {
      this.update({ phase: "reconnecting", error });
      this.schedule();
    }
  }

  private schedule(): void {
    const attempt = this.current.attempt;
    const delay = Math.min(1000 * Math.pow(1.5, attempt) + this.random() * 500, MAX_RECONNECT_DELAY);
    this.update({ attempt: attempt + 1 });
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.attempt();
    }, delay);
  }

  private update(change: Partial<RemoteState>): void {
    this.current = { ...this.current, ...change };
    for (const cb of [...this.stateListeners]) cb(this.current);
  }

  private now(): number {
    return this.deps.now?.() ?? Date.now();
  }

  private random(): number {
    return this.deps.random?.() ?? Math.random();
  }
}

/** A socket this class lets go of: its events reach nothing any more. */
function detach(ws: HubWebSocket): HubWebSocket {
  ws.onopen = null;
  ws.onmessage = null;
  ws.onclose = null;
  ws.onerror = null;
  return ws;
}

type Raw = Record<string, unknown>;

function parseObject(text: string): (Raw & { type?: unknown }) | undefined {
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Raw) : undefined;
  } catch {
    return undefined;
  }
}

function readyInfo(msg: Raw): RemoteReadyInfo | null {
  const daemon = msg.daemon as Raw | undefined;
  if (typeof daemon !== "object" || daemon === null) return null;
  if (typeof daemon.version !== "string" || typeof daemon.protocol !== "number") return null;
  return {
    target: plainText(msg.target, MAX_NAME),
    daemon: {
      version: plainText(daemon.version, MAX_NAME),
      protocol: daemon.protocol,
      keepsSessions: daemon.keepsSessions === true,
      started: daemon.started === true,
    },
    screens: hostScreens(msg.screens),
  };
}

/** The host's screens of a `remote.ready`; entries without an id are dropped. */
export function hostScreens(value: unknown): HostScreen[] {
  if (!Array.isArray(value)) return [];
  const screens: HostScreen[] = [];
  for (const entry of value) {
    if (typeof entry !== "object" || entry === null || typeof entry.id !== "string") continue;
    const panes = typeof entry.panes === "number" && Number.isInteger(entry.panes) && entry.panes > 0 ? entry.panes : 0;
    screens.push({ id: entry.id, name: plainText(entry.name, MAX_NAME) || plainText(entry.id, MAX_NAME), panes });
  }
  return screens;
}

function failure(msg: Raw): RemoteFailure {
  const code = HUB_CODES.find((c) => c === msg.code) ?? "internal";
  const result: RemoteFailure = { code, message: plainLines(msg.message, MAX_TEXT) };
  const hint = plainText(msg.hint, MAX_TEXT);
  if (hint) result.hint = hint;
  if (typeof msg.version === "string") result.version = plainText(msg.version, MAX_TITLE);
  if (typeof msg.protocol === "number") result.protocol = msg.protocol;
  if (typeof msg.keepsSessions === "boolean") result.keepsSessions = msg.keepsSessions;
  if (Array.isArray(msg.screens)) result.screens = hostScreens(msg.screens);
  return result;
}
