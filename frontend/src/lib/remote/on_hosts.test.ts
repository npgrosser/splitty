import { describe, expect, it } from "vitest";
import { hostGroups, hostNeeded, type HostConnectionView } from "./on_hosts.ts";
import type { RemoteState } from "./remote_socket.ts";
import type { ScreenRef } from "../connection/routing.ts";
import type { HostCatalog } from "./host_directory.ts";

const state = (phase: RemoteState["phase"]): RemoteState => ({ phase, progress: [], attempt: 0 });
const screen = (id: string, conn: ScreenRef["conn"], wireId = id): ScreenRef => ({ id, conn, wireId });
const infra = { id: "s2", name: "infra", panes: 3 };
const cache: HostCatalog = { target: "vm1", lastSeen: 100, screens: [infra] };
const ready: HostConnectionView = { key: "host:vm1", state: state("ready"), available: [infra] };

describe("persistent host groups", () => {
  it("keeps a recent host and its last seen screens after the last detach", () => {
    const [group] = hostGroups([screen("L", "local")], [], [{ target: "vm1", source: "recent" }], [cache]);
    expect(group).toMatchObject({ key: "host:vm1", open: [], screens: [infra], stale: true, lastSeen: 100 });
  });
  it("shows a recent host even without a cached directory, but does not list every SSH alias", () => {
    const groups = hostGroups([], [], [{ target: "vm1", source: "recent" }, { target: "vm2", source: "ssh-config" }], []);
    expect(groups.map((g) => g.target)).toEqual(["vm1"]);
    expect(groups[0].screens).toEqual([]);
  });
  it("keeps an explicitly connected host with no open screens and no recent entry yet", () => {
    expect(hostGroups([], [ready], [], [])[0]).toMatchObject({ target: "vm1", screens: [infra], stale: false });
  });
  it("uses a successful empty directory instead of bringing old cached screens back", () => {
    expect(hostGroups([], [{ ...ready, available: [] }], [], [cache])[0].screens).toEqual([]);
  });
  it("keeps cached screens during a failed connection", () => {
    expect(hostGroups([], [{ ...ready, state: state("failed"), available: [] }], [], [cache])[0])
      .toMatchObject({ stale: true, screens: [infra], state: { phase: "failed" } });
  });
  it("excludes only the owning host's open wire ids", () => {
    const groups = hostGroups([screen("a", "host:vm1", "s2"), screen("b", "host:vm2", "s1")], [], [], [cache, { ...cache, target: "vm2" }]);
    expect(groups[0].screens).toEqual([]);
    expect(groups[1].screens).toEqual([infra]);
  });
  it("orders open hosts first then recent and cached hosts without duplicates", () => {
    const groups = hostGroups([screen("a", "host:vm2"), screen("b", "host:vm1")], [ready],
      [{ target: "vm3", source: "recent" }, { target: "vm1", source: "recent" }], [cache]);
    expect(groups.map((g) => g.target)).toEqual(["vm2", "vm1", "vm3"]);
  });
  it("lists the hosts the user arranged first, in that order", () => {
    const groups = hostGroups([screen("a", "host:vm2"), screen("b", "host:vm1")], [ready],
      [{ target: "vm3", source: "recent" }], [cache], [], [], ["vm3", "vm1"]);
    expect(groups.map((g) => g.target)).toEqual(["vm3", "vm1", "vm2"]);
  });
});

describe("host connection interest", () => {
  it("does not connect merely because cached hosts exist", () => {
    expect(hostNeeded("host:vm1", [], new Set())).toBe(false);
  });
  it("retains a browse with no open screen and releases it when folded", () => {
    const browsing = new Set<ScreenRef["conn"]>(["host:vm1"]);
    expect(hostNeeded("host:vm1", [], browsing)).toBe(true);
    browsing.clear();
    expect(hostNeeded("host:vm1", [], browsing)).toBe(false);
  });
  it("folding a group does not disconnect its attached screens or another host", () => {
    expect(hostNeeded("host:vm1", [screen("a", "host:vm1")], new Set())).toBe(true);
    expect(hostNeeded("host:vm1", [screen("b", "host:vm2")], new Set(["host:vm2"]))).toBe(false);
  });
});

it("keeps hosts with only port mappings visible without creating connection interest", () => {
  const groups = hostGroups([], [], [], [], [{ target: "saved-vm" }, { target: "saved-vm" }]);
  expect(groups).toHaveLength(1);
  expect(groups[0]).toMatchObject({ target: "saved-vm", stale: true, open: [], screens: [] });
  expect(hostNeeded(groups[0].key, [], new Set())).toBe(false);
});

it("keeps display aliases and hidden status separate from host identity and screen routing", () => {
  const [group] = hostGroups([screen("a", "host:vm1")], [ready], [], [], [], [{ target: "vm1", alias: "Work VM", hidden: true }]);
  expect(group).toMatchObject({ target: "vm1", key: "host:vm1", alias: "Work VM", hidden: true });
  expect(group.open).toHaveLength(1);
  expect(hostGroups([], [], [], [], [], [{ target: "vm2", alias: "Other" }])[0].target).toBe("vm2");
  expect(hostGroups([], [], [], [], [], [{ target: "vm2", hidden: true }])).toEqual([]);
});

it("clears an old known-host alias when preferences explicitly remove it", () => {
  const [group] = hostGroups([], [], [{ target: "vm1", source: "recent", alias: "Old" }], [], [], [{ target: "vm1" }]);
  expect(group.alias).toBeUndefined();
});
