/** A renderer connection assigned by the native broker. The actual caller,
 * document, role, target, quotas and receipt ledger are checked natively too.
 * No URL or global variable can construct a service connection here. */
import { SocketHandlers, type ConnSocket } from "./socket.ts";
import type { IncomingMessage } from "./types.ts";
import { describeRequest, isNativeReply, NativeRequestError, type NativeApi, type NativeReply, type NativeRequest } from "./native_api.ts";
import { application, dataFrame, MAX_NATIVE_API_BYTES, MAX_NATIVE_FRAME_BYTES, NativeKind, type NativeFrame } from "./native_wire.ts";

import type { NativeAssignment } from "../../../../frontend-electron/src/native_types.ts";
export type { NativeAssignment } from "../../../../frontend-electron/src/native_types.ts";
export interface NativeDelivery {
  readonly handle: string;
  readonly document: string;
  readonly frame: NativeFrame;
}
export interface NativeProgress { phase: string; message: string }
/** subscribe/start must not lose the initial welcome. The broker retains all
 * output leases until receipt consumption, also across IPC fragmentation. */
export interface NativeChannel {
  readonly assignment: NativeAssignment;
  subscribe(receive: (delivery: NativeDelivery) => void): () => void;
  start(): void;
  write(frame: NativeFrame): void;
  close(): void;
}
interface Pending {
  resolve(reply: NativeReply): void;
  reject(error: Error): void;
  cleanup(): void;
  abandoned: boolean;
  mutation: boolean;
}
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
const MAX_PENDING = 64;
const MAX_BYTES = 32 * 1024 * 1024;
const MAX_DELIVERIES = 64;
function hex(bytes: Uint8Array): string { return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(""); }
function control(kind: NativeKind, value: unknown): NativeFrame { return { kind, payload: encoder.encode(JSON.stringify(value)) }; }

export class NativeConnectionError extends Error {
  constructor(message: string, readonly code: string, readonly retryable: boolean) { super(message); }
}

export class NativeSocket extends SocketHandlers implements ConnSocket, NativeApi {
  private channel: NativeChannel | undefined;
  private unsubscribe: (() => void) | undefined;
  private nextId = 0;
  private pending = new Map<number, Pending>();
  private outstanding = new Map<string, number>();
  private outgoingBytes = 0;
  private assigned: Readonly<NativeAssignment> | undefined;
  onConnected: (() => void) | null = null;
  onDisconnected: (() => void) | null = null;
  onError: ((error: Error) => void) | null = null;

  get assignment(): Readonly<NativeAssignment> | undefined { return this.assigned; }

  /** Only the shell adapter calls this with the broker's successful assignment. */
  adopt(channel: NativeChannel): void {
    this.revoke();
    const assignment = channel.assignment;
    if (!assignment.handle || !assignment.document || !assignment.instance
      || !Number.isInteger(assignment.generation) || assignment.generation < 1 || assignment.generation > 0xffffffff
      || !["main", "remote"].includes(assignment.role)
      || (assignment.role === "remote" && !assignment.target)) {
      channel.close(); throw new Error("Invalid native connection assignment");
    }
    this.assigned = Object.freeze({ ...assignment });
    this.channel = channel;
    try {
      const unsubscribe = channel.subscribe((delivery) => {
        if (this.channel !== channel) return;
        if (delivery.handle !== assignment.handle || delivery.document !== assignment.document) {
          this.fail(new Error("Native delivery belongs to another document/connection")); return;
        }
        try { this.receive(delivery.frame); } catch (error) { this.fail(error instanceof Error ? error : new Error(String(error))); }
      });
      if (this.channel !== channel) { this.cleanup(unsubscribe); return; }
      this.unsubscribe = unsubscribe;
      this.onConnected?.();
      if (this.channel === channel) channel.start();
    } catch (error) { this.fail(error instanceof Error ? error : new Error(String(error))); }
  }

  /** Disconnected input has no queue. Welcome-driven clients rebuild state. */
  send(message: IncomingMessage): void {
    if (!this.channel) return;
    try { this.write(NativeKind.text, encoder.encode(JSON.stringify(message))); }
    catch (error) { this.fail(error instanceof Error ? error : new Error(String(error))); }
  }

