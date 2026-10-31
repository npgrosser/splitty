import { describe, expect, it } from "vitest";
import { updateImpact, updateNote, updateWarning } from "./update.ts";

describe("updateImpact", () => {
  it("counts the host's screens and panes from its last remote.ready and the window's layouts", () => {
    const lastReady = [{ id: "s1", name: "api", panes: 2 }, { id: "s2", name: "infra", panes: 1 }];
    // The window shows s1 with 4 panes now, and s3, created since.
    expect(updateImpact(lastReady, [{ wireId: "s1", panes: 4 }, { wireId: "s3", panes: 1 }]))
      .toEqual({ known: true, screens: 3, panes: 6 });
    expect(updateImpact([], [])).toEqual({ known: true, screens: 0, panes: 0 });
    // A shown screen without a layout keeps the host's count; one the host did not list is missing there.
    expect(updateImpact(lastReady, [{ wireId: "s1" }, { wireId: "s4" }])).toEqual({ known: true, screens: 2, panes: 3 });
  });

  it("is unknown without a remote.ready, and says how many screens the window shows", () => {
    expect(updateImpact(undefined, [{ wireId: "s1", panes: 2 }, { wireId: "s2" }])).toEqual({ known: false, shown: 2 });
  });
});

describe("updateWarning", () => {
  it("names the versions and what ends on the host", () => {
    const warning = updateWarning("vm1", { hostVersion: "0.3.9", hostProtocol: 1, localVersion: "0.4.2" },
      { known: true, screens: 2, panes: 5 });
    expect(warning).toEqual({
      title: "Update Splitty on vm1?",
      versions: "vm1 runs Splitty 0.3.9 (protocol 1). Updating stops it and starts Splitty 0.4.2, the version of this app.",
      ends: "Updating ends 2 screens with 5 panes on vm1, and every process running in them.",
    });
    expect(updateWarning("vm1", {}, { known: true, screens: 1, panes: 1 }).ends)
      .toBe("Updating ends 1 screen with 1 pane on vm1, and every process running in them.");
    expect(updateWarning("vm1", {}, { known: true, screens: 0, panes: 0 }).ends)
      .toBe("No screens run on vm1 now; updating ends nothing there.");
  });

  it("says so when the count is not known", () => {
    const warning = updateWarning("vm1", { hostVersion: "9.0.0", hostProtocol: 7 }, { known: false, shown: 2 });
    expect(warning.versions).toBe("vm1 runs Splitty 9.0.0 (protocol 7). Updating stops it and starts the version of this app.");
    expect(warning.ends).toBe(
      "Updating ends every screen on vm1 and every process running in them. "
      + "How many there are is not known: this Splitty on vm1 cannot report them to this app. "
      + "This window shows 2 of them.",
    );
    expect(updateWarning("vm1", {}, { known: false, shown: 0 }).ends).not.toContain("This window shows");
  });
});

describe("updateWarning for a daemon that keeps its sessions", () => {
  it("says they keep running and names nothing that ends", () => {
    const warning = updateWarning("vm1", { hostVersion: "0.1.17", hostProtocol: 1, keepsSessions: true, localVersion: "0.1.18" },
      { known: true, screens: 2, panes: 5 });
    expect(warning.ends).toBe(
      "The sessions on vm1 keep running, with every process in them: "
      + "updating restarts its Splitty, and the connection drops for a moment.",
    );
    expect(warning.ends).not.toContain("screens");
    // Unknown counts do not matter then.
    expect(updateWarning("vm1", { keepsSessions: true }, { known: false, shown: 2 }).ends).toContain("keep running");
    // A daemon that does not say keeps none.
    expect(updateWarning("vm1", { keepsSessions: false }, { known: true, screens: 1, panes: 1 }).ends)
      .toBe("Updating ends 1 screen with 1 pane on vm1, and every process running in them.");
  });
});

describe("updateNote", () => {
  it("is there only when both versions are known and differ", () => {
    expect(updateNote("0.4.1", "0.4.2")).toBe("update to 0.4.2 available");
    expect(updateNote("0.4.2", "0.4.2")).toBeUndefined();
    expect(updateNote(undefined, "0.4.2")).toBeUndefined();
    expect(updateNote("0.4.1", undefined)).toBeUndefined();
  });
});
