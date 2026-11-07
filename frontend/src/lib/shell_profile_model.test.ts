import { describe, expect, it } from "vitest";
import { chooseDefaultProfile, defaultProfileChoice, systemDefaultLabel, visibleShellProfiles } from "./shell_profile_model.ts";
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
  it("keeps menu order and hides disabled profiles", () => {
    const state: ShellProfilesState = {
      defaultProfileId: "fish",
      profiles: [profile("fish"), profile("bash", false), profile("pwsh", true, false)],
    };
    expect(visibleShellProfiles(state).map((item) => item.id)).toEqual(["fish", "pwsh"]);
    expect(visibleShellProfiles(state)[1].available).toBe(false);
  });

  it("offers the system default and turns a pick into an explicit choice", () => {
    const state: ShellProfilesState = {
      defaultProfileId: "zsh",
      profiles: [profile("bash"), profile("zsh")],
      followSystemDefault: true,
      systemDefaultProfileId: "zsh",
    };
    expect(defaultProfileChoice(state)).toBe("");
    expect(systemDefaultLabel(state)).toBe("System default (zsh)");

    const explicit = chooseDefaultProfile(state, "bash");
    expect(explicit).toMatchObject({ defaultProfileId: "bash", followSystemDefault: false });
    expect(defaultProfileChoice(explicit)).toBe("bash");

    expect(chooseDefaultProfile(explicit, "")).toMatchObject({ defaultProfileId: "zsh", followSystemDefault: true });
  });

  it("keeps explicit choices for servers without a system default", () => {
    const state: ShellProfilesState = { defaultProfileId: "bash", profiles: [profile("bash")] };
    expect(defaultProfileChoice(state)).toBe("bash");
    expect(chooseDefaultProfile(state, "")).toMatchObject({ defaultProfileId: "", followSystemDefault: false });
  });
});
