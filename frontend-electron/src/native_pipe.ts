/** A single assigned backend over inherited pipes. This owns framing, flow
 * control and liveness; it neither discovers a daemon nor chooses a backend. */
import type { Readable, Writable } from "node:stream";
import { NativeConnectionBudget } from "./native_budget";
import {
  application, encodeNativeFrame, isData, MAX_IPC_CHUNK_BYTES, MAX_NATIVE_API_BYTES,
  MAX_NATIVE_FRAME_BYTES, NATIVE_MAGIC, NativeFrameDecoder, NativeKind, type NativeFrame,
} from "./native_wire";

export interface NativeReady {
  generation: number;
  instance: string;
  muxProtocol: number;
  purpose: { kind: "direct" } | { kind: "host"; target: string } | { kind: "owner" };
  limits: { frameBytes: number; apiBytes: number; pendingRequests: number; connectionBytes: number };
}
export interface NativeClosed { code: string; message: string; retryable: boolean }
export interface PipeEvents {
  progress?(value: { phase: string; message: string }): void;
  closed(value: NativeClosed): void;
}
const utf8 = new TextDecoder("utf-8", { fatal: true });
// Matches splitty_remote::tunnel::PREPARE_TIMEOUT; only assigned host streams
// need the install-upload allowance. Progress cannot renew either deadline.
export const HOST_PREPARATION_MS = 600_000;
const encoder = new TextEncoder();
const hex = (bytes: Uint8Array): string => Buffer.from(bytes).toString("hex");
function record(value: unknown, keys: string[]): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)
    || Object.keys(value).some(key => !keys.includes(key))) throw Error("Invalid native control fields");
}
function uint(value: unknown, min = 0, max = 0xffffffff): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= min && value <= max;
}
function json(frame: NativeFrame): unknown { return JSON.parse(utf8.decode(frame.payload)); }
function control(kind: NativeKind, value: unknown): NativeFrame { return { kind, payload: encoder.encode(JSON.stringify(value)) }; }

export function validateNativeReady(value: unknown, target?: string, owner = false): NativeReady {
  record(value, ["generation", "instance", "muxProtocol", "purpose", "limits"]);
  const limits = value.limits, purpose = value.purpose;
  record(limits, ["frameBytes", "apiBytes", "pendingRequests", "connectionBytes"]);
  record(purpose, ["kind", "target"]);
  if (!uint(value.generation, 1) || typeof value.instance !== "string" || !/^[0-9a-f]{32}$/.test(value.instance)
    || value.muxProtocol !== 1 || !uint(limits.frameBytes, 26, MAX_NATIVE_FRAME_BYTES)
    || !uint(limits.apiBytes, 1, MAX_NATIVE_API_BYTES) || !uint(limits.pendingRequests, 1, 64)
    || !uint(limits.connectionBytes, limits.frameBytes, 32 * 1024 * 1024)
    || (target === undefined ? purpose.kind !== (owner ? "owner" : "direct") || purpose.target !== undefined
      : purpose.kind !== "host" || purpose.target !== target)) throw Error("Native service differs from assigned backend or supported protocol");
  return value as unknown as NativeReady;
}

interface Outgoing {
  bytes: Uint8Array;
  finishWrite(): void;
  consume(): void;
  control: boolean;
  sent: boolean;
}
interface SharedLease { (): void; retain(): () => void }
function sharedLease(release: () => void): SharedLease {
  let references = 1, owned = true;
  const drop = () => { if (--references === 0) release(); };
  const root = (() => { if (owned) { owned = false; drop(); } }) as SharedLease;
  root.retain = () => {
    if (!references) throw Error("Native delivery has already been released");
    references++; let held = true;
    return () => { if (held) { held = false; drop(); } };
  };
  return root;
}
interface Incoming { frame: NativeFrame; release: SharedLease; delivered: boolean }

