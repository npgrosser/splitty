import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { Terminal } from "@xterm/xterm";
import {
  AttachRegistry,
  AttachTransferredError,
  deleteIfCurrent,
  type AttachSocket,
} from "./attach_registry.ts";
import type { IncomingMessage, OutgoingMessage } from "../protocol/types.ts";

class FakeSocket implements AttachSocket {
  sent: IncomingMessage[] = [];
  private handlers = new Set<(msg: OutgoingMessage) => void>();
  private binaryHandlers = new Set<(id: string, data: Uint8Array) => void>();

  send(msg: IncomingMessage): void {
    this.sent.push(msg);
  }
  onMessage(handler: (msg: OutgoingMessage) => void): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }
  onBinaryMessage(handler: (id: string, data: Uint8Array) => void): () => void {
    this.binaryHandlers.add(handler);
    return () => this.binaryHandlers.delete(handler);
  }

  frame(sessionId: string, text: string): void {
    const data = new TextEncoder().encode(text);
    for (const h of this.binaryHandlers) h(sessionId, data);
  }
  attached(sessionId: string): void {
    this.emit({ type: "attached", id: "e-1", timestamp: 0, sessionId, homeDir: "/home/u" });
  }
  error(sessionId: string, error: string): void {
    this.emit({ type: "error", id: "e-2", timestamp: 0, sessionId, error });
  }
  sentOfType(type: IncomingMessage["type"]): IncomingMessage[] {
    return this.sent.filter((m) => m.type === type);
  }
  handlerCount(): number {
    return this.handlers.size + this.binaryHandlers.size;
  }
  private emit(msg: OutgoingMessage): void {
    for (const h of this.handlers) h(msg);
  }
}

/** Collects what a view's emulator would be written. */
function recorder() {
  const chunks: string[] = [];
  const decoder = new TextDecoder();
  return { chunks, sink: (data: Uint8Array) => chunks.push(decoder.decode(data)) };
}

let socket: FakeSocket;
let registry: AttachRegistry;

beforeEach(() => {
  socket = new FakeSocket();
  registry = new AttachRegistry(socket);
});

