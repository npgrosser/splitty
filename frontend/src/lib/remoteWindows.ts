// The shell bridge for remote windows (`desktop-remote`, Remote Window Shell
// API): list the known hosts, open one in a window of its own, cancel an open
// in flight, and follow its progress. Only the Tauri shell has it; browsers and
// remote sessions get a bridge that is unavailable.

import { shellKind, type ShellKind } from "./platform.ts";
import { parseKnownHosts, type KnownHost } from "./remote/known_hosts.ts";
import { migrationOffer, type MigrationOffer } from "./remote/update.ts";

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
  migration?: MigrationOffer;
}

export interface RemoteWindowsAPI {
  /** Only in a desktop shell's trusted UI, never in a remote session. */
  readonly available: boolean;
  hosts(): Promise<KnownHost[]>;
  /** Resolves once the remote window shows the host; rejects with a {@link RemoteOpenError}. */
  open(target: string, options?: { update?: boolean }): Promise<void>;
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
    const { message, hint, migration: offered } = cause as { message?: unknown; hint?: unknown; migration?: unknown };
    if (typeof message === "string" && message) {
      const result: RemoteOpenError = typeof hint === "string" && hint ? { message, hint } : { message };
      const migration = migrationOffer(offered);
      if (migration) result.migration = migration;
      return result;
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
    scope.__SPLITTY_TAURI__.invoke(command, args);

  const callbacks = new Set<(progress: RemoteProgress) => void>();
  const pending = new Set<string>();
  const polling = new Set<string>();
  const last = new Map<string, string>();
  let timer: ReturnType<typeof setInterval> | undefined;
  const poll = () => {
    for (const target of pending) {
      if (polling.has(target)) continue;
      polling.add(target);
      void invoke("remote_progress", { target }).then(value => {
        if (!pending.has(target) || !isProgress(value)) return;
        const key = JSON.stringify(value);
        if (last.get(target) === key) return;
        last.set(target, key);
        for (const callback of callbacks) callback(value);
      }).catch(() => {}).finally(() => polling.delete(target));
    }
  };
  return {
    available: true,
    async hosts() {
      return parseKnownHosts(await invoke("remote_hosts"));
    },
    async open(target, options) {
      pending.add(target);
      if (!timer) timer = setInterval(poll, 150);
      try {
        await invoke("remote_open", { target, ...(options?.update === true ? { update: true } : {}) });
      } catch (cause) {
        throw toOpenError(cause);
      } finally {
        pending.delete(target); last.delete(target);
        if (!pending.size) { clearInterval(timer); timer = undefined; }
      }
    },
    async cancel(target) {
      await invoke("remote_cancel", { target }).catch(() => {});
    },
    onProgress(callback) {
      callbacks.add(callback);
      return () => { callbacks.delete(callback); };
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
  return unavailable;
}

/** `shellKind` is `web` in a remote session, so the bridge is unavailable there. */
export const remoteWindows: RemoteWindowsAPI = createRemoteWindows(shellKind);
