import { describe, expect, it } from "vitest";
import {
  LOOPBACK_HOSTS, MAX_PORTS, appearedPorts, forwardedLink, loopbackLink, parseForward, parseForwards, parsePorts,
  portLists, withForward, parseSavedForwards, parseSavedForward, MAX_SAVED_FORWARDS, type DetectedPort, type RemoteForward, type SavedForward,
} from "./ports.ts";
import { MAX_TITLE } from "../protocol/untrusted.ts";
import { hostKey } from "../connection/server_connection.ts";

const port = (n: number, process = "node", sessionId = "s1"): DetectedPort => ({ port: n, sessionId, process });
const forward = (target: string, remote: number, local = remote, state: "up" | "down" = "up"): RemoteForward =>
  ({ target, remote, local, url: `http://localhost:${local}`, state });

describe("parsePorts", () => {
  it("keeps valid ports, ordered by port, each once", () => {
    expect(parsePorts([
      { port: 8000, sessionId: "s2", process: "python3" },
      { port: 5173, sessionId: "s1", process: "node" },
      { port: 8000, sessionId: "s1", process: "again" },
      { port: 0, sessionId: "s1", process: "zero" },
      { port: 65536, sessionId: "s1", process: "high" },
      { port: 80.5, sessionId: "s1", process: "fraction" },
      { port: "3000", sessionId: "s1", process: "string" },
      null,
      "junk",
    ])).toEqual([port(5173, "node"), port(8000, "python3", "s2")]);
  });

  it("makes the host's strings plain text with the caps", () => {
    const [p] = parsePorts([{ port: 3000, sessionId: 7, process: "ev\u001b[31mil‮" + "x".repeat(MAX_TITLE) }])!;
    expect(p.sessionId).toBe("");
    expect(p.process.startsWith("ev[31mil")).toBe(true);
    expect(p.process).not.toMatch(/[\u001b‮]/);
    expect(p.process.length).toBe(MAX_TITLE);
  });

  it("is null without a list and bounded with a long one", () => {
    expect(parsePorts(undefined)).toBeNull();
    expect(parsePorts({ port: 80 })).toBeNull();
    const many = Array.from({ length: MAX_PORTS + 50 }, (_, i) => ({ port: i + 1, sessionId: "s", process: "p" }));
    expect(parsePorts(many)).toHaveLength(MAX_PORTS);
  });
});

describe("appearedPorts", () => {
  it("counts nothing in a connection's first list", () => {
    expect(appearedPorts(undefined, [port(3000), port(8000)])).toEqual([]);
  });

  it("returns only the ports the last list did not have", () => {
    expect(appearedPorts([port(3000), port(8000)], [port(3000), port(5173), port(9000)]))
      .toEqual([port(5173), port(9000)]);
    expect(appearedPorts([port(3000), port(8000)], [port(3000)])).toEqual([]);
    expect(appearedPorts([], [port(3000)])).toEqual([port(3000)]);
  });
});

