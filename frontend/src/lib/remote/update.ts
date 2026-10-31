// Updating a host's Splitty from the window (`remote-screens`, Host Update).
//
// The hub replaces a daemon of another version only for a socket opened with
// `update=1`, which stops the daemon. A daemon that keeps its sessions
// (`keepsSessions`) leaves them running for the new one; any other ends every
// session on the host. The window asks for that only after the user confirmed
// a warning saying which. What ends is known from the screens of the host's
// `incompatible` error or its last `remote.ready`, and the screens the window
// shows. A hub that sends neither leaves the count unknown, and the warning
// says so.

import type { HostScreen } from "./remote_socket.ts";

/** What an update ends on the host. */
export type UpdateImpact =
  | { known: true; screens: number; panes: number }
  /** No `remote.ready` came from this host; the window shows `shown` of its screens. */
  | { known: false; shown: number };

/**
 * A screen of the host that the window shows, with its panes as the window
 * has them now; none when the window has no layout for it yet.
 */
export interface ShownHostScreen {
  wireId: string;
  panes?: number;
}

/**
 * The host's screens and panes: those of its last `remote.ready`, with the
 * window's own pane count for the screens it has a layout of, plus shown
 * screens created since. Unknown without a `remote.ready`.
 */
export function updateImpact(lastReady: readonly HostScreen[] | undefined, shown: readonly ShownHostScreen[]): UpdateImpact {
  if (!lastReady) return { known: false, shown: shown.length };
  const panes = new Map<string, number>();
  for (const screen of lastReady) panes.set(screen.id, screen.panes);
  // A shown screen without a layout that the host did not list is missing there.
  for (const screen of shown) if (screen.panes !== undefined) panes.set(screen.wireId, screen.panes);
  let total = 0;
  for (const count of panes.values()) total += count;
  return { known: true, screens: panes.size, panes: total };
}

export interface UpdateWarning {
  title: string;
  /** Which versions: the host's now and the one it gets. */
  versions: string;
  /** What ends on the host. */
  ends: string;
}

export interface UpdateVersions {
  /** The host daemon's version and protocol, from its `remote.error` or `remote.ready`. */
  hostVersion?: string;
  hostProtocol?: number;
  /** The host daemon's sessions outlive it, so the update ends none. */
  keepsSessions?: boolean;
  /** The local server's version, which the host gets. */
  localVersion?: string;
}

/** The warning the user confirms before the window opens the host's hub socket with `update=1`. */
export function updateWarning(target: string, versions: UpdateVersions, impact: UpdateImpact): UpdateWarning {
  const running = versions.hostVersion
    ? `Splitty ${versions.hostVersion}${versions.hostProtocol !== undefined ? ` (protocol ${versions.hostProtocol})` : ""}`
    : "another Splitty";
  const next = versions.localVersion ? `Splitty ${versions.localVersion}, the version of this app` : "the version of this app";
  let ends: string;
  if (versions.keepsSessions) {
    ends = `The sessions on ${target} keep running, with every process in them: `
      + `updating restarts its Splitty, and the connection drops for a moment.`;
  } else if (impact.known) {
    ends = impact.screens === 0
      ? `No screens run on ${target} now; updating ends nothing there.`
      : `Updating ends ${count(impact.screens, "screen")} with ${count(impact.panes, "pane")} on ${target}, and every process running in them.`;
  } else {
    ends = `Updating ends every screen on ${target} and every process running in them. `
      + `How many there are is not known: this Splitty on ${target} cannot report them to this app.`;
    if (impact.shown > 0) ends += ` This window shows ${impact.shown} of them.`;
  }
  return {
    title: `Update Splitty on ${target}?`,
    versions: `${target} runs ${running}. Updating stops it and starts ${next}.`,
    ends,
  };
}

/**
 * The tooltip's note for a host whose daemon runs another version than the
 * local server: an update to the local version is available.
 */
export function updateNote(hostVersion: string | undefined, localVersion: string | undefined): string | undefined {
  if (!hostVersion || !localVersion || hostVersion === localVersion) return undefined;
  return `update to ${localVersion} available`;
}

function count(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}
