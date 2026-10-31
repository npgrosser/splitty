import { describe, it, expect } from "vitest";
import type { KnownHost } from "./remoteWindows.ts";
import {
  filterHosts,
  formatLastConnected,
  hostRows,
  initialPickerState,
  isPlausibleTarget,
  moveSelection,
  pickerReducer,
  rowTarget,
  type PickerEvent,
  type PickerState,
} from "./host_picker.ts";

const hosts: KnownHost[] = [
  { target: "vm1", source: "recent", lastConnected: 1_790_000_000 },
  { target: "me@build-box", source: "recent", lastConnected: 1_780_000_000 },
  { target: "api.devpod", source: "ssh-config" },
  { target: "VM2", source: "ssh-config" },
];

describe("filterHosts", () => {
  it("keeps every host, in order, for an empty query", () => {
    expect(filterHosts(hosts, "  ")).toEqual(hosts);
  });

  it("matches a part of the target, ignoring case", () => {
    expect(filterHosts(hosts, "vm").map((h) => h.target)).toEqual(["vm1", "VM2"]);
    expect(filterHosts(hosts, "BUILD").map((h) => h.target)).toEqual(["me@build-box"]);
    expect(filterHosts(hosts, "nothing")).toEqual([]);
  });
});

describe("hostRows", () => {
  it("offers the query as a target after the matching hosts", () => {
    const rows = hostRows(hosts, "vm");

    expect(rows.map(rowTarget)).toEqual(["vm1", "VM2", "vm"]);
    expect(rows[2].kind).toBe("typed");
  });

  it("offers a typed user@host:port that matches no known host", () => {
    expect(hostRows(hosts, " root@vm1:2222 ")).toEqual([{ kind: "typed", target: "root@vm1:2222" }]);
  });

  it("does not repeat a known host as a typed row", () => {
    expect(hostRows(hosts, "vm1").map((row) => row.kind)).toEqual(["known"]);
  });

  /// access-control: the shell rejects these anyway; the picker does not
  /// suggest them as targets in the first place.
  it("does not offer an option-like or malformed query", () => {
    expect(hostRows(hosts, "-oProxyCommand=x")).toEqual([]);
    expect(hostRows(hosts, "–oProxyCommand=x")).toEqual([]);
    expect(hostRows(hosts, "vm 1")).toEqual([]);
  });
});

describe("isPlausibleTarget", () => {
  it("accepts [user@]host[:port]", () => {
    for (const target of ["vm1", "me@vm1", "vm1:2222", "me@[::1]:22", "api.devpod"]) {
      expect(isPlausibleTarget(target)).toBe(true);
    }
  });

  it("rejects empty, dash-led and whitespace or control character targets", () => {
    for (const target of ["", "-x", "−x", "－x", "a\tb", "a\u0007b", "a\u0085b"]) {
      expect(isPlausibleTarget(target)).toBe(false);
    }
  });
});

describe("moveSelection", () => {
  it("wraps at both ends", () => {
    expect(moveSelection(0, -1, 3)).toBe(2);
    expect(moveSelection(2, 1, 3)).toBe(0);
    expect(moveSelection(1, 1, 3)).toBe(2);
    expect(moveSelection(0, 1, 0)).toBe(0);
  });
});

describe("formatLastConnected", () => {
  const now = 1_790_000_000_000;

  it("says how long ago, in the largest whole unit", () => {
    expect(formatLastConnected(now / 1000 - 10, now)).toBe("just now");
    expect(formatLastConnected(now / 1000 - 5 * 60, now)).toBe("5 minutes ago");
    expect(formatLastConnected(now / 1000 - 3 * 3600, now)).toBe("3 hours ago");
    expect(formatLastConnected(now / 1000 - 86400, now)).toBe("yesterday");
    expect(formatLastConnected(now / 1000 - 15 * 86400, now)).toBe("2 weeks ago");
  });

  it("does not report a clock skew as the future", () => {
    expect(formatLastConnected(now / 1000 + 120, now)).toBe("just now");
  });
});

describe("pickerReducer", () => {
  const run = (events: PickerEvent[], from: PickerState = initialPickerState) =>
    events.reduce(pickerReducer, from);
  const progress = (target: string, message: string): PickerEvent =>
    ({ type: "progress", progress: { target, step: "install", message } });

  it("collects the progress of the open in flight", () => {
    const state = run([
      { type: "open", target: "vm1" },
      progress("vm1", "Installing Splitty on vm1"),
      progress("vm1", "Installing Splitty on vm1"),
      progress("vm2", "Another host's progress"),
      progress("vm1", "Started a Splitty daemon on vm1"),
    ]);

    expect(state).toMatchObject({ phase: "opening", target: "vm1", attempt: 1 });
    expect(state.progress).toEqual(["Installing Splitty on vm1", "Started a Splitty daemon on vm1"]);
  });

  it("goes from progress to an error, and a retry starts a new attempt", () => {
    const failed = run([
      { type: "open", target: "vm1" },
      progress("vm1", "Installing Splitty on vm1"),
      { type: "failed", attempt: 1, error: { message: "Host key verification failed.", hint: "Run `ssh vm1` once." } },
    ]);

    expect(failed).toMatchObject({ phase: "failed", target: "vm1", error: { hint: "Run `ssh vm1` once." } });
    expect(failed.progress).toEqual(["Installing Splitty on vm1"]);

    const retried = run([{ type: "open", target: "vm1" }], failed);

    expect(retried).toMatchObject({ phase: "opening", target: "vm1", attempt: 2, progress: [] });
    expect(retried.error).toBeUndefined();
    expect(run([{ type: "succeeded", attempt: 2 }], retried).phase).toBe("done");
  });

  it("cancels back to the list and ignores the cancelled open's late result", () => {
    const cancelled = run([{ type: "open", target: "vm1" }, { type: "cancel" }]);

    expect(cancelled.phase).toBe("choosing");

    const reopened = run([
      { type: "open", target: "vm1" },
      { type: "failed", attempt: 1, error: { message: "cancelled" } },
    ], cancelled);

    expect(reopened).toMatchObject({ phase: "opening", attempt: 2 });
  });

  it("runs one open at a time", () => {
    const state = run([{ type: "open", target: "vm1" }, { type: "open", target: "vm2" }]);

    expect(state).toMatchObject({ target: "vm1", attempt: 1 });
  });

  it("goes back from an error to the list", () => {
    const state = run([
      { type: "open", target: "vm1" },
      { type: "failed", attempt: 1, error: { message: "failed" } },
      { type: "back" },
    ]);

    expect(state).toMatchObject({ phase: "choosing", progress: [] });
    expect(state.error).toBeUndefined();
  });
});

it("finds a display alias and still selects the exact SSH target", () => {
  const hosts = [{ target: "me@192.0.2.44", source: "recent" as const, alias: "Home Lab" }];
  const rows = hostRows(hosts, "home lab");
  expect(rows).toHaveLength(1);
  expect(rowTarget(rows[0])).toBe("me@192.0.2.44");
  expect(hostRows(hosts, "192.0.2")[0]).toMatchObject({ kind: "known", host: hosts[0] });
});
