import type { ShellProfile, ShellProfilesState } from "./shell_profiles.ts";

/** Enabled profiles shown in the new-tab menu, preserving backend order. */
export function visibleShellProfiles(state: ShellProfilesState): ShellProfile[] {
  return state.profiles.filter((profile) => profile.enabled);
}