describe("AttachRegistry", () => {
  it("does not let asynchronous emulator parsing answer historical reports", async () => {
    const terminal = new Terminal();
    const replies: string[] = [];
    terminal.onData((data) => replies.push(data));
    try {
      const pending = registry.attach("s1", 80, 24);
      socket.frame("s1", "\x1b[6n");
      socket.attached("s1");
      (await pending).wire((data) => terminal.write(data));
      await new Promise<void>((done) => terminal.write("", done));
      expect(replies).toEqual([]);
      socket.frame("s1", "\x1b[6n");
      await new Promise<void>((done) => terminal.write("", done));
      expect(replies).toEqual(["\x1b[1;1R"]);
    } finally {
      terminal.dispose();
    }
  });

  it("does not replay clipboard writes through xterm's addon, but allows live copies", async () => {
    const terminal = new Terminal();
    const copied = vi.fn();
    vi.stubGlobal("self", globalThis);
    const { ClipboardAddon, Base64 } = await import("@xterm/addon-clipboard");
    vi.unstubAllGlobals();
    terminal.loadAddon(new ClipboardAddon(new Base64(), { readText: () => "", writeText: copied }));
    try {
      const pending = registry.attach("s1", 80, 24);
      socket.frame("s1", "\x1b]52;c;");
      socket.frame("s1", "b2xk\x07");
      socket.attached("s1");
      (await pending).wire((data) => terminal.write(data));
      await new Promise<void>((done) => terminal.write("", done));
      expect(copied).not.toHaveBeenCalled();
      socket.frame("s1", "\x1b]52;c;bmV3\x07");
      await new Promise<void>((done) => terminal.write("", done));
      expect(copied).toHaveBeenCalledExactlyOnceWith("c", "new");
    } finally {
      terminal.dispose();
    }
  });

  it("removes replayed queries but preserves live queries arriving before wiring", async () => {
    const pending = registry.attach("s1", 80, 24);
    socket.frame("s1", "old\x1b]11;");
    socket.frame("s1", "?\x07prompt$ ");
    socket.attached("s1");
    socket.frame("s1", "\x1b]11;?\x07");
    const view = recorder();
    (await pending).wire(view.sink);
    socket.frame("s1", "\x1b[6n");
    expect(view.chunks.join("")).toBe("oldprompt$ \x1b]11;?\x07\x1b[6n");
  });

  // terminal-sessions, Buffered output shown on attach
  it("shows buffered output that arrives before `attached`, before later output", async () => {
    const pending = registry.attach("s1", 80, 24);
    expect(socket.sentOfType("attach")).toEqual([{ type: "attach", id: "s1", cols: 80, rows: 24 }]);

    socket.frame("s1", "prompt$ ");
    socket.attached("s1");
    const attached = await pending;
    expect(attached.homeDir).toBe("/home/u");

    const view = recorder();
    attached.wire(view.sink);
    socket.frame("s1", "live");

    expect(view.chunks).toEqual(["prompt$ ", "live"]);
  });

  it("keeps buffering between `attached` and the view being wired", async () => {
    const pending = registry.attach("s1", 80, 24);
    socket.frame("s1", "a");
    socket.attached("s1");
    socket.frame("s1", "b"); // before the owner's continuation runs
    const attached = await pending;

    const view = recorder();
    attached.wire(view.sink);
    socket.frame("s1", "c");

    expect(view.chunks).toEqual(["a", "b", "c"]);
  });

  // terminal-sessions, View replaced while its attach is in flight
  it("lets a newer view take over an attach in flight: one attach, no detach", async () => {
    const first = registry.attach("s1", 80, 24);
    const second = registry.attach("s1", 100, 30);

    await expect(first).rejects.toBeInstanceOf(AttachTransferredError);
    expect(socket.sentOfType("attach")).toHaveLength(1);

    socket.frame("s1", "replay");
    socket.attached("s1");
    const view = recorder();
    (await second).wire(view.sink);
    socket.frame("s1", "live");

    expect(view.chunks).toEqual(["replay", "live"]);
    expect(socket.sentOfType("detach")).toEqual([]);
  });

  it("takes over a completed attach whose owner has not wired a view yet", async () => {
    const first = registry.attach("s1", 80, 24);
    socket.frame("s1", "replay");
    socket.attached("s1");
    const stale = await first;

    const newer = await registry.attach("s1", 80, 24);
    expect(() => stale.wire(recorder().sink)).toThrow(AttachTransferredError);

    const view = recorder();
    newer.wire(view.sink);
    expect(view.chunks).toEqual(["replay"]);
    expect(socket.sentOfType("attach")).toHaveLength(1);
  });

  // terminal-sessions, Last view closed
  it("sends detach when the last view of a session lets go", async () => {
    const pending = registry.attach("s1", 80, 24);
    socket.attached("s1");
    const handle = (await pending).wire(recorder().sink);

    registry.detach(handle);

    expect(socket.sentOfType("detach")).toEqual([{ type: "detach", id: "s1" }]);
  });

  it("does not detach while another view holds the session, and stops routing to the stale view", () => {
    const stale = recorder();
    const current = recorder();
    const staleHandle = registry.addView("s1", stale.sink);
    registry.addView("s1", current.sink);

    registry.detach(staleHandle);
    socket.frame("s1", "after");

    expect(socket.sentOfType("detach")).toEqual([]);
    expect(stale.chunks).toEqual([]);
    expect(current.chunks).toEqual(["after"]);
  });

  it("does not detach while an attach for the session is in flight", () => {
    const old = registry.addView("s1", recorder().sink);
    void registry.attach("s1", 80, 24);

    registry.detach(old);

    expect(socket.sentOfType("detach")).toEqual([]);
  });

  it("sends nothing when a view goes away with its closed session", () => {
    const handle = registry.addView("s1", recorder().sink);
    registry.drop(handle);
    expect(socket.sent).toEqual([]);
  });

  it("routes output only to views of the frame's session", () => {
    const a = recorder();
    const b = recorder();
    registry.addView("a", a.sink);
    registry.addView("b", b.sink);

    socket.frame("a", "for a");

    expect(a.chunks).toEqual(["for a"]);
    expect(b.chunks).toEqual([]);
  });

  it("rejects the attach on an error for the session", async () => {
    const pending = registry.attach("s1", 80, 24);
    socket.error("s1", "session not found");
    await expect(pending).rejects.toThrow("session not found");
  });

  describe("timeout", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it("rejects an attach the server never confirms", async () => {
      const timed = new AttachRegistry(socket, 5000);
      const pending = timed.attach("s1", 80, 24);
      const assertion = expect(pending).rejects.toThrow("Timeout waiting for session attach");
      vi.advanceTimersByTime(5000);
      await assertion;
    });
  });
});

describe("AttachRegistry.dispose", () => {
  it("stops routing, fails attaches in flight and sends nothing", async () => {
    const view = recorder();
    registry.addView("s1", view.sink);
    const pending = registry.attach("s2", 80, 24);
    const sentBefore = socket.sent.length;

    registry.dispose();

    await expect(pending).rejects.toThrow("connection closed");
    socket.frame("s1", "late");
    socket.attached("s2");
    expect(view.chunks).toEqual([]);
    expect(socket.sent.length).toBe(sentBefore);
  });

  it("unsubscribes from its socket", () => {
    registry.dispose();
    expect(socket.handlerCount()).toBe(0);
  });
});

describe("deleteIfCurrent", () => {
  it("keeps an entry that a newer value replaced", () => {
    const stale = { name: "stale" };
    const current = { name: "current" };
    const map = new Map([["s1", current]]);

    deleteIfCurrent(map, "s1", stale);
    expect(map.get("s1")).toBe(current);

    deleteIfCurrent(map, "s1", current);
    expect(map.has("s1")).toBe(false);
  });
});
