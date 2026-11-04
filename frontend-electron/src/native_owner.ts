/** App-lifetime embedded service with document-lifetime native connections.
 * Only the main process owns this carrier. Renderer ports never see its open
 * commands; they receive a single assigned child NativePipe through the broker. */
import { PassThrough, Writable } from "node:stream";
import { randomBytes } from "node:crypto";
import { NativeProcessBudget, nativeProcessBudget } from "./native_budget";
import { startNativeHelper } from "./native_helper";
import { NativePipe, type NativeClosed, type PipeEvents } from "./native_pipe";
import { application, dataFrame, NativeKind as K, type NativeFrame } from "./native_wire";
import { isAcceptableTarget } from "./remote";

const CHUNK = 64 * 1024;
type Callback = (error?: Error | null) => void;
interface Stream {
  input: PassThrough;
  output: Writable;
  pipe?: NativePipe;
  closing: boolean;
  release(): void;
}
interface Pending { id: number; complete(error?: Error): void }
export interface OwnedConnection { pipe: NativePipe; released: Promise<void> }

export class NativeOwner {
  readonly ready: Promise<string>;
  readonly helper: ReturnType<typeof startNativeHelper>;
  private generation = 0;
  private nextId = 0;
  private streams = new Map<number, Stream>();
  private pending = new Map<string, Pending>();
  private stopped = false;
  get counts() { return { streams: this.streams.size, chunks: this.pending.size }; }

  constructor(binary: string, env: NodeJS.ProcessEnv, private readonly events: PipeEvents,
    private readonly processBudget: NativeProcessBudget = nativeProcessBudget) {
    this.helper = startNativeHelper(binary, env, {
      closed: reason => { this.ended(reason); }, progress: events.progress,
    }, undefined, processBudget, true);
    this.ready = this.helper.pipe.ready.then(ready => {
      if (this.stopped) throw Error("Embedded owner closed before admission");
      this.generation = ready.generation;
      this.helper.pipe.activate(frame => this.receive(frame));
      return ready.instance;
    });
  }

  /** Call only from a verified native main-window broker. All local streams
   * share this runtime; a host stream enters that runtime's existing Hub. */
  open(events: PipeEvents, target?: string, update = false): NativePipe {
    return this.connect(events, target, update).pipe;
  }
  connect(events: PipeEvents, target?: string, update = false): OwnedConnection {
    if (!this.generation || this.stopped) throw Error("Embedded owner is not ready");
    if (this.streams.size >= 32 || this.nextId === 0xffffffff) throw Error("Embedded stream capacity exhausted");
    if (target !== undefined && (!isAcceptableTarget(target) || target.length > 4096)) throw Error("Invalid embedded host target");
    if (update && target === undefined) throw Error("Update requires a fixed host");
    const id = this.nextId + 1;
    const command = { type: "open", id, ...(target === undefined ? {} : { target }), update };
    if (Buffer.byteLength(JSON.stringify(command)) > 4096) throw Error("Embedded host open exceeds its command limit");
    this.nextId = id;
    const budget = this.processBudget.connection();
    // Bounded input buffering and one outgoing chunk construction buffer. The
    // NativePipe separately accounts for full frames and their encoded copies.
    const release = budget.reserve(CHUNK * 2 + 4);
    const input = new PassThrough({ highWaterMark: CHUNK });
    const output = new Writable({ highWaterMark: CHUNK,
      write: (bytes: Buffer, _encoding, callback) => this.write(id, bytes, callback),
    });
    let resolveReleased!: () => void;
    const released = new Promise<void>(resolve => { resolveReleased = resolve; });
    const stream: Stream = { input, output, closing: false, release: () => {
      release();
      // A per-stream Closed proves cleanup while the owner is alive. Once the
      // owner carrier fails, only observed helper exit supplies that evidence.
      if (this.stopped) void this.helper.exited.then(resolveReleased);
      else resolveReleased();
    } };
    this.streams.set(id, stream);
    try {
      this.command(command);
      stream.pipe = new NativePipe(input, output, budget, {
        progress: events.progress,
        closed: reason => { this.closeStream(id); events.closed(reason); },
      }, target);
      return { pipe: stream.pipe, released };
    } catch (error) { this.closeStream(id); throw error; }
  }

  close(): void { this.helper.close(); }

