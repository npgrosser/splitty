import { describe, expect, it } from "vitest";
import {
  blocksInput, describeFailure, hostIndicator, hostTooltip, offersUpdate, remoteScreenStatus, type RemoteScreenFlags,
} from "./screen_status.ts";
import type { HostScreen, RemoteState } from "./remote_socket.ts";

const plain: RemoteScreenFlags = { pending: false, needsChoice: false, missing: false };
const offer: HostScreen[] = [{ id: "s1", name: "api", panes: 3 }];

const host = (phase: RemoteState["phase"], extra: Partial<RemoteState> = {}): RemoteState => ({
  phase, progress: [], attempt: 0, ...extra,
});

describe("remoteScreenStatus", () => {
  const cases: [string, RemoteState | undefined, Partial<RemoteScreenFlags>, string][] = [
    ["first connect", host("connecting", { progress: [{ step: "install", message: "Installing…" }] }), { pending: true }, "connecting"],
    ["first connect failed", host("failed", { error: { code: "auth", message: "denied" } }), { pending: true }, "failed"],
    ["reopen offer", host("ready"), { pending: true, needsChoice: true }, "offer"],
    ["new screen waiting for the welcome", host("ready"), { pending: true }, "connecting"],
    ["running", host("ready"), {}, "none"],
    ["missing on the host", host("ready"), { missing: true }, "missing"],
    ["connection dropped", host("reconnecting", { attempt: 2 }), {}, "reconnecting"],
    ["reconnect stopped on a fatal error", host("failed", { error: { code: "host_key", message: "changed" } }), {}, "failed"],
    ["host stayed unreachable", host("offline"), {}, "offline"],
    ["connection gone", undefined, {}, "offline"],
    ["connection closed", host("closed"), {}, "offline"],
  ];

  it.each(cases)("%s", (_name, state, flags, kind) => {
    const status = remoteScreenStatus("vm1", state, { ...plain, ...flags }, offer);
    expect(status.kind).toBe(kind);
    expect(blocksInput(status)).toBe(kind !== "none");
  });

  it("offers nothing when the host has nothing else running", () => {
    expect(remoteScreenStatus("vm1", host("ready"), { ...plain, needsChoice: true }, []).kind).toBe("none");
  });

  it("carries what each state shows", () => {
    expect(remoteScreenStatus("vm1", host("connecting", { progress: [{ step: "daemon", message: "Asking vm1…" }] }), plain, []))
      .toEqual({ kind: "connecting", target: "vm1", progress: [{ step: "daemon", message: "Asking vm1…" }], fresh: false });
    expect(remoteScreenStatus("vm1", host("connecting"), { ...plain, pending: true }, []))
      .toEqual({ kind: "connecting", target: "vm1", progress: [], fresh: true });
    expect(remoteScreenStatus("vm1", host("ready"), { ...plain, pending: true, needsChoice: true }, offer))
      .toEqual({ kind: "offer", target: "vm1", screens: offer });
    expect(remoteScreenStatus("vm1", host("reconnecting", { attempt: 3, error: { code: "unreachable", message: "x" } }), plain, []))
      .toEqual({ kind: "reconnecting", target: "vm1", attempt: 3, progress: [], error: { code: "unreachable", message: "x" } });
  });

  it("offers an update only for an incompatible host", () => {
    const incompatible = host("failed", { error: { code: "incompatible", message: "", version: "0.3.9", protocol: 1 } });
    expect(offersUpdate(remoteScreenStatus("vm1", incompatible, plain, []))).toBe(true);
    for (const state of [
      host("failed", { error: { code: "unsupported_host", message: "" } }),
      host("offline", { error: { code: "incompatible", message: "" } }),
      host("ready"),
      undefined,
    ]) {
      expect(offersUpdate(remoteScreenStatus("vm1", state, plain, []))).toBe(false);
    }
  });

  it("shows the update's progress while an update runs as a reconnect", () => {
    const updating = host("reconnecting", { progress: [{ step: "update", message: "Stopping Splitty 0.3.9 on vm1..." }] });
    const status = remoteScreenStatus("vm1", updating, plain, []);
    expect(status.kind === "reconnecting" && status.progress.map((p) => p.step)).toEqual(["update"]);
  });
});

