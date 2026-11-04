import { afterEach, describe, expect, it, vi } from "vitest";
import { NativeSocket, type NativeChannel, type NativeDelivery } from "./native_socket.ts";
import { application, dataFrame, NativeKind, type NativeFrame } from "./native_wire.ts";
const encoder = new TextEncoder();
function channel(generation = 1, role: "main" | "remote" = "main") {
  let receive: ((delivery: NativeDelivery) => void) | undefined;
  const assignment = { handle: `connection-${generation}`, document: "document-1", instance: "service-1", generation, role, ...(role === "remote" ? { target: "vm1" } : {}) };
  const writes: NativeFrame[] = [];
  const value: NativeChannel = { assignment,
    subscribe(callback) { receive = callback; return () => {}; },
    start: vi.fn(), write: (frame) => { writes.push(frame); }, close: vi.fn(),
  };
  let receipt = 0;
  return { value, writes,
    frame(frame: NativeFrame, envelope = assignment) { receive?.({ handle: envelope.handle, document: envelope.document, frame }); },
    data(kind: NativeKind, payload: string | Uint8Array, gen = generation) {
      this.frame(dataFrame(kind, { generation: gen, receipt: new Uint8Array(16).fill(++receipt) }, typeof payload === "string" ? encoder.encode(payload) : payload));
    },
    reply(id: number, body = "{}") { this.data(NativeKind.reply, JSON.stringify({ id, status: 200, contentType: "json", body })); },
    credit(frame: NativeFrame) { const { delivery } = application(frame); this.frame({ kind: NativeKind.credit, payload: encoder.encode(JSON.stringify({ generation: delivery.generation, receipt: Array.from(delivery.receipt, b => b.toString(16).padStart(2, "0")).join("") })) }); },
  };
}
afterEach(() => vi.useRealTimers());
describe("assigned native connection", () => {
  it("delivers ordered text and binary before returning consumption receipts", () => {
    const socket = new NativeSocket(), ch = channel(); const seen: string[] = [];
    socket.onMessage((message) => { seen.push(message.type); expect(ch.writes).toHaveLength(0); });
    socket.onBinaryMessage((id, bytes) => seen.push(`${id}:${Array.from(bytes)}`)); socket.adopt(ch.value);
    ch.data(NativeKind.text, '{"type":"mux.welcome","clientId":"c","screens":[]}');
    ch.data(NativeKind.binary, new Uint8Array([2, 115, 49, 0, 255, 10]));
    expect(seen).toEqual(["mux.welcome", "s1:0,255,10"]);
    expect(ch.writes.map((frame) => frame.kind)).toEqual([NativeKind.credit, NativeKind.credit]); socket.dispose();
  });
  it("rejects other connections, documents and stale service generations", () => {
    for (const cause of ["handle", "document", "generation"]) {
      const socket = new NativeSocket(), ch = channel(); const receive = vi.fn(), fail = vi.fn();
      socket.onMessage(receive); socket.onError = fail; socket.adopt(ch.value);
      const frame = dataFrame(NativeKind.text, { generation: cause === "generation" ? 2 : 1, receipt: new Uint8Array(16) }, encoder.encode('{"type":"bell"}'));
      ch.frame(frame, { ...ch.value.assignment, ...(cause === "handle" ? { handle: "other" } : cause === "document" ? { document: "old" } : {}) });
      expect(receive).not.toHaveBeenCalled(); expect(fail).toHaveBeenCalledOnce(); expect(ch.value.close).toHaveBeenCalledOnce();
    }
  });
  it("revokes old pending replies and drops unsent input before a replacement assignment", async () => {
    const socket = new NativeSocket(), old = channel(); socket.adopt(old.value);
    const pending = socket.request({ operation: "template.save", args: { slug: "a", body: {} } });
    const failed = expect(pending).rejects.toMatchObject({ uncertain: true }); socket.revoke();
    socket.send({ type: "input", id: "s1", data: "delete\r" });
    const next = channel(2); socket.adopt(next.value); old.reply(1); expect(next.writes).toEqual([]); await failed;
    const fresh = socket.request({ operation: "build.get", args: {} }); next.reply(1, '{"version":"new"}');
    expect((await fresh).body).toBe('{"version":"new"}'); socket.dispose();
  });
  it("denies hub operations on assigned remote roles before IPC", async () => {
    const socket = new NativeSocket(), ch = channel(1, "remote"); socket.adopt(ch.value);
    await expect(socket.request({ operation: "reverse.open", args: { body: { target: "vm2", port: 1234 } } })).rejects.toThrow("not available");
    expect(ch.writes).toEqual([]); expect(socket.assignment?.target).toBe("vm1"); socket.dispose();
  });
  it("cancels without releasing pending capacity until the service acknowledges", async () => {
    const socket = new NativeSocket(), ch = channel(); socket.adopt(ch.value); const pending: Promise<unknown>[] = [];
    for (let i = 0; i < 64; i++) {
      const controller = new AbortController();
      const result = socket.request({ operation: "build.get", args: {} }, { signal: controller.signal });
      pending.push(expect(result).rejects.toMatchObject({ uncertain: false })); ch.credit(ch.writes.at(-1)!); controller.abort();
    }
    await Promise.all(pending); await expect(socket.request({ operation: "build.get", args: {} })).rejects.toThrow("capacity");
    expect(ch.writes.filter(frame => frame.kind === NativeKind.cancel)).toHaveLength(64); ch.reply(1);
    const recovered = socket.request({ operation: "build.get", args: {} }); ch.reply(65); await recovered; socket.dispose();
  });
  it("reports timed out mutations as uncertain and never retries them", async () => {
    vi.useFakeTimers(); const socket = new NativeSocket(), ch = channel(); socket.adopt(ch.value);
    const result = socket.request({ operation: "template.save", args: { slug: "a", body: {} } });
    const failed = expect(result).rejects.toMatchObject({ uncertain: true }); await vi.advanceTimersByTimeAsync(30_000); await failed;
    expect(ch.writes.map(frame => frame.kind)).toEqual([NativeKind.request, NativeKind.cancel]); socket.dispose();
  });
  it("closes a frozen consumer rather than growing an output queue", () => {
    const socket = new NativeSocket(), ch = channel(); socket.adopt(ch.value);
    for (let i = 0; i < 65; i++) socket.send({ type: "input", id: "s1", data: "a" });
    expect(ch.writes).toHaveLength(64); expect(ch.value.close).toHaveBeenCalledOnce(); expect(socket.assignment).toBeUndefined();
  });
  it("rejects duplicate credit and unsolicited replies", () => {
    const socket = new NativeSocket(), ch = channel(); socket.adopt(ch.value);
    socket.send({ type: "input", id: "s1", data: "a" }); const first = ch.writes[0]!;
    ch.credit(first); ch.credit(first); expect(ch.value.close).toHaveBeenCalledOnce();
    const next = channel(2); socket.adopt(next.value); next.reply(123); expect(next.value.close).toHaveBeenCalledOnce();
  });
});

