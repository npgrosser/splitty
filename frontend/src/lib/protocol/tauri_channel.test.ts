import { expect, it, vi } from "vitest";
import { openTauriChannel, type TauriNativeBridge } from "./tauri_channel.ts";
import { NativeSocket } from "./native_socket.ts";
import { dataFrame, encodeNativeFrame, NativeKind as K } from "./native_wire.ts";

function bridge() {
  let receive!: (chunk: Uint8Array, token: string) => void;
  let closed!: (reason: unknown) => void;
  const value: TauriNativeBridge = {
    state: vi.fn(),
    cancelOpen: vi.fn(),
    open: async () => ({ handle: "assigned", document: "document", instance: "a".repeat(32), generation: 1, role: "main" }),
    listen: (_handle, incoming, ended) => { receive = incoming; closed = ended; return vi.fn(); },
    start: vi.fn(async () => {}), write: vi.fn(async () => {}), consumed: vi.fn(async () => {}), close: vi.fn(),
  };
  return { value, receive: (chunk: Uint8Array, token = "token") => receive(chunk, token), closed: (reason: unknown) => closed(reason) };
}
const settle = async () => { for (let turn = 0; turn < 15; turn++) await Promise.resolve(); };
it("copies fragmented binary data before acknowledging native chunks", async () => {
  const b = bridge(), socket = new NativeSocket(), seen = vi.fn();
  socket.onBinaryMessage(seen); socket.adopt(await openTauriChannel(b.value));
  const bytes = encodeNativeFrame(dataFrame(K.binary, { generation: 1, receipt: new Uint8Array(16) }, new Uint8Array([2, 115, 49, 0, 255, 128])));
  for (const byte of bytes) b.receive(new Uint8Array([byte]));
  expect(seen).toHaveBeenCalledWith("s1", new Uint8Array([0, 255, 128]));
  expect(b.value.consumed).toHaveBeenCalledTimes(bytes.length);
  await settle(); socket.dispose();
  b.receive(bytes); expect(seen).toHaveBeenCalledTimes(1);
});
it("waits for native start and each write completion before sending another chunk", async () => {
  const b = bridge();
  let start!: () => void, accepted!: () => void;
  b.value.start = vi.fn(() => new Promise<void>(resolve => { start = resolve; }));
  b.value.write = vi.fn(() => new Promise<void>(resolve => { accepted = resolve; }));
  const channel = await openTauriChannel(b.value);
  channel.subscribe(() => {});
  channel.write(dataFrame(K.text, { generation: 1, receipt: new Uint8Array(16) }, new Uint8Array(130_000)));
  channel.start();
  expect(b.value.write).not.toHaveBeenCalled();
  start(); await settle();
  expect(b.value.write).toHaveBeenCalledTimes(1);
  expect(vi.mocked(b.value.write).mock.calls[0][1].length).toBe(65536);
  accepted(); await settle();
  expect(b.value.write).toHaveBeenCalledTimes(2);
  channel.close(); accepted(); await settle();
  expect(b.value.close).toHaveBeenCalledOnce();
});
it("cancels pending opens and closes late assignments after cancellation", async () => {
  const b = bridge(), abort = new AbortController();
  let complete!: (value: Awaited<ReturnType<TauriNativeBridge["open"]>>) => void;
  b.value.open = vi.fn(() => new Promise<Awaited<ReturnType<TauriNativeBridge["open"]>>>(resolve => { complete = resolve; }));
  const opening = openTauriChannel(b.value, undefined, abort.signal);
  abort.abort();
  complete({ handle: "late", document: "document", instance: "a".repeat(32), generation: 1, role: "main" });
  await expect(opening).rejects.toMatchObject({ name: "AbortError" });
  expect(b.value.cancelOpen).toHaveBeenCalledOnce();
  expect(b.value.close).toHaveBeenCalledWith("late");
});
it("rejects excess queued frames and oversized incoming chunks independently", async () => {
  const b = bridge(), channel = await openTauriChannel(b.value);
  channel.subscribe(() => {});
  for (let index = 0; index < 64; index++) channel.write(dataFrame(K.text, { generation: 1, receipt: new Uint8Array(16) }, new Uint8Array([1])));
  expect(() => channel.write(dataFrame(K.text, { generation: 1, receipt: new Uint8Array(16) }, new Uint8Array([1])))).toThrow("queue is full");
  expect(b.value.close).toHaveBeenCalledOnce();
  const c = bridge(), other = await openTauriChannel(c.value), receive = vi.fn();
  other.subscribe(receive); c.receive(new Uint8Array(65537));
  expect(c.value.consumed).not.toHaveBeenCalled();
  expect(c.value.close).toHaveBeenCalledOnce();
});
it("preserves native preparation retry policy without transport fallback", async () => {
  const b = bridge();
  b.value.open = vi.fn(async () => { throw "SplittyNativeFailure:" + JSON.stringify({ code: "backendLost", message: "Host identity changed", retryable: false }); });
  await expect(openTauriChannel(b.value)).rejects.toMatchObject({ code: "backendLost", message: "Host identity changed", retryable: false });
  expect(b.value.start).not.toHaveBeenCalled();
});
