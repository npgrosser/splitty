import type { DockviewApi } from "dockview-core";
import { bindingForApi } from "./connection/screen_binding.ts";
import { connections, localConnection } from "./connection/registry.ts";
import type { ServerConnection } from "./connection/server_connection.ts";
import type { CloseAssessment, CloseLoss, CloseScope } from "./protocol/types.ts";

/**
 * What a close would end comes from the server of the screen
 * (`mux-protocol`, Close Assessment); this module only asks the question and
 * words it. Without the server's answer (an older server, a failed or late
 * answer) it asks a generic question.
 */
export interface CloseRequest {
  paneId: string;
  /** The running process that closing ends; empty when none runs. */
  processName: string;
  /** Application title set via OSC 2 (e.g. Claude Code's task description). */
  applicationTitle?: string;
  /** How many of the closing terminals are still starting. */
  starting: number;
  /** How many of the closing panes have a scratchpad that holds text (`session-scratchpad`). */
  scratchpads: number;
  /** How many annotations the closing viewer panes hold, which closing them ends (`viewer-annotations`). */
  annotations: number;
  /** The server did not say what closing ends. */
  unknown: boolean;
  /** Closing panes, or replacing the screen by a template (`templates`, Confirmed Screen Replacement). */
  action: "close" | "replace";
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

/** The server's answer, or null when the server cannot give one. */
async function assess(conn: ServerConnection, scope: CloseScope): Promise<CloseAssessment | null> {
  if (!conn.mux.closeAssessments) return null;
  try {
    return await conn.mux.assessClose(scope);
  } catch {
    return null;
  }
}

/** The process name and program title shown for a running entry. */
function named(conn: ServerConnection, entry: CloseLoss): { name: string; title?: string } {
  const session = entry.sessionId ? conn.sessions.get(entry.sessionId) : undefined;
  return {
    name: entry.title || session?.lastTitle || session?.lastCmdline || "unknown",
    title: session?.lastOscTitle || undefined,
  };
}

/**
 * The question for `entries` of an assessment, or a generic one when there is
 * no assessment. Pure apart from reading session titles.
 */
export function closeQuestion(
  conn: ServerConnection,
  assessment: CloseAssessment | null,
  entries: readonly CloseLoss[],
): Omit<CloseRequest, "paneId" | "close" | "action"> {
  if (!assessment) return { processName: "", starting: 0, scratchpads: 0, annotations: 0, unknown: true };
  const running = entries.filter((entry) => entry.running);
  const first = running[0] ? named(conn, running[0]) : undefined;
  const suffix = running.length > 1 ? ` (and ${running.length - 1} more)` : "";
  return {
    processName: first ? first.name + suffix : "",
    applicationTitle: first?.title,
    starting: entries.filter((entry) => entry.starting).length,
    scratchpads: entries.filter((entry) => entry.scratchpad).length,
    annotations: entries.reduce((sum, entry) => sum + entry.annotations, 0),
    unknown: false,
  };
}

/**
 * Attempt to close a pane of the screen whose layout is `api`; asks first
 * when the server says the close ends something.
 */
export function guardedClose(api: DockviewApi, paneId: string, close: () => void): Promise<void> {
  return guardedCloseMultiple(api, [{ id: paneId, close }]);
}

/**
 * Close panels of the screen whose layout is `api`. Panels the server says
 * lose nothing close at once; the others get one confirmation and close
 * together on confirm. Without the server's answer all of them are asked
 * about.
 */
export async function guardedCloseMultiple(api: DockviewApi, panels: { id: string; close: () => void }[]): Promise<void> {
  if (panels.length === 0) return;
  const binding = bindingForApi(api);
  if (!binding) {
    // No server stands behind this layout, so closing ends nothing there.
    for (let i = panels.length - 1; i >= 0; i--) panels[i].close();
    return;
  }
  const conn = binding.conn;
  const assessment = await assess(conn, { kind: "panes", screenId: binding.wireId, paneIds: panels.map((p) => p.id) });
  const losing = new Map<string, CloseLoss>();
  for (const entry of assessment?.entries ?? []) {
    if (entry.paneId) losing.set(entry.paneId, entry);
  }
  const asking = assessment ? panels.filter((p) => losing.has(p.id)) : panels;
  const idle = panels.filter((p) => !asking.includes(p));
  for (let i = idle.length - 1; i >= 0; i--) idle[i].close();
  if (asking.length === 0 || !listener) return;
  const entries = asking.flatMap((p) => losing.get(p.id) ?? []);
  const first = entries.find((entry) => entry.running)?.paneId ?? asking[0].id;
  listener({
    paneId: first,
    ...closeQuestion(conn, assessment, entries),
    action: "close",
    close: () => {
      for (let i = asking.length - 1; i >= 0; i--) asking[i].close();
    },
  });
}

/**
 * Replace the screen `screenId` of `conn` by a template: `apply` runs at once
 * when the server says the replacement ends nothing, after a confirmation
 * otherwise, and after a generic confirmation without the server's answer.
 * Returns false when a needed confirmation cannot be shown.
 */
export async function guardedReplace(conn: ServerConnection, screenId: string, apply: () => void): Promise<boolean> {
  const assessment = await assess(conn, { kind: "screens", screenIds: [screenId] });
  if (assessment && !assessment.confirm) {
    apply();
    return true;
  }
  if (!listener) return false;
  const entries = assessment?.entries ?? [];
  listener({
    paneId: entries.find((entry) => entry.running)?.paneId ?? "",
    ...closeQuestion(conn, assessment, entries),
    action: "replace",
    close: apply,
  });
  return true;
}

/** A running process that quitting would end: its process name, and the title its program set. */
export interface RunningProcess {
  name: string;
  /** Set via OSC 2, such as Claude Code's task. */
  title?: string;
}

/** What quitting the window ends with the local server. */
export interface QuitImpact {
  processes: RunningProcess[];
  starting: number;
  scratchpads: number;
  annotations: number;
  /** The local server did not say what quitting ends. */
  unknown: boolean;
}

/**
 * What quitting ends, from the local server's assessment of its whole
 * workspace; null when it ends nothing. A daemon keeps its sessions when the
 * window goes, and quitting never ends the sessions of another server.
 */
export async function quitImpact(): Promise<QuitImpact | null> {
  const local = localConnection();
  if (local.mux.serverMode === "daemon") return null;
  const assessment = await assess(local, { kind: "workspace" });
  if (!assessment) return { processes: [], starting: 0, scratchpads: 0, annotations: 0, unknown: true };
  if (!assessment.confirm) return null;
  return {
    processes: assessment.entries.filter((entry) => entry.running).map((entry) => named(local, entry)),
    starting: assessment.entries.filter((entry) => entry.starting).length,
    scratchpads: assessment.scratchpads,
    annotations: assessment.annotations,
    unknown: false,
  };
}

/** Whether a host's screens are open, which quitting only detaches. */
export function hostScreensOpen(): boolean {
  return connections().some((conn) => conn.kind !== "local");
}
