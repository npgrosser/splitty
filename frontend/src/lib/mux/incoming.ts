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
  FloatingEntry, LayoutNode, PaneState, ScreenInfo, ScreenState, SessionState,
} from "../protocol/types.ts";
import { MAX_NAME, MAX_TITLE, MAX_VALUE, optionalBounded, optionalText } from "../protocol/untrusted.ts";
import { SCRATCHPAD_LIMIT } from "../scratchpad.ts";

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
  return {
    id,
    ...(typeof raw.jobId === "string" ? { jobId: optionalBounded(raw.jobId, 256) } : {}),
    sessionId: optionalString(raw.sessionId),
    title: optionalText(raw.title, MAX_TITLE),
    cwd: optionalBounded(raw.cwd, MAX_VALUE),
    role,
  };
}

function sessionState(id: string, raw: Raw): SessionState {
  const status = raw.status === "running" || raw.status === "exited" ? raw.status : "starting";
  return {
    id,
    ...(typeof raw.jobId === "string" ? { jobId: optionalBounded(raw.jobId, 256) } : {}),
    ...(raw.restartPolicy === "never" ? { restartPolicy: "never" as const } : {}),
    shellProfileId: optionalString(raw.shellProfileId),
    command: optionalBounded(raw.command, MAX_VALUE),
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
