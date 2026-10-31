// The local server's side of remote screens: its known hosts, the targets it
// accepts, and the hub socket (`docs/protocol.ts`, section 10).

import { apiFetch, fetchWsTicket } from "../protocol/http.ts";
import { isPlausibleTarget } from "../host_picker.ts";
import { parseKnownHosts, type KnownHost } from "./known_hosts.ts";
import { RemoteSocket, type HubWebSocket } from "./remote_socket.ts";

/** Longer targets are refused before anything is sent; the hub validates the rest. */
export const MAX_TARGET = 255;

/** The known hosts of the user the local server runs as (`GET /api/remote/hosts`). */
export async function fetchRemoteHosts(fetcher: typeof apiFetch = apiFetch): Promise<KnownHost[]> {
  const res = await fetcher("/api/remote/hosts");
  if (!res.ok) throw new Error(`listing hosts failed: HTTP ${res.status}`);
  return parseKnownHosts(await res.json());
}

/**
 * The target to connect, or null when it cannot be one: empty, starting with a
 * dash, with whitespace or control characters, or too long. Only a first
 * sieve; the hub validates the target as `[user@]host[:port]` before any `ssh`
 * runs (`access-control`).
 */
export function validateTarget(input: string): string | null {
  const target = input.trim();
  if (!isPlausibleTarget(target) || target.length > MAX_TARGET) return null;
  return target;
}

/**
 * The hub's URL for `target` on the server at `location`, with a ticket, and
 * `update=1` (the only value the hub takes) when the user confirmed an update.
 */
export function hubUrl(
  location: { protocol: string; host: string },
  target: string,
  ticket: string,
  update = false,
): string {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  let query = `target=${encodeURIComponent(target)}&ticket=${encodeURIComponent(ticket)}`;
  if (update) query += "&update=1";
  return `${scheme}//${location.host}/ws/remote?${query}`;
}

/** A hub socket to `target` through the server that served this page. */
export function openRemoteSocket(target: string): RemoteSocket {
  return new RemoteSocket(target, {
    fetchTicket: fetchWsTicket,
    open: (to, ticket, { update }) => {
      const ws = new WebSocket(hubUrl(window.location, to, ticket, update));
      return ws as unknown as HubWebSocket;
    },
  });
}
