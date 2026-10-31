import { describe, expect, it } from "vitest";
import { parseKnownHosts } from "../../../../frontend-electron/src/remote.ts";

describe("Electron known host bridge", () => {
  it("passes display aliases through without changing SSH destinations", () => {
    expect(parseKnownHosts(JSON.stringify([
      { target: "me@192.0.2.44", source: "recent", alias: "Home Lab", lastConnected: 42 },
      { target: "work-server", source: "ssh-config", alias: " Work " },
    ]))).toEqual([
      { target: "me@192.0.2.44", source: "recent", alias: "Home Lab", lastConnected: 42 },
      { target: "work-server", source: "ssh-config", alias: "Work" },
    ]);
  });

  it("keeps hosts with absent or invalid aliases and rejects malformed host rows", () => {
    expect(parseKnownHosts(JSON.stringify([
      { target: "vm1", source: "recent" },
      { target: "vm2", source: "ssh-config", alias: null },
      { target: "vm3", source: "recent", alias: 123 },
      { target: "vm4", source: "ssh-config", alias: "   " },
      { target: "vm5", source: "unknown", alias: "Invalid" },
      null,
    ]))).toEqual([
      { target: "vm1", source: "recent" },
      { target: "vm2", source: "ssh-config" },
      { target: "vm3", source: "recent" },
      { target: "vm4", source: "ssh-config" },
    ]);
    expect(parseKnownHosts("not JSON")).toEqual([]);
    expect(parseKnownHosts('{"hosts":[]}')).toEqual([]);
  });
});