export class NativePipe {
  readonly ready: Promise<NativeReady>;
  private resolve!: (ready: NativeReady) => void;
  private reject!: (error: Error) => void;
  private stage: "accept" | "ready" | "active" | "closed" = "accept";
  private identity?: NativeReady;
  private magic = 0;
  private readonly decoder: NativeFrameDecoder;
  private deadline: ReturnType<typeof setTimeout> | undefined;
  private assembly: ReturnType<typeof setTimeout> | undefined;
  private heartbeat: ReturnType<typeof setInterval> | undefined;
  private ping?: { nonce: number; time: number };
  private nonce = 0;
  private dataQueue: Outgoing[] = [];
  private controlQueue: Outgoing[] = [];
  private writing = false;
  private outgoing = new Map<string, Outgoing>();
  private incoming = new Map<string, Incoming>();
  private receiver?: (frame: NativeFrame) => void;
  private deliveryBytes = 0;

  constructor(private readonly read: Readable, private readonly write: Writable,
    readonly budget: NativeConnectionBudget, private readonly events: PipeEvents, private readonly target?: string, private readonly owner = false) {
    this.ready = new Promise((resolve, reject) => { this.resolve = resolve; this.reject = reject; });
    this.decoder = new NativeFrameDecoder((bytes, control) => sharedLease(budget.reserve(bytes, control)));
    read.on("data", this.onData);
    read.once("end", this.onEnd);
    read.once("error", this.onError);
    write.once("error", this.onError);
    this.setDeadline(5000, "Native framing handshake timed out");
    // Return a closed/rejected connection even when the initial reservations
    // fail. A thrown constructor would strand the ready promise and its pipes.
    try {
      this.enqueueBytes(NATIVE_MAGIC, true);
      this.enqueue(control(NativeKind.hello, { transportVersion: 1 }));
    } catch (error) { this.fail(error); }
  }

  /** Attach only after the document-bound delivery channel has been assigned.
   * Welcome and later output remain charged while awaiting that attachment. */
  activate(receive: (frame: NativeFrame) => void): void {
    if (this.stage !== "active" || this.receiver) throw Error("Native pipe is not available for attachment");
    this.receiver = receive;
    clearTimeout(this.deadline);
    try {
      for (const entry of this.incoming.values()) {
        entry.delivered = true;
        receive(entry.frame);
        if (this.stage !== "active") break;
      }
    } catch (error) { this.fail(error); }
  }

  /** IPC completion and the native receipt can race. An adapter retaining the
   * same payload must hold its own reference until its last chunk is consumed. */
  retain(frame: NativeFrame): () => void {
    const receipt = hex(application(frame).delivery.receipt);
    const entry = this.incoming.get(receipt);
    if (!entry || entry.frame !== frame) throw Error("Unknown retained native delivery");
    return entry.release.retain();
  }

  /** The broker has already checked document/handle and assembled bounded IPC
   * chunks. Only client data, cancellation and genuine delivery receipts pass. */
  send(frame: NativeFrame): void {
    try {
      if (this.stage !== "active" || !this.receiver) throw Error("Native backend is not attached");
      const ready = this.identity!;
      if (frame.kind === NativeKind.credit) {
        const receipt = this.credit(frame);
        const entry = this.incoming.get(receipt);
        if (!entry?.delivered) throw Error("Unknown or undelivered native receipt");
        this.incoming.delete(receipt);
        this.deliveryBytes -= entry.frame.payload.length + 5;
        entry.release();
        this.enqueue(frame);
      } else if (frame.kind === NativeKind.cancel) {
        const value = json(frame);
        record(value, ["generation", "id"]);
        if (value.generation !== ready.generation || !uint(value.id, 1)) throw Error("Invalid native cancellation");
        this.enqueue(frame);
      } else if (frame.kind === NativeKind.text || frame.kind === NativeKind.request || (this.owner && frame.kind === NativeKind.binary)) {
        const { delivery, bytes } = application(frame);
        const receipt = hex(delivery.receipt);
        if (delivery.generation !== ready.generation || frame.payload.length + 5 > ready.limits.frameBytes
          || (frame.kind === NativeKind.request && bytes.length > ready.limits.apiBytes)
          || this.outgoing.size >= 64 || this.outgoing.has(receipt)) throw Error("Invalid or over-limit native delivery");
        this.enqueue(frame, receipt);
      } else throw Error("Unexpected renderer frame kind");
    } catch (error) { this.fail(error); throw error; }
  }

