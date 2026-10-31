// What a remote screen shows instead of, or over, its panes, derived from its
// host's connection and the screen's own flags. Kept out of the component so
// it can be tested without rendering.

import type { HostScreen, RemoteFailure, RemoteProgressStep, RemoteState } from "./remote_socket.ts";
import { updateNote } from "./update.ts";

export type RemoteScreenStatus =
  | { kind: "none" }
  /**
   * The first connect (or a new screen waiting for its host): progress with
   * Cancel; `fresh` for a new screen, otherwise a screen the host has (a
   * remembered one), which Close removes from the window.
   */
  | { kind: "connecting"; target: string; progress: readonly RemoteProgressStep[]; fresh: boolean }
  /** An error that needs the user: message, hint, details, Copy, Retry. */
  | { kind: "failed"; target: string; error: RemoteFailure; progress: readonly RemoteProgressStep[] }
  /** The host runs screens the window does not show: open them, or create a new one. */
  | { kind: "offer"; target: string; screens: readonly HostScreen[] }
  /** The connection dropped; the panes stay visible but take no input. */
  | { kind: "reconnecting"; target: string; attempt: number; progress: readonly RemoteProgressStep[]; error?: RemoteFailure }
  /** The host stayed unreachable: Retry. */
  | { kind: "offline"; target: string; error?: RemoteFailure }
  /** The host no longer has this screen: Remove. */
  | { kind: "missing"; target: string };

/** The screen's own state, kept by its host's `RemoteScreenSync`. */
export interface RemoteScreenFlags {
  /** Created in this window, `create_screen` not sent yet. */
  pending: boolean;
  /** Waiting for the answer to the reopen offer. */
  needsChoice: boolean;
  missing: boolean;
}

/**
 * The status of a remote screen of `target`. `host` is its connection's state,
 * undefined when the connection is gone; `offer` the host's screens the window
 * does not show.
 */
export function remoteScreenStatus(
  target: string,
  host: RemoteState | undefined,
  flags: RemoteScreenFlags,
  offer: readonly HostScreen[],
): RemoteScreenStatus {
  if (!host || host.phase === "closed") return { kind: "offline", target };
  switch (host.phase) {
    case "connecting":
      return { kind: "connecting", target, progress: host.progress, fresh: flags.pending };
    case "failed":
      return {
        kind: "failed",
        target,
        error: host.error ?? { code: "internal", message: "The connection failed." },
        progress: host.progress,
      };
    case "offline":
      return { kind: "offline", target, error: host.error };
    case "reconnecting":
      return { kind: "reconnecting", target, attempt: host.attempt, progress: host.progress, error: host.error };
    case "ready":
      if (flags.needsChoice && offer.length > 0) return { kind: "offer", target, screens: offer };
      if (flags.missing) return { kind: "missing", target };
      // A new screen whose `create_screen` waits for the host's welcome.
      if (flags.pending) return { kind: "connecting", target, progress: [], fresh: true };
      return { kind: "none" };
  }
}

/** Whether the screen's panes must not take input. */
export function blocksInput(status: RemoteScreenStatus): boolean {
  return status.kind !== "none";
}

/**
 * Whether the status offers "Update now": the host's daemon is incompatible,
 * and only an update (which ends its sessions) lets the window use it.
 */
export function offersUpdate(status: RemoteScreenStatus): boolean {
  return status.kind === "failed" && status.error.code === "incompatible";
}

// ── Errors ──

export interface FailureView {
  headline: string;
  /** The hub's message, shown as it is, where it is the substance (paths, versions). */
  message?: string;
  hint?: string;
  /** The hub's message folded away under "Details", where it is `ssh`'s error output. */
  details?: string;
  /** What [Copy] copies: the command to run, built here from the target, or the error. */
  copy: string;
}

/** How an error of a host's connection reads. The target and every hub string stay plain text. */
export function describeFailure(target: string, error: RemoteFailure): FailureView {
  const headline = HEADLINES[error.code](target);
  const folded = error.code === "host_key" || error.code === "auth" || error.code === "unreachable" || error.code === "bridge";
  let message = error.message;
  if (error.code === "incompatible" && !message && error.version) {
    message = `${target} runs Splitty ${error.version}${error.protocol !== undefined ? ` (protocol ${error.protocol})` : ""}.`;
  }
  const view: FailureView = {
    headline,
    copy: error.code === "host_key" || error.code === "auth"
      ? `ssh ${target}`
      : [message, error.hint].filter(Boolean).join("\n"),
  };
  if (error.hint) view.hint = error.hint;
  if (message) {
    if (folded) view.details = message;
    else view.message = message;
  }
  return view;
}

const HEADLINES: Record<RemoteFailure["code"], (target: string) => string> = {
  invalid_target: (t) => `${t} is not a host Splitty can connect to.`,
  host_key: (t) => `${t} is not known yet.`,
  auth: (t) => `SSH login to ${t} failed.`,
  unreachable: (t) => `${t} could not be reached.`,
  unsupported_host: (t) => `${t} cannot run Splitty.`,
  install: (t) => `Splitty could not be installed on ${t}.`,
  daemon: (t) => `The Splitty server on ${t} did not start.`,
  incompatible: (t) => `${t} runs an incompatible Splitty.`,
  bridge: (t) => `The connection to ${t} ended before its server answered.`,
  internal: (t) => `Splitty could not connect to ${t}.`,
  ticket: (t) => `The local Splitty server did not let this window connect to ${t}.`,
  closed: (t) => `The connection to ${t} closed before it was ready.`,
  protocol: (t) => `The connection to ${t} failed.`,
};

// ── Screen List ──

/**
 * The state dot of a host in the screen list. `idle`: the window has no
 * connection to the host yet (a remembered screen that was not shown);
 * `failed`: an error that needs the user.
 */
export type HostIndicator = "connected" | "connecting" | "reconnecting" | "failed" | "offline" | "idle";

export function hostIndicator(host: RemoteState | undefined): HostIndicator {
  if (!host) return "idle";
  switch (host.phase) {
    case "ready":
      return "connected";
    case "connecting":
      return "connecting";
    case "reconnecting":
      return "reconnecting";
    case "failed":
      return "failed";
    default:
      return "offline";
  }
}

/**
 * The tooltip of a remote screen's host badge: `user@host · Splitty <version>
 * · <state>`, and a note when the host's daemon runs another version than the
 * local server (`localVersion`).
 */
export function hostTooltip(target: string, host: RemoteState | undefined, localVersion?: string): string {
  const parts = [target];
  if (host?.ready) {
    parts.push(`Splitty ${host.ready.daemon.version}`);
    const note = updateNote(host.ready.daemon.version, localVersion);
    if (note) parts.push(note);
  }
  const indicator = hostIndicator(host);
  if (indicator === "idle") parts.push("not connected");
  else parts.push(indicator);
  return parts.join(" · ");
}
