import { describe, expect, it } from "vitest";
import { migrationOffer, updateAvailability, updateNote, updateOffered, updateWarning } from "./update.ts";

describe("updateAvailability", () => {
  it("keeps the hub's plan and its version", () => {
    expect(updateAvailability({ available: true, keepsSessions: true, toVersion: "0.4.0" }))
      .toEqual({ available: true, keepsSessions: true, toVersion: "0.4.0" });
    expect(updateAvailability({ available: false, keepsSessions: true })).toEqual({ available: false, keepsSessions: true });
    for (const toVersion of ["", "x".repeat(65), "0.4.0\u001b[31m", 4]) {
      expect(updateAvailability({ available: true, keepsSessions: true, toVersion })).toEqual({ available: true, keepsSessions: true });
    }
    expect(updateAvailability({ available: "true", keepsSessions: true })).toBeUndefined();
  });

  it("offers an update only when it keeps the sessions", () => {
    expect(updateOffered({ available: true, keepsSessions: true })).toBe(true);
    expect(updateOffered({ available: true, keepsSessions: false })).toBe(false);
    expect(updateOffered({ available: false, keepsSessions: true })).toBe(false);
    expect(updateOffered(undefined)).toBe(false);
  });
});

describe("updateWarning", () => {
  it("states the verified keeper count and makes migration explicit", () => {
    const migration = { available: true as const, fromVersion: "0.1.27", toVersion: "0.2.0-dev.2", sessions: 3 };
    const warning = updateWarning("vm1", { migration });
    expect(warning.title).toBe("Update server and connect to vm1?");
    expect(warning.action).toBe("Update server and connect");
    expect(warning.versions).toContain("0.1.27");
    expect(warning.versions).toContain("0.2.0-dev.2");
    expect(warning.ends).toContain("3 keeper-backed sessions checked on vm1 keep running");
    expect(warning.ends).toContain("If the server can no longer be verified, the update stops");
  });

  it("names the hub's version and says that the sessions keep running", () => {
    const warning = updateWarning("vm1", { hostVersion: "0.3.9", hostProtocol: 1, toVersion: "0.4.0", localVersion: "0.4.2" });
    expect(warning).toEqual({
      title: "Update Splitty on vm1?",
      versions: "vm1 runs Splitty 0.3.9 (protocol 1). Updating stops it and starts Splitty 0.4.0.",
      ends: "The sessions on vm1 keep running, with every process in them: "
        + "updating restarts its Splitty, and the connection drops for a moment.",
    });
    expect(warning.ends).not.toContain("screen");
    expect(warning.ends).not.toContain("pane");
  });

  it("names the local version for a hub without one", () => {
    expect(updateWarning("vm1", { hostVersion: "0.3.9", localVersion: "0.4.2" }).versions)
      .toBe("vm1 runs Splitty 0.3.9. Updating stops it and starts Splitty 0.4.2, the version of this app.");
    expect(updateWarning("vm1", {}).versions).toBe("vm1 runs another Splitty. Updating stops it and starts the version of this app.");
  });
});

describe("migrationOffer", () => {
  const offer = { available: true, fromVersion: "0.1.27", toVersion: "0.2.0-dev.2", sessions: 0 };
  it("keeps only bounded descriptive metadata, never migration authority", () => {
    expect(migrationOffer({ ...offer, token: "secret", pid: 10 })).toEqual(offer);
    for (const value of [undefined, { ...offer, available: false }, { ...offer, sessions: -1 },
      { ...offer, sessions: 0.5 }, { ...offer, sessions: 2 ** 32 }, { ...offer, fromVersion: "x".repeat(65) }]) {
      expect(migrationOffer(value)).toBeUndefined();
    }
  });
});

describe("updateNote", () => {
  it("requires a positive hub decision, never just differing versions", () => {
    expect(updateNote("0.4.1", "0.4.2", {available: true, keepsSessions: true})).toBe("update to 0.4.2 available");
    expect(updateNote("0.4.1", "0.4.2", {available: true, keepsSessions: true, toVersion: "0.4.3"})).toBe("update to 0.4.3 available");
    expect(updateNote("0.4.1", "0.4.2", {available: true, keepsSessions: false})).toBeUndefined();
    expect(updateNote("0.5.0", "0.4.2", {available: false, keepsSessions: true})).toBeUndefined();
    expect(updateNote("0.4.1", "0.4.2")).toBeUndefined();
    expect(updateNote("0.4.2", "0.4.2")).toBeUndefined();
    expect(updateNote(undefined, "0.4.2")).toBeUndefined();
    expect(updateNote("0.4.1", undefined)).toBeUndefined();
  });
});