  close(reason: NativeClosed = { code: "closed", retryable: false, message: "Native connection closed" }): void {
    if (this.stage === "closed") return;
    this.stage = "closed";
    clearTimeout(this.deadline); clearTimeout(this.assembly); clearInterval(this.heartbeat);
    try { this.decoder.end(); } catch { /* Partial input is already revoked. */ }
    this.read.off("data", this.onData);
    this.read.destroy(); this.write.destroy();
    for (const entry of this.incoming.values()) entry.release();
    this.incoming.clear(); this.deliveryBytes = 0;
    for (const entry of [...this.dataQueue, ...this.controlQueue]) { entry.consume(); entry.finishWrite(); }
    this.dataQueue = []; this.controlQueue = [];
    // Active writes retain their physical buffer lease until Node's callback.
    for (const entry of this.outgoing.values()) entry.consume();
    this.outgoing.clear(); this.receiver = undefined;
    this.reject(Error(reason.message));
    try { this.events.closed(reason); }
    catch (error) { console.error("[native] close notification failed", error); }
  }

  private fail(error: unknown): void {
    this.close({ code: "protocol", retryable: false, message: error instanceof Error ? error.message : String(error) });
  }
  private onError = (error: Error): void => { this.close({ code: "backendLost", retryable: true, message: error.message }); };
  private onEnd = (): void => {
    try { this.decoder.end(); } catch (error) { this.fail(error); return; }
    this.close({ code: "backendLost", retryable: true, message: "Native backend pipe ended" });
  };
  private onData = (chunk: Buffer): void => {
    try {
      let offset = 0;
      while (this.magic < NATIVE_MAGIC.length && offset < chunk.length) {
        if (chunk[offset++] !== NATIVE_MAGIC[this.magic++]) throw Error("Unsupported native magic");
      }
      while (offset < chunk.length && this.stage !== "closed") {
        // Node may coalesce reads; decoder/IPC chunks still have a fixed cap.
        const end = Math.min(chunk.length, offset + MAX_IPC_CHUNK_BYTES);
        if (!this.assembly) this.assembly = setTimeout(() => this.fail(Error("Native frame assembly timed out")), 60_000);
        this.decoder.feed(chunk.subarray(offset, end), (frame, release) => {
          clearTimeout(this.assembly); this.assembly = undefined;
          try { this.receive(frame, release); } catch (error) { release(); throw error; }
        });
        if (this.decoder.bufferedBytes && !this.assembly) this.assembly = setTimeout(() => this.fail(Error("Native frame assembly timed out")), 60_000);
        offset = end;
      }
    } catch (error) { this.fail(error); }
  };

