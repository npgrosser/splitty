import { jobCanAttach } from "../../layout/job_snapshot.ts";

interface JobMountParams {
  jobId?: string;
  jobState?: string;
  jobError?: string | null;
  existingSessionId?: string;
}

/** A managed pane can attach or show an outcome, never request a new shell. */
export function mountManagedJob<T>(params: JobMountParams, actions: {
  attach: (sessionId: string) => Promise<T>;
  showOutcome: (text: string) => void;
}): { promise?: Promise<T> } | undefined {
  if (!params.jobId) return undefined;
  if (jobCanAttach(params.jobState) && params.existingSessionId) {
    return { promise: actions.attach(params.existingSessionId) };
  }
  actions.showOutcome(`${params.jobState ?? "Job unavailable"}${typeof params.jobError === "string" ? `: ${params.jobError.slice(0, 2000)}` : ""}`);
  return {};
}
