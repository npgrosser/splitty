import type { IncomingMessage, OutgoingMessage } from "./types.ts";
import { fetchWsTicket } from "./http.ts";
import { belongsToConnection, SocketHandlers, type ConnSocket } from "./socket.ts";

let debugMode: boolean | undefined;

function isDebug(): boolean {
  debugMode ??= typeof window !== "undefined"
    && new URLSearchParams(window.location.search).has("debug");
  return debugMode;
}

function logDebug(direction: "ws->client" | "client->ws" | "client->ws:queue", payload: unknown) {
  if (!isDebug()) return;
  console.debug(`[splitty debug] ${direction}`, payload);
}

/**
 * The socket to the local server, the one that served this page.
 *
 * Two rules here hold for the local server only: a socket that fails before it
 * opened is an authentication error (the session is gone), and messages sent
 * while it is down are queued and sent once it is open again.
 */
export class SplittyWebSocket extends SocketHandlers implements ConnSocket {
  private ws: WebSocket | null = null;
  private queue: IncomingMessage[] = [];
  private reconnectTimer: number | undefined;
  private reconnectAttempt = 0;
  private static readonly MAX_RECONNECT_DELAY = 10_000;
  private connecting = false;

  onConnected: (() => void) | null = null;
  onAuthError: (() => void) | null = null;
  onDisconnected: (() => void) | null = null;

  /** `url` is read on every connect, not when the socket is created. */
  constructor(private readonly url: () => string) {
    super();
  }

  /**
   * Open the socket.
   *
   * Every connect fetches a fresh single-use ticket (security review
   * SR-04). The socket cannot authenticate with the session cookie
   * because WebSocket handshakes bypass CORS, so a cookie would let any origin
   * that can reach the port open one.
   *
   * Because a ticket is spent on use, reconnects must fetch their own — reusing
   * the previous one always fails.
   */
  async connect(): Promise<void> {
    if (this.ws || this.connecting) return;
    this.connecting = true;

    const ticket = await fetchWsTicket();
    this.connecting = false;

    if (!ticket) {
      // No session yet, or it is no longer valid. Surfacing this as an auth
      // error rather than retrying avoids a reconnect loop against a server
      // that will keep refusing.
      this.onAuthError?.();
      return;
    }

    if (this.ws) return;

    let wasOpen = false;
    const baseUrl = this.url();
    const separator = baseUrl.includes("?") ? "&" : "?";
    const ws = new WebSocket(`${baseUrl}${separator}ticket=${encodeURIComponent(ticket)}`);
    ws.binaryType = "arraybuffer";

    ws.onopen = () => {
      wasOpen = true;
      this.reconnectAttempt = 0;
      for (const msg of this.queue) {
        logDebug("client->ws", msg);
        ws.send(JSON.stringify(msg));
      }
      this.queue = [];
      this.onConnected?.();
    };

    // Binary frames are terminal output, text frames JSON control messages;
    // malformed frames and failing handlers stay inside `deliver`.
    ws.onmessage = (ev) => this.deliver(ev.data);

    ws.onerror = () => {
      // Suppress console noise — onclose handles reconnect
    };

    ws.onclose = () => {
      if (this.ws !== ws) return; // Already disposed
      this.ws = null;

      if (!wasOpen) {
        // Connection was never established — likely auth failure (HTTP 401
        // before upgrade) or server not reachable. Invoke auth error callback
        // instead of auto-reconnecting to avoid an infinite loop.
        this.onAuthError?.();
        return;
      }

      this.onDisconnected?.();
      this.disconnected();

      const delay = Math.min(
        1000 * Math.pow(1.5, this.reconnectAttempt) + Math.random() * 500,
        SplittyWebSocket.MAX_RECONNECT_DELAY,
      );
      this.reconnectAttempt++;
      this.reconnectTimer = window.setTimeout(() => { void this.connect(); }, delay);
    };

    this.ws = ws;
  }

  reconnect(): void {
    clearTimeout(this.reconnectTimer);
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    // onclose ignores a socket that is no longer current, so report the lost
    // connection here.
    if (ws) this.disconnected();
    void this.connect();
  }

  send(msg: IncomingMessage): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      logDebug("client->ws", msg);
      this.ws.send(JSON.stringify(msg));
    } else if (!belongsToConnection(msg)) {
      // Images, uploads, file offers and editor messages belong to this connection, never a reconnect.
      logDebug("client->ws:queue", msg);
      this.queue.push(msg);
    }
  }

  protected override trace(msg: OutgoingMessage): void {
    console.log(`[ws] ${msg.type}`, msg);
  }

  dispose(): void {
    clearTimeout(this.reconnectTimer);
    const ws = this.ws;
    this.ws = null;
    ws?.close();
    this.clearHandlers();
    this.queue = [];
  }
}

// The local server's socket. Always same-origin.
//
// The `host` and `port` query parameters are gone (security review
// SR-04): they let a crafted app URL redirect the socket to an
// arbitrary server or another local port, and the client would then hand that
// destination its credential. A credential's audience must not be settable by
// whoever wrote the URL.
function getWsUrl(): string {
  if (import.meta.env.VITE_WS_URL) return import.meta.env.VITE_WS_URL;
  const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${window.location.host}/ws`;
}

export const socket = new SplittyWebSocket(getWsUrl);
