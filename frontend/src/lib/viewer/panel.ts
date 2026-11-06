/**
 * Viewer panes as dockview panels (`viewer-panes`).
 *
 * A panel is a viewer pane by its component, never by its id: the window's own
 * viewer panes are `viewer-…`, those the CLI opens are `pane-…` like any
 * other pane the server names.
 */

import type { DockviewApi, IDockviewPanel } from "dockview-core";
import type { PaneState, ViewerRecord, ViewerSource } from "../protocol/types.ts";
import { directoryOf, fileName } from "./presentation.ts";

/** The dockview component of viewer panes. */
export const VIEWER_COMPONENT = "viewer";

// A type, not an interface: dockview's panel params are an index signature.
export type ViewerPanelParams = {
  readonly viewer: {
    /** Missing or malformed in the snapshot: the pane says it cannot be shown. */
    readonly record?: ViewerRecord;
    /** Whether the pane's server announced viewer panes. */
    readonly supported: boolean;
    /** The pane's working directory as the server has it. */
    readonly cwd?: string;
  };
};

/** The viewer part of a panel's params, or undefined for any other panel. */
export function viewerParams(params: unknown): ViewerPanelParams["viewer"] | undefined {
  const viewer = (params as Partial<ViewerPanelParams> | undefined)?.viewer;
  return viewer && typeof viewer === "object" ? viewer : undefined;
}

interface PanelLike {
  readonly params?: unknown;
  readonly view?: { readonly contentComponent?: string };
}

export function isViewerPanel(panel: PanelLike | undefined): boolean {
  return panel?.view?.contentComponent === VIEWER_COMPONENT;
}

/** The record of a viewer panel. */
export function viewerRecordOf(panel: PanelLike | undefined): ViewerRecord | undefined {
  return isViewerPanel(panel) ? viewerParams(panel?.params)?.record : undefined;
}

/** The file a viewer panel shows, if it shows one. */
export function viewerFileOf(panel: PanelLike | undefined): string | undefined {
  const source = viewerRecordOf(panel)?.source;
  return source?.kind === "file" ? source.path : undefined;
}

/** A viewer pane's directory: its working directory, or its file's. */
export function viewerDirectoryOf(panel: PanelLike | undefined): string | undefined {
  if (!isViewerPanel(panel)) return undefined;
  const viewer = viewerParams(panel?.params);
  const file = viewerFileOf(panel);
  return viewer?.cwd ?? (file ? directoryOf(file) : undefined);
}

/** Whether two records of one pane name the same document: the same file, or a text of the same title. */
export function sameViewerSource(a: ViewerSource | undefined, b: ViewerSource | undefined): boolean {
  if (!a || !b) return false;
  if (a.kind === "file") return b.kind === "file" && a.path === b.path;
  return b.kind === "text" && a.title === b.title;
}

/** The title of a viewer pane: its file's name or its text's title. */
export function viewerPaneTitle(record: ViewerRecord | undefined): string {
  if (!record) return "Viewer";
  return record.source.kind === "file" ? fileName(record.source.path) : record.source.title;
}

/**
 * The params of a snapshot's viewer pane. Its annotations and their counter
 * stay with the connection's store (`annotations.ts`): a change of them alone
 * leaves the params as they are, so it never touches the layout. A wait for a
 * review stays in the record, whose banner the pane shows.
 */
export function viewerPanelParams(pane: PaneState, supported: boolean): ViewerPanelParams {
  const record = pane.viewer ? panelRecord(pane.viewer) : undefined;
  return { viewer: { ...(record ? { record } : {}), supported, ...(pane.cwd ? { cwd: pane.cwd } : {}) } };
}

/** A viewer record without its annotations and their counter. */
function panelRecord(record: ViewerRecord): ViewerRecord {
  if (!record.annotations && record.lastAnnotationNumber === undefined) return record;
  const { annotations: _annotations, lastAnnotationNumber: _last, ...rest } = record;
  return rest;
}

/**
 * The viewer panel that a viewer opened beside the terminal `terminalId` joins
 * as a tab, as the server groups the CLI's viewers per terminal
 * (`viewer-panes`, Opening A Viewer): the first panel in layout order whose
 * origin is that terminal. `order` holds pane ids in layout order; panels it
 * does not hold, such as those added since, rank after it in the order given.
 */
export function firstViewerOf<P extends PanelLike & { readonly id: string }>(
  terminalId: string,
  panels: readonly P[],
  order: readonly string[] = [],
): P | undefined {
  const rank = new Map<string, number>();
  order.forEach((id, index) => { if (!rank.has(id)) rank.set(id, index); });
  let first: P | undefined;
  let firstRank = Infinity;
  panels.forEach((panel, index) => {
    if (viewerRecordOf(panel)?.originPaneId !== terminalId) return;
    const position = rank.get(panel.id) ?? order.length + index;
    if (position < firstRank) {
      first = panel;
      firstRank = position;
    }
  });
  return first;
}

/** The viewer panel of the screen whose layout is `api` that shows `path`. */
export function findViewerPanel(api: DockviewApi, path: string): IDockviewPanel | undefined {
  return api.panels.find((panel) => viewerFileOf(panel) === path);
}
