import { describe, expect, it } from "vitest";
import {
  addConnection, connectionFor, connections, ensureRemote, forEachConnection, localConnection, remoteSocketFor,
  removeConnection,
} from "./registry.ts";
import { fakeServer } from "../testing/fake_server.ts";
import { RemoteSocket, type HubWebSocket } from "../remote/remote_socket.ts";
import { attachNotifications, getNotifications } from "../notifications.ts";

/** A hub socket that answers with `remote.ready` at once. */
function readyHub(target: string): HubWebSocket {
  const hub: HubWebSocket & { sent: string[] } = {
    binaryType: "blob", readyState: 1, onopen: null, onmessage: null, onclose: null, onerror: null,
    sent: [],
    send(data: string) { hub.sent.push(data); },
    close() {},
  };
  queueMicrotask(() => hub.onmessage?.({
    data: JSON.stringify({ type: "remote.ready", target, daemon: { version: "0.4.2", protocol: 1, started: true }, screens: [] }),
  }));
  return hub;
}

describe("connection registry", () => {
  it("always has the local connection, first", () => {
    expect(connections()[0]).toBe(localConnection());
    expect(connectionFor("local")).toBe(localConnection());
    expect(localConnection().kind).toBe("local");
  });

  it("sets up every connection, also later ones, and cleans up removed ones", () => {
    const seen: string[] = [];
    const stop = forEachConnection((conn) => {
      seen.push(`+${conn.key}`);
      return () => seen.push(`-${conn.key}`);
    });
    const { socket, conn } = fakeServer("host:vm1");

    addConnection(conn);
    expect(connectionFor("host:vm1")).toBe(conn);
    expect(() => addConnection(conn)).toThrow();
    removeConnection("host:vm1");
    stop();

    expect(seen).toEqual(["+local", "+host:vm1", "-host:vm1", "-local"]);
    expect(connectionFor("host:vm1")).toBeUndefined();
    // Removing disposes the connection: its socket has no listeners left.
    const before = conn.mux.clientId;
    socket.message({ type: "mux.welcome", clientId: "late", screens: [] });
    expect(conn.mux.clientId).toBe(before);
  });

  it("opens a host's connection once, through the hub, and closes its socket with it", async () => {
    const opened: string[] = [];
    const open = (target: string) => {
      opened.push(target);
      return new RemoteSocket(target, { fetchTicket: async () => "t", open: readyHub });
    };
    const conn = ensureRemote("me@vm2", open);
    expect(conn.key).toBe("host:me@vm2");
    expect(conn.kind).toBe("remote");
    expect(ensureRemote("me@vm2", open)).toBe(conn);
    expect(opened).toEqual(["me@vm2"]);

    const socket = remoteSocketFor(conn.key);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(socket?.state.phase).toBe("ready");

    // Its notifications name the host, which it cannot remove.
    const stop = attachNotifications(conn);
    socket?.deliver(JSON.stringify({ id: "n-1", timestamp: 0, type: "notification", severity: "info", title: "build done" }));
    stop();
    expect(getNotifications().find((n) => n.id === "host:me@vm2/n-1")?.title).toBe("me@vm2: build done");

    removeConnection(conn.key);
    expect(socket?.state.phase).toBe("closed");
    expect(remoteSocketFor(conn.key)).toBeUndefined();
    expect(connectionFor(conn.key)).toBeUndefined();
  });
});
