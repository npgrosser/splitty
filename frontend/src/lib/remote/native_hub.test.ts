import { it, expect, vi } from "vitest";
import { NativeHubSocket } from "./native_hub.ts";
import { RemoteSocket } from "./remote_socket.ts";
import type { NativeChannel, NativeDelivery } from "../protocol/native_socket.ts";
import { dataFrame, NativeKind as K } from "../protocol/native_wire.ts";
function fixture() {
  const assignment = { handle: "host", document: "document", generation: 1, instance: "instance", role: "remote" as const, target: "vm1" };
  let receive!: (delivery: NativeDelivery) => void, id = 0;
  const channel: NativeChannel = { assignment, subscribe: callback => { receive = callback; return () => {}; },
    start: vi.fn(), close: vi.fn(), write: vi.fn() };
  const deliver = (kind: K, bytes: Uint8Array) => {
    const receipt = new Uint8Array(16); receipt[0] = ++id;
    receive({ handle: assignment.handle, document: assignment.document, frame: dataFrame(kind, { generation: 1, receipt }, bytes) });
  };
  return { channel, deliver, json: (value: unknown) => deliver(K.text, new TextEncoder().encode(JSON.stringify(value))) };
}
const ready = { type: "remote.ready", target: "vm1", daemon: { version: "1", protocol: 1, keepsSessions: true, started: false }, screens: [] };
const tick = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
it("uses native host prelude and binary output without browser authentication", async () => {
  const f = fixture(), ticket = vi.fn(), browser = vi.fn(), message = vi.fn(), output = vi.fn();
  const remote = new RemoteSocket("vm1", { fetchTicket: ticket, open: browser,
    openNative: (target, { update }) => new NativeHubSocket(target, update, "document", async () => f.channel) });
  remote.onMessage(message); remote.onBinaryMessage(output); remote.connect(); await tick();
  f.json({ type: "remote.progress", step: "bridge", message: "ready" }); f.json(ready);
  f.json({ type: "mux.welcome", clientId: "host-client", screens: [] });
  f.deliver(K.binary, new Uint8Array([2, 115, 49, 0, 255, 128]));
  expect(remote.state.phase).toBe("ready"); expect(message).toHaveBeenCalledWith({ type: "mux.welcome", clientId: "host-client", screens: [] });
  expect(output).toHaveBeenCalledWith("s1", new Uint8Array([0, 255, 128]));
  expect(ticket).not.toHaveBeenCalled(); expect(browser).not.toHaveBeenCalled();
  remote.dispose(); expect(f.channel.close).toHaveBeenCalledOnce();
});
it("retains typed host incompatibility across the native error message", async () => {
  const remote = new RemoteSocket("vm1", { openNative: (target, { update }) => new NativeHubSocket(target, update, "document", async () => {
    throw Error(JSON.stringify({ type: "remote.error", code: "incompatible", message: "Update needed", version: "2", protocol: 7, keepsSessions: true }));
  }) });
  remote.connect(); await tick();
  expect(remote.state).toMatchObject({ phase: "failed", error: { code: "incompatible", protocol: 7, keepsSessions: true } }); remote.dispose();
});
it("closes a late host assignment after cancellation without starting it", async () => {
  const f = fixture(); let resolve!: (channel: NativeChannel) => void;
  const socket = new NativeHubSocket("vm1", false, "document", () => new Promise(r => { resolve = r; }));
  socket.close(); resolve(f.channel); await tick();
  expect(f.channel.close).toHaveBeenCalledOnce(); expect(f.channel.start).not.toHaveBeenCalled();
});
it("rejects a native assignment for another host or document", async () => {
  const f = fixture(); const closed = vi.fn();
  const socket = new NativeHubSocket("vm2", false, "document", async () => f.channel);
  socket.onclose = closed; await tick(); expect(closed).toHaveBeenCalledOnce(); expect(f.channel.start).not.toHaveBeenCalled();
});

it("aborts an unfinished SSH preparation when its host view closes", async () => {
  let signal: AbortSignal | undefined;
  const socket = new NativeHubSocket("vm1", false, "document", (_target, _update, preparation) => {
    signal = preparation;
    return new Promise((_resolve, reject) => preparation!.addEventListener("abort", () => reject(Error("cancelled")), { once: true }));
  });
  socket.close(); expect(signal?.aborted).toBe(true); await tick();
});
