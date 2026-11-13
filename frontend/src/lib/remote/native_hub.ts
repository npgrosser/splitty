/** Adapt a native host stream to the hub's existing progress/ready/reconnect
 * state machine. The byte stream stays on IPC; no ticket or WebSocket exists. */
import { NativeSocket, NativeConnectionError, type NativeChannel, type NativeProgress } from "../protocol/native_socket.ts";
import type { IncomingMessage } from "../protocol/types.ts";
import type { HubWebSocket } from "./remote_socket.ts";

export type NativeHostOpener = (target: string, update: boolean, signal?: AbortSignal, progress?: (value: NativeProgress) => void) => Promise<NativeChannel>;
export class NativeHubSocket implements HubWebSocket {
  binaryType: BinaryType = "arraybuffer";
  readyState = 0;
  onopen: HubWebSocket["onopen"] = null;
  onmessage: HubWebSocket["onmessage"] = null;
  onclose: HubWebSocket["onclose"] = null;
  onerror: HubWebSocket["onerror"] = null;
  private ended = false;
  private readonly preparation = new AbortController();
  private readonly socket: NativeSocket;

  constructor(target: string, update: boolean, document: string, open: NativeHostOpener) {
    // Preparation progress arrives once, as native transport reports before
    // the stream is accepted. A hub of the previous release repeats it as
    // text before remote.ready; drop that text.
    let reported = 0;
    let prelude = true;
    const deliver = (data: unknown) => {
      if (this.ended) return;
      if (prelude && typeof data === "string") {
        try {
          const message = JSON.parse(data);
          if (message.type === "remote.ready") prelude = false;
          else if (message.type === "remote.progress") return;
        } catch { /* The normal connection parser owns malformed messages. */ }
      }
      this.onmessage?.({ data });
    };
    const progress = (value: NativeProgress) => {
      if (this.ended || reported >= 8 || typeof value?.phase !== "string" || value.phase.length > 64
        || typeof value.message !== "string" || value.message.length > 4096) return;
      const report = { phase: value.phase, message: value.message };
      reported += 1;
      // A synchronous opener may report before the caller installs handlers.
      queueMicrotask(() => { if (!this.ended) this.onmessage?.({ data: JSON.stringify({ type: "remote.progress", step: report.phase, message: report.message }) }); });
    };
    // NativeSocket still owns receipts, framing, generations and cancellation.
    // Hub preludes and byte output reach the existing state machine unchanged.
    this.socket = new class extends NativeSocket { override deliver(data: unknown) { deliver(data); } }();
    this.socket.onDisconnected = () => queueMicrotask(() => this.lost());
    this.socket.onError = error => this.failed(error);
    void open(target, update, this.preparation.signal, progress).then(channel => {
      if (this.ended) { channel.close(); return; }
      const assigned = channel.assignment;
      if (assigned.role !== "remote" || assigned.target !== target || assigned.document !== document) {
        channel.close(); throw new NativeConnectionError("Native host assignment differs from the requested connection", "assignment", false);
      }
      this.readyState = 1;
      this.socket.adopt(channel);
    }).catch(error => this.failed(error instanceof Error ? error : Error(String(error))));
  }
  send(data: string): void {
    if (this.ended || this.readyState !== 1) return;
    try { this.socket.send(JSON.parse(data) as IncomingMessage); }
    catch (error) { this.failed(error instanceof Error ? error : Error(String(error))); }
  }
  close(): void {
    if (this.ended) return;
    this.ended = true; this.readyState = 3; this.preparation.abort(); this.socket.dispose();
  }
  private failed(error: Error): void {
    if (this.ended) return;
    // Rust carries the bounded, typed host failure inside Closed.message.
    // Preserve incompatibility/update details through the shell's IPC, whose
    // Error cloning otherwise drops custom fields.
    let failure: unknown;
    try { const parsed = JSON.parse(error.message); if (parsed?.type === "remote.error") failure = parsed; } catch { /* Ordinary pipe error. */ }
    this.onmessage?.({ data: JSON.stringify(failure ?? { type: "remote.error", code: "closed", message: error.message, retryable: error instanceof NativeConnectionError && error.retryable }) });
    this.lost();
  }
  private lost(): void {
    if (this.ended) return;
    this.close(); this.onclose?.({ code: 4001, reason: "Native host connection ended" });
  }
}
