import type { ShellProfile, ShellProfilesState } from "./shell_profiles.ts";

/** Enabled profiles shown in the new-tab menu, preserving backend order. */
export function visibleShellProfiles(state: ShellProfilesState): ShellProfile[] {
  return state.profiles.filter((profile) => profile.enabled);
}

/** Whether the server can follow the system default (older servers cannot). */
export function supportsSystemDefault(state: ShellProfilesState): boolean {
  return typeof state.systemDefaultProfileId === "string" && state.systemDefaultProfileId !== "";
}

/** The default choice for the settings select: "" while following the system default. */
export function defaultProfileChoice(state: ShellProfilesState): string {
  return state.followSystemDefault && supportsSystemDefault(state) ? "" : state.defaultProfileId;
}

/** Apply a default choice: "" follows the system default, an id is an explicit choice. */
export function chooseDefaultProfile(state: ShellProfilesState, choice: string): ShellProfilesState {
  if (choice === "" && supportsSystemDefault(state)) {
    return { ...state, followSystemDefault: true, defaultProfileId: state.systemDefaultProfileId ?? "" };
  }
  return { ...state, followSystemDefault: false, defaultProfileId: choice };
}

/** Label of the system default choice, naming the shell it currently resolves to. */
export function systemDefaultLabel(state: ShellProfilesState): string {
  const profile = state.profiles.find((candidate) => candidate.id === state.systemDefaultProfileId);
  return profile ? `System default (${profile.name})` : "System default";
}
