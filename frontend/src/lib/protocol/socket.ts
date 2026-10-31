/**
 * The handler side of a server socket: who listens, and how a received frame
 * reaches them.
 *
 * Every handler sees every frame of its socket and nothing else. A malformed
 * frame is dropped, and a handler that throws is logged without keeping the
 * later handlers from the frame: one bad message, from any server, must not
 * stop a connection's other listeners (mux client, attaches, sessions,
 * notifications, commands).
 */

import type { IncomingMessage, OutgoingMessage } from "./types.ts";

export type MessageHandler = (msg: OutgoingMessage) => void;
export type BinaryHandler = (id: string, data: Uint8Array) => void;

/** What a server connection needs from its socket. */
export interface ConnSocket {
  send(msg: IncomingMessage): void;
  onMessage(handler: MessageHandler): () => void;
  /** Terminal output frames, by session id. */
  onBinaryMessage(handler: BinaryHandler): () => void;
  /** The connection was lost; any number may listen. */
  onDisconnect(handler: () => void): () => void;
}

/**
 * Split a binary output frame: `[1 byte id length] [id bytes] [payload]`.
 * Returns null for a frame too short for the id length it announces.
 */
/**
 * Messages that belong to the connection they were sent on: images, file
 * chunks, file offers and editor requests answer state of that connection
 * only, so a socket never queues them for a reconnect.
 */
export function belongsToConnection(msg: IncomingMessage): boolean {
  return msg.type === "save_image" || msg.type === "upload" || msg.type.startsWith("file.") || msg.type.startsWith("editor.");
}

export function decodeOutputFrame(bytes: Uint8Array): { id: string; payload: Uint8Array } | null {
  if (bytes.length === 0) return null;
  const idLen = bytes[0];
  if (idLen === 0 || 1 + idLen > bytes.length) return null;
  const id = String.fromCharCode(...bytes.subarray(1, 1 + idLen));
  return { id, payload: bytes.subarray(1 + idLen) };
}

export class SocketHandlers {
  private handlers = new Set<MessageHandler>();
  private binaryHandlers = new Set<BinaryHandler>();
  private disconnectHandlers = new Set<() => void>();

  onMessage(handler: MessageHandler): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  onBinaryMessage(handler: BinaryHandler): () => void {
    this.binaryHandlers.add(handler);
    return () => this.binaryHandlers.delete(handler);
  }

  onDisconnect(handler: () => void): () => void {
    this.disconnectHandlers.add(handler);
    return () => this.disconnectHandlers.delete(handler);
  }

  /** Hand a received frame to the handlers: text is a JSON message, an ArrayBuffer is output. */
  deliver(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      this.deliverOutput(new Uint8Array(data));
    } else if (typeof data === "string") {
      this.deliverText(data);
    }
  }

  /** Called with every parsed message before the handlers see it. */
  protected trace(_msg: OutgoingMessage): void {}

  /** Whether a parsed message reaches the handlers at all; a dropped one is not traced. */
  protected admit(_msg: OutgoingMessage): boolean {
    return true;
  }

  /** Tell the disconnect handlers that the connection was lost. */
  protected disconnected(): void {
    for (const h of [...this.disconnectHandlers]) isolate(h);
  }

  protected clearHandlers(): void {
    this.handlers.clear();
    this.binaryHandlers.clear();
    this.disconnectHandlers.clear();
  }

  private deliverText(text: string): void {
    let msg: unknown;
    try {
      msg = JSON.parse(text);
    } catch {
      console.warn("[ws] dropped a frame that is not JSON");
      return;
    }
    if (!msg || typeof msg !== "object" || typeof (msg as { type?: unknown }).type !== "string") {
      console.warn("[ws] dropped a message without a type");
      return;
    }
    const message = msg as OutgoingMessage;
    if (!this.admit(message)) return;
    this.trace(message);
    // A copy: a handler may unsubscribe itself or others while running.
    for (const h of [...this.handlers]) isolate(() => h(message));
  }

  private deliverOutput(bytes: Uint8Array): void {
    const frame = decodeOutputFrame(bytes);
    if (!frame) {
      console.warn("[ws] dropped a malformed output frame");
      return;
    }
    for (const h of [...this.binaryHandlers]) isolate(() => h(frame.id, frame.payload));
  }
}

function isolate(fn: () => void): void {
  try {
    fn();
  } catch (err) {
    console.error("[ws] handler failed", err);
  }
}