describe("parseForward", () => {
  it("takes the local server's answer, announced at localhost only", () => {
    expect(parseForward({ target: "me@vm1", remote: 5173, local: 5174, url: "http://127.0.0.1:5174", state: "up" }))
      .toEqual(forward("me@vm1", 5173, 5174));
    expect(parseForward({ target: "vm1", remote: 80, local: 40000, state: "down" })).toEqual(forward("vm1", 80, 40000, "down"));
    expect(parseForward({ target: "vm1", remote: 80, local: 40000 })?.state).toBe("up");
  });

  it("drops what has not the shape", () => {
    expect(parseForward({ target: "-oProxyCommand=sh", remote: 80, local: 80 })).toBeNull();
    expect(parseForward({ target: "vm1 x", remote: 80, local: 80 })).toBeNull();
    expect(parseForward({ target: "vm1", remote: 0, local: 80 })).toBeNull();
    expect(parseForward({ target: "vm1", remote: 80, local: 70000 })).toBeNull();
    expect(parseForward(null)).toBeNull();
  });

  it("constructs the service URL from its local port without copying credentials from an API URL", () => {
    expect(parseForward({ target: "vm1", remote: 5173, local: 15173, state: "up",
      url: "http://user:secret@127.0.0.1:9800/?token=secret&bootstrap=code&ticket=t#session",
    })?.url).toBe("http://localhost:15173");
  });

  it("lists forwards by target and port", () => {
    expect(parseForwards([
      { target: "vm2", remote: 80, local: 8080 },
      { target: "vm1", remote: 9000, local: 9000 },
      { target: "vm1", remote: 3000, local: 3000 },
      { target: "", remote: 1, local: 1 },
    ]).map((f) => `${f.target}:${f.remote}`)).toEqual(["vm1:3000", "vm1:9000", "vm2:80"]);
    expect(parseForwards({})).toEqual([]);
  });

  it("replaces a forward of the same target and port", () => {
    const list = withForward([forward("vm1", 3000), forward("vm2", 80)], forward("vm1", 3000, 3001, "down"));
    expect(list).toEqual([forward("vm1", 3000, 3001, "down"), forward("vm2", 80)]);
  });
});

describe("portLists", () => {
  const vm1 = hostKey("vm1");
  const vm2 = hostKey("me@vm2");

  it("joins each host's ports with its forwards", () => {
    const lists = portLists(
      [{ key: vm1, ports: [port(3000, "node"), port(8000, "python3")] }, { key: vm2, ports: [port(3000, "ruby")] }],
      [forward("vm1", 8000, 8001, "down"), forward("vm1", 9000), forward("me@vm2", 3000)],
    );
    expect(lists).toEqual([
      { target: "vm1", rows: [
        { port: 3000, process: "node" },
        { port: 8000, process: "python3", forward: forward("vm1", 8000, 8001, "down") },
        // Forwarded, but the host lists no process on it now.
        { port: 9000, forward: forward("vm1", 9000) },
      ] },
      { target: "me@vm2", rows: [{ port: 3000, process: "ruby", forward: forward("me@vm2", 3000) }] },
    ]);
  });

  it("lists forwards of hosts the window is not connected to, and leaves out hosts without rows", () => {
    const lists = portLists(
      [{ key: vm1, ports: [] }, { key: "local", ports: [port(5173)] }, { key: vm2, ports: undefined }],
      [forward("old", 4000)],
    );
    expect(lists).toEqual([{ target: "old", rows: [{ port: 4000, forward: forward("old", 4000) }] }]);
  });

  it("marks the ports the client is forwarding or stopping", () => {
    const lists = portLists([{ key: vm1, ports: [port(3000), port(8000)] }], [forward("vm1", 8000)],
      (target, p) => (target === "vm1" && p === 3000 ? "forward" : target === "vm1" && p === 8000 ? "stop" : undefined));
    expect(lists[0].rows.map((r) => r.pending)).toEqual(["forward", "stop"]);
  });
});

