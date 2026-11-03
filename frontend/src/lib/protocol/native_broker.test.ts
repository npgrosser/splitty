import { EventEmitter } from "node:events";
import { afterEach, expect, it, vi } from "vitest";
import { NativeBroker, type NativeWindowGrant } from "../../../../frontend-electron/src/native_broker.ts";
import { NativePort, type NativeMessagePort } from "../../../../frontend-electron/src/native_port.ts";
import { NativeProcessBudget } from "../../../../frontend-electron/src/native_budget.ts";
import type { NativePipe, NativeReady, PipeEvents } from "../../../../frontend-electron/src/native_pipe.ts";
import type { NativeAssignment } from "../../../../frontend-electron/src/native_types.ts";
import { NativeKind as K, dataFrame, encodeNativeFrame, type NativeFrame } from "./native_wire.ts";

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
class Port extends EventEmitter {
  messages: any[] = [];
  ended = false;
  postMessage(value: unknown) { this.messages.push(structuredClone(value)); }
  start() {}
  close() { if (!this.ended) { this.ended = true; this.emit("close"); } }
  input(value: unknown) { this.emit("message", { data: value }); }
}
class Contents extends EventEmitter {
  mainFrame = { parent: null, url: "splitty://app/index.html", postMessage: vi.fn(), send: vi.fn() };
  isDestroyed() { return false; }
  isLoadingMainFrame() { return false; }
  navigate(url = this.mainFrame.url) {
    this.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false });
    this.mainFrame.url = url;
    this.emit("did-navigate");
  }
}
const ready: NativeReady = { generation: 1, instance: "a".repeat(32), muxProtocol: 1,
  purpose: { kind: "direct" }, limits: { frameBytes: 16 * 1024 * 1024, apiBytes: 1024 * 1024, pendingRequests: 64, connectionBytes: 32 * 1024 * 1024 } };
const assignment: NativeAssignment = { handle: "native-handle", document: "native-document", generation: 1, instance: ready.instance, role: "main" };
const disposers: (() => void)[] = [];
afterEach(() => { disposers.splice(0).forEach(dispose => dispose()); vi.useRealTimers(); });
function pipe(events: PipeEvents = { closed() {} }) {
  const process = new NativeProcessBudget(), readiness = deferred<NativeReady>(), released = deferred<void>();
  let receive!: (frame: NativeFrame) => void, closed = false;
  const value = {
    ready: readiness.promise, budget: process.connection(), send: vi.fn(), retain: () => () => {},
    activate: vi.fn((callback: (frame: NativeFrame) => void) => { receive = callback; }),
    close: vi.fn(() => { if (!closed) { closed = true; readiness.reject(Error("closed")); events.closed({ code: "closed", retryable: false, message: "closed" }); } }),
  };
  void value.ready.catch(() => {});
  disposers.push(() => { value.close(); released.resolve(); });
  return { value, native: value as unknown as NativePipe, readiness, released, process, receive: (frame: NativeFrame) => receive(frame) };
}
function broker(navigationAllowed?: ConstructorParameters<typeof NativeBroker>[1]) {
  const channels: { port1: Port; port2: Port }[] = [], pipes: ReturnType<typeof pipe>[] = [];
  const broker = new NativeBroker(() => {
    const channel = { port1: new Port(), port2: new Port() }; channels.push(channel);
    return channel as unknown as ReturnType<ConstructorParameters<typeof NativeBroker>[0]>;
  }, navigationAllowed);
  const open = vi.fn((events: PipeEvents) => {
    const connection = pipe(events); pipes.push(connection);
    return { pipe: connection.native, released: connection.released.promise };
  });
  const documents = new WeakMap<object, string>();
  const realOpen = broker.open.bind(broker);
  broker.open = (event, input) => realOpen(event, { document: documents.get(event.sender), ...(input as object) });
  function authorize(contents: Contents) {
    broker.document(event(contents));
    const message = contents.mainFrame.send.mock.calls.filter(call => call[0] === "native:document").at(-1);
    if (message) documents.set(contents, message[1].document);
    return documents.get(contents);
  }
  function grant(contents: Contents, policy: Partial<NativeWindowGrant> = {}) {
    broker.grant(contents as unknown as Parameters<NativeBroker["grant"]>[0], { role: "main", open, ...policy });
    authorize(contents);
  }
  const event = (contents: Contents, frame: unknown = contents.mainFrame) => ({ sender: contents, senderFrame: frame }) as unknown as Parameters<NativeBroker["open"]>[0];
  disposers.push(() => pipes.forEach(p => { p.value.close(); p.released.resolve(); }));
  return { broker, channels, pipes, open, grant, event, authorize };
}
async function tick() { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); }

