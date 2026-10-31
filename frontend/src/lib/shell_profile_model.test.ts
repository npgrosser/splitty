import { describe, expect, it } from "vitest";
import { validDefaultProfileId, visibleShellProfiles } from "./shell_profile_model.ts";
import type { ShellProfile, ShellProfilesState } from "./shell_profiles.ts";

function profile(id: string, enabled = true, available = true): ShellProfile {
  return {
    id,
    name: id,
    command: id,
    args: [],
    enabled,
    available,
    target: "local",
    source: "detected",
    experimental: false,
  };
}

describe("shell profile model", () => {
  it("falls back to the first enabled available profile", () => {
    const state: ShellProfilesState = {
      defaultProfileId: "zsh",
      profiles: [profile("zsh", false), profile("missing", true, false), profile("fish")],
    };
    expect(validDefaultProfileId(state)).toBe("fish");
  });

  it("keeps menu order and hides disabled profiles", () => {
    const state: ShellProfilesState = {
      defaultProfileId: "fish",
      profiles: [profile("fish"), profile("bash", false), profile("pwsh", true, false)],
    };
    expect(visibleShellProfiles(state).map((item) => item.id)).toEqual(["fish", "pwsh"]);
    expect(visibleShellProfiles(state)[1].available).toBe(false);
  });
});