  private command(value: unknown): void {
    this.helper.pipe.send(dataFrame(K.text, { generation: this.generation, receipt: randomBytes(16) }, Buffer.from(JSON.stringify(value))));
  }
  private write(id: number, bytes: Buffer, callback: Callback): void {
    let offset = 0, finished = false;
    const advance = (error?: Error): void => {
      if (finished) return;
      if (error || this.stopped || !this.streams.has(id) || this.streams.get(id)!.closing) {
        finished = true; callback(error ?? Error("Embedded stream closed")); return;
      }
      if (offset === bytes.length) { finished = true; callback(); return; }
      try {
        if (this.pending.size >= 32) throw Error("Embedded chunk capacity exhausted");
        const size = Math.min(CHUNK, bytes.length - offset), payload = Buffer.allocUnsafe(size + 4);
        payload.writeUInt32BE(id); bytes.copy(payload, 4, offset, offset + size); offset += size;
        const receipt = randomBytes(16), key = receipt.toString("hex");
        if (this.pending.has(key)) throw Error("Duplicate embedded chunk receipt");
        this.pending.set(key, { id, complete: advance });
        try { this.helper.pipe.send(dataFrame(K.binary, { generation: this.generation, receipt }, payload)); }
        catch (error) { this.pending.delete(key); throw error; }
      } catch (error) { finished = true; callback(error instanceof Error ? error : Error(String(error))); }
    };
    advance();
  }

  private receive(frame: NativeFrame): void {
    if (frame.kind === K.credit) {
      const credit = JSON.parse(new TextDecoder("utf8", { fatal: true }).decode(frame.payload));
      const pending = this.pending.get(credit.receipt);
      this.pending.delete(credit.receipt); pending?.complete(); return;
    }
    const { delivery, bytes } = application(frame);
    let consumed = false;
    const consume = () => {
      if (consumed || this.stopped) return;
      consumed = true;
      this.helper.pipe.send({ kind: K.credit, payload: Buffer.from(JSON.stringify({ generation: this.generation,
        receipt: Buffer.from(delivery.receipt).toString("hex") })) });
    };
    if (frame.kind === K.text) {
      const value = JSON.parse(new TextDecoder("utf8", { fatal: true }).decode(bytes));
      if (!value || value.type !== "closed" || !Number.isInteger(value.id) || value.id < 1 || value.id > this.nextId
        || typeof value.message !== "string" || typeof value.retryable !== "boolean"
        || Object.keys(value).some(key => !["type", "id", "message", "retryable"].includes(key))) throw Error("Invalid embedded stream result");
      consume(); this.retire(value.id, { code: "backendLost", retryable: value.retryable, message: value.message }); return;
    }
    if (frame.kind !== K.binary || bytes.length <= 4 || bytes.length > CHUNK + 4) throw Error("Invalid embedded stream chunk");
    const id = new DataView(bytes.buffer, bytes.byteOffset, 4).getUint32(0, false);
    if (!id || id > this.nextId) throw Error("Unknown embedded stream");
    const stream = this.streams.get(id);
    if (!stream || stream.closing) { consume(); return; }
    // The ring-buffer reservation survives delivery; the outer receipt stays
    // charged until Node has accepted the chunk, not merely scheduled a write.
    stream.input.write(Buffer.from(bytes.subarray(4)), error => {
      try { consume(); } catch (error) { this.helper.pipe.close({ code: "protocol", retryable: false, message: String(error) }); }
      if (error) this.closeStream(id);
    });
  }

  private closeStream(id: number): void {
    const stream = this.streams.get(id);
    if (!stream || stream.closing) return;
    stream.closing = true;
    stream.input.destroy(); stream.output.destroy();
    for (const [receipt, pending] of this.pending) {
      if (pending.id === id) { this.pending.delete(receipt); pending.complete(Error("Embedded stream closed")); }
    }
    if (!this.stopped) {
      try { this.helper.pipe.send({ kind: K.cancel,
        payload: Buffer.from(JSON.stringify({ generation: this.generation, id })) }); }
      catch { this.helper.close(); }
    }
    // Capacity remains held until the helper confirms actual stream cleanup.
  }
  private retire(id: number, reason: NativeClosed): void {
    const stream = this.streams.get(id);
    if (!stream) return;
    // Install the typed reason before cancelling pending physical writes;
    // their generic stream errors must not replace a permanent host failure.
    try { stream.pipe?.close(reason); }
    finally {
      try { this.closeStream(id); }
      finally { this.streams.delete(id); stream.release(); }
    }
  }
  private ended(reason: NativeClosed): void {
    if (this.stopped) return;
    this.stopped = true;
    for (const id of Array.from(this.streams.keys())) {
      try { this.retire(id, reason); }
      catch (error) { console.error("[native] stream cleanup notification failed", error); }
    }
    try { this.events.closed(reason); }
    catch (error) { console.error("[native] owner close notification failed", error); }
  }
}
