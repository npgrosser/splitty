import { NativeConnectionError, type NativeAssignment, type NativeChannel, type NativeDelivery } from "./native_socket.ts";
import { encodeNativeFrame, NativeFrameDecoder, NativeKind, type NativeFrame } from "./native_wire.ts";

/** The actual port stays in the isolated preload. This surface only addresses
 * connections the native process has assigned to this document. */
export interface ElectronNativeBridge {
  state(state: "connected" | "reconnecting" | "disconnected"): void;
  onRetry(callback: () => void): () => void;
  open(options?: { target?: string; update?: boolean }, request?: string): Promise<NativeAssignment>;
  cancelOpen(request: string): void;
  listen(handle: string, incoming: (chunk: Uint8Array) => void, closed: (reason: unknown) => void): () => void;
  start(handle: string): void;
  write(handle: string, bytes: Uint8Array): void;
  close(handle: string): void;
}

export async function openElectronChannel(bridge: ElectronNativeBridge, options?: { target?: string; update?: boolean }, signal?: AbortSignal): Promise<NativeChannel> {
  signal?.throwIfAborted();
  const request = crypto.randomUUID();
  const opening = bridge.open(options, request);
  const cancel = () => bridge.cancelOpen(request);
  signal?.addEventListener("abort", cancel, { once: true });
  if (signal?.aborted) cancel();
  let assignment: Readonly<NativeAssignment>;
  try {
    assignment = Object.freeze({ ...await opening });
    if (signal?.aborted) { bridge.close(assignment.handle); signal.throwIfAborted(); }
  } catch (error) {
    if (error instanceof Error && error.message.startsWith("SplittyNativeFailure:")) {
      const value = JSON.parse(error.message.slice("SplittyNativeFailure:".length));
      throw new NativeConnectionError(value.message, value.code, value.retryable === true);
    }
    throw error;
  } finally { signal?.removeEventListener("abort", cancel); }
  // The private preload reserves the announced reassembly bytes before it
  // forwards the first chunk to this document, including concurrent streams.
  const decoder = new NativeFrameDecoder();
  let receive: ((delivery: NativeDelivery) => void) | undefined, off: (() => void) | undefined, ended = false;
  const deliver = (frame: NativeFrame) => receive?.({ handle: assignment.handle, document: assignment.document, frame });
  return {
    assignment,
    subscribe(callback) {
      if (ended || receive) throw Error("Native document subscription is unavailable");
      receive = callback;
      off = bridge.listen(assignment.handle, chunk => {
        if (ended) return;
        decoder.feed(chunk, (frame, release) => { try { deliver(frame); } finally { release(); } });
      }, reason => {
        if (ended) return;
        deliver({ kind: NativeKind.closed, payload: new TextEncoder().encode(JSON.stringify(reason)) });
      });
      return () => { receive = undefined; off?.(); off = undefined; };
    },
    start() { if (ended) throw Error("Native document connection is closed"); bridge.start(assignment.handle); },
    write(frame) { if (ended) throw Error("Native document connection is closed"); bridge.write(assignment.handle, encodeNativeFrame(frame)); },
    close() {
      if (ended) return;
      ended = true; receive = undefined;
      try { decoder.end(); } catch { /* Partial input is revoked with its document. */ }
      try { off?.(); } finally { off = undefined; bridge.close(assignment.handle); }
    },
  };
}
