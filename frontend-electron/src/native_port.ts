/** Document-bound MessagePort framing. The private preload keeps the actual
 * port; each direction permits one <=64KiB IPC chunk until its acknowledgement.
 * Native frame receipts remain independent and survive IPC fragmentation. */
import { randomBytes } from "node:crypto";
import type { MessagePortMain } from "electron";
import type { NativeAssignment } from "./native_types";
import type { NativePipe, NativeClosed } from "./native_pipe";
import { NativeFrameDecoder, isData, MAX_IPC_CHUNK_BYTES, type NativeFrame } from "./native_wire";

interface Output { frame: NativeFrame; offset: number; release(): void }
interface Flight { token: string; release(): void; timer: ReturnType<typeof setTimeout> }
export type NativeMessagePort = Pick<MessagePortMain, "on" | "postMessage" | "start" | "close">;

export class NativePort {
  private readonly decoder: NativeFrameDecoder;
  private closed = false;
  private started = false;
  private output: Output[] = [];
  private flight?: Flight;
  private assembly?: ReturnType<typeof setTimeout>;
  private dataFrames = 0;
  private controls = 0;
  private readonly inputScratch: () => void;

  constructor(private readonly port: NativeMessagePort, private readonly pipe: NativePipe,
    readonly assignment: NativeAssignment, private readonly current: () => boolean) {
    // One private-preload chunk may be in transit. Reserve that allowance
    // before data fills the window, so receipt/cancel input is still accepted.
    this.inputScratch = pipe.budget.reserve(MAX_IPC_CHUNK_BYTES);
    this.decoder = new NativeFrameDecoder((bytes, control) => pipe.budget.reserve(bytes, control));
    port.on("message", event => {
      try { this.receive(event.data); }
      catch (error) { this.close({ code: "protocol", retryable: false, message: error instanceof Error ? error.message : String(error) }); }
    });
    port.on("close", () => this.close());
    port.start();
  }

  close(reason: NativeClosed = { code: "closed", retryable: false, message: "Native document connection closed" }): void {
    if (this.closed) return;
    this.closed = true;
    clearTimeout(this.assembly);
    try { this.decoder.end(); } catch { /* Revocation discards partial input. */ }
    if (this.flight) { clearTimeout(this.flight.timer); this.flight.release(); this.flight = undefined; }
    for (const entry of this.output) entry.release();
    this.output = []; this.dataFrames = 0; this.controls = 0;
    // This port itself belongs to the old document. A revocation notice is
    // safe after its grant changes and lets that document reject pending work;
    // application data still requires current() on every send.
    try { this.post({ type: "closed", reason }); } catch { /* The document may already be gone. */ }
    try { this.port.close(); }
    finally { this.inputScratch(); this.pipe.close(reason); }
  }

  private post(value: Record<string, unknown>): void {
    this.port.postMessage({ ...value, handle: this.assignment.handle, document: this.assignment.document });
  }
  private receive(raw: unknown): void {
    if (this.closed) return;
    if (!this.current()) { this.close(); return; }
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw Error("Invalid native port envelope");
    const value = raw as Record<string, unknown>;
    if (value.handle !== this.assignment.handle || value.document !== this.assignment.document) throw Error("Native port belongs to another document");
    const fields = value.type === "chunk" ? ["type", "handle", "document", "token", "bytes"]
      : value.type === "consumed" ? ["type", "handle", "document", "token"] : ["type", "handle", "document"];
    if (Object.keys(value).some(key => !fields.includes(key))) throw Error("Unexpected native port fields");
    if (value.type === "close") { this.close(); return; }
    if (value.type === "start") {
      if (this.started) throw Error("Native port already started");
      this.started = true; this.pipe.activate(frame => this.queue(frame)); return;
    }
    if (!this.started) throw Error("Native port is not started");
    if (value.type === "consumed") {
      const flight = this.flight;
      if (!flight || value.token !== flight.token) throw Error("Unknown native IPC receipt");
      this.flight = undefined; clearTimeout(flight.timer); flight.release();
      const entry = this.output[0]!;
      if (entry.offset === entry.frame.payload.length + 5) {
        this.output.shift(); entry.release();
        if (isData(entry.frame.kind)) this.dataFrames--; else this.controls--;
      }
      this.pump(); return;
    }
    if (value.type !== "chunk" || !(value.bytes instanceof Uint8Array) || value.bytes.length < 1
      || value.bytes.length > MAX_IPC_CHUNK_BYTES || typeof value.token !== "string" || !/^[0-9a-f]{32}$/.test(value.token)) throw Error("Invalid native IPC chunk");
    {
      if (!this.assembly) this.assembly = setTimeout(() => this.close({ code: "timeout", retryable: false, message: "Native IPC frame assembly timed out" }), 60_000);
      this.decoder.feed(value.bytes, (frame, consumed) => {
        clearTimeout(this.assembly); this.assembly = undefined;
        try { this.pipe.send(frame); } finally { consumed(); }
      });
      if (this.decoder.bufferedBytes && !this.assembly) this.assembly = setTimeout(() => this.close({ code: "timeout", retryable: false, message: "Native IPC frame assembly timed out" }), 60_000);
      if (!this.closed) this.post({ type: "accepted", token: value.token });
    }
  }

  private queue(frame: NativeFrame): void {
    if (this.closed || !this.current()) { this.close(); return; }
    const data = isData(frame.kind);
    if ((data ? this.dataFrames : this.controls) >= 64) throw Error("Native IPC frame capacity exhausted");
    // A renderer can return the native receipt before the final IPC chunk ack.
    // Both owners retain the same lease until their independent delivery ends.
    const release = data ? this.pipe.retain(frame) : this.pipe.budget.reserve(frame.payload.length + 5, true);
    this.output.push({ frame, offset: 0, release });
    if (data) this.dataFrames++; else this.controls++;
    this.pump();
  }
  private pump(): void {
    if (this.closed || this.flight || !this.output.length) return;
    if (!this.current()) { this.close(); return; }
    const entry = this.output[0]!, size = Math.min(MAX_IPC_CHUNK_BYTES, entry.frame.payload.length + 5 - entry.offset);
    const release = this.pipe.budget.reserve(size, !isData(entry.frame.kind));
    try {
      const bytes = new Uint8Array(size);
      let offset = 0;
      if (entry.offset === 0) {
        bytes[0] = entry.frame.kind;
        new DataView(bytes.buffer).setUint32(1, entry.frame.payload.length, false);
        offset = 5;
      }
      const payloadOffset = Math.max(0, entry.offset - 5);
      bytes.set(entry.frame.payload.subarray(payloadOffset, payloadOffset + size - offset), offset);
      entry.offset += size;
      const token = randomBytes(16).toString("hex");
      this.flight = { token, release, timer: setTimeout(() => this.close({ code: "timeout", retryable: true, message: "Native IPC consumer stalled" }), 30_000) };
      this.post({ type: "chunk", bytes, token });
    } catch (error) {
      if (this.flight) { clearTimeout(this.flight.timer); this.flight = undefined; }
      release(); throw error;
    }
  }
}
