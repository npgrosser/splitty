/**
 * Mux messages as they arrive, checked before anything acts on them.
 *
 * A server's welcome, snapshot and screen names are data from that server;
 * once a window shows screens of several servers, not all of them are trusted.
 * The checks keep a malformed message from leaving a screen half rebuilt:
 * anything of the wrong shape is dropped or replaced by an empty value, and
 * text is capped (`untrusted.ts`).
 */

import type {
  FloatingEntry, LayoutNode, PaneState, ReviewWait, ScreenInfo, ScreenState, SessionState, ViewerRecord, ViewerSource,
} from "../protocol/types.ts";
import { MAX_NAME, MAX_TITLE, MAX_VALUE, optionalBounded, optionalText, plainText } from "../protocol/untrusted.ts";
import { SCRATCHPAD_LIMIT } from "../scratchpad.ts";
import { REPLY_TITLE_LIMIT, isWaitId, receivedAnnotations } from "../viewer/annotations.ts";
import { isViewerPath, isViewerTitle } from "../viewer/limits.ts";

/** Deeper layout trees are cut off; no real layout comes close. */
const MAX_LAYOUT_DEPTH = 64;

type Raw = Record<string, unknown>;

function isRecord(value: unknown): value is Raw {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
}

function numbers(value: unknown): number[] {
  return Array.isArray(value) ? value.filter((v): v is number => typeof v === "number" && Number.isFinite(v)) : [];
}

/** The screen list of a `mux.welcome`; entries without an id are dropped. */
export function screenList(value: unknown): ScreenInfo[] {
  if (!Array.isArray(value)) return [];
  const screens: ScreenInfo[] = [];
  for (const entry of value) {
    if (!isRecord(entry) || typeof entry.id !== "string") continue;
    screens.push({ id: entry.id, name: optionalText(entry.name, MAX_NAME) });
  }
  return screens;
}

/** The screen of a `mux.screen_snapshot`, or null when it has no id or no layout. */
export function screenState(value: unknown): ScreenState | null {
  if (!isRecord(value) || typeof value.id !== "string") return null;
  const root = layoutNode(value.root, 0);
  if (!root) return null;
  return {
    id: value.id,
    name: optionalText(value.name, MAX_NAME),
    revision: typeof value.revision === "number" ? value.revision : 0,
    root,
    floating: Array.isArray(value.floating) ? value.floating.map(floatingEntry).filter((f) => f !== null) : [],
    panes: table(value.panes, paneState),
    sessions: table(value.sessions, sessionState),
    ...(isRecord(value.jobs) ? { jobs: table(value.jobs, (id, raw) => ({
      jobId: id,
      state: jobState(raw.state),
      error: optionalText(raw.error, MAX_VALUE),
    })) } : {}),
  };
}

function layoutNode(value: unknown, depth: number): LayoutNode | null {
  if (depth > MAX_LAYOUT_DEPTH || !isRecord(value) || typeof value.id !== "string") return null;
  if (value.type === "slot") {
    return { type: "slot", id: value.id, paneIds: strings(value.paneIds), activePaneId: optionalString(value.activePaneId) };
  }
  if (value.type === "split") {
    const children = Array.isArray(value.children)
      ? value.children.map((child) => layoutNode(child, depth + 1)).filter((c) => c !== null)
      : [];
    return {
      type: "split",
      id: value.id,
      axis: value.axis === "vertical" ? "vertical" : "horizontal",
      children,
      sizes: numbers(value.sizes),
    };
  }
  return null;
}

function floatingEntry(value: unknown): FloatingEntry | null {
  if (!isRecord(value) || typeof value.id !== "string" || !isRecord(value.bounds)) return null;
  const root = layoutNode(value.root, 0);
  if (!root) return null;
  const n = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
  const { x, y, w, h } = value.bounds;
  return { id: value.id, root, bounds: { x: n(x), y: n(y), w: n(w), h: n(h) } };
}

/**
 * A table of entries keyed by id. `Object.fromEntries` defines own properties,
 * so a key such as `__proto__` stays an ordinary entry.
 */
function table<T>(value: unknown, entry: (id: string, raw: Raw) => T): Record<string, T> {
  if (!isRecord(value)) return {};
  return Object.fromEntries(
    Object.entries(value)
      .filter((pair): pair is [string, Raw] => isRecord(pair[1]))
      .map(([id, raw]) => [id, entry(id, raw)]),
  );
}

