import { contextBridge, ipcRenderer, type IpcRendererEvent } from "electron";
import type { NativeAssignment } from "./native_types";

// Sandboxed preloads cannot require package-local runtime modules. Keep this
// small port gate here; the application and main process share the wire codec.
type NativeCallback = (chunk: Uint8Array) => unknown;
interface NativeQueued { bytes: Uint8Array; offset: number; release(): void }
interface NativePreloadPort {
  assignment: NativeAssignment;
  port: MessagePort;
  started: boolean;
  closed: boolean;
  data: number;
  controls: number;
  queue: NativeQueued[];
  controlQueue: NativeQueued[];
  writing?: NativeQueued;
  pending?: string;
  receiving?: { total: number; received: number; release(): void };
  incoming?: NativeCallback;
  ended?: (reason: unknown) => void;
  scratch(): void;
}
const nativePorts = new Map<string, NativePreloadPort>();
type NativeProgress = { phase: string; message: string };
const nativeOpens = new Map<string, { progress?: (value: NativeProgress) => void; document?: string; resolve(value: NativeAssignment): void; reject(error: Error): void; timer: ReturnType<typeof setTimeout> }>();
let nativeData = 0, nativeControls = 0;
const chunkBytes = 64 * 1024;
let documentChallenge: Promise<string> | undefined;
let documentToken: string | undefined;
let acceptDocument: ((value: string) => void) | undefined;
let retryCallback: (() => void) | undefined;
ipcRenderer.on("native:retry", (_event: IpcRendererEvent, value: { document?: string }) => {
  if (value?.document !== documentToken) return;
  try { retryCallback?.(); } catch { /* Renderer callbacks cannot change native authority. */ }
});
ipcRenderer.on("native:document", (_event: IpcRendererEvent, value: { document?: string }) => {
  if (typeof value?.document === "string" && /^[a-zA-Z0-9-]{1,64}$/.test(value.document)) {
    documentToken = value.document; acceptDocument?.(value.document);
    documentChallenge = Promise.resolve(value.document);
  }
});
function nativeDocument(): Promise<string> {
  return documentChallenge ??= new Promise((resolve, reject) => {
    const request = () => ipcRenderer.send("native:document");
    // A preload can run before commit. One bounded retry timer avoids waiting
    // on DOMContentLoaded while the application's module awaits this promise.
    const retry = setInterval(request, 100);
    const timer = setTimeout(() => { clearInterval(retry); reject(Error("Native document was not authorized")); }, 5000);
    acceptDocument = value => { clearInterval(retry); clearTimeout(timer); resolve(value); };
    request();
  });
}
function charge(port: NativePreloadPort, bytes: number, control: boolean): () => void {
  if ((control ? port.controls : port.data) + bytes > (control ? 65536 : 32 * 1024 * 1024 - 65536)
    || (control ? nativeControls : nativeData) + bytes > (control ? 8 : 120) * 1024 * 1024) throw Error("Native renderer transport capacity exhausted");
  if (control) { port.controls += bytes; nativeControls += bytes; } else { port.data += bytes; nativeData += bytes; }
  let held = true;
  return () => {
    if (!held) return; held = false;
    if (control) { port.controls -= bytes; nativeControls -= bytes; } else { port.data -= bytes; nativeData -= bytes; }
  };
}
function nativePost(port: NativePreloadPort, value: Record<string, unknown>): void {
  port.port.postMessage({ ...value, handle: port.assignment.handle, document: port.assignment.document });
}
function closeNative(port: NativePreloadPort, reason: unknown = { code: "closed", retryable: false, message: "Native document connection closed" }): void {
  if (port.closed) return;
  port.closed = true; nativePorts.delete(port.assignment.handle);
  try { nativePost(port, { type: "close" }); } catch { /* Detached document. */ }
  port.port.close();
  port.receiving?.release(); port.receiving = undefined;
  port.writing?.release(); port.writing = undefined;
  for (const queued of [...port.queue, ...port.controlQueue]) queued.release();
  port.queue = []; port.controlQueue = []; port.pending = undefined; port.scratch();
  try { port.ended?.(reason); } catch { /* Notifications cannot prevent cleanup. */ }
}
function nativePump(port: NativePreloadPort): void {
  if (port.closed || port.pending || !port.started) return;
  port.writing ??= port.controlQueue.shift() ?? port.queue.shift();
  const current = port.writing;
  if (!current) return;
  const token = crypto.randomUUID().replaceAll("-", "");
  const end = Math.min(current.bytes.length, current.offset + chunkBytes);
  const bytes = current.bytes.subarray(current.offset, end); current.offset = end;
  port.pending = token; nativePost(port, { type: "chunk", bytes, token });
}
async function nativeMessage(port: NativePreloadPort, value: any): Promise<void> {
  if (port.closed) return;
  if (!value || value.handle !== port.assignment.handle || value.document !== port.assignment.document) throw Error("Stale native port message");
  if (value.type === "closed") { closeNative(port, value.reason); return; }
  if (value.type === "accepted") {
    if (!port.pending || value.token !== port.pending || !port.writing) throw Error("Unknown native IPC acknowledgement");
    port.pending = undefined;
    if (port.writing.offset === port.writing.bytes.length) { port.writing.release(); port.writing = undefined; }
    nativePump(port); return;
  }
  if (value.type !== "chunk" || !port.started || !port.incoming || !(value.bytes instanceof Uint8Array)
    || value.bytes.length < 1 || value.bytes.length > chunkBytes || typeof value.token !== "string" || !/^[0-9a-f]{32}$/.test(value.token)) throw Error("Invalid native IPC delivery");
  if (!port.receiving) {
    if (value.bytes.length < 5) throw Error("Missing native frame header");
    const kind = value.bytes[0], total = new DataView(value.bytes.buffer, value.bytes.byteOffset, 5).getUint32(1, false) + 5;
    const control = kind === 10 || kind === 12;
    if ((!control && ![1, 2, 6].includes(kind)) || total > (control ? 4096 : 16 * 1024 * 1024)
      || (!control && total < 25)) throw Error("Invalid native renderer frame");
    port.receiving = { total, received: 0, release: charge(port, total, control) };
  }
  const assembly = port.receiving;
  assembly.received += value.bytes.length;
  if (assembly.received > assembly.total) throw Error("Native IPC delivery crossed a frame boundary");
  // Awaiting the private callback bounds both the browser IPC queue and the
  // renderer's reassembly buffer. It does not acknowledge the native frame.
  await port.incoming(value.bytes);
  if (port.closed) return;
  if (assembly.received === assembly.total) { assembly.release(); port.receiving = undefined; }
  nativePost(port, { type: "consumed", token: value.token });
}
ipcRenderer.on("native:assigned", (event: IpcRendererEvent, value: { request: string; assignment: NativeAssignment }) => {
  const pending = nativeOpens.get(value?.request), assigned = value?.assignment;
  const received = event.ports[0];
  if (!pending || event.ports.length !== 1 || !received || !assigned || nativePorts.size >= 32
    || assigned.document !== documentToken
    || typeof assigned.handle !== "string" || typeof assigned.document !== "string" || !["main", "remote"].includes(assigned.role)
    || nativePorts.has(assigned.handle)) { event.ports.forEach(port => port.close()); return; }
  nativeOpens.delete(value.request); clearTimeout(pending.timer);
  const port: NativePreloadPort = { assignment: Object.freeze({ ...assigned }), port: received, started: false, closed: false,
    data: 0, controls: 0, queue: [], controlQueue: [], scratch: () => {} };
  try {
    // One input and one output chunk can cross isolated-world/IPC boundaries.
    port.scratch = charge(port, chunkBytes * 2, false);
    nativePorts.set(assigned.handle, port);
    received.onmessage = message => { void nativeMessage(port, message.data).catch(error => closeNative(port, { code: "protocol", retryable: false, message: String(error) })); };
    received.onmessageerror = () => closeNative(port);
    received.start(); pending.resolve(port.assignment);
  } catch (error) { closeNative(port); pending.reject(error instanceof Error ? error : Error(String(error))); }
});
ipcRenderer.on("native:progress", (_event: IpcRendererEvent, value: { request?: string; document?: string; phase?: string; message?: string }) => {
  const pending = typeof value?.request === "string" ? nativeOpens.get(value.request) : undefined;
  if (!pending || !pending.document || value.document !== pending.document || value.document !== documentToken
    || typeof value.phase !== "string" || value.phase.length > 64
    || typeof value.message !== "string" || value.message.length > 4096) return;
  try { pending.progress?.({ phase: value.phase, message: value.message }); }
  catch { /* Progress observers cannot change the opening's lifetime. */ }
});
ipcRenderer.on("native:failed", (_event: IpcRendererEvent, value: { request?: string; message?: string; code?: string; retryable?: boolean }) => {
  const pending = typeof value?.request === "string" ? nativeOpens.get(value.request) : undefined;
  if (!pending) return;
  nativeOpens.delete(value.request!); clearTimeout(pending.timer);
  pending.reject(Error("SplittyNativeFailure:" + JSON.stringify({
    message: typeof value.message === "string" ? value.message : "Native backend could not be opened",
    code: typeof value.code === "string" ? value.code : "protocol", retryable: value.retryable === true,
  })));
});
function assignedPort(handle: string): NativePreloadPort {
  const port = nativePorts.get(handle);
  if (!port || port.closed) throw Error("Unknown native document connection");
  return port;
}
const nativeBridge = {
  state: (state: "connected" | "reconnecting" | "disconnected"): void => {
    if (!["connected", "reconnecting", "disconnected"].includes(state)) throw Error("Invalid native window state");
    void nativeDocument().then(document => ipcRenderer.send("native:state", { document, state }), () => {});
  },
  onRetry: (callback: () => void): (() => void) => {
    if (typeof callback !== "function" || retryCallback) throw Error("Native retry listener is unavailable");
    retryCallback = callback;
    return () => { if (retryCallback === callback) retryCallback = undefined; };
  },
  open: (options: { target?: string; update?: boolean } = {}, request = crypto.randomUUID(), progress?: (value: NativeProgress) => void): Promise<NativeAssignment> => {
    if (!options || typeof options !== "object" || Object.keys(options).some(key => !["target", "update"].includes(key))
      || (options.target !== undefined && (typeof options.target !== "string" || options.target.length > 4096))
      || (options.update !== undefined && typeof options.update !== "boolean")) return Promise.reject(Error("Invalid native open intent"));
    if (progress !== undefined && typeof progress !== "function") return Promise.reject(Error("Invalid native progress listener"));
    if (nativeOpens.size >= 4 || nativePorts.size + nativeOpens.size >= 32) return Promise.reject(Error("Native connection capacity exhausted"));
    if (typeof request !== "string" || !/^[a-zA-Z0-9-]{1,64}$/.test(request) || nativeOpens.has(request)) return Promise.reject(Error("Invalid native open request"));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        nativeOpens.delete(request); ipcRenderer.send("native:cancel-open", { request, document: documentToken }); reject(Error("Native backend preparation timed out"));
      // Host opens may include an install upload. Main owns the shorter local
      // deadline and the fixed ten-minute host deadline; this is a final guard.
      }, 605_000);
      nativeOpens.set(request, { resolve, reject, timer, progress });
      void nativeDocument().then(document => {
        const pending = nativeOpens.get(request);
        if (pending) { pending.document = document; ipcRenderer.send("native:open", { request, document, ...options }); }
      }, error => { nativeOpens.delete(request); clearTimeout(timer); reject(error); });
    });
  },
  cancelOpen: (request: string): void => {
    const pending = nativeOpens.get(request);
    if (!pending) return;
    nativeOpens.delete(request); clearTimeout(pending.timer);
    ipcRenderer.send("native:cancel-open", { request, document: documentToken });
    pending.reject(Error("Native connection preparation cancelled"));
  },
  listen: (handle: string, incoming: NativeCallback, ended: (reason: unknown) => void): (() => void) => {
    const port = assignedPort(handle);
    if (port.incoming || typeof incoming !== "function" || typeof ended !== "function") throw Error("Invalid native listener");
    port.incoming = incoming; port.ended = ended;
    return () => { port.incoming = undefined; port.ended = undefined; };
  },
  start: (handle: string): void => {
    const port = assignedPort(handle);
    if (port.started || !port.incoming) throw Error("Native document is not ready to start");
    port.started = true; nativePost(port, { type: "start" }); nativePump(port);
  },
  write: (handle: string, bytes: Uint8Array): void => {
    const port = assignedPort(handle);
    try {
      if (!(bytes instanceof Uint8Array) || bytes.length < 5 || bytes.length > 16 * 1024 * 1024
        || new DataView(bytes.buffer, bytes.byteOffset, 5).getUint32(1, false) !== bytes.length - 5) throw Error("Invalid native outgoing frame");
      const control = bytes[0] === 7 || bytes[0] === 10;
      if ((!control && bytes[0] !== 1 && bytes[0] !== 5) || (control && bytes.length > 4096)) throw Error("Invalid native outgoing kind");
      const queue = control ? port.controlQueue : port.queue;
      if (queue.length >= 64) throw Error("Native renderer queue exhausted");
      const release = charge(port, bytes.length, control);
      try { queue.push({ bytes: bytes.slice(), offset: 0, release }); } catch (error) { release(); throw error; }
      nativePump(port);
    } catch (error) { closeNative(port); throw error; }
  },
  close: (handle: string): void => { const port = nativePorts.get(handle); if (port) closeNative(port); },
};

