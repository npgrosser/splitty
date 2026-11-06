/**
 * How a viewer pane's view was left: its mode, its wrap choice, where
 * reading stood (`viewer-panes`, Keeping A Shown Document Current) and
 * whether its annotation list was open (`viewer-annotations`). A view
 * built again for the same pane starts from it: after a snapshot rebuilt the
 * pane's screen (`layout/screen_snapshot.ts`), after a move to or from a
 * popout window, and on the screen a pane was moved to.
 *
 * One store per window, by pane ID: a pane's renderer keeps its view's state
 * when it is disposed and takes it when the pane's view is built again. The
 * state of a pane that its screen no longer lists is dropped with that
 * screen's next snapshot, and the store holds a bounded number of panes.
 */

export type ViewerMode = "rendered" | "source";

/** The source view's first visible line, or how far a rendered document is scrolled (0 to 1). */
export type ViewerPosition = { readonly line: number } | { readonly fraction: number };

export interface ViewerViewState {
  readonly mode: ViewerMode;
  /** Line wrapping as chosen in the pane; without a choice it follows the document's kind. */
  readonly wrap?: boolean;
  readonly position?: ViewerPosition;
  /**
   * The annotation list as the user left it (`viewer-annotations`, D7);
   * without a choice it is open while the pane holds annotations.
   */
  readonly annotations?: "open" | "closed";
}

/** More than the panes of a screen rebuilt at once; the oldest goes first. */
export const MAX_KEPT_STATES = 64;

const kept = new Map<string, { readonly screen: string; readonly state: ViewerViewState }>();

/** Keep the state of the view of pane `paneId` on the screen `screen` (its binding's key). */
export function keepViewerState(paneId: string, screen: string, state: ViewerViewState): void {
  kept.delete(paneId);
  kept.set(paneId, { screen, state });
  for (const id of kept.keys()) {
    if (kept.size <= MAX_KEPT_STATES) break;
    kept.delete(id);
  }
}

/** The kept state of the view of pane `paneId`; it is no longer kept then. */
export function takeViewerState(paneId: string): ViewerViewState | undefined {
  const entry = kept.get(paneId);
  kept.delete(paneId);
  return entry?.state;
}

/** Drop the kept states of the panes of `screen` that `listed` no longer holds. */
export function dropViewerStates(screen: string, listed: (paneId: string) => boolean): void {
  for (const [paneId, entry] of kept) {
    if (entry.screen === screen && !listed(paneId)) kept.delete(paneId);
  }
}
