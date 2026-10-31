import { apiFetch } from "./protocol/http.ts";

export type ShellTarget = "local" | "windowsInterop";
export type ShellProfileSource = "detected" | "custom";

export interface DetectedProfileDefaults {
  name: string;
  command: string;
  args: string[];
  target: ShellTarget;
}

export interface ShellProfile {
  id: string;
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  target: ShellTarget;
  source: ShellProfileSource;
  available: boolean;
  reason?: string;
  experimental: boolean;
  detectedDefaults?: DetectedProfileDefaults;
}

export interface ShellProfilesState {
  defaultProfileId: string;
  profiles: ShellProfile[];
}

let current: ShellProfilesState = { defaultProfileId: "", profiles: [] };
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

export async function loadShellProfiles(): Promise<ShellProfilesState> {
  try {
    const response = await apiFetch("/api/shell-profiles");
    if (response.ok) {
      current = await response.json() as ShellProfilesState;
      notify();
    }
  } catch {
    // The legacy create request still lets older servers choose their shell.
  }
  return current;
}

export function getShellProfiles(): ShellProfilesState {
  return current;
}

export function getDefaultShellProfileId(): string | undefined {
  return current.defaultProfileId || undefined;
}

export function getLaunchableProfile(id?: string): ShellProfile | undefined {
  const requested = id
    ? current.profiles.find((profile) => profile.id === id)
    : current.profiles.find((profile) => profile.id === current.defaultProfileId);
  if (requested?.enabled && requested.available) return requested;
  return current.profiles.find((profile) => profile.enabled && profile.available);
}

export function subscribeShellProfiles(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export async function saveShellProfiles(
  state: ShellProfilesState,
): Promise<ShellProfilesState> {
  const response = await apiFetch("/api/shell-profiles", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(state),
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({})) as { error?: string };
    throw new Error(body.error ?? `Failed to save shell profiles (${response.status})`);
  }
  current = await response.json() as ShellProfilesState;
  notify();
  return current;
}

export function shellProfileIcon(profile: ShellProfile): string {
  const executable = profile.command.split(/[\\/]/).pop()?.toLowerCase() ?? "";
  if (executable.includes("powershell") || executable.startsWith("pwsh")) return "❯_";
  if (executable === "fish") return "><>";
  if (executable === "zsh") return "%";
  if (executable === "bash") return "$";
  return ">_";
}
