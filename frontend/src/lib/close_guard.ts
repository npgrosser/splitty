import type { DockviewApi } from "dockview-core";
import { bindingForApi, sessionForPane } from "./connection/screen_binding.ts";
import { connections, localConnection } from "./connection/registry.ts";

export interface CloseRequest {
  paneId: string;
  /** The running process that closing ends; empty when none runs. */
  processName: string;
  /** Application title set via OSC 2 (e.g. Claude Code's task description). */
  applicationTitle?: string;
  /** How many of the closing panes have a scratchpad that holds text (`session-scratchpad`). */
  scratchpads: number;
  close: () => void;
}

type Listener = (req: CloseRequest) => void;
let listener: Listener | null = null;

/**
 * Register the confirmation dialog handler (called by App).
 * Returns an unsubscribe function.
 */
export function onCloseRequest(fn: Listener): () => void {
  listener = fn;
  return () => { listener = null; };
}

/** Whether a pane's scratchpad holds text, which closing the pane ends. */
function holdsScratchpad(api: DockviewApi, paneId: string): boolean {
  const binding = bindingForApi(api);
  if (!binding) return false;
  const { scratchpads } = binding.conn;
  const session = scratchpads.sessionOf(paneId) ?? binding.sessionForPane(paneId)?.id;
  return session !== undefined && scratchpads.text(session) !== "";
}

/**
 * Attempt to close a pane of the screen whose layout is `api`. If a process is
 * running or its scratchpad holds text, triggers the confirmation dialog
 * instead of closing immediately.
 */
export function guardedClose(api: DockviewApi, paneId: string, close: () => void): void {
  const session = sessionForPane(api, paneId);
  const running = session && !session.lastIsBaseShell ? session : undefined;
  const scratchpads = holdsScratchpad(api, paneId) ? 1 : 0;
  if ((running || scratchpads > 0) && listener) {
    listener({
      paneId,
      processName: running ? running.lastTitle || running.lastCmdline || "unknown" : "",
      applicationTitle: running?.lastOscTitle || undefined,
      scratchpads,
      close,
    });
  } else {
    close();
  }
}

/** A running process that quitting would end: its process name, and the title its program set. */
export interface RunningProcess {
  name: string;
  /** Set via OSC 2, such as Claude Code's task. */
  title?: string;
}

/**
 * The local server's sessions with a running (non-base-shell) process, which
 * quitting ends. A daemon keeps its sessions when the window goes, so there
 * are none then; and quitting never ends the sessions of another server.
 */
export function runningOnQuit(): RunningProcess[] {
  const local = localConnection();
  if (local.mux.serverMode === "daemon") return [];
  const running: RunningProcess[] = [];
  for (const session of local.sessions.all()) {
    if (session.lastIsBaseShell) continue;
    running.push({ name: session.lastTitle || session.lastCmdline || "unknown", title: session.lastOscTitle || undefined });
  }
  return running;
}

/**
 * How many of the local server's sessions have a scratchpad that holds text,
 * which quitting ends with them. A daemon keeps them (`session-scratchpad`).
 */
export function scratchpadsOnQuit(): number {
  const local = localConnection();
  if (local.mux.serverMode === "daemon") return 0;
  return [...local.sessions.all()].filter((session) => local.scratchpads.text(session.id) !== "").length;
}

/** Whether a host's screens are open, which quitting only detaches. */
export function hostScreensOpen(): boolean {
  return connections().some((conn) => conn.kind !== "local");
}

/**
 * Close multiple panels of the screen whose layout is `api`. Idle panels are
 * closed immediately. If any run a process or hold scratchpad text, a single
 * confirmation dialog is shown; on confirm they are closed together.
 */
export function guardedCloseMultiple(api: DockviewApi, panels: { id: string; close: () => void }[]): void {
  const idle: typeof panels = [];
  const asking: typeof panels = [];
  const running: typeof panels = [];
  let scratchpads = 0;
  for (const p of panels) {
    const session = sessionForPane(api, p.id);
    const runs = Boolean(session && !session.lastIsBaseShell);
    const holds = holdsScratchpad(api, p.id);
    if (runs) running.push(p);
    if (holds) scratchpads++;
    if (runs || holds) {
      asking.push(p);
    } else {
      idle.push(p);
    }
  }
  for (let i = idle.length - 1; i >= 0; i--) {
    idle[i].close();
  }
  if (asking.length > 0 && listener) {
    const first = running[0] ?? asking[0];
    const session = running.length > 0 ? sessionForPane(api, first.id) : undefined;
    const name = session ? session.lastTitle || session.lastCmdline || "unknown" : "";
    const suffix = running.length > 1 ? ` (and ${running.length - 1} more)` : "";
    listener({
      paneId: first.id,
      processName: name ? name + suffix : "",
      applicationTitle: session?.lastOscTitle || undefined,
      scratchpads,
      close: () => {
        for (let i = asking.length - 1; i >= 0; i--) {
          asking[i].close();
        }
      },
    });
  }
}