it("accepts only a registered main frame on the exact packaged entry document", async () => {
  const f = broker(), main = new Contents(); f.grant(main);
  for (const url of ["http://127.0.0.1:9800/?desktop=1", "splitty://app/setup.html", "splitty://other/index.html", "splitty://app:80/index.html"]) {
    main.mainFrame.url = url; f.broker.open(f.event(main), { request: "denied" });
  }
  main.mainFrame.url = "splitty://app/index.html?host=forged&desktop=1";
  f.broker.open(f.event(new Contents()), { request: "popout" });
  f.broker.open(f.event(main, { ...main.mainFrame, parent: main.mainFrame }), { request: "child" });
  f.broker.open(f.event(main, { ...main.mainFrame }), { request: "copied-frame" });
  f.broker.open(f.event(main), { request: "forged-role", role: "main" });
  expect(f.open).not.toHaveBeenCalled();
  f.broker.open(f.event(main), { request: "own-document" });
  f.pipes[0]!.readiness.resolve(ready); await tick();
  const [, assigned] = main.mainFrame.postMessage.mock.calls[0]!;
  expect(assigned.assignment.role).toBe("main"); expect(assigned.assignment.target).toBeUndefined();
  expect(f.open).toHaveBeenCalledTimes(1);
});

it("fixes remote authority natively and refuses target changes or owner carriers", async () => {
  const f = broker(), remote = new Contents(); f.grant(remote, { role: "remote", target: "vm" });
  f.broker.open(f.event(remote), { request: "other", target: "local" });
  expect(f.open).not.toHaveBeenCalled();
  f.broker.open(f.event(remote), { request: "assigned" });
  f.pipes[0]!.readiness.resolve({ ...ready, purpose: { kind: "host", target: "vm" } }); await tick();
  expect(remote.mainFrame.postMessage.mock.calls[0]![1].assignment).toMatchObject({ role: "remote", target: "vm" });
  const main = new Contents(); f.grant(main); f.broker.open(f.event(main), { request: "owner" });
  f.pipes[1]!.readiness.resolve({ ...ready, purpose: { kind: "owner" } }); await tick();
  expect(main.mainFrame.postMessage).not.toHaveBeenCalled(); expect(f.pipes[1]!.value.close).toHaveBeenCalled();
});

it("does not deliver a late readiness result after reload, even when Electron reuses the frame object", async () => {
  const f = broker(), main = new Contents(); f.grant(main);
  f.broker.open(f.event(main), { request: "old" }); main.navigate();
  f.pipes[0]!.readiness.resolve(ready); await tick();
  expect(main.mainFrame.postMessage).not.toHaveBeenCalled();
  expect(f.broker.counts).toEqual({ connections: 1, opening: 1 });
  f.pipes[0]!.released.resolve(); await tick(); expect(f.broker.counts).toEqual({ connections: 0, opening: 0 });
});

it("retains pending-open quotas across cancellation/reload until actual cleanup", async () => {
  const f = broker(), main = new Contents(); f.grant(main);
  for (let i = 0; i < 4; i++) f.broker.open(f.event(main), { request: `r${i}` });
  main.navigate(); f.authorize(main); f.broker.open(f.event(main), { request: "new-document" });
  expect(f.open).toHaveBeenCalledTimes(4); expect(f.broker.counts.opening).toBe(4);
  f.pipes[0]!.released.resolve(); await tick();
  f.broker.open(f.event(main), { request: "now-admitted" }); expect(f.open).toHaveBeenCalledTimes(5);
  f.pipes[1]!.released.reject(Error("exit was not confirmed")); await tick();
  expect(f.broker.counts.opening).toBe(4);
});

it("blocks the old document during navigation and rejects its queued challenge after frame reuse", async () => {
  const f = broker(), main = new Contents(); f.grant(main);
  const old = f.authorize(main);
  main.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false });
  f.broker.open(f.event(main), { request: "during-navigation", document: old });
  f.broker.document(f.event(main));
  expect(f.open).not.toHaveBeenCalled();
  main.emit("did-navigate");
  f.broker.open(f.event(main), { request: "queued-old", document: old });
  expect(f.open).not.toHaveBeenCalled();
  const next = f.authorize(main); expect(next).not.toBe(old);
  f.broker.open(f.event(main), { request: "new", document: next });
  expect(f.open).toHaveBeenCalledTimes(1);
});

