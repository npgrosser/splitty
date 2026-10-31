// The shell bridge for remote windows (`desktop-remote`, Remote Window Shell
// API): list the known hosts, open one in a window of its own, cancel an open
// in flight, and follow its progress. The contract is the same in both shells;
// only the transport differs.

import { shellKind, type ShellKind } from "./platform.ts";
import { parseKnownHosts, type KnownHost } from "./remote/known_hosts.ts";

export { parseKnownHosts, type KnownHost };

/** A step of an open in flight, as `splitty connect --json` reports it. */
export interface RemoteProgress {
  target: string;
  /** `install`, `daemon`, `started`, `tunnel`, or `notice`. */
  step: string;
  message: string;
}

/** Why an open failed; `hint` says what the user can do about it. */
export interface RemoteOpenError {
  message: string;
  hint?: string;
}

export interface RemoteWindowsAPI {
  /** Only in a desktop shell's trusted UI, never in a remote session. */
  readonly available: boolean;
  hosts(): Promise<KnownHost[]>;
  /** Resolves once the remote window shows the host; rejects with a {@link RemoteOpenError}. */
  open(target: string): Promise<void>;
  /** The pending open of `target` then rejects with `{ message: "cancelled" }`. */
  cancel(target: string): Promise<void>;
  onProgress(callback: (progress: RemoteProgress) => void): () => void;
}

/**
 * Turn whatever a shell rejected with into a {@link RemoteOpenError}.
 *
 * Tauri rejects with the command's serialized error, `{ message, hint? }`;
 * a plain string or an `Error` is accepted as well.
 */
export function toOpenError(cause: unknown): RemoteOpenError {
  if (typeof cause === "string" && cause) return { message: cause };
  if (cause && typeof cause === "object") {
    const { message, hint } = cause as { message?: unknown; hint?: unknown };
    if (typeof message === "string" && message) {
      return typeof hint === "string" && hint ? { message, hint } : { message };
    }
  }
  return { message: "The host could not be opened." };
}

function isProgress(value: unknown): value is RemoteProgress {
  const p = value as RemoteProgress | null;
  return !!p && typeof p.target === "string" && typeof p.step === "string" && typeof p.message === "string";
}

// ---------------------------------------------------------------------------
// Tauri: app commands and the `remote-progress` event
// ---------------------------------------------------------------------------

function tauriBridge(scope: any): RemoteWindowsAPI {
  const invoke = (command: string, args?: Record<string, unknown>): Promise<unknown> =>
    scope.__TAURI_INTERNALS__.invoke(command, args);

  return {
    available: true,
    async hosts() {
      return parseKnownHosts(await invoke("remote_hosts"));
    },
    async open(target) {
      try {
        await invoke("remote_open", { target });
      } catch (cause) {
        throw toOpenError(cause);
      }
    },
    async cancel(target) {
      await invoke("remote_cancel", { target }).catch(() => {});
    },
    onProgress(callback) {
      // `listen` registers asynchronously; an unsubscribe before that finishes
      // drops the listener as soon as it exists.
      let unlisten: (() => void) | undefined;
      let disposed = false;
      void import("@tauri-apps/api/event")
        .then(({ listen }) => listen<unknown>("remote-progress", (event) => {
          if (isProgress(event.payload)) callback(event.payload);
        }))
        .then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        })
        .catch((error: unknown) => {
          console.warn("could not follow remote window progress", error);
        });
      return () => {
        disposed = true;
        unlisten?.();
      };
    },
  };
}

// ---------------------------------------------------------------------------
// Electron: preload channels
// ---------------------------------------------------------------------------

/** What `remote:open` resolves with: `ipcRenderer.invoke` rejections lose their fields. */
type ElectronOpenResult = { ok: true } | { ok: false; error?: unknown };

function electronBridge(scope: any): RemoteWindowsAPI {
  const electron = scope.__SPLITTY_ELECTRON__;

  return {
    // A preload without the subscription predates remote windows.
    available: typeof electron?.onRemoteProgress === "function",
    async hosts() {
      return parseKnownHosts(await electron.invoke("remote:hosts"));
    },
    async open(target) {
      let result: ElectronOpenResult | undefined;
      try {
        result = await electron.invoke("remote:open", target);
      } catch (cause) {
        throw toOpenError(cause);
      }
      if (!result?.ok) throw toOpenError(result?.error);
    },
    async cancel(target) {
      await electron.invoke("remote:cancel", target).catch(() => {});
    },
    onProgress(callback) {
      return electron.onRemoteProgress((progress: unknown) => {
        if (isProgress(progress)) callback(progress);
      });
    },
  };
}

// ---------------------------------------------------------------------------
// Web and remote sessions: nothing to open
// ---------------------------------------------------------------------------

const unavailable: RemoteWindowsAPI = {
  available: false,
  hosts: () => Promise.resolve([]),
  open: () => Promise.reject({ message: "Remote windows need the Splitty desktop app." } satisfies RemoteOpenError),
  cancel: () => Promise.resolve(),
  onProgress: () => () => {},
};

/** The bridge for a shell; `scope` holds the shell's globals (`window`). */
export function createRemoteWindows(kind: ShellKind, scope: any = window): RemoteWindowsAPI {
  if (kind === "tauri") return tauriBridge(scope);
  if (kind === "electron") {
    const bridge = electronBridge(scope);
    return bridge.available ? bridge : unavailable;
  }
  return unavailable;
}

/** `shellKind` is `web` in a remote session, so the bridge is unavailable there. */
export const remoteWindows: RemoteWindowsAPI = createRemoteWindows(shellKind);
