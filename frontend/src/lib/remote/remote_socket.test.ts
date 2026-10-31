import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  GIVE_UP_AFTER, MAX_RECONNECT_DELAY, REMOTE_HOST_LOST, RemoteSocket, retryable,
  type HubWebSocket, type RemoteReadyInfo,
} from "./remote_socket.ts";
import type { OutgoingMessage } from "../protocol/types.ts";

/** A hub socket the test plays the local server's side of. */
class FakeHub implements HubWebSocket {
  binaryType: BinaryType = "blob";
  readyState = 1;
  onopen: HubWebSocket["onopen"] = null;
  onmessage: HubWebSocket["onmessage"] = null;
  onclose: HubWebSocket["onclose"] = null;
  onerror: HubWebSocket["onerror"] = null;
  sent: unknown[] = [];
  closedWith: number | undefined;

  constructor(readonly target: string, readonly ticket: string, readonly update = false) {}

  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }

  close(code?: number): void {
    this.closedWith = code ?? 1005;
    this.readyState = 3;
  }

  /** The hub sends a JSON message. */
  json(msg: Record<string, unknown>): void {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }

  binary(bytes: number[]): void {
    this.onmessage?.({ data: new Uint8Array(bytes).buffer });
  }

  /** The socket closes from the hub's side. */
  end(code: number, reason = ""): void {
    this.readyState = 3;
    this.onclose?.({ code, reason });
  }

  ready(screens: unknown[] = []): void {
    this.json({
      type: "remote.ready",
      target: this.target,
      daemon: { version: "0.4.2", protocol: 1, started: false },
      screens,
    });
  }
}

interface Rig {
  socket: RemoteSocket;
  hubs: FakeHub[];
  tickets: number;
  /** The next ticket requests fail. */
  failTickets: number;
  readonly hub: FakeHub;
  messages: OutgoingMessage[];
  disconnects: number;
}

function rig(): Rig {
  const r = {
    hubs: [] as FakeHub[],
    tickets: 0,
    failTickets: 0,
    messages: [] as OutgoingMessage[],
    disconnects: 0,
    get hub() { return this.hubs[this.hubs.length - 1]; },
  } as Rig;
  r.socket = new RemoteSocket("vm1", {
    fetchTicket: async () => {
      r.tickets++;
      if (r.failTickets > 0) {
        r.failTickets--;
        return null;
      }
      return `ticket-${r.tickets}`;
    },
    open: (target, ticket, { update }) => {
      const hub = new FakeHub(target, ticket, update);
      r.hubs.push(hub);
      return hub;
    },
    random: () => 0,
  });
  r.socket.onMessage((msg) => r.messages.push(msg));
  r.socket.onDisconnect(() => r.disconnects++);
  return r;
}

