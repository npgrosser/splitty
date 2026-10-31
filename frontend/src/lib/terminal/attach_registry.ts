/**
 * Attaches in flight and the routing of terminal output frames.
 *
 * The server streams a session at most once per connection (terminal-sessions,
 * Client Handling of Session Streams), so the client keeps at most one `attach`
 * per session in flight, hands its buffered output to whichever view ends up
 * owning it, and sends `detach` only when no view holds or waits for the
 * session any more.
 *
 * Every binary frame goes through one router: while an attach for its session
 * is in flight, frames are buffered for it (the server sends the replay before
 * `attached`); otherwise they go to the views wired to that session.
 */

import { ReplayOutput } from "./replay_output.ts";
import type { IncomingMessage, OutgoingMessage } from "../protocol/types.ts";

/** The part of the socket the registry uses. */
export interface AttachSocket {
  send(msg: IncomingMessage): void;
  onMessage(handler: (msg: OutgoingMessage) => void): () => void;
  onBinaryMessage(handler: (id: string, data: Uint8Array) => void): () => void;
  /** The connection was lost; attaches in flight on it will never complete. */
  onDisconnect?(handler: () => void): () => void;
}

/** Receives a session's output frames for one view. */
export type FrameSink = (data: Uint8Array) => void;

/**
 * The attach was taken over by a newer view of the same session before it
 * completed. Not a failure: the newer view receives the session's output.
 */
export class AttachTransferredError extends Error {
  constructor(readonly sessionId: string) {
    super(`attach of session ${sessionId} was taken over by a newer view`);
    this.name = "AttachTransferredError";
  }
}

/** A completed attach, waiting for its owner to wire a view to it. */
export interface AttachedSession {
  readonly homeDir?: string;
  /**
   * Wire the view's sink: the buffered output is written to it first, then
   * every later frame. Throws {@link AttachTransferredError} when a newer view
   * took the attach over in the meantime.
   */
  wire(sink: FrameSink): ViewHandle;
}

/** A view wired to a session's output. */
export interface ViewHandle {
  readonly sessionId: string;
}

interface Owner {
  token: number;
  resolve: (attached: AttachedSession) => void;
  reject: (err: Error) => void;
}

interface InFlight {
  owner: Owner;
  /** Frames received for the session since `attach` was sent. */
  buffer: { data: Uint8Array; replay: boolean }[];
  /** `attached` arrived; the owner has not wired a view yet. */
  completed: boolean;
  homeDir?: string;
  timer: ReturnType<typeof setTimeout>;
}

interface View extends ViewHandle {
  sink: FrameSink;
}

export class AttachRegistry {
  private inFlight = new Map<string, InFlight>();
  private views = new Map<string, Set<View>>();
  private nextToken = 1;
  private unsubscribes: (() => void)[];

  constructor(
    private socket: AttachSocket,
    private timeoutMs = 5000,
  ) {
    this.unsubscribes = [
      socket.onBinaryMessage((id, data) => this.route(id, data)),
      socket.onMessage((msg) => this.handleMessage(msg)),
    ];
    // An attach sent on a lost connection never completes; a view of the
    // same session after the reconnect must send its own instead of taking
    // the dead one over.
    const unsubDisconnect = socket.onDisconnect?.(() => this.failInFlight("connection lost"));
    if (unsubDisconnect) this.unsubscribes.push(unsubDisconnect);
  }

  /**
   * Stop routing this socket's frames. Attaches still in flight fail and views
   * get no further output; nothing is sent.
   */
  dispose(): void {
    for (const unsubscribe of this.unsubscribes) unsubscribe();
    this.unsubscribes = [];
    this.failInFlight("connection closed");
    this.views.clear();
  }

  private failInFlight(reason: string): void {
    for (const [sessionId, entry] of this.inFlight) {
      clearTimeout(entry.timer);
      entry.owner.reject(new Error(`attach of session ${sessionId} ended: ${reason}`));
    }
    this.inFlight.clear();
  }

