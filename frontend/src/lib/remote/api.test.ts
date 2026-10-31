import { describe, expect, it } from "vitest";
import { MAX_TARGET, fetchRemoteHosts, hubUrl, validateTarget } from "./api.ts";

describe("validateTarget", () => {
  it("takes [user@]host[:port], trimmed", () => {
    expect(validateTarget(" vm1 ")).toBe("vm1");
    expect(validateTarget("me@api.devpod:2222")).toBe("me@api.devpod:2222");
  });

  it.each([
    ["empty", ""],
    ["an option", "-oProxyCommand=sh"],
    ["a dash lookalike", "−oProxyCommand=sh"],
    ["whitespace", "vm1 -L 1:2:3"],
    ["a control character", "vm1\u0007"],
    ["too long", "a".repeat(MAX_TARGET + 1)],
  ])("refuses %s", (_name, input) => {
    expect(validateTarget(input)).toBeNull();
  });
});

describe("hubUrl", () => {
  it("names the hub on the page's server, with the target and ticket encoded", () => {
    expect(hubUrl({ protocol: "http:", host: "127.0.0.1:9800" }, "me@vm1", "t&k"))
      .toBe("ws://127.0.0.1:9800/ws/remote?target=me%40vm1&ticket=t%26k");
    expect(hubUrl({ protocol: "https:", host: "box" }, "vm1", "t")).toBe("wss://box/ws/remote?target=vm1&ticket=t");
  });

  it("asks for an update with update=1, the only value the hub takes, and only when told to", () => {
    expect(hubUrl({ protocol: "http:", host: "127.0.0.1:9800" }, "vm1", "t", true))
      .toBe("ws://127.0.0.1:9800/ws/remote?target=vm1&ticket=t&update=1");
    expect(hubUrl({ protocol: "http:", host: "127.0.0.1:9800" }, "vm1", "t", false))
      .toBe("ws://127.0.0.1:9800/ws/remote?target=vm1&ticket=t");
  });
});

describe("fetchRemoteHosts", () => {
  it("keeps the entries of the documented shape", async () => {
    const fetcher = async (path: string) => {
      expect(path).toBe("/api/remote/hosts");
      return new Response(JSON.stringify([
        { target: "vm1", source: "recent", lastConnected: 1_790_000_000 },
        { target: "api.devpod", source: "ssh-config" },
        { target: 5, source: "recent" },
      ]));
    };
    expect(await fetchRemoteHosts(fetcher)).toEqual([
      { target: "vm1", source: "recent", lastConnected: 1_790_000_000 },
      { target: "api.devpod", source: "ssh-config" },
    ]);
  });

  it("fails on an error status", async () => {
    await expect(fetchRemoteHosts(async () => new Response("", { status: 401 }))).rejects.toThrow("401");
  });
});
