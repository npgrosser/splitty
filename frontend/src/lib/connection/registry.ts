/**
 * The window's server connections. The local connection, to the server that
 * served the page, always exists; others are added and removed at runtime.
 *
 * A host's connection (`remote-screens`) is opened lazily, when the first
 * screen of the host is created/opened or its directory explicitly expanded,
 * and goes when no screen or explicit browse needs it. Its socket is the
 * hub's (`/ws/remote`), owned here.
 */

import { socket } from "../protocol/websocket.ts";
import type { RemoteSocket } from "../remote/remote_socket.ts";
import { ServerConnection, hostKey, type ConnKey } from "./server_connection.ts";

let local: ServerConnection | undefined;
const others = new Map<ConnKey, ServerConnection>();
const remoteSockets = new Map<ConnKey, RemoteSocket>();
const listeners = new Set<() => void>();

/** The connection to the local server, over the page's own socket. */
export function localConnection(): ServerConnection {
  local ??= new ServerConnection("local", "Local", socket);
  return local;
}

/** All connections, the local one first. */
export function connections(): ServerConnection[] {
  return [localConnection(), ...others.values()];
}

export function connectionFor(key: ConnKey): ServerConnection | undefined {
  return key === "local" ? localConnection() : others.get(key);
}

export function addConnection(conn: ServerConnection): void {
  if (conn.kind === "local" || others.has(conn.key)) {
    throw new Error(`connection ${conn.key} exists already`);
  }
  others.set(conn.key, conn);
  notify();
}

export function removeConnection(key: ConnKey): void {
  const conn = others.get(key);
  if (!conn) return;
  others.delete(key);
  conn.dispose();
  // A hub socket opened here ends with its connection; the host's daemon
  // and its sessions keep running.
  remoteSockets.get(key)?.dispose();
  remoteSockets.delete(key);
  notify();
}

/**
 * The connection to `target` through the hub, opened and connecting if it
 * does not exist. `open` makes the hub socket (`lib/remote/api.ts`).
 */
export function ensureRemote(target: string, open: (target: string) => RemoteSocket): ServerConnection {
  const key = hostKey(target);
  const existing = others.get(key);
  if (existing) return existing;
  const remote = open(target);
  const conn = new ServerConnection(key, target, remote);
  remoteSockets.set(key, remote);
  addConnection(conn);
  remote.connect();
  return conn;
}

/** The hub socket of a host's connection opened by {@link ensureRemote}. */
export function remoteSocketFor(key: ConnKey): RemoteSocket | undefined {
  return remoteSockets.get(key);
}

/** Called when a connection was added or removed. Returns unsubscribe. */
export function subscribeConnections(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/**
 * Run `setup` for every connection, now and as they are added; its cleanup
 * runs when the connection is removed, or for all of them when the returned
 * function is called.
 */
export function forEachConnection(setup: (conn: ServerConnection) => () => void): () => void {
  const cleanups = new Map<ServerConnection, () => void>();
  const sync = () => {
    const current = connections();
    for (const [conn, cleanup] of cleanups) {
      if (!current.includes(conn)) {
        cleanup();
        cleanups.delete(conn);
      }
    }
    for (const conn of current) {
      if (!cleanups.has(conn)) cleanups.set(conn, setup(conn));
    }
  };
  sync();
  const unsubscribe = subscribeConnections(sync);
  return () => {
    unsubscribe();
    for (const cleanup of cleanups.values()) cleanup();
    cleanups.clear();
  };
}

function notify(): void {
  for (const cb of listeners) cb();
}
