/**
 * Test doubles for a server connection: a socket whose frames go through the
 * real handler delivery (`SocketHandlers`), and an emulator that records what
 * it is written. Used by vitest only.
 */

import { SocketHandlers, type ConnSocket } from "../protocol/socket.ts";
import type { IncomingMessage } from "../protocol/types.ts";
import { ServerConnection, type ConnKey } from "../connection/server_connection.ts";
import type { TerminalEmulator } from "../terminal/terminal_emulator.ts";

let eventCounter = 0;

export class FakeSocket extends SocketHandlers implements ConnSocket {
  sent: IncomingMessage[] = [];

  send(msg: IncomingMessage): void {
    this.sent.push(msg);
  }

  /** The server sends a JSON message; any shape, also a malformed one. `id` and `timestamp` are filled in. */
  message(fields: Record<string, unknown>): void {
    this.deliver(JSON.stringify({ id: `ev-${++eventCounter}`, timestamp: 0, ...fields }));
  }

  /** The server sends a text frame as it is. */
  text(raw: string): void {
    this.deliver(raw);
  }

  /** Terminal output for a session, as a binary frame. */
  output(sessionId: string, text: string): void {
    const id = new TextEncoder().encode(sessionId);
    const payload = new TextEncoder().encode(text);
    const frame = new Uint8Array(1 + id.length + payload.length);
    frame[0] = id.length;
    frame.set(id, 1);
    frame.set(payload, 1 + id.length);
    this.binary(frame);
  }

  /** A binary frame as it is. */
  binary(bytes: Uint8Array): void {
    this.deliver(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
  }

  /** The connection drops. */
  drop(): void {
    this.disconnected();
  }

  sentOfType<T extends IncomingMessage["type"]>(type: T): Extract<IncomingMessage, { type: T }>[] {
    return this.sent.filter((m): m is Extract<IncomingMessage, { type: T }> => m.type === type);
  }
}

export function fakeServer(key: ConnKey, label: string = key): { socket: FakeSocket; conn: ServerConnection } {
  const socket = new FakeSocket();
  return { socket, conn: new ServerConnection(key, label, socket) };
}

export interface FakeEmulator extends TerminalEmulator {
  /** Everything written to the terminal, decoded. */
  readonly written: string[];
  focusCount: number;
  disposed: boolean;
  /** The grid that fits the pane, what `measure()` returns; null without layout. */
  pane: { cols: number; rows: number } | null;
  /** The user types into the terminal. */
  type(data: string): void;
}

/** An emulator that records what it is written; only what sessions use is implemented. */
export function fakeEmulator(): FakeEmulator {
  const decoder = new TextDecoder();
  const dataListeners = new Set<(data: string) => void>();
  const resizeListeners = new Set<(cols: number, rows: number) => void>();
  const emulator = {
    written: [] as string[],
    focusCount: 0,
    disposed: false,
    cols: 80,
    rows: 24,
    pane: { cols: 80, rows: 24 } as { cols: number; rows: number } | null,
    element: undefined,
    cursorRow: 0,
    isAlternateScreen: false,
    write(data: Uint8Array) { emulator.written.push(decoder.decode(data)); },
    onData(cb: (data: string) => void) { dataListeners.add(cb); return () => dataListeners.delete(cb); },
    onResize(cb: (cols: number, rows: number) => void) { resizeListeners.add(cb); return () => resizeListeners.delete(cb); },
    resize(cols: number, rows: number) {
      if (cols === emulator.cols && rows === emulator.rows) return;
      emulator.cols = cols;
      emulator.rows = rows;
      for (const cb of [...resizeListeners]) cb(cols, rows);
    },
    measure() { return emulator.pane ? { ...emulator.pane } : undefined; },
    cellSize() { return { width: 8, height: 16 }; },
    fit() { return { cols: emulator.cols, rows: emulator.rows }; },
    focus() { emulator.focusCount++; },
    dispose() { emulator.disposed = true; },
    paste(text: string) { emulator.type(text); },
    clear() {},
    getSelection() { return ""; },
    type(data: string) { for (const cb of dataListeners) cb(data); },
  };
  return emulator as unknown as FakeEmulator;
}
