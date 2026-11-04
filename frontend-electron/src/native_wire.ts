/** Native transport v1 framing. Domain operations and window authority are
 * validated by the assigned native broker/service, never inferred here. */
export const NATIVE_MAGIC = new Uint8Array([83, 80, 76, 73, 84, 84, 89, 1]);
export const FRAME_HEADER_BYTES = 5;
export const DELIVERY_BYTES = 20;
export const MAX_NATIVE_FRAME_BYTES = 16 * 1024 * 1024;
export const MAX_NATIVE_CONTROL_BYTES = 4096;
export const MAX_NATIVE_API_BYTES = 1024 * 1024;
export const MAX_IPC_CHUNK_BYTES = 64 * 1024;

export const NativeKind = {
  hello: 0, text: 1, binary: 2, accept: 3, ready: 4, request: 5, reply: 6,
  cancel: 7, ping: 8, pong: 9, credit: 10, progress: 11, closed: 12, readyAccept: 13,
} as const;
export type NativeKind = (typeof NativeKind)[keyof typeof NativeKind];
export interface NativeFrame { kind: NativeKind; payload: Uint8Array }
export interface Delivery { generation: number; receipt: Uint8Array }

export function isData(kind: NativeKind): boolean {
  return kind === NativeKind.text || kind === NativeKind.binary
    || kind === NativeKind.request || kind === NativeKind.reply;
}

function checkedKind(kind: number): NativeKind {
  if (!Number.isInteger(kind) || kind < 0 || kind > 13) throw new Error("Unknown native frame kind");
  return kind as NativeKind;
}

function checkLength(kind: NativeKind, length: number): void {
  const maximum = kind === NativeKind.request ? MAX_NATIVE_API_BYTES + DELIVERY_BYTES
    : (isData(kind) ? MAX_NATIVE_FRAME_BYTES : MAX_NATIVE_CONTROL_BYTES) - FRAME_HEADER_BYTES;
  if (length > maximum || (isData(kind) && length < DELIVERY_BYTES)) {
    throw new Error("Invalid native frame length");
  }
}

export function application(frame: NativeFrame): { delivery: Delivery; bytes: Uint8Array } {
  if (!isData(frame.kind) || frame.payload.length < DELIVERY_BYTES) {
    throw new Error("Missing native delivery header");
  }
  const generation = new DataView(frame.payload.buffer, frame.payload.byteOffset, 4).getUint32(0, false);
  if (generation === 0) throw new Error("Invalid native generation");
  return {
    delivery: { generation, receipt: frame.payload.subarray(frame.payload.length - 16) },
    bytes: frame.payload.subarray(4, frame.payload.length - 16),
  };
}

export function dataFrame(kind: NativeKind, delivery: Delivery, bytes: Uint8Array): NativeFrame {
  checkedKind(kind);
  if (!isData(kind) || !Number.isInteger(delivery.generation)
    || delivery.generation < 1 || delivery.generation > 0xffffffff || delivery.receipt.length !== 16) {
    throw new Error("Invalid native delivery header");
  }
  checkLength(kind, bytes.length + DELIVERY_BYTES);
  const payload = new Uint8Array(bytes.length + DELIVERY_BYTES);
  new DataView(payload.buffer).setUint32(0, delivery.generation, false);
  payload.set(bytes, 4);
  payload.set(delivery.receipt, 4 + bytes.length);
  return { kind, payload };
}

export function encodeNativeFrame(frame: NativeFrame): Uint8Array {
  checkedKind(frame.kind);
  checkLength(frame.kind, frame.payload.length);
  if (isData(frame.kind)) application(frame);
  const bytes = new Uint8Array(frame.payload.length + FRAME_HEADER_BYTES);
  bytes[0] = frame.kind;
  new DataView(bytes.buffer).setUint32(1, frame.payload.length, false);
  bytes.set(frame.payload, FRAME_HEADER_BYTES);
  return bytes;
}

/** Emits frames as they complete instead of accumulating a coalesced batch.
 * Buffers at most one complete frame, charges its announced allocation, and
 * rejects an invalid header before allocating its payload. Callers retain the
 * delivery receipt until IPC consumption and enforce the 60-second deadline. */
export class NativeFrameDecoder {
  constructor(private readonly reserve: (bytes: number, control: boolean) => () => void = () => () => {}) {}
  private readonly header = new Uint8Array(FRAME_HEADER_BYTES);
  private headerUsed = 0;
  private payload: Uint8Array | null = null;
  private payloadUsed = 0;
  private kind: NativeKind = NativeKind.hello;
  private failed = false;
  private release: (() => void) | undefined;

  get bufferedBytes(): number {
    return this.payload ? FRAME_HEADER_BYTES + this.payload.length : this.headerUsed;
  }

  feed(chunk: Uint8Array, receive: (frame: NativeFrame, release: () => void) => void): void {
    if (this.failed) throw new Error("Native decoder is closed");
    try {
      if (chunk.length > MAX_IPC_CHUNK_BYTES) throw new Error("Native IPC chunk exceeds limit");
      let offset = 0;
      while (offset < chunk.length && !this.failed) {
        if (!this.payload) {
          const count = Math.min(FRAME_HEADER_BYTES - this.headerUsed, chunk.length - offset);
          this.header.set(chunk.subarray(offset, offset + count), this.headerUsed);
          this.headerUsed += count;
          offset += count;
          if (this.headerUsed < FRAME_HEADER_BYTES) continue;
          this.kind = checkedKind(this.header[0]!);
          const length = new DataView(this.header.buffer).getUint32(1, false);
          checkLength(this.kind, length);
          this.release = this.reserve(FRAME_HEADER_BYTES + length, !isData(this.kind));
          this.payload = new Uint8Array(length);
          this.payloadUsed = 0;
        }
        const count = Math.min(this.payload.length - this.payloadUsed, chunk.length - offset);
        this.payload.set(chunk.subarray(offset, offset + count), this.payloadUsed);
        this.payloadUsed += count;
        offset += count;
        if (this.payloadUsed === this.payload.length) {
          const frame = { kind: this.kind, payload: this.payload };
          if (isData(frame.kind)) application(frame);
          this.headerUsed = 0;
          this.payload = null;
          this.payloadUsed = 0;
          const release = this.release!;
          this.release = undefined;
          try { receive(frame, release); } catch (error) { release(); throw error; }
        }
      }
    } catch (error) {
      this.failed = true;
      this.release?.(); this.release = undefined;
      this.payload = null;
      this.headerUsed = 0;
      throw error;
    }
  }

  end(): void {
    this.failed = true;
    if (this.bufferedBytes !== 0) {
      this.release?.(); this.release = undefined;
      this.payload = null;
      this.headerUsed = 0;
      throw new Error("Truncated native frame");
    }
  }
}