/** Let the ticket request settle. */
const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("RemoteSocket before remote.ready", () => {
  it("collects the hub's progress and opens the hub with a fresh ticket", async () => {
    const r = rig();
    r.socket.connect();
    await settle();

    expect(r.hub.target).toBe("vm1");
    expect(r.hub.ticket).toBe("ticket-1");
    r.hub.json({ type: "remote.progress", step: "install", message: "Installing Splitty 0.4.2 on vm1 (x86_64)..." });
    r.hub.json({ type: "remote.progress", step: "daemon", message: "Asking vm1 for its Splitty daemon..." });

    expect(r.socket.state.phase).toBe("connecting");
    expect(r.socket.state.progress.map((p) => p.step)).toEqual(["install", "daemon"]);
    expect(r.messages).toEqual([]);
  });

  it("keeps whether the host's daemon keeps its sessions", async () => {
    const r = rig();
    r.socket.connect();
    await settle();
    r.hub.json({
      type: "remote.ready", target: "vm1",
      daemon: { version: "0.1.18", protocol: 1, keepsSessions: true, started: false }, screens: [],
    });
    expect(r.socket.state.ready?.daemon).toEqual({ version: "0.1.18", protocol: 1, keepsSessions: true, started: false });

    // An older hub does not say: its daemon keeps none.
    const old = rig();
    old.socket.connect();
    await settle();
    old.hub.ready();
    expect(old.socket.state.ready?.daemon.keepsSessions).toBe(false);

    const incompatible = rig();
    incompatible.socket.connect();
    await settle();
    incompatible.hub.json({ type: "remote.error", code: "incompatible", message: "", version: "0.9.0", protocol: 3, keepsSessions: true });
    expect(incompatible.socket.state.error?.keepsSessions).toBe(true);
  });

  it("holds messages until ready, then sends them in order", async () => {
    const r = rig();
    r.socket.connect();
    r.socket.send({ type: "mux.resync", screenId: "s1" });
    await settle();
    r.socket.send({ type: "mux.resync", screenId: "s2" });
    expect(r.hub.sent).toEqual([]);

    const ready: RemoteReadyInfo[] = [];
    r.socket.onReady((info) => ready.push(info));
    r.hub.ready([{ id: "s9", name: "api", panes: 3 }]);

    expect(r.hub.sent).toEqual([{ type: "mux.resync", screenId: "s1" }, { type: "mux.resync", screenId: "s2" }]);
    expect(r.socket.state.phase).toBe("ready");
    expect(ready[0]).toEqual({
      target: "vm1",
      daemon: { version: "0.4.2", protocol: 1, keepsSessions: false, started: false },
      screens: [{ id: "s9", name: "api", panes: 3 }],
    });
    r.socket.send({ type: "mux.resync", screenId: "s3" });
    expect(r.hub.sent).toHaveLength(3);
  });

  it("never holds typed input or an image for a paste", async () => {
    const r = rig();
    r.socket.connect();
    await settle();
    r.socket.send({ type: "input", id: "sess-1", data: "rm -rf ~\r" });
    r.socket.send({ type: "save_image", requestId: "img-1", data: "iVBORw0KGgo=" });
    r.socket.send({ type: "editor.register" });
    r.socket.send({ type: "editor.result", requestId: "req-1", outcome: "dispatched" });
    r.socket.send({ type: "resize", id: "sess-1", cols: 80, rows: 24 });
    r.hub.ready();
    expect(r.hub.sent).toEqual([{ type: "resize", id: "sess-1", cols: 80, rows: 24 }]);
  });

  it.each([
    ["a host message", (hub: FakeHub) => hub.json({ type: "mux.welcome", clientId: "c", screens: [] })],
    ["a binary frame", (hub: FakeHub) => hub.binary([1, 0x73, 0x41])],
    ["a frame that is not JSON", (hub: FakeHub) => hub.onmessage?.({ data: "{" })],
    ["a ready without a daemon", (hub: FakeHub) => hub.json({ type: "remote.ready", target: "vm1", screens: [] })],
  ])("ends the attempt on %s", async (_name, send) => {
    const r = rig();
    r.socket.connect();
    await settle();
    send(r.hub);

    expect(r.hub.closedWith).toBe(1000);
    expect(r.socket.state.phase).toBe("failed");
    expect(r.socket.state.error?.code).toBe("protocol");
    expect(r.messages).toEqual([]);
  });

  it("shows a first connect's error and does not retry on its own", async () => {
    const r = rig();
    r.socket.connect();
    await settle();
    r.hub.json({ type: "remote.progress", step: "daemon", message: "Asking vm1..." });
    r.hub.json({
      type: "remote.error",
      code: "unreachable",
      message: "Failed to reach vm1: ssh to vm1 failed: Connection refused",
    });
    r.hub.end(1000, "unreachable");

    expect(r.socket.state.phase).toBe("failed");
    expect(r.socket.state.error).toEqual({
      code: "unreachable",
      message: "Failed to reach vm1: ssh to vm1 failed: Connection refused",
    });
    expect(r.socket.state.progress).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(r.tickets).toBe(1);

    r.socket.retry();
    await settle();
    expect(r.tickets).toBe(2);
    expect(r.socket.state.phase).toBe("connecting");
  });

  it("keeps an incompatible host's versions and drops unknown codes to internal", async () => {
    const r = rig();
    r.socket.connect();
    await settle();
    r.hub.json({
      type: "remote.error", code: "incompatible", version: "9.0.0", protocol: 7,
      message: "vm1 runs Splitty 9.0.0 (protocol 7); this Splitty needs protocol 1.",
      hint: "Update this Splitty to use vm1. Its daemon keeps running.",
      screens: [{ id: "s1", name: "api", panes: 3 }, { id: "s2", panes: 1 }],
    });
    expect(r.socket.state.error).toMatchObject({ code: "incompatible", version: "9.0.0", protocol: 7 });
    expect(r.socket.state.error?.keepsSessions).toBeUndefined();
    // What an update would end, for the warning.
    expect(r.socket.state.error?.screens).toEqual([
      { id: "s1", name: "api", panes: 3 },
      { id: "s2", name: "s2", panes: 1 },
    ]);

    const s = rig();
    s.socket.connect();
    await settle();
    s.hub.json({ type: "remote.error", code: "pwned", message: "x" });
    expect(s.socket.state.error?.code).toBe("internal");
  });

  it("asks the hub for an update only in the retry the user confirmed", async () => {
    const r = rig();
    r.socket.connect();
    await settle();
    expect(r.hub.update).toBe(false);
    r.hub.json({ type: "remote.error", code: "incompatible", message: "", version: "0.3.9", protocol: 0 });
    r.hub.end(1000, "incompatible");

    r.socket.retry({ update: true });
    await settle();
    expect(r.hub.update).toBe(true);
    r.hub.json({ type: "remote.progress", step: "daemon", message: "Asking vm1..." });
    r.hub.json({ type: "remote.progress", step: "update", message: "Stopping Splitty 0.3.9 on vm1..." });
    expect(r.socket.state.phase).toBe("connecting");
    expect(r.socket.state.progress.map((p) => p.step)).toEqual(["daemon", "update"]);
    r.hub.json({ type: "remote.error", code: "daemon", message: "Splitty 0.3.9 on vm1 is still running." });
    r.hub.end(1000, "daemon");

    // A plain retry after it does not update again.
    r.socket.retry();
    await settle();
    expect(r.hubs.map((h) => h.update)).toEqual([false, true, false]);
  });

  it("updates a connected host of another version with a new attempt, which only it carries", async () => {
    const r = rig();
    r.socket.connect();
    await settle();
    r.hub.ready();
    const dropped = vi.fn();
    r.socket.onDisconnect(dropped);
    // A plain retry leaves a connected socket alone.
    r.socket.retry();
    expect(r.hubs).toHaveLength(1);

    r.socket.retry({ update: true });
    expect(r.hubs[0].closedWith).toBe(1000);
    expect(dropped).toHaveBeenCalledOnce();
    expect(r.socket.state.phase).toBe("reconnecting");
    await settle();
    expect(r.hubs.map((h) => h.update)).toEqual([false, true]);
    r.hub.ready();
    expect(r.socket.state.phase).toBe("ready");
  });

  it("does not carry an update into the automatic reconnect attempts", async () => {
    const r = rig();
    r.socket.connect();
    await settle();
    r.hub.ready();
    r.hub.end(REMOTE_HOST_LOST);
    await vi.advanceTimersByTimeAsync(1_000);
    r.hub.json({ type: "remote.error", code: "incompatible", message: "", version: "0.5.0", protocol: 9 });
    r.hub.end(1000, "incompatible");
    expect(r.socket.state.phase).toBe("failed");

    r.socket.retry({ update: true });
    await settle();
    expect(r.socket.state.phase).toBe("reconnecting");
    r.hub.end(1006);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(r.hubs.map((h) => h.update)).toEqual([false, false, true, false]);
  });
});

