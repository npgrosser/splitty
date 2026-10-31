import type { ShellProfile, ShellProfilesState } from "./shell_profiles.ts";

/** Enabled profiles shown in the new-tab menu, preserving backend order. */
export function visibleShellProfiles(state: ShellProfilesState): ShellProfile[] {
  return state.profiles.filter((profile) => profile.enabled);
}

/** Pick a startable default after disabling or deleting the old default. */
export function validDefaultProfileId(state: ShellProfilesState): string {
  const current = state.profiles.find((profile) => profile.id === state.defaultProfileId);
  if (current?.enabled && current.available) return current.id;
  return state.profiles.find((profile) => profile.enabled && profile.available)?.id ?? "";
}