it("retains the live document when the native navigation guard rejects a destination", async () => {
  const f = broker((_contents, url) => url === "splitty://app/index.html"), main = new Contents(); f.grant(main);
  const current = f.authorize(main);
  f.broker.open(f.event(main), { request: "live" });
  f.pipes[0]!.readiness.resolve(ready); await tick();
  main.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false, url: "splitty://app/setup.html" });
  main.emit("did-stop-loading");
  expect(f.authorize(main)).toBe(current);
  expect(f.broker.authorized(f.event(main), current)).toBe(true);
  expect(f.channels[0]!.port1.ended).toBe(false);
  main.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false, url: "splitty://app/index.html" });
  expect(f.broker.authorized(f.event(main), current)).toBe(false);
  expect(f.channels[0]!.port1.ended).toBe(true);
});

it("publishes a fresh challenge after an admitted navigation aborts without reviving old handles", async () => {
  const f = broker(), main = new Contents(); f.grant(main);
  const old = f.authorize(main);
  f.broker.open(f.event(main), { request: "live" });
  f.pipes[0]!.readiness.resolve(ready); await tick();
  main.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false });
  main.isLoadingMainFrame = () => true;
  main.emit("did-stop-loading");
  expect(f.broker.authorized(f.event(main), old)).toBe(false);
  main.isLoadingMainFrame = () => false;
  main.emit("did-stop-loading");
  const fresh = f.authorize(main); expect(fresh).not.toBe(old);
  expect(f.broker.authorized(f.event(main), old)).toBe(false);
  expect(f.broker.authorized(f.event(main), fresh)).toBe(true);
  expect(f.channels[0]!.port1.ended).toBe(true);
  f.broker.open(f.event(main), { request: "stale", document: old });
  expect(f.open).toHaveBeenCalledTimes(1);
});

it("binds presentation status and native retry to the actual current remote document", () => {
  const f = broker(), main = new Contents(), remote = new Contents(), other = new Contents();
  f.grant(main); f.grant(remote, { role: "remote", target: "192.0.2.1" });
  f.grant(other, { role: "remote", target: "192.0.2.2" });
  const document = f.authorize(remote), otherDocument = f.authorize(other);
  const input = { document, state: "disconnected" };
  expect(f.broker.remoteState(f.event(remote), input)).toBe("disconnected");
  expect(f.broker.remoteState(f.event(main), { document: f.authorize(main), state: "connected" })).toBeUndefined();
  expect(f.broker.remoteState(f.event(other), input)).toBeUndefined();
  expect(f.broker.remoteState(f.event(remote, { ...remote.mainFrame, parent: remote.mainFrame }), input)).toBeUndefined();
  expect(f.broker.remoteState(f.event(remote), { ...input, target: "192.0.2.2" })).toBeUndefined();
  expect(f.broker.remoteState(f.event(remote), { ...input, state: "arbitrary title" })).toBeUndefined();
  expect(f.broker.retryRemote(f.event(main).sender)).toBe(false);
  expect(f.broker.retryRemote(f.event(remote).sender)).toBe(true);
  expect(remote.mainFrame.send).toHaveBeenLastCalledWith("native:retry", { document });
  expect(other.mainFrame.send).toHaveBeenLastCalledWith("native:document", { document: otherDocument });
  remote.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false });
  expect(f.broker.retryRemote(f.event(remote).sender)).toBe(false);
  remote.emit("did-navigate");
  expect(f.broker.remoteState(f.event(remote), input)).toBeUndefined();
});

it("denies copied connection handles and revokes active ports on main-document navigation", async () => {
  const f = broker(), a = new Contents(), b = new Contents(); f.grant(a); f.grant(b);
  f.broker.open(f.event(a), { request: "a" }); f.broker.open(f.event(b), { request: "b" });
  f.pipes.forEach(p => p.readiness.resolve(ready)); await tick();
  const assignedA = a.mainFrame.postMessage.mock.calls[0]![1].assignment;
  const assignedB = b.mainFrame.postMessage.mock.calls[0]![1].assignment;
  f.channels[0]!.port1.input({ type: "start", handle: assignedA.handle, document: assignedA.document });
  f.channels[1]!.port1.input({ type: "start", handle: assignedA.handle, document: assignedB.document });
  expect(f.channels[1]!.port1.ended).toBe(true); expect(f.channels[0]!.port1.ended).toBe(false);
  a.emit("did-start-navigation", { isMainFrame: false, isSameDocument: false });
  expect(f.channels[0]!.port1.ended).toBe(false);
  a.navigate(); expect(f.channels[0]!.port1.ended).toBe(true);
  expect(f.channels[0]!.port1.messages.at(-1)).toMatchObject({ type: "closed", document: assignedA.document, handle: assignedA.handle });
});

