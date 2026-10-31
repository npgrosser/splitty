// State of the host picker, kept out of the component so it can be tested
// without rendering: which rows the query shows, and the open's phases.

import type { KnownHost, RemoteOpenError, RemoteProgress } from "./remoteWindows.ts";

/** A row of the picker: a known host, or the query itself as a new target. */
export type HostRow =
  | { kind: "known"; host: KnownHost }
  | { kind: "typed"; target: string };

export function rowTarget(row: HostRow): string {
  return row.kind === "known" ? row.host.target : row.target;
}

/** Known hosts whose target contains the query (case-insensitive), in their order. */
export function filterHosts(hosts: KnownHost[], query: string): KnownHost[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return hosts;
  return hosts.filter((host) => host.target.toLowerCase().includes(needle) || host.alias?.toLowerCase().includes(needle));
}

/**
 * Whether the query can be offered as a target of its own.
 *
 * Only a first sieve for the row: the shell validates the target as
 * `[user@]host[:port]` before anything runs (`access-control`).
 */
export function isPlausibleTarget(query: string): boolean {
  const target = query.trim();
  if (!target || /^[-‐-―−﹘﹣－]/.test(target)) return false;
  // Whitespace and control characters (C0, DEL, C1) are never part of a target.
  return !/[\s\u0000-\u001F\u007F-\u009F]/.test(target);
}

/**
 * The rows for a query: the matching known hosts, then the query as a typed
 * target unless a known host already has exactly that name. The typed row
 * comes last so that Enter while filtering opens the best known match.
 */
export function hostRows(hosts: KnownHost[], query: string): HostRow[] {
  const rows: HostRow[] = filterHosts(hosts, query).map((host) => ({ kind: "known", host }));
  const typed = query.trim();
  if (isPlausibleTarget(typed) && !hosts.some((host) => host.target === typed)) {
    rows.push({ kind: "typed", target: typed });
  }
  return rows;
}

/** The selection after an arrow key, wrapping at both ends. */
export function moveSelection(index: number, delta: number, count: number): number {
  if (count === 0) return 0;
  return (((index + delta) % count) + count) % count;
}

/** "5 minutes ago", "yesterday", … for a recent host's `lastConnected`. */
export function formatLastConnected(lastConnected: number, nowMs: number = Date.now()): string {
  const seconds = Math.round(nowMs / 1000 - lastConnected);
  if (seconds < 60) return "just now";
  const format = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
  const units: [Intl.RelativeTimeFormatUnit, number][] = [
    ["year", 365 * 86400],
    ["month", 30 * 86400],
    ["week", 7 * 86400],
    ["day", 86400],
    ["hour", 3600],
    ["minute", 60],
  ];
  for (const [unit, size] of units) {
    if (seconds >= size) return format.format(-Math.floor(seconds / size), unit);
  }
  return "just now";
}

// ---------------------------------------------------------------------------
// Phases of an open
// ---------------------------------------------------------------------------

export type PickerPhase = "choosing" | "opening" | "failed" | "done";

export interface PickerState {
  phase: PickerPhase;
  /** The target of the current or last open. */
  target: string;
  /**
   * Counts the opens. A result is applied only to the open it belongs to, so a
   * cancelled open that rejects late cannot fail its successor.
   */
  attempt: number;
  /** The progress messages of the current open, oldest first. */
  progress: string[];
  error?: RemoteOpenError;
}

export type PickerEvent =
  | { type: "open"; target: string }
  | { type: "progress"; progress: RemoteProgress }
  | { type: "succeeded"; attempt: number }
  | { type: "failed"; attempt: number; error: RemoteOpenError }
  /** The user cancelled the open in flight. */
  | { type: "cancel" }
  /** Back from an error to the list. */
  | { type: "back" };

export const initialPickerState: PickerState = { phase: "choosing", target: "", attempt: 0, progress: [] };

export function pickerReducer(state: PickerState, event: PickerEvent): PickerState {
  switch (event.type) {
    case "open":
      // One open at a time; a retry is an open from the failed phase.
      if (state.phase !== "choosing" && state.phase !== "failed") return state;
      return { phase: "opening", target: event.target, attempt: state.attempt + 1, progress: [] };
    case "progress": {
      const { target, message } = event.progress;
      if (state.phase !== "opening" || target !== state.target) return state;
      if (state.progress[state.progress.length - 1] === message) return state;
      return { ...state, progress: [...state.progress, message] };
    }
    case "succeeded":
      if (state.phase !== "opening" || event.attempt !== state.attempt) return state;
      return { ...state, phase: "done" };
    case "failed":
      if (state.phase !== "opening" || event.attempt !== state.attempt) return state;
      return { ...state, phase: "failed", error: event.error };
    case "cancel":
      if (state.phase !== "opening") return state;
      return { ...state, phase: "choosing", progress: [] };
    case "back":
      if (state.phase !== "failed") return state;
      return { ...state, phase: "choosing", progress: [], error: undefined };
  }
}
