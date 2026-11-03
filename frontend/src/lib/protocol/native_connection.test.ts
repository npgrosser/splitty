import { afterEach, describe, it, expect, vi } from "vitest";
import { NativeConnection } from "./native_connection.ts";
import { NativeConnectionError, type NativeChannel, type NativeDelivery } from "./native_socket.ts";
import { NativeKind } from "./native_wire.ts";
import { ServerConnection } from "../connection/server_connection.ts";
import { commandAllowed } from "../connection/routing.ts";

afterEach(() => vi.useRealTimers());

const assigned = { handle: "handle", document: "doc", instance: "instance", generation: 1, role: "remote" as const, target: "vm1" };
function channel(assignment = assigned): NativeChannel {
  return { assignment, subscribe: vi.fn(() => () => {}), start: vi.fn(), write: vi.fn(), close: vi.fn() };
}

describe("native document connection", () => {
  it("shares one pending open and closes a late assignment after disposal", async () => {
    let resolve!: (channel: NativeChannel) => void;
    const open = vi.fn(() => new Promise<NativeChannel>(r => { resolve = r; }));
    const socket = new NativeConnection(open, assigned);
    const pending = socket.connect();
    expect(socket.connect()).toBe(pending);
    socket.reconnect(); expect(open).toHaveBeenCalledOnce();
    socket.dispose(); const late = channel(); resolve(late); await pending;
    expect(late.close).toHaveBeenCalledOnce(); expect(late.start).not.toHaveBeenCalled();
  });
  it("does not replay disconnected terminal input or prompt for browser credentials", async () => {
    const ch = channel(); const open = vi.fn().mockRejectedValueOnce(Error("SSH denied")).mockResolvedValue(ch);
    const socket = new NativeConnection(open, assigned);
    socket.onAuthError = vi.fn(); socket.onConnectionError = vi.fn();
    socket.send({ type: "input", id: "session", data: "never replay" });
    await socket.connect(); expect(socket.onAuthError).not.toHaveBeenCalled(); expect(socket.onConnectionError).toHaveBeenCalledOnce();
    await socket.connect(); expect(ch.write).not.toHaveBeenCalled();
    socket.send({ type: "input", id: "session", data: "current" }); expect(ch.write).toHaveBeenCalledOnce(); socket.dispose();
  });
  it.each(["target", "document", "role"])("rejects a changed %s without starting it", async key => {
    const ch = channel({ ...assigned, [key]: key === "role" ? "main" : "other" });
    const socket = new NativeConnection(async () => ch, assigned);
    socket.onConnectionError = vi.fn(); await socket.connect();
    expect(ch.close).toHaveBeenCalledOnce(); expect(ch.start).not.toHaveBeenCalled();
    expect(socket.onConnectionError).toHaveBeenCalledOnce();
  });
  it("applies remote command restrictions to a remote window's root connection", () => {
    const socket = new NativeConnection(async () => channel(), assigned);
    const conn = new ServerConnection("local", assigned.target, socket, "remote");
    expect(conn.kind).toBe("remote");
    expect(commandAllowed(conn.kind, { type: "pane.paste" }, { allowClipboardWrite: true })).toBe(false);
    conn.dispose(); socket.dispose();
  });
});

it("reconnects after a transient native loss without replaying input or requiring a token", async () => {
  vi.useFakeTimers();
  const first = channel(), next = channel({ ...assigned, instance: "replacement", generation: 2 });
  let receive!: (delivery: NativeDelivery) => void;
  first.subscribe = callback => { receive = callback; return () => {}; };
  const open = vi.fn().mockResolvedValueOnce(first).mockResolvedValue(next);
  const socket = new NativeConnection(open, assigned); socket.onAuthError = vi.fn();
  await socket.connect();
  receive({ handle: assigned.handle, document: assigned.document, frame: { kind: NativeKind.closed,
    payload: new TextEncoder().encode(JSON.stringify({ code: "backendLost", retryable: true, message: "connection lost" })) } });
  socket.send({ type: "input", id: "session", data: "never replay" });
  await vi.advanceTimersByTimeAsync(1000);
  expect(open).toHaveBeenCalledTimes(2); expect(socket.assignment?.instance).toBe("replacement");
  expect(next.write).not.toHaveBeenCalled(); expect(socket.onAuthError).not.toHaveBeenCalled(); socket.dispose();
});
it("stops native recovery on a permanent failure and bounds repeated transient failures", async () => {
  vi.useFakeTimers();
  for (const permanent of [true, false]) {
    const first = channel(); let receive!: (delivery: NativeDelivery) => void;
    first.subscribe = callback => { receive = callback; return () => {}; };
    const open = vi.fn().mockResolvedValueOnce(first).mockRejectedValue(new NativeConnectionError("backend unavailable", "backendLost", !permanent));
    const socket = new NativeConnection(open, assigned); await socket.connect();
    receive({ handle: assigned.handle, document: assigned.document, frame: { kind: NativeKind.closed,
      payload: new TextEncoder().encode(JSON.stringify({ code: "backendLost", retryable: true, message: "lost" })) } });
    await vi.advanceTimersByTimeAsync(620_000);
    const count = open.mock.calls.length;
    expect(count).toBeLessThan(70); expect(count).toBeGreaterThanOrEqual(2);
    if (permanent) expect(count).toBe(2);
    expect(socket.connectionState).toBe("disconnected");
    await vi.advanceTimersByTimeAsync(620_000); expect(open).toHaveBeenCalledTimes(count); socket.dispose();
  }
});