describe("RemoteSocket after remote.ready", () => {
  async function ready(): Promise<Rig> {
    const r = rig();
    r.socket.connect();
    await settle();
    r.hub.ready();
    return r;
  }

  it("relays the host's messages and output, and drops any remote.* the host sends", async () => {
    const r = await ready();
    const output: string[] = [];
    r.socket.onBinaryMessage((id, data) => output.push(`${id}:${new TextDecoder().decode(data)}`));

    r.hub.json({ type: "mux.welcome", clientId: "c1", screens: [] });
    r.hub.json({ type: "remote.error", code: "auth", message: "fake" });
    r.hub.json({ type: "remote.progress", step: "install", message: "fake" });
    r.hub.json({ type: "remote.ready", target: "evil", daemon: { version: "1", protocol: 1 }, screens: [] });
    r.hub.binary([2, 0x73, 0x31, 0x68, 0x69]);

    expect(r.messages.map((m) => m.type)).toEqual(["mux.welcome"]);
    expect(output).toEqual(["s1:hi"]);
    expect(r.socket.state.phase).toBe("ready");
    expect(r.socket.state.error).toBeUndefined();
    expect(r.socket.state.ready?.target).toBe("vm1");
  });

  it("reconnects when the host is lost, with a fresh ticket per attempt", async () => {
    const r = await ready();
    r.hub.end(REMOTE_HOST_LOST);

    expect(r.socket.state.phase).toBe("reconnecting");
    expect(r.disconnects).toBe(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(r.hubs).toHaveLength(2);
    expect(r.hub.ticket).toBe("ticket-2");

    // The host is still gone: the next attempt fails and another follows.
    r.hub.json({ type: "remote.error", code: "unreachable", message: "no route" });
    expect(r.socket.state.phase).toBe("reconnecting");
    expect(r.socket.state.error?.code).toBe("unreachable");
    await vi.advanceTimersByTimeAsync(1500);
    expect(r.hubs).toHaveLength(3);
    expect(r.hub.ticket).toBe("ticket-3");

    r.hub.ready();
    expect(r.socket.state.phase).toBe("ready");
    expect(r.socket.state.error).toBeUndefined();
    expect(r.socket.state.attempt).toBe(0);
  });

  it("drops what waited for the lost connection, and holds what is sent while reconnecting", async () => {
    const r = await ready();
    const first = r.hub;
    first.readyState = 3;
    r.socket.send({ type: "mux.resync", screenId: "lost" });
    first.end(1006);
    r.socket.send({ type: "mux.resync", screenId: "later" });

    await vi.advanceTimersByTimeAsync(1000);
    r.hub.ready();
    expect(r.hub.sent).toEqual([{ type: "mux.resync", screenId: "later" }]);
  });

  it("waits longer after every failed attempt, at most 10 seconds", async () => {
    const r = await ready();
    r.hub.end(REMOTE_HOST_LOST);
    const delays: number[] = [];
    let last = Date.now();
    for (let i = 0; i < 12; i++) {
      const before = r.hubs.length;
      while (r.hubs.length === before) await vi.advanceTimersByTimeAsync(100);
      delays.push(Date.now() - last);
      r.hub.json({ type: "remote.error", code: "bridge", message: "ended" });
      last = Date.now();
    }
    for (let i = 1; i < delays.length; i++) expect(delays[i]).toBeGreaterThanOrEqual(delays[i - 1]);
    expect(Math.max(...delays)).toBeLessThanOrEqual(MAX_RECONNECT_DELAY + 100);
    expect(delays[delays.length - 1]).toBeGreaterThanOrEqual(MAX_RECONNECT_DELAY);
  });

  it("retries failed ticket requests without ever raising the local login", async () => {
    const { socket: local } = await import("../protocol/websocket.ts");
    const authError = vi.fn();
    local.onAuthError = authError;

    const r = await ready();
    r.failTickets = 3;
    r.hub.end(REMOTE_HOST_LOST);
    await vi.advanceTimersByTimeAsync(20_000);

    expect(r.tickets).toBeGreaterThanOrEqual(5);
    expect(r.hubs).toHaveLength(2);
    expect(r.socket.state.phase).toBe("reconnecting");
    expect(authError).not.toHaveBeenCalled();
    local.onAuthError = null;
  });

  it("stops retrying on an error retrying cannot fix", async () => {
    const r = await ready();
    r.hub.end(REMOTE_HOST_LOST);
    await vi.advanceTimersByTimeAsync(1000);
    r.hub.json({ type: "remote.error", code: "host_key", message: "Host key verification failed.", hint: "Run `ssh vm1` once." });

    expect(r.socket.state.phase).toBe("failed");
    expect(r.socket.state.error?.code).toBe("host_key");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(r.hubs).toHaveLength(2);
  });

  it("is offline once the host stayed unreachable, and retries on request", async () => {
    const r = await ready();
    r.hub.end(REMOTE_HOST_LOST);
    while (r.socket.state.phase === "reconnecting") {
      await vi.advanceTimersByTimeAsync(MAX_RECONNECT_DELAY);
      if (r.hub.onmessage) r.hub.json({ type: "remote.error", code: "unreachable", message: "no route" });
    }
    expect(r.socket.state.phase).toBe("offline");
    expect(Date.now()).toBeGreaterThanOrEqual(GIVE_UP_AFTER);
    const attempts = r.hubs.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(r.hubs).toHaveLength(attempts);

    r.socket.retry();
    await settle();
    expect(r.socket.state.phase).toBe("reconnecting");
    r.hub.ready();
    expect(r.socket.state.phase).toBe("ready");
  });

  it("closes for good: no reconnect, nothing sent", async () => {
    const r = await ready();
    const hub = r.hub;
    r.socket.close();
    r.socket.send({ type: "mux.resync", screenId: "s1" });
    await vi.advanceTimersByTimeAsync(60_000);

    expect(hub.closedWith).toBe(1000);
    expect(hub.sent).toEqual([]);
    expect(r.hubs).toHaveLength(1);
    expect(r.socket.state.phase).toBe("closed");
  });

  it("drops a ticket that arrives after the socket was closed", async () => {
    const r = rig();
    r.socket.connect();
    r.socket.close();
    await settle();
    expect(r.hubs).toHaveLength(0);
  });
});

describe("retryable", () => {
  it("is true for transient failures and the client's own", () => {
    expect(["unreachable", "daemon", "bridge", "internal", "ticket", "closed", "protocol"].every((c) => retryable(c as never))).toBe(true);
    expect(["invalid_target", "host_key", "auth", "unsupported_host", "install", "incompatible"].some((c) => retryable(c as never))).toBe(false);
  });
});
