import { PassThrough, Writable } from "node:stream";
import { setImmediate as nextTurn } from "node:timers/promises";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NativeProcessBudget } from "../../../../frontend-electron/src/native_budget.ts";
import { HOST_PREPARATION_MS, NativePipe, validateNativeReady, type NativeReady } from "../../../../frontend-electron/src/native_pipe.ts";
import { NativeKind as K, NATIVE_MAGIC, NativeFrameDecoder, dataFrame, encodeNativeFrame, type NativeFrame } from "./native_wire.ts";

const encode = new TextEncoder();
const control = (kind: NativeFrame["kind"], value: unknown): NativeFrame => ({ kind, payload: encode.encode(JSON.stringify(value)) });
const identity: NativeReady = { generation: 7, instance: "0123456789abcdef0123456789abcdef", muxProtocol: 1,
  purpose: { kind: "direct" }, limits: { frameBytes: 16 * 1024 * 1024, apiBytes: 1024 * 1024, pendingRequests: 64, connectionBytes: 32 * 1024 * 1024 } };
const live: NativePipe[] = [];
afterEach(() => { live.forEach(pipe => pipe.close()); live.length = 0; vi.useRealTimers(); });
function fixture(options: { target?: string; holdWrites?: boolean } = {}) {
  const process = new NativeProcessBudget(), read = new PassThrough(), output: Buffer[] = [];
  const callbacks: (() => void)[] = [];
  const write = new Writable({ write(chunk, _encoding, callback) {
    output.push(Buffer.from(chunk));
    if (options.holdWrites) callbacks.push(callback); else callback();
  } });
  const closed = vi.fn(), progress = vi.fn(), receive = vi.fn();
  const pipe = new NativePipe(read, write, process.connection(), { closed, progress }, options.target);
  // Failures before admission are asserted through both ready and close events.
  void pipe.ready.catch(() => {}); live.push(pipe);
  const input = (...frames: NativeFrame[]) => read.write(Buffer.concat(frames.map(frame => encodeNativeFrame(frame))));
  const start = (ready = identity) => {
    read.write(NATIVE_MAGIC); input(control(K.accept, { transportVersion: 1 }), control(K.ready, ready));
    pipe.activate(receive);
  };
  const sent = () => {
    const bytes = Buffer.concat(output), frames: NativeFrame[] = [], decoder = new NativeFrameDecoder();
    for (let offset = 8; offset < bytes.length; offset += 65536) decoder.feed(bytes.subarray(offset, offset + 65536), frame => frames.push(frame));
    return frames;
  };
  return { pipe, process, read, write, output, callbacks, closed, progress, receive, input, start, sent };
}
function data(kind: NativeFrame["kind"] = K.text, receiptByte = 1, generation = 7, body = new Uint8Array([0, 255, 128])) {
  return dataFrame(kind, { generation, receipt: new Uint8Array(16).fill(receiptByte) }, body);
}
const credit = (receiptByte = 1, generation = 7) => control(K.credit, { generation, receipt: receiptByte.toString(16).padStart(2, "0").repeat(16) });
async function tick() { await nextTurn(); }

