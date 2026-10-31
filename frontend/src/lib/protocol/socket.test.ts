import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { decodeOutputFrame } from "./socket.ts";
import { FakeSocket } from "../testing/fake_server.ts";
import type { OutgoingMessage } from "./types.ts";

let socket: FakeSocket;

beforeEach(() => {
  socket = new FakeSocket();
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("SocketHandlers", () => {
  it("delivers a message to the later handlers when one throws", () => {
    const seen: string[] = [];
    socket.onMessage(() => { throw new Error("broken handler"); });
    socket.onMessage((msg) => seen.push(msg.type));

    socket.message({ type: "mux.nudge", reason: "x" });

    expect(seen).toEqual(["mux.nudge"]);
    expect(console.error).toHaveBeenCalled();
  });

  it("drops a frame that is not JSON, or a message without a type, and keeps going", () => {
    const seen: OutgoingMessage[] = [];
    socket.onMessage((msg) => seen.push(msg));

    socket.text("{not json");
    socket.text("null");
    socket.text("[1,2]");
    socket.text(JSON.stringify({ type: 42 }));
    socket.message({ type: "bell", sessionId: "s1" });

    expect(seen.map((m) => m.type)).toEqual(["bell"]);
  });

  it("delivers output frames to the later handlers when one throws", () => {
    const seen: string[] = [];
    socket.onBinaryMessage(() => { throw new Error("broken handler"); });
    socket.onBinaryMessage((id, data) => seen.push(`${id}:${new TextDecoder().decode(data)}`));

    socket.output("sess-1", "hello");

    expect(seen).toEqual(["sess-1:hello"]);
  });

  it("drops an output frame whose id length exceeds the frame", () => {
    const seen: string[] = [];
    socket.onBinaryMessage((id) => seen.push(id));

    socket.binary(new Uint8Array([200, 0x73, 0x31]));
    socket.binary(new Uint8Array([0, 0x41]));
    socket.binary(new Uint8Array([]));
    socket.output("s1", "ok");

    expect(seen).toEqual(["s1"]);
  });

  it("isolates disconnect handlers", () => {
    let later = false;
    socket.onDisconnect(() => { throw new Error("broken handler"); });
    socket.onDisconnect(() => { later = true; });

    socket.drop();

    expect(later).toBe(true);
  });

  it("lets a handler unsubscribe itself while a message is delivered", () => {
    const seen: string[] = [];
    const unsub = socket.onMessage(() => { unsub(); seen.push("first"); });
    socket.onMessage(() => seen.push("second"));

    socket.message({ type: "bell", sessionId: "s1" });
    socket.message({ type: "bell", sessionId: "s1" });

    expect(seen).toEqual(["first", "second", "second"]);
  });
});

describe("decodeOutputFrame", () => {
  it("splits the session id from the payload", () => {
    const frame = new Uint8Array([2, 0x73, 0x31, 0x61, 0x62]);
    const decoded = decodeOutputFrame(frame);
    expect(decoded?.id).toBe("s1");
    expect(Array.from(decoded?.payload ?? [])).toEqual([0x61, 0x62]);
  });
});
