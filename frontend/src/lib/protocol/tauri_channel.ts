import { NativeConnectionError, type NativeAssignment, type NativeChannel, type NativeDelivery } from "./native_socket.ts";
import { encodeNativeFrame, NativeFrameDecoder, NativeKind, type NativeFrame } from "./native_wire.ts";

const CHUNK_BYTES = 64 * 1024;
const QUEUED_BYTES = 32 * 1024 * 1024;
const QUEUED_FRAMES = 64;

/** The main-document capsule owns authentication and Tauri Channels. Neither
 * proofs nor raw command invocation are exposed through this interface. */
export interface TauriNativeBridge {
  state(state: "connected" | "reconnecting" | "disconnected"): void;
  open(options: { target?: string; update?: boolean } | undefined, request: number): Promise<NativeAssignment>;
  cancelOpen(request: number): void;
  listen(handle: string, data: (bytes: Uint8Array, token: string) => void, closed: (reason: unknown) => void): () => void;
  start(handle: string): Promise<void>;
  write(handle: string, bytes: Uint8Array): Promise<void>;
  consumed(handle: string, token: string): Promise<void>;
  close(handle: string): void;
}
let nextRequest = 0;

export async function openTauriChannel(bridge: TauriNativeBridge,
  options?: { target?: string; update?: boolean }, signal?: AbortSignal): Promise<NativeChannel> {
  signal?.throwIfAborted();
  if (nextRequest === 0xffffffff) throw Error("Native request IDs exhausted; reload this window");
  const request = ++nextRequest;
  const cancel = () => bridge.cancelOpen(request);
  signal?.addEventListener("abort", cancel, { once: true });
  let assignment: Readonly<NativeAssignment>;
  try {
    assignment = Object.freeze({ ...await bridge.open(options, request) });
    if (signal?.aborted) { bridge.close(assignment.handle); signal.throwIfAborted(); }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.startsWith("SplittyNativeFailure:")) {
      const failure = JSON.parse(message.slice("SplittyNativeFailure:".length));
      throw new NativeConnectionError(failure.message, failure.code, failure.retryable === true);
    }
    throw error;
  } finally { signal?.removeEventListener("abort", cancel); }

  const decoder = new NativeFrameDecoder();
  let receive: ((delivery: NativeDelivery) => void) | undefined;
  let off: (() => void) | undefined;
  let ended = false, started = false, writable = false, draining = false;
  let queuedBytes = 0;
  const outgoing: Uint8Array[] = [];
  const deliver = (frame: NativeFrame) => receive?.({ handle: assignment.handle, document: assignment.document, frame });
  const close = () => {
    if (ended) return;
    ended = true; receive = undefined;
    outgoing.length = 0; queuedBytes = 0;
    try { decoder.end(); } catch { /* Revocation discards incomplete frames. */ }
    try { off?.(); } finally { off = undefined; bridge.close(assignment.handle); }
  };
  const failed = (error: unknown) => {
    if (ended) return;
    const failure = error && typeof error === "object" && "code" in error && "retryable" in error ? error : {
      code: "transportLost", retryable: true, message: error instanceof Error ? error.message : String(error),
    };
    try { deliver({ kind: NativeKind.closed, payload: new TextEncoder().encode(JSON.stringify(failure)) }); } finally { close(); }
  };
  const drain = async () => {
    if (draining || !writable || ended) return;
    draining = true;
    try {
      while (outgoing.length && !ended) {
        const bytes = outgoing[0];
        for (let offset = 0; offset < bytes.length && !ended; offset += CHUNK_BYTES) {
          await bridge.write(assignment.handle, bytes.subarray(offset, Math.min(offset + CHUNK_BYTES, bytes.length)));
        }
        if (!ended) { outgoing.shift(); queuedBytes -= bytes.length; }
      }
    } catch (error) { failed(error); }
    finally { draining = false; }
  };
  return {
    assignment,
    subscribe(callback) {
      if (ended || receive) throw Error("Native document subscription is unavailable");
      receive = callback;
      off = bridge.listen(assignment.handle, (bytes, token) => {
        if (ended) return;
        if (!bytes.length || bytes.length > CHUNK_BYTES) { failed(Error("Invalid native IPC chunk")); return; }
        try {
          decoder.feed(bytes, (frame, release) => { try { deliver(frame); } finally { release(); } });
          // Acknowledgement follows the synchronous copy into the bounded
          // decoder. Native frame credit separately follows frame consumption.
          void bridge.consumed(assignment.handle, token).catch(failed);
        } catch (error) { failed(error); }
      }, failed);
      return () => { receive = undefined; off?.(); off = undefined; };
    },
    start() {
      if (ended || started) throw Error("Native document cannot start");
      started = true;
      void bridge.start(assignment.handle).then(() => { writable = true; return drain(); }).catch(failed);
    },
    write(frame) {
      if (ended) throw Error("Native document connection is closed");
      const bytes = encodeNativeFrame(frame);
      if (outgoing.length >= QUEUED_FRAMES || queuedBytes + bytes.length > QUEUED_BYTES) {
        close(); throw new NativeConnectionError("Native IPC queue is full", "backpressure", true);
      }
      outgoing.push(bytes); queuedBytes += bytes.length; void drain();
    },
    close,
  };
}