describe("native helper pipe", () => {
  it("negotiates split magic and coalesced frames, retaining welcome until document attachment", async () => {
    const f = fixture();
    for (const byte of NATIVE_MAGIC) f.read.write(new Uint8Array([byte]));
    f.input(control(K.accept, { transportVersion: 1 }), control(K.ready, identity), data(K.binary));
    expect(await f.pipe.ready).toEqual(identity);
    expect(f.receive).not.toHaveBeenCalled(); expect(f.process.counts.data).toBe(28);
    f.pipe.activate(f.receive); expect(f.receive).toHaveBeenCalledWith(data(K.binary));
    f.pipe.send(credit()); await tick(); expect(f.process.counts.data).toBe(0);
    expect(f.sent().map(frame => frame.kind)).toEqual([K.hello, K.readyAccept, K.credit]);
  });
  it("checks the native-assigned host and limits instead of trusting a ready envelope", () => {
    expect(validateNativeReady({ ...identity, purpose: { kind: "host", target: "vm" } }, "vm")).toBeTruthy();
    for (const bad of [
      { ...identity, generation: 0 }, { ...identity, instance: "HOST" }, { ...identity, muxProtocol: 2 },
      { ...identity, role: "main" }, { ...identity, limits: { ...identity.limits, connectionBytes: 1 } },
      { ...identity, purpose: { kind: "host", target: "other" } }, { ...identity, purpose: { kind: "direct", target: "vm" } },
    ]) expect(() => validateNativeReady(bad, "vm")).toThrow();
  });
  it("keeps launcher-owner readiness separate from direct and host assignments", () => {
    const owner = { ...identity, purpose: { kind: "owner" }, limits: { ...identity.limits, frameBytes: 65565 } };
    expect(validateNativeReady(owner, undefined, true).purpose.kind).toBe("owner");
    expect(() => validateNativeReady(owner)).toThrow();
    expect(() => validateNativeReady(owner, "vm")).toThrow();
    expect(() => validateNativeReady(identity, undefined, true)).toThrow();
  });
  it.each(["badMagic", "earlyData", "version", "hostMismatch", "lateReady", "badJson"])("rejects %s before effects", async variant => {
    const f = fixture();
    if (variant === "badMagic") f.read.write(Buffer.from("SPLITTY\x02"));
    else {
      f.read.write(NATIVE_MAGIC);
      if (variant === "earlyData") f.input(data());
      if (variant === "version") f.input(control(K.accept, { transportVersion: 2 }));
      if (variant === "badJson") f.input({ kind: K.accept, payload: new Uint8Array([255]) });
      if (variant === "hostMismatch") f.input(control(K.accept, { transportVersion: 1 }), control(K.ready, { ...identity, purpose: { kind: "host", target: "vm" } }));
      if (variant === "lateReady") {
        f.input(control(K.accept, { transportVersion: 1 }), control(K.ready, identity));
        f.pipe.activate(f.receive); f.input(control(K.ready, identity));
      }
    }
    expect(f.closed).toHaveBeenCalledTimes(1); expect(f.receive).not.toHaveBeenCalled();
    await tick(); expect(f.process.counts.data).toBe(0);
  });
  it("never extends the preparation deadline for progress and cleans partial frames", async () => {
    vi.useFakeTimers(); const f = fixture();
    f.read.write(NATIVE_MAGIC); f.input(control(K.accept, { transportVersion: 1 }));
    for (let i = 0; i < 3; i++) { vi.advanceTimersByTime(30_000); f.input(control(K.progress, { phase: "ssh", message: "waiting" })); }
    expect(f.progress).toHaveBeenCalledTimes(3); vi.advanceTimersByTime(30_000);
    await expect(f.pipe.ready).rejects.toThrow("preparation timed out");
    const partial = fixture(); partial.start(); partial.read.write(encodeNativeFrame(data()).subarray(0, 6));
    expect(partial.process.counts.data).toBe(28); vi.advanceTimersByTime(60_000);
    expect(partial.closed).toHaveBeenCalled(); expect(partial.process.counts.data).toBe(0);
  });
  it("allows a slow host installation past two minutes while retaining a fixed preparation deadline", async () => {
    vi.useFakeTimers();
    const host = fixture({ target: "vm" });
    host.read.write(NATIVE_MAGIC); host.input(control(K.accept, { transportVersion: 1 }));
    vi.advanceTimersByTime(180_000);
    expect(host.closed).not.toHaveBeenCalled();
    const ready: NativeReady = { ...identity, purpose: { kind: "host", target: "vm" } };
    host.input(control(K.ready, ready));
    await expect(host.pipe.ready).resolves.toEqual(ready);
    host.pipe.activate(host.receive);
    host.pipe.close();

    const stalled = fixture({ target: "vm" });
    stalled.read.write(NATIVE_MAGIC); stalled.input(control(K.accept, { transportVersion: 1 }));
    for (let elapsed = 60_000; elapsed < HOST_PREPARATION_MS; elapsed += 60_000) {
      vi.advanceTimersByTime(60_000);
      stalled.input(control(K.progress, { phase: "install", message: "waiting" }));
    }
    expect(stalled.closed).not.toHaveBeenCalled();
    vi.advanceTimersByTime(60_000);
    await expect(stalled.pipe.ready).rejects.toThrow("preparation timed out");
    // A physical Node write keeps its lease until its callback completes.
    await tick();
    expect(stalled.process.counts).toEqual({ data: 0, controls: 0, workers: 0 });
  });
  it("holds outgoing memory until both pipe delivery and the backend receipt", async () => {
    const f = fixture(); f.start(); await tick();
    f.pipe.send(data(K.text)); await tick();
    expect(f.process.counts.data).toBe(56);
    f.input(credit()); expect(f.receive).toHaveBeenCalledWith(credit());
    expect(f.process.counts.data).toBe(0);
    f.input(credit()); expect(f.closed).toHaveBeenCalledTimes(1);
  });
  it("retains received payload bytes independently through IPC acknowledgement and native credit", () => {
    const f = fixture(); let consumed!: () => void;
    f.receive.mockImplementation(frame => { consumed = f.pipe.retain(frame); });
    f.start(); f.input(data(K.binary));
    expect(f.process.counts.data).toBe(28);
    f.pipe.send(credit()); expect(f.process.counts.data).toBe(28);
    consumed(); consumed(); expect(f.process.counts.data).toBe(0);
  });
  it("does not bypass retained IPC ownership when a decoder callback throws", () => {
    const f = fixture(); let consumed!: () => void;
    f.receive.mockImplementation(frame => { consumed = f.pipe.retain(frame); throw Error("delivery failed"); });
    f.start(); f.input(data(K.binary));
    expect(f.closed).toHaveBeenCalledTimes(1); expect(f.process.counts.data).toBe(28);
    consumed(); expect(f.process.counts.data).toBe(0);
  });
  it("retains a blocked physical write after close until its write callback", async () => {
    const f = fixture({ holdWrites: true }); f.start();
    // Flush the three handshake writes; the next data write remains in Node.
    for (let i = 0; i < 3; i++) { f.callbacks.shift()!(); await tick(); }
    f.pipe.send(data(K.text)); expect(f.process.counts.data).toBe(56);
    f.input(credit()); expect(f.process.counts.data).toBe(56);
    f.pipe.close(); expect(f.process.counts.data).toBe(56);
    f.callbacks.shift()!(); await tick(); expect(f.process.counts.data).toBe(0);
  });
  it("rejects a backend credit for data still queued behind a blocked write", async () => {
    const f = fixture({ holdWrites: true }); f.start();
    f.pipe.send(data(K.text)); f.input(credit());
    expect(f.closed).toHaveBeenCalledTimes(1); expect(f.process.counts.data).toBe(0);
    f.callbacks.shift()!(); await tick(); expect(f.process.counts.controls).toBe(0);
  });
  it("does not treat a complete header as permission to exceed the process byte budget", () => {
    const f = fixture(); f.start();
    const reserve = Array.from({ length: 4 }, () => f.process.connection().reserve(30 * 1024 * 1024));
    f.read.write(encodeNativeFrame(data()).subarray(0, 5));
    expect(f.closed).toHaveBeenCalledTimes(1);
    expect(f.process.counts.data).toBe(120 * 1024 * 1024);
    reserve.forEach(release => release()); expect(f.process.counts.data).toBe(0);
  });
  it("rejects forged, stale and duplicate renderer credits", () => {
    for (const acknowledge of [credit(2), credit(1, 6)]) {
      const f = fixture(); f.start(); f.input(data());
      expect(() => f.pipe.send(acknowledge)).toThrow(); expect(f.process.counts.data).toBe(0);
    }
    const f = fixture(); f.start(); f.input(data()); f.pipe.send(credit());
    expect(() => f.pipe.send(credit())).toThrow();
  });
  it("caps delivery count independently, and preserves the other connection", async () => {
    const f = fixture(), other = fixture(); f.start(); other.start();
    for (let i = 0; i < 65; i++) f.input(data(K.binary, i));
    expect(f.closed).toHaveBeenCalledTimes(1); expect(f.process.counts.data).toBe(0);
    other.input(data()); other.pipe.send(credit()); await tick();
    expect(other.closed).not.toHaveBeenCalled(); expect(other.process.counts.data).toBe(0);
  });
  it("rejects wrong directions and stale generations", () => {
    for (const frame of [data(K.request), data(K.text, 1, 9), control(K.cancel, { id: 1, generation: 7 })]) {
      const f = fixture(); f.start(); f.input(frame); expect(f.closed).toHaveBeenCalledTimes(1);
    }
    const f = fixture(); f.start(); expect(() => f.pipe.send(data(K.reply))).toThrow();
  });
  it("services ping while renderer consumption is stalled, and requires the matching pong", async () => {
    vi.useFakeTimers(); const f = fixture(); f.start(); f.input(data());
    f.input(control(K.ping, { nonce: 123 })); await tick();
    vi.advanceTimersByTime(15_000); await tick();
    expect(f.sent().some(frame => frame.kind === K.pong)).toBe(true);
    f.input(control(K.pong, { nonce: 1 })); expect(f.closed).not.toHaveBeenCalled();
    f.input(control(K.pong, { nonce: 1 })); expect(f.closed).toHaveBeenCalledTimes(1);
  });
  it("preserves permanent typed failures and revokes only once on EOF", async () => {
    const f = fixture(); f.read.write(NATIVE_MAGIC); f.input(control(K.accept, { transportVersion: 1 }));
    f.input(control(K.closed, { code: "incompatible", message: "upgrade required", retryable: false }));
    await expect(f.pipe.ready).rejects.toThrow("upgrade required");
    f.read.end(); f.pipe.close(); expect(f.closed).toHaveBeenCalledTimes(1);
    expect(f.closed).toHaveBeenCalledWith({ code: "incompatible", message: "upgrade required", retryable: false });
  });
});