const ALLOWED_CHANNELS = [
  "window:minimize",
  "window:maximize",
  "window:unmaximize",
  "window:toggle-maximize",
  "window:close",
  "window:force-close",
  "window:is-maximized",
  "window:start-drag",
  "window:set-zoom",
  "shell:open-external",
  "editor:open-remote",
  "editor:open-local",
  "download:reveal",
  // Remote windows (desktop-remote). Remote windows themselves get no preload.
  "remote:hosts",
  "remote:open",
  "remote:cancel",
];

contextBridge.exposeInMainWorld("__SPLITTY_ELECTRON__", {
  native: nativeBridge,
  invoke: (channel: string, ...args: unknown[]) => {
    if (!ALLOWED_CHANNELS.includes(channel)) {
      return Promise.reject(new Error(`IPC channel not allowed: ${channel}`));
    }
    return nativeDocument().then(document => ipcRenderer.invoke(channel, { document, args }));
  },
  onCloseRequested: (callback: () => void) => {
    const listener = () => callback();
    ipcRenderer.on("window:close-requested", listener);
    return () => { ipcRenderer.removeListener("window:close-requested", listener); };
  },
  /** Progress of a `remote:open` this window started: `{ target, step, message }`. */
  onRemoteProgress: (callback: (progress: unknown) => void) => {
    const listener = (_event: IpcRendererEvent, progress: unknown) => callback(progress);
    ipcRenderer.on("remote:progress", listener);
    return () => { ipcRenderer.removeListener("remote:progress", listener); };
  },
  platform: process.platform,
});