function portFixture() {
  const p = pipe(), port = new Port(); let current = true;
  const bridge = new NativePort(port as unknown as NativeMessagePort, p.native, assignment, () => current);
  const input = (value: Record<string, unknown>) => port.input({ ...value, handle: assignment.handle, document: assignment.document });
  input({ type: "start" }); disposers.push(() => bridge.close());
  return { p, port, bridge, input, stale: () => { current = false; } };
}
it("assembles input under byte limits and releases its own scratch reservation on close", () => {
  const f = portFixture();
  const frame = dataFrame(K.request, { generation: 1, receipt: new Uint8Array(16) }, new TextEncoder().encode('{"id":1,"operation":"mux.status","args":{}}'));
  const bytes = encodeNativeFrame(frame), token = "a".repeat(32);
  f.input({ type: "chunk", token, bytes: bytes.subarray(0, 6) });
  expect(f.p.process.counts.data).toBe(65536 + bytes.length);
  f.input({ type: "chunk", token, bytes: bytes.subarray(6) });
  expect(f.p.value.send).toHaveBeenCalledWith(frame); expect(f.p.process.counts.data).toBe(65536);
  f.bridge.close(); expect(f.p.process.counts.data).toBe(0);
});

it("permits only one output chunk in flight and requires its unpredictable receipt", () => {
  const f = portFixture();
  f.p.receive(dataFrame(K.binary, { generation: 1, receipt: new Uint8Array(16) }, new Uint8Array(200_000)));
  let chunks = f.port.messages.filter(message => message.type === "chunk");
  expect(chunks).toHaveLength(1); expect(chunks[0].bytes.length).toBe(65536);
  const first = chunks[0].token;
  f.input({ type: "consumed", token: first });
  chunks = f.port.messages.filter(message => message.type === "chunk");
  expect(chunks).toHaveLength(2); expect(chunks[1].token).not.toBe(first);
  f.input({ type: "consumed", token: first });
  expect(f.port.ended).toBe(true); expect(f.p.process.counts.data).toBe(0);
});

it("keeps partial-frame deadlines fixed and closes stalled delivery independently", () => {
  vi.useFakeTimers(); const f = portFixture(), other = portFixture();
  f.input({ type: "chunk", token: "a".repeat(32), bytes: new Uint8Array([K.text, 0, 0, 1, 0]) });
  vi.advanceTimersByTime(30_000);
  f.input({ type: "chunk", token: "b".repeat(32), bytes: new Uint8Array([0]) });
  vi.advanceTimersByTime(30_000);
  expect(f.port.ended).toBe(true); expect(other.port.ended).toBe(false);
  other.p.receive(dataFrame(K.binary, { generation: 1, receipt: new Uint8Array(16) }, new Uint8Array([255])));
  vi.advanceTimersByTime(30_000); expect(other.port.ended).toBe(true);
});

it("retains reserved input space for a credit when the data window is full", () => {
  const f = portFixture();
  const release = f.p.native.budget.reserve(32 * 1024 * 1024 - 128 * 1024);
  const frame = { kind: K.credit, payload: new TextEncoder().encode(JSON.stringify({ generation: 1, receipt: "a".repeat(32) })) };
  f.input({ type: "chunk", token: "b".repeat(32), bytes: encodeNativeFrame(frame) });
  expect(f.p.value.send).toHaveBeenCalledWith(frame); expect(f.port.ended).toBe(false);
  release(); f.bridge.close(); expect(f.p.process.counts).toEqual({ data: 0, controls: 0, workers: 0 });
});

it("binds every native shell command to the current main-document challenge", () => {
  const f = broker(), main = new Contents(), remote = new Contents();
  f.grant(main); f.grant(remote, { role: "remote", target: "vm1" });
  const document = f.authorize(main), remoteDocument = f.authorize(remote);
  expect(f.broker.authorized(f.event(main), document)).toBe(true);
  expect(f.broker.authorized(f.event(remote), document)).toBe(false);
  expect(f.broker.authorized(f.event(remote), remoteDocument)).toBe(false);
  expect(f.broker.authorized(f.event(new Contents()), document)).toBe(false);
  expect(f.broker.authorized(f.event(main, { ...main.mainFrame, parent: main.mainFrame }), document)).toBe(false);
  main.emit("did-start-navigation", { isMainFrame: true, isSameDocument: false });
  expect(f.broker.authorized(f.event(main), document)).toBe(false);
  main.emit("did-navigate");
  expect(f.broker.authorized(f.event(main), document)).toBe(false);
  expect(f.broker.authorized(f.event(main), f.authorize(main))).toBe(true);
});