it("keeps remote recovery active after two minutes and stops precisely at ten", async () => {
  vi.useFakeTimers();
  const open = vi.fn().mockRejectedValue(new NativeConnectionError("SSH unavailable", "backendLost", true));
  const socket = new NativeConnection(open, assigned); socket.onStateChange = vi.fn();
  await socket.connect();
  await vi.advanceTimersByTimeAsync(120_000);
  expect(socket.connectionState).toBe("reconnecting");
  const afterTwoMinutes = open.mock.calls.length;
  await vi.advanceTimersByTimeAsync(479_999);
  expect(socket.connectionState).toBe("reconnecting"); expect(open.mock.calls.length).toBeGreaterThan(afterTwoMinutes);
  await vi.advanceTimersByTimeAsync(1);
  expect(socket.connectionState).toBe("disconnected");
  expect(socket.onStateChange).toHaveBeenNthCalledWith(1, "reconnecting");
  expect(socket.onStateChange).toHaveBeenLastCalledWith("disconnected");
  const attempts = open.mock.calls.length;
  await vi.advanceTimersByTimeAsync(60_000); expect(open).toHaveBeenCalledTimes(attempts); socket.dispose();
});

it("cancels in-flight preparation at the deadline, rejects its late result and allows a deliberate retry", async () => {
  vi.useFakeTimers();
  let finish!: (channel: NativeChannel) => void, signal: AbortSignal | undefined;
  const open = vi.fn().mockRejectedValueOnce(new NativeConnectionError("lost", "backendLost", true))
    .mockImplementationOnce((value?: AbortSignal) => { signal = value; return new Promise<NativeChannel>(resolve => { finish = resolve; }); })
    .mockResolvedValue(channel({ ...assigned, instance: "replacement", generation: 2 }));
  const socket = new NativeConnection(open, assigned); socket.onConnectionError = vi.fn();
  await socket.connect(); await vi.advanceTimersByTimeAsync(1000);
  expect(signal?.aborted).toBe(false);
  await vi.advanceTimersByTimeAsync(599_000);
  expect(signal?.aborted).toBe(true); expect(socket.connectionState).toBe("disconnected");
  expect(socket.onConnectionError).toHaveBeenLastCalledWith(expect.objectContaining({ code: "reconnectTimeout" }));
  socket.reconnect(); expect(open).toHaveBeenCalledTimes(2); // Cleanup has not confirmed completion yet.
  const late = channel(); finish(late); await vi.advanceTimersByTimeAsync(0);
  expect(late.close).toHaveBeenCalledOnce(); expect(late.start).not.toHaveBeenCalled();
  socket.send({ type: "input", id: "session", data: "must not replay" });
  socket.reconnect(); await vi.advanceTimersByTimeAsync(0);
  expect(open).toHaveBeenCalledTimes(3); expect(socket.connectionState).toBe("connected");
  expect(socket.assignment?.instance).toBe("replacement"); socket.dispose();
});

it("retains the shorter local recovery deadline", async () => {
  vi.useFakeTimers();
  const open = vi.fn().mockRejectedValue(new NativeConnectionError("busy", "busy", true));
  const socket = new NativeConnection(open, { document: "local-doc", role: "main" });
  await socket.connect(); await vi.advanceTimersByTimeAsync(120_000);
  expect(socket.connectionState).toBe("disconnected"); socket.dispose();
});

it("rejects a result past the deadline even before the overdue timer can run", async () => {
  vi.useFakeTimers(); vi.setSystemTime(0);
  let finish!: (channel: NativeChannel) => void;
  const open = vi.fn().mockRejectedValueOnce(new NativeConnectionError("lost", "backendLost", true))
    .mockImplementationOnce(() => new Promise<NativeChannel>(resolve => { finish = resolve; }));
  const socket = new NativeConnection(open, assigned);
  await socket.connect(); await vi.advanceTimersByTimeAsync(1000);
  vi.setSystemTime(600_001); // Renderer resumes; queued promise precedes overdue timers.
  const late = channel(); finish(late);
  await Promise.resolve(); await Promise.resolve();
  expect(late.close).toHaveBeenCalledOnce(); expect(late.start).not.toHaveBeenCalled();
  expect(socket.assignment).toBeUndefined(); expect(socket.connectionState).toBe("disconnected"); socket.dispose();
});
