import type { ScreenState, LayoutNode } from "../lib/protocol/types.ts";

export function jobParams(screen: ScreenState, paneId: string) {
  const pane = screen.panes[paneId];
  const job = pane?.jobId ? screen.jobs?.[pane.jobId] : undefined;
  return { jobId: pane?.jobId, jobState: job?.state, jobError: job?.error };
}

export function jobCanAttach(state?: string): boolean {
  return state === "running" || state === "stopping" || state === "exited";
}

/** Compare surviving topology, ignoring newly inserted/removed jobs and sizes. */
export function topology(node: LayoutNode, omit: Set<string>): unknown {
  if (node.type === "slot") {
    const panes = (node.paneIds ?? []).filter(id => !omit.has(id));
    return panes.length ? { panes } : null;
  }
  const children = (node.children ?? []).map(n => topology(n, omit)).filter(Boolean);
  return children.length === 1 ? children[0] : children.length ? { axis: node.axis, children } : null;
}