  request(request: NativeRequest, options?: { signal?: AbortSignal }): Promise<NativeReply> {
    const description = describeRequest(request);
    const channel = this.channel;
    const assignment = this.assigned;
    if (!channel || !assignment) return Promise.reject(new NativeRequestError("The assigned backend is disconnected.", false));
    if (assignment.role === "remote" && description.scope === "localHub") return Promise.reject(new Error("Operation is not available to this connection"));
    if (this.pending.size >= MAX_PENDING || this.nextId === 0xffffffff) return Promise.reject(new Error("Native request capacity exhausted"));
    if (options?.signal?.aborted) return Promise.reject(options.signal.reason);
    const id = ++this.nextId;
    const bytes = encoder.encode(JSON.stringify({ id, ...request }));
    // The service's per-operation limit excludes the transport request id.
    if (bytes.length > MAX_NATIVE_API_BYTES || encoder.encode(JSON.stringify(request)).length > description.maxBodyBytes) {
      return Promise.reject(new Error("Native request exceeds its limit"));
    }
    return new Promise((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const abort = () => {
        const pending = this.pending.get(id);
        if (!pending || pending.abandoned) return;
        pending.abandoned = true;
        pending.cleanup();
        reject(new NativeRequestError("The request was cancelled; an admitted mutation may have completed.", pending.mutation));
        try { channel.write(control(NativeKind.cancel, { generation: assignment.generation, id })); }
        catch (error) { this.fail(error instanceof Error ? error : new Error(String(error))); }
        // Keep the slot until an actual reply or generation revocation. Merely
        // cancelling the caller's wait cannot create unbounded admitted work.
      };
      const cleanup = () => { clearTimeout(timer); options?.signal?.removeEventListener("abort", abort); };
      this.pending.set(id, { resolve, reject, cleanup, abandoned: false, mutation: description.cancellation !== "read" });
      options?.signal?.addEventListener("abort", abort, { once: true });
      if (description.deadlineMs !== null) timer = setTimeout(abort, description.deadlineMs);
      try { this.write(NativeKind.request, bytes); }
      catch (error) {
        cleanup(); this.pending.delete(id);
        reject(new NativeRequestError(String(error), description.cancellation !== "read"));
        this.fail(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  revoke(): void {
    const wasConnected = !!this.channel;
    const channel = this.channel;
    this.channel = undefined;
    const unsubscribe = this.unsubscribe;
    this.unsubscribe = undefined;
    this.assigned = undefined;
    for (const pending of this.pending.values()) {
      pending.cleanup();
      if (!pending.abandoned) pending.reject(new NativeRequestError("Connection ended before the result arrived.", pending.mutation));
    }
    this.pending.clear(); this.outstanding.clear(); this.outgoingBytes = 0; this.nextId = 0;
    if (unsubscribe) this.cleanup(unsubscribe);
    if (channel) this.cleanup(() => channel.close());
    if (wasConnected) { this.disconnected(); if (this.onDisconnected) this.cleanup(this.onDisconnected); }
  }
  dispose(): void { this.revoke(); this.clearHandlers(); }

  private cleanup(callback: () => void): void {
    try { callback(); } catch (error) { console.error("[native] connection cleanup failed", error); }
  }
  private fail(error: Error): void { this.revoke(); if (this.onError) this.cleanup(() => this.onError?.(error)); }
  private write(kind: NativeKind, bytes: Uint8Array): void {
    const assignment = this.assigned;
    const channel = this.channel;
    if (!channel || !assignment) throw new Error("Native connection is closed");
    const size = bytes.length + 25;
    if (size > MAX_NATIVE_FRAME_BYTES || this.outstanding.size >= MAX_DELIVERIES || this.outgoingBytes + size > MAX_BYTES) {
      throw new Error("Native delivery capacity exhausted");
    }
    const receipt = crypto.getRandomValues(new Uint8Array(16));
    const key = hex(receipt);
    if (this.outstanding.has(key)) throw new Error("Duplicate native receipt");
    const frame = dataFrame(kind, { generation: assignment.generation, receipt }, bytes);
    this.outstanding.set(key, size); this.outgoingBytes += size;
    channel.write(frame);
  }
  private receive(frame: NativeFrame): void {
    const assignment = this.assigned!;
    const channel = this.channel!;
    if (frame.kind === NativeKind.ping) {
      const value: unknown = JSON.parse(decoder.decode(frame.payload));
      if (!value || typeof value !== "object" || Array.isArray(value)
        || Object.keys(value).length !== 1 || !("nonce" in value)
        || typeof value.nonce !== "number" || !Number.isInteger(value.nonce)
        || value.nonce < 0 || value.nonce > 0xffffffff) throw new Error("Malformed native heartbeat");
      // This stream's actual handle/document envelope was checked before
      // receive(). Heartbeat controls have no application generation field.
      // They do not consume data receipts or pending request capacity.
      channel.write(control(NativeKind.pong, { nonce: value.nonce }));
      return;
    }
    if (frame.kind === NativeKind.credit) {
      const value = JSON.parse(decoder.decode(frame.payload));
      const size = value.generation === assignment.generation ? this.outstanding.get(value.receipt) : undefined;
      if (size === undefined) throw new Error("Unknown native credit");
      this.outstanding.delete(value.receipt); this.outgoingBytes -= size; return;
    }
    if (frame.kind === NativeKind.closed) {
      const value = JSON.parse(decoder.decode(frame.payload));
      throw new NativeConnectionError(typeof value.message === "string" ? value.message : "Native connection closed",
        typeof value.code === "string" ? value.code : "protocol", value.retryable === true);
    }
    if (frame.kind !== NativeKind.text && frame.kind !== NativeKind.binary && frame.kind !== NativeKind.reply) throw new Error("Unexpected renderer frame");
    const { delivery, bytes } = application(frame);
    if (delivery.generation !== assignment.generation) throw new Error("Stale native generation");
    if (frame.kind === NativeKind.text) this.deliver(decoder.decode(bytes));
    else if (frame.kind === NativeKind.binary) this.deliver(bytes);
    else {
      const reply: unknown = JSON.parse(decoder.decode(bytes));
      if (!isNativeReply(reply)) throw new Error("Malformed native API reply");
      const pending = Number.isInteger(reply.id) ? this.pending.get(reply.id) : undefined;
      if (!pending) throw new Error("Unsolicited native API reply");
      this.pending.delete(reply.id); pending.cleanup();
      if (!pending.abandoned) pending.resolve(reply);
    }
    if (this.channel === channel) channel.write(control(NativeKind.credit, { generation: assignment.generation, receipt: hex(delivery.receipt) }));
  }
}