  private receive(frame: NativeFrame, release: () => void): void {
    if (this.stage === "closed") { release(); return; }
    if (frame.kind === NativeKind.closed) {
      const value = json(frame);
      record(value, ["code", "message", "retryable"]);
      if (typeof value.code !== "string" || !["incompatible", "busy", "timeout", "protocol", "backendLost", "closed", "internal"].includes(value.code)
        || typeof value.message !== "string" || typeof value.retryable !== "boolean") throw Error("Invalid native close reason");
      release(); this.close(value as unknown as NativeClosed); return;
    }
    if (this.stage === "accept") {
      if (frame.kind !== NativeKind.accept) throw Error("Expected native acceptance");
      const value = json(frame); record(value, ["transportVersion"]);
      if (value.transportVersion !== 1) throw Error("Unsupported native transport version");
      release(); this.stage = "ready";
      this.setDeadline(this.target === undefined ? 120_000 : HOST_PREPARATION_MS, "Native backend preparation timed out"); return;
    }
    if (this.stage === "ready") {
      if (frame.kind === NativeKind.progress) {
        const value = json(frame); record(value, ["phase", "message"]);
        if (typeof value.phase !== "string" || typeof value.message !== "string") throw Error("Invalid native progress");
        release(); this.events.progress?.(value as { phase: string; message: string }); return;
      }
      if (frame.kind !== NativeKind.ready) throw Error("Application input before native service admission");
      const ready = validateNativeReady(json(frame), this.target, this.owner);
      release(); this.identity = ready; this.stage = "active";
      this.enqueue(control(NativeKind.readyAccept, { generation: ready.generation, instance: ready.instance }));
      this.setDeadline(5000, "Native document attachment timed out");
      this.heartbeat = setInterval(() => {
        try {
          if (this.ping) {
            if (Date.now() - this.ping.time >= 30_000) this.close({ code: "timeout", retryable: true, message: "Native heartbeat timed out" });
          } else {
            this.ping = { nonce: ++this.nonce >>> 0, time: Date.now() };
            this.enqueue(control(NativeKind.ping, { nonce: this.ping.nonce }));
          }
        } catch (error) { this.fail(error); }
      }, 15_000);
      this.resolve(ready); return;
    }
    if (frame.kind === NativeKind.ping || frame.kind === NativeKind.pong) {
      const value = json(frame); record(value, ["nonce"]);
      if (!uint(value.nonce)) throw Error("Invalid native heartbeat");
      if (frame.kind === NativeKind.pong) {
        if (!this.ping || value.nonce !== this.ping.nonce) throw Error("Unsolicited native pong");
        this.ping = undefined;
      } else this.enqueue(control(NativeKind.pong, value));
      release(); return;
    }
    if (frame.kind === NativeKind.credit) {
      const receipt = this.credit(frame), entry = this.outgoing.get(receipt);
      if (!entry?.sent) throw Error("Unknown or unsent native credit");
      this.outgoing.delete(receipt); entry.consume();
      try { this.receiver?.(frame); } finally { release(); }
      return;
    }
    if (frame.kind !== NativeKind.text && frame.kind !== NativeKind.binary && frame.kind !== NativeKind.reply) throw Error("Unexpected backend frame kind");
    const { delivery } = application(frame), receipt = hex(delivery.receipt);
    const size = frame.payload.length + 5;
    if (delivery.generation !== this.identity!.generation || this.incoming.size >= 64 || this.incoming.has(receipt)
      || size > this.identity!.limits.frameBytes || this.deliveryBytes + size > this.identity!.limits.connectionBytes - 64 * 1024) throw Error("Invalid or over-limit backend delivery");
    const entry = { frame, release: release as SharedLease, delivered: !!this.receiver };
    this.incoming.set(receipt, entry); this.deliveryBytes += size;
    this.receiver?.(frame);
  }
  private credit(frame: NativeFrame): string {
    const value = json(frame); record(value, ["generation", "receipt"]);
    if (value.generation !== this.identity!.generation || typeof value.receipt !== "string" || !/^[0-9a-f]{32}$/.test(value.receipt)) throw Error("Invalid native credit");
    return value.receipt;
  }
  private setDeadline(ms: number, message: string): void {
    clearTimeout(this.deadline);
    this.deadline = setTimeout(() => this.close({ code: "timeout", retryable: true, message }), ms);
  }
  private enqueue(frame: NativeFrame, receipt?: string): void {
    const size = frame.payload.length + 5, control = !isData(frame.kind);
    // Account for the encoded copy before allocating it. The caller's frame is
    // also covered until the write/remote receipt completes.
    const release = this.budget.reserve(size * 2, control);
    try { this.enqueueBytes(encodeNativeFrame(frame), control, receipt, release); }
    catch (error) { release(); throw error; }
  }
  private enqueueBytes(bytes: Uint8Array, control: boolean, receipt?: string, reserved?: () => void): void {
    const queue = control ? this.controlQueue : this.dataQueue;
    if (queue.length >= 64 || this.stage === "closed") throw Error("Native writer capacity exhausted");
    const release = reserved ?? this.budget.reserve(bytes.length, control);
    let written = false, consumed = !receipt;
    const finish = () => { if (written && consumed) release(); };
    const entry: Outgoing = { bytes, control, sent: false,
      finishWrite: () => { written = true; finish(); }, consume: () => { consumed = true; finish(); } };
    if (receipt) this.outgoing.set(receipt, entry);
    queue.push(entry); this.pump();
  }
  private pump(): void {
    if (this.writing || this.stage === "closed") return;
    const entry = this.controlQueue.shift() ?? this.dataQueue.shift();
    if (!entry) return;
    this.writing = true; entry.sent = true;
    try {
      this.write.write(entry.bytes, (error?: Error | null) => {
        entry.finishWrite(); this.writing = false;
        if (error) this.onError(error); else this.pump();
      });
    } catch (error) { entry.finishWrite(); this.writing = false; this.fail(error); }
  }
}
