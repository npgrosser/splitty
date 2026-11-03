import { expect, it, vi } from "vitest";
import { openElectronChannel, type ElectronNativeBridge } from "./electron_channel.ts";
import { NativeSocket, type NativeAssignment } from "./native_socket.ts";
import { dataFrame, encodeNativeFrame, NativeKind as K } from "./native_wire.ts";

function bridge() {
  let receive!: (chunk: Uint8Array) => void, closed!: (reason: unknown) => void;
  const order: string[] = [];
  const value: ElectronNativeBridge = {
    state: vi.fn(), onRetry: vi.fn(() => () => {}),
    cancelOpen: vi.fn(),
    open: async () => ({ handle: "assigned", document: "document", instance: "a".repeat(32), generation: 1, role: "main" }),
    listen: (_handle, incoming, ended) => { receive = incoming; closed = ended; return vi.fn(); },
    start: vi.fn(() => { order.push("start"); }), write: vi.fn(() => { order.push("write"); }), close: vi.fn(),
  };
  return { value, order, receive: (chunk: Uint8Array) => receive(chunk), closed: (reason: unknown) => closed(reason) };
}
it("uses native assignment and assembles fragmented binary deliveries through the private bridge", async () => {
  const b = bridge(), socket = new NativeSocket(), seen = vi.fn();
  socket.onBinaryMessage(seen); socket.adopt(await openElectronChannel(b.value));
  const bytes = encodeNativeFrame(dataFrame(K.binary, { generation: 1, receipt: new Uint8Array(16) }, new Uint8Array([2, 115, 49, 0, 255, 128])));
  for (const byte of bytes) b.receive(new Uint8Array([byte]));
  expect(seen).toHaveBeenCalledWith("s1", new Uint8Array([0, 255, 128]));
  expect(b.value.write).toHaveBeenCalledTimes(1); socket.dispose();
  b.receive(bytes); expect(seen).toHaveBeenCalledTimes(1); expect(b.value.close).toHaveBeenCalledWith("assigned");
});
it("allows the connection callback to enqueue input before activating delivery", async () => {
  const b = bridge(), socket = new NativeSocket();
  socket.onConnected = () => socket.send({ type: "mux.resync", screenId: "screen" });
  socket.adopt(await openElectronChannel(b.value));
  expect(b.order).toEqual(["write", "start"]); socket.dispose();
});
it("surfaces native closure as an uncertain mutation without replay or HTTP fallback", async () => {
  const b = bridge(), socket = new NativeSocket(); socket.adopt(await openElectronChannel(b.value));
  const pending = socket.request({ operation: "template.save", args: { slug: "one", body: {} } });
  const rejected = expect(pending).rejects.toMatchObject({ uncertain: true });
  b.closed({ code: "backendLost", message: "document ended", retryable: true });
  await rejected; expect(b.value.write).toHaveBeenCalledTimes(1); expect(socket.assignment).toBeUndefined();
});

it("cancels native preparation immediately when the caller aborts", async () => {
  const b = bridge(), abort = new AbortController();
  let reject!: (error: Error) => void;
  b.value.open = vi.fn(() => new Promise<NativeAssignment>((_resolve, fail) => { reject = fail; }));
  b.value.cancelOpen = vi.fn(() => reject(Error("cancelled")));
  const opening = openElectronChannel(b.value, { target: "vm1" }, abort.signal);
  abort.abort(); await expect(opening).rejects.toThrow("cancelled");
  expect(b.value.cancelOpen).toHaveBeenCalledWith(vi.mocked(b.value.open).mock.calls[0]![1]);
  expect(b.value.start).not.toHaveBeenCalled();
});