describe("describeFailure", () => {
  it("folds ssh's output away and copies the command to run, built from the target", () => {
    const view = describeFailure("vm1", {
      code: "host_key",
      message: "Failed to reach vm1: ssh to vm1 failed: Host key verification failed.",
      hint: "Run `ssh vm1` once in a terminal and check the fingerprint.",
    });
    expect(view).toEqual({
      headline: "vm1 is not known yet.",
      hint: "Run `ssh vm1` once in a terminal and check the fingerprint.",
      details: "Failed to reach vm1: ssh to vm1 failed: Host key verification failed.",
      copy: "ssh vm1",
    });
  });

  it("shows an incompatible host's message naming both versions", () => {
    const view = describeFailure("vm1", {
      code: "incompatible",
      message: "vm1 runs Splitty 0.3.9 (protocol 1); this Splitty needs protocol 2 or 3.",
      hint: "Update Splitty on vm1 to use it here. Its daemon and sessions keep running until then.",
      version: "0.3.9",
      protocol: 1,
    });
    expect(view.headline).toBe("vm1 runs an incompatible Splitty.");
    expect(view.message).toBe("vm1 runs Splitty 0.3.9 (protocol 1); this Splitty needs protocol 2 or 3.");
    expect(view.details).toBeUndefined();

    expect(describeFailure("vm1", { code: "incompatible", message: "", version: "9.0.0", protocol: 7 }).message)
      .toBe("vm1 runs Splitty 9.0.0 (protocol 7).");
  });

  it("shows an install failure's message, which names the path", () => {
    const view = describeFailure("vm1", { code: "install", message: "Failed to install Splitty on vm1: no space left in /home/me" });
    expect(view.message).toContain("/home/me");
    expect(view.copy).toBe("Failed to install Splitty on vm1: no space left in /home/me");
  });
});

describe("the screen list's host state", () => {
  it("is connected, reconnecting, failed or offline, with the host and version in the tooltip", () => {
    const ready = host("ready", { ready: { target: "vm1", daemon: { version: "0.4.2", protocol: 1, keepsSessions: false, started: false }, screens: [] } });
    expect(hostIndicator(ready)).toBe("connected");
    expect(hostIndicator(host("connecting"))).toBe("connecting");
    expect(hostIndicator(host("reconnecting"))).toBe("reconnecting");
    expect(hostIndicator(host("offline"))).toBe("offline");
    expect(hostIndicator(host("failed"))).toBe("failed");
    expect(hostTooltip("me@vm1", ready)).toBe("me@vm1 · Splitty 0.4.2 · connected");
    expect(hostTooltip("me@vm1", host("failed"))).toBe("me@vm1 · failed");
  });

  it("is idle for a host the window has not connected yet (a remembered screen)", () => {
    expect(hostIndicator(undefined)).toBe("idle");
    expect(hostTooltip("vm1", undefined)).toBe("vm1 · not connected");
  });

  it("says that an update is available when the host runs another version than the local server", () => {
    const ready = host("ready", { ready: { target: "vm1", daemon: { version: "0.4.1", protocol: 1, keepsSessions: false, started: false }, screens: [] } });
    expect(hostTooltip("vm1", ready, "0.4.2")).toBe("vm1 · Splitty 0.4.1 · update to 0.4.2 available · connected");
    expect(hostTooltip("vm1", ready, "0.4.1")).toBe("vm1 · Splitty 0.4.1 · connected");
    // The local version is not known yet: nothing to compare.
    expect(hostTooltip("vm1", ready)).toBe("vm1 · Splitty 0.4.1 · connected");
  });
});
