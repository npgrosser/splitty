import { validateTarget } from "./api.ts";
import { forwardLabel, isPort, MAX_FORWARD_LABEL } from "./ports.ts";

export interface ManualPortInput {
  target: string;
  remote: string;
  local: string;
  remember: boolean;
  label: string;
}

export interface ManualPort {
  target: string;
  port: number;
  local?: number;
  remember: boolean;
  label?: string;
}

/** Accept decimal port numbers, never URL fragments, exponent notation or truncated input. */
export function portNumber(input: string): number | null {
  const text = input.trim();
  const port = /^\d+$/.test(text) ? Number(text) : NaN;
  return isPort(port) ? port : null;
}

export function manualPort(input: ManualPortInput): { value: ManualPort; error?: never } | { error: string; value?: never } {
  const target = validateTarget(input.target);
  if (!target) return { error: "Enter an SSH target such as user@host or an SSH alias." };
  const port = portNumber(input.remote);
  if (port === null) return { error: "Remote port must be a whole number from 1 to 65535." };
  const local = input.local.trim() === "" ? undefined : portNumber(input.local);
  if (local === null) return { error: "Local port must be a whole number from 1 to 65535, or empty." };
  if (input.remember && [...input.label].length > MAX_FORWARD_LABEL) {
    return { error: `Label must be at most ${MAX_FORWARD_LABEL} characters.` };
  }
  return { value: { target, port, ...(local === undefined ? {} : { local }), remember: input.remember,
    ...(input.remember ? { label: forwardLabel(input.label) } : {}) } };
}
