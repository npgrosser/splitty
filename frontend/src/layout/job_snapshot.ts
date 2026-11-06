import type { ScreenState } from "../lib/protocol/types.ts";

export function jobParams(screen: ScreenState, paneId: string) {
  const pane = screen.panes[paneId];
  const job = pane?.jobId ? screen.jobs?.[pane.jobId] : undefined;
  return { jobId: pane?.jobId, jobState: job?.state, jobError: job?.error };
}

export function jobCanAttach(state?: string): boolean {
  return state === "running" || state === "stopping" || state === "exited";
}