describe("loopback links", () => {
  it("knows the four hosts of the host itself", () => {
    expect(LOOPBACK_HOSTS).toEqual(["localhost", "127.0.0.1", "0.0.0.0", "[::1]"]);
    for (const host of LOOPBACK_HOSTS) {
      expect(loopbackLink(`http://${host}:5173/app`)?.port).toBe(5173);
    }
    expect(loopbackLink("http://LOCALHOST:3000/")?.port).toBe(3000);
    expect(loopbackLink("http://[0:0:0:0:0:0:0:1]:3000/")?.port).toBe(3000);
  });

  it("leaves other hosts and schemes alone", () => {
    for (const raw of [
      "http://example.com:5173/", "http://10.0.0.5:8080/", "http://127.0.0.2:8000/", "http://localhost.example:80/",
      "ftp://localhost:21/", "javascript:alert(1)", "file:///etc/passwd", "not a url", "http://localhost:0/",
    ]) {
      expect(loopbackLink(raw), raw).toBeNull();
    }
  });

  it("takes the scheme's port when the link has none", () => {
    expect(loopbackLink("http://localhost/x")?.port).toBe(80);
    expect(loopbackLink("https://127.0.0.1/x")?.port).toBe(443);
  });

  it("rewrites to localhost and the local port, keeping scheme, path, query and fragment", () => {
    const at = (raw: string, local: number) => forwardedLink(loopbackLink(raw)!, local);
    expect(at("http://localhost:5173/app/x?y=1&z=%20#top", 5174)).toBe("http://localhost:5174/app/x?y=1&z=%20#top");
    expect(at("http://127.0.0.1:8000/x", 8000)).toBe("http://localhost:8000/x");
    expect(at("http://0.0.0.0:8000", 40123)).toBe("http://localhost:40123/");
    expect(at("http://[::1]:8000/a?b#c", 8001)).toBe("http://localhost:8001/a?b#c");
    expect(at("https://localhost:8443/secure", 8443)).toBe("https://localhost:8443/secure");
    expect(at("http://localhost/x", 40000)).toBe("http://localhost:40000/x");
    expect(at("http://localhost:8080/x", 80)).toBe("http://localhost/x");
  });
});

const saved = (target = "vm1", remote = 5173, local = 15173): SavedForward =>
  ({ target, remote, local, label: "", enabled: true });

describe("saved forward responses", () => {
  it("validates the envelope and preserves empty labels and paused state", () => {
    expect(parseSavedForwards({ version: 1, forwards: [{ ...saved(), enabled: false }] }))
      .toEqual([{ ...saved(), enabled: false }]);
    for (const value of [[], null, {}, { version: 2, forwards: [] }, { version: 1, forwards: [saved(), saved()] },
      { version: 1, forwards: [{ ...saved(), enabled: "yes" }] }]) expect(parseSavedForwards(value)).toBeNull();
    for (const change of [{ remote: 0 }, { local: 65536 }, { target: "-option" }, { label: null }]) {
      expect(parseSavedForward({ ...saved(), ...change })).toBeNull();
    }
  });

  it("bounds Unicode labels and sanitizes label/error controls without interpreting markup", () => {
    const result = parseSavedForward({ ...saved(), label: "\u001b\u202e<img>" + "🚀".repeat(210),
      lastError: { code: "local_port_conflict\u0000", message: "busy\u202e\n<script>", hint: "try\u001b again" },
    })!;
    expect([...result.label]).toHaveLength(200);
    expect(result.label).toBe("<img>" + "🚀".repeat(195));
    expect(result.lastError).toEqual({ code: "local_port_conflict", message: "busy\n<script>", hint: "try again" });
  });

  it("rejects over-capacity snapshots and never mistakes malformed state for an empty list", () => {
    expect(parseSavedForwards({ version: 1, forwards: Array.from({ length: 33 }, (_, i) => saved("vm1", i + 1)) })).toBeNull();
    expect(parseSavedForwards({ version: 1, forwards: Array.from({ length: MAX_SAVED_FORWARDS + 1 }, (_, i) => saved(`vm${i}`)) })).toBeNull();
    expect(parseSavedForwards({ version: 1, forwards: [] })).toEqual([]);
  });

  it("joins discovery, active and saved rows once, retaining undiscovered and disconnected services", () => {
    const frontend = saved();
    const api = { ...saved("vm1", 8000, 18000), enabled: false, label: "API" };
    const other = saved("vm2");
    const rows = portLists([{ key: hostKey("vm1"), ports: [port(5173)] }], [forward("vm1", 5173, 15173)], () => undefined,
      [frontend, api, other]);
    expect(rows).toEqual([
      { target: "vm1", rows: [{ port: 5173, process: "node", forward: forward("vm1", 5173, 15173), saved: frontend }, { port: 8000, saved: api }] },
      { target: "vm2", rows: [{ port: 5173, saved: other }] },
    ]);
    expect(portLists([], [], () => undefined, [frontend, api])[0].rows.map((r) => r.process)).toEqual([undefined, undefined]);
  });
});