it("treats malformed mutation replies as a lost outcome before removing the pending request", async () => {
  const socket = new NativeSocket(), ch = channel(); socket.adopt(ch.value);
  const pending = socket.request({ operation: "template.save", args: { slug: "a", body: {} } });
  const failed = expect(pending).rejects.toMatchObject({ uncertain: true });
  ch.data(NativeKind.reply, '{"id":1,"status":42,"contentType":"json","body":"{}"}');
  await failed; expect(socket.assignment).toBeUndefined(); expect(ch.value.close).toHaveBeenCalledOnce();
});
it("closes an adapter that throws while subscribing, even when its cleanup throws too", () => {
  const socket = new NativeSocket(), ch = channel(); const failed = vi.fn(); socket.onError = failed;
  const diagnostic = vi.spyOn(console, "error").mockImplementation(() => {});
  ch.value.subscribe = () => { throw new Error("subscription failed"); };
  ch.value.close = vi.fn(() => { throw new Error("close failed"); }); socket.adopt(ch.value);
  expect(socket.assignment).toBeUndefined(); expect(ch.value.close).toHaveBeenCalledOnce(); expect(failed).toHaveBeenCalledOnce(); diagnostic.mockRestore();
});
it("does not announce connected after a synchronous close during subscription", () => {
  const socket = new NativeSocket(), ch = channel(); const connected = vi.fn(), unsubscribe = vi.fn(); socket.onConnected = connected;
  ch.value.subscribe = receive => {
    receive({ ...ch.value.assignment, frame: { kind: NativeKind.closed, payload: encoder.encode('{"message":"closed"}') } }); return unsubscribe;
  };
  socket.adopt(ch.value);
  expect(connected).not.toHaveBeenCalled(); expect(unsubscribe).toHaveBeenCalledOnce(); expect(ch.value.start).not.toHaveBeenCalled();
});

it("answers native heartbeats on the assigned stream even with full data admission", () => {
  const socket = new NativeSocket(), ch = channel(); const fail = vi.fn(); socket.onError = fail; socket.adopt(ch.value);
  for (let index = 0; index < 64; index++) socket.send({ type: "input", id: "session", data: "x" });
  for (const nonce of [0, 1, 0xffffffff]) ch.frame({ kind: NativeKind.ping, payload: encoder.encode(JSON.stringify({ nonce })) });
  expect(ch.writes.slice(-3).map(frame => ({ kind: frame.kind, body: JSON.parse(new TextDecoder().decode(frame.payload)) }))).toEqual([0, 1, 0xffffffff].map(nonce => ({ kind: NativeKind.pong, body: { nonce } })));
  expect(fail).not.toHaveBeenCalled(); expect(socket.assignment?.document).toBe("document-1"); socket.dispose();
});
it.each([null, [], {}, { nonce: -1 }, { nonce: 0x100000000 }, { nonce: 1.5 }, { nonce: "1" }, { nonce: 1, generation: 2 }])("rejects malformed heartbeat %j without sending a pong", value => {
  const socket = new NativeSocket(), ch = channel(); const fail = vi.fn(); socket.onError = fail; socket.adopt(ch.value);
  ch.frame({ kind: NativeKind.ping, payload: encoder.encode(JSON.stringify(value)) });
  expect(fail).toHaveBeenCalledOnce(); expect(ch.writes).toEqual([]); expect(socket.assignment).toBeUndefined();
});
it("does not answer an old document heartbeat after adopting a replacement", () => {
  const socket = new NativeSocket(), old = channel(); socket.adopt(old.value);
  const current = channel(2); socket.adopt(current.value);
  old.frame({ kind: NativeKind.ping, payload: encoder.encode('{"nonce":1}') });
  expect(current.writes).toEqual([]); expect(old.writes).toEqual([]); expect(socket.assignment?.generation).toBe(2); socket.dispose();
});
