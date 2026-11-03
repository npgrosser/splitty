import { describe, expect, it } from "vitest";
import fixture from "../../../../docs/native-transport-v1.json";
import { application, dataFrame, encodeNativeFrame, NativeFrameDecoder, NativeKind,
  NATIVE_MAGIC, MAX_IPC_CHUNK_BYTES, type NativeFrame } from "./native_wire.ts";

function unhex(hex: string): Uint8Array {
  return Uint8Array.from(hex.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
}
function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

describe("native transport v1", () => {
  it("stops a coalesced batch immediately when its callback revokes the decoder", () => {
    const frame = encodeNativeFrame({ kind: NativeKind.ping, payload: new TextEncoder().encode('{"nonce":1}') });
    const batch = new Uint8Array(frame.length * 2);
    batch.set(frame); batch.set(frame, frame.length);
    const decoder = new NativeFrameDecoder();
    let delivered = 0;
    decoder.feed(batch, () => { delivered++; decoder.end(); });
    expect(delivered).toBe(1);
    expect(() => decoder.feed(frame, () => delivered++)).toThrow(/closed/);
  });
  it("matches Rust's byte-exact golden fixtures at every possible split", () => {
    expect(hex(NATIVE_MAGIC)).toBe(fixture.magicHex);
    for (const row of fixture.frames) {
      const bytes = unhex(row.frameHex);
      for (let split = 0; split <= bytes.length; split++) {
        const decoder = new NativeFrameDecoder();
        const frames: NativeFrame[] = [];
        decoder.feed(bytes.subarray(0, split), (frame) => frames.push(frame));
        decoder.feed(bytes.subarray(split), (frame) => frames.push(frame));
        decoder.end();
        expect(frames).toHaveLength(1);
        const frame = frames[0]!;
        expect(frame.kind).toBe(row.kind);
        expect(hex(frame.payload)).toBe(row.payloadHex);
        expect(hex(encodeNativeFrame(frame))).toBe(row.frameHex);
      }
    }
  });

  it("preserves coalesced frames, generations and NUL/non-UTF-8 binary bytes", () => {
    const receipt = new Uint8Array(16).fill(255);
    const frame = dataFrame(NativeKind.binary, { generation: 10, receipt }, new Uint8Array([0, 255, 128, 0]));
    const bytes = encodeNativeFrame(frame);
    const together = new Uint8Array(bytes.length * 2);
    together.set(bytes); together.set(bytes, bytes.length);
    const decoded: NativeFrame[] = [];
    new NativeFrameDecoder().feed(together, (frame) => decoded.push(frame));
    expect(decoded).toHaveLength(2);
    expect(application(decoded[0]!)).toEqual({ delivery: { generation: 10, receipt }, bytes: new Uint8Array([0, 255, 128, 0]) });
  });

  it("rejects oversized/control/unknown headers before reassembly and stays closed", () => {
    for (const header of [[1, 255, 255, 255, 255], [0, 0, 0, 16, 0], [255, 0, 0, 0, 0], [1, 0, 0, 0, 1]]) {
      const decoder = new NativeFrameDecoder();
      expect(() => decoder.feed(new Uint8Array(header), () => { throw new Error("must not deliver"); })).toThrow();
      expect(decoder.bufferedBytes).toBe(0);
      expect(() => decoder.feed(new Uint8Array(), () => {})).toThrow("closed");
    }
    expect(() => new NativeFrameDecoder().feed(new Uint8Array(MAX_IPC_CHUNK_BYTES + 1), () => {})).toThrow("chunk");
  });

  it("counts announced reassembly and rejects truncated EOF", () => {
    const decoder = new NativeFrameDecoder();
    decoder.feed(new Uint8Array([1, 0, 0, 1, 0]), () => {});
    expect(decoder.bufferedBytes).toBe(261);
    expect(() => decoder.end()).toThrow("Truncated");
    expect(decoder.bufferedBytes).toBe(0);
  });
});