  /**
   * Attach to a session, or take over the attach already in flight for it.
   * Resolves once the server confirmed with `attached`.
   */
  attach(sessionId: string, cols: number, rows: number): Promise<AttachedSession> {
    return new Promise((resolve, reject) => {
      const owner: Owner = { token: this.nextToken++, resolve, reject };
      const entry = this.inFlight.get(sessionId);

      if (entry) {
        // Another view's attach is still in flight (or completed but not yet
        // wired): the newer view takes it over instead of sending a second
        // attach, whose replay would arrive on top of the first one.
        const previous = entry.owner;
        entry.owner = owner;
        previous.reject(new AttachTransferredError(sessionId));
        if (entry.completed) owner.resolve(this.attached(sessionId, entry, owner.token));
        return;
      }

      const timer = setTimeout(() => {
        const current = this.inFlight.get(sessionId);
        if (!current || current.completed) return;
        this.inFlight.delete(sessionId);
        current.owner.reject(new Error("Timeout waiting for session attach"));
      }, this.timeoutMs);

      this.inFlight.set(sessionId, { owner, buffer: [], completed: false, timer });
      this.socket.send({ type: "attach", id: sessionId, cols, rows });
    });
  }

  /** Wire a view to a session whose stream was set up without an attach (`create`). */
  addView(sessionId: string, sink: FrameSink): ViewHandle {
    const view: View = { sessionId, sink };
    let set = this.views.get(sessionId);
    if (!set) {
      set = new Set();
      this.views.set(sessionId, set);
    }
    set.add(view);
    return view;
  }

  /**
   * The view lets go of the session without closing it. `detach` is sent only
   * when no other view holds the session and no attach for it is in flight.
   */
  detach(handle: ViewHandle): void {
    if (!this.removeView(handle)) return;
    if (this.views.has(handle.sessionId) || this.inFlight.has(handle.sessionId)) return;
    this.socket.send({ type: "detach", id: handle.sessionId });
  }

  /** The view goes away because its session is closed; nothing is sent. */
  drop(handle: ViewHandle): void {
    this.removeView(handle);
  }

  private removeView(handle: ViewHandle): boolean {
    const set = this.views.get(handle.sessionId);
    if (!set) return false;
    let removed = false;
    for (const view of set) {
      if (view === handle) {
        set.delete(view);
        removed = true;
      }
    }
    if (set.size === 0) this.views.delete(handle.sessionId);
    return removed;
  }

  private route(sessionId: string, data: Uint8Array): void {
    const entry = this.inFlight.get(sessionId);
    if (entry) {
      entry.buffer.push({ data, replay: !entry.completed });
      return;
    }
    const set = this.views.get(sessionId);
    if (!set) return;
    for (const view of set) view.sink(data);
  }

  private handleMessage(msg: OutgoingMessage): void {
    if (msg.type === "attached") {
      const entry = this.inFlight.get(msg.sessionId);
      if (!entry || entry.completed) return;
      clearTimeout(entry.timer);
      entry.completed = true;
      entry.homeDir = msg.homeDir;
      entry.owner.resolve(this.attached(msg.sessionId, entry, entry.owner.token));
    } else if (msg.type === "error" && msg.sessionId) {
      const entry = this.inFlight.get(msg.sessionId);
      if (!entry || entry.completed) return;
      clearTimeout(entry.timer);
      this.inFlight.delete(msg.sessionId);
      entry.owner.reject(new Error(msg.error));
    }
  }

  private attached(sessionId: string, entry: InFlight, token: number): AttachedSession {
    return {
      homeDir: entry.homeDir,
      wire: (sink: FrameSink): ViewHandle => {
        if (this.inFlight.get(sessionId) !== entry || entry.owner.token !== token) {
          throw new AttachTransferredError(sessionId);
        }
        // Synchronous from here on: nothing can be routed between writing the
        // buffer and registering the view.
        this.inFlight.delete(sessionId);
        const replay = new ReplayOutput();
        for (const { data, replay: historical } of entry.buffer) {
          if (!historical) {
            const tail = replay.finish();
            if (tail.length) sink(tail);
          }
          const output = historical ? replay.push(data) : data;
          if (output.length) sink(output);
        }
        // An incomplete historical control sequence must not consume live
        // output or turn its continuation into a fresh terminal query.
        const tail = replay.finish();
        if (tail.length) sink(tail);
        return this.addView(sessionId, sink);
      },
    };
  }
}

/** Remove `key` from `map` only if it still maps to `value`. */
export function deleteIfCurrent<K, V>(map: Map<K, V>, key: K, value: V): void {
  if (map.get(key) === value) map.delete(key);
}