function paneState(id: string, raw: Raw): PaneState {
  const role = raw.role === "webview" || raw.role === "viewer" ? raw.role : "terminal";
  const viewer = role === "viewer" ? viewerRecord(raw.viewer) : undefined;
  return {
    id,
    ...(typeof raw.jobId === "string" ? { jobId: optionalBounded(raw.jobId, 256) } : {}),
    sessionId: optionalString(raw.sessionId),
    title: optionalText(raw.title, MAX_TITLE),
    cwd: optionalBounded(raw.cwd, MAX_VALUE),
    role,
    ...(viewer ? { viewer } : {}),
  };
}

/**
 * A viewer pane's record, or undefined when it is malformed: the pane then
 * stays a viewer pane that says it cannot be shown (`viewer-panes`, Viewer
 * Capability). A source of an unknown kind is malformed. Nothing but the
 * listed fields is kept, so a text a server put into a snapshot goes no further.
 * Its annotations are checked by the server's rules (`viewer-annotations`);
 * malformed ones are dropped and the list is bounded by the pane's limits.
 * A malformed counter of numbers or wait is left out.
 */
function viewerRecord(value: unknown): ViewerRecord | undefined {
  if (!isRecord(value) || !isRecord(value.source)) return undefined;
  const source = viewerSource(value.source);
  const revision = value.revision;
  if (!source || typeof revision !== "number" || !Number.isSafeInteger(revision) || revision < 0) return undefined;
  const origin = value.originPaneId;
  const annotations = receivedAnnotations(value.annotations);
  const last = value.lastAnnotationNumber;
  const wait = reviewWait(value.reviewWait);
  return {
    source,
    revision,
    ...(typeof origin === "string" && origin.length > 0 && origin.length <= 256 ? { originPaneId: origin } : {}),
    ...(annotations.length > 0 ? { annotations } : {}),
    ...(typeof last === "number" && Number.isSafeInteger(last) && last > 0 ? { lastAnnotationNumber: last } : {}),
    ...(wait ? { reviewWait: wait } : {}),
  };
}

/**
 * The wait of a process for a review of the pane (`viewer-annotations`,
 * Waiting For A Review). The waiting pane's title is untrusted text, shown
 * in the pane: controls and bidirectional formatting characters go.
 */
function reviewWait(value: unknown): ReviewWait | undefined {
  if (!isRecord(value) || !isWaitId(value.id)) return undefined;
  const { paneId } = value;
  if (typeof paneId !== "string" || paneId.length === 0 || paneId.length > 256) return undefined;
  const title = plainText(value.title, REPLY_TITLE_LIMIT).trim();
  return { id: value.id, paneId, title };
}

function viewerSource(raw: Raw): ViewerSource | undefined {
  if (raw.kind === "file") {
    return typeof raw.path === "string" && isViewerPath(raw.path) ? { kind: "file", path: raw.path } : undefined;
  }
  if (raw.kind === "text") {
    if (typeof raw.title !== "string" || !isViewerTitle(raw.title)) return undefined;
    // Any other format shows as plain text.
    return raw.format === "markdown" ? { kind: "text", title: raw.title, format: "markdown" } : { kind: "text", title: raw.title };
  }
  return undefined;
}

function sessionState(id: string, raw: Raw): SessionState {
  const status = raw.status === "running" || raw.status === "exited" ? raw.status : "starting";
  return {
    id,
    ...(typeof raw.jobId === "string" ? { jobId: optionalBounded(raw.jobId, 256) } : {}),
    ...(raw.restartPolicy === "never" ? { restartPolicy: "never" as const } : {}),
    shellProfileId: optionalString(raw.shellProfileId),
    command: optionalBounded(raw.command, MAX_VALUE),
    ...(raw.restoreMode === "replay" || raw.restoreMode === "shell" || raw.restoreMode === "close" ? { restoreMode: raw.restoreMode } : {}),
    cwd: optionalBounded(raw.cwd, MAX_VALUE),
    shell: optionalBounded(raw.shell, MAX_VALUE),
    status,
    exitCode: typeof raw.exitCode === "number" ? raw.exitCode : undefined,
    // The user's own text keeps its characters, like a value that goes back.
    ...(typeof raw.scratchpad === "string" ? { scratchpad: optionalBounded(raw.scratchpad, SCRATCHPAD_LIMIT) } : {}),
  };
}

function jobState(value: unknown): "starting" | "running" | "stopping" | "exited" | "failed" | "interrupted" {
  switch (value) {
    case "starting": case "running": case "stopping": case "exited": case "failed": return value;
    default: return "interrupted";
  }
}
