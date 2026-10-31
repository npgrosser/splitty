import { describe, expect, it } from "vitest";
import { fakeServer } from "../testing/fake_server.ts";
import { ACTIVITY_INTERVAL_MS, Presence } from "./presence.ts";

type Server = ReturnType<typeof fakeServer>;

function welcome(server: Server, capabilities = ["pty-size-v1"]) {
  server.socket.message({ type: "mux.welcome", clientId: "c1", screens: [], capabilities });
}

/** A window whose active screen is on `active()`, visible and focused as set. */
function presenceOf(active: () => Server | undefined) {
  const state = { visible: true, focused: true, now: 0 };
  const presence = new Presence(
    () => active()?.conn,
    { visible: () => state.visible, focused: () => state.focused },
    () => state.now,
  );
  return { presence, state };
}

const presenceMessages = (server: Server) =>
  server.socket.sent.filter((m) => m.type === "activity" || m.type === "idle");

describe("Presence", () => {
  it("greets an unfocused window's servers with idle and sends no activity without an action (Automatic actions do not take the size)", () => {
    const host = fakeServer("host:vm1");
    const { presence, state } = presenceOf(() => host);
    state.focused = false;
    presence.watch(host.conn);
    welcome(host);
    expect(presenceMessages(host)).toEqual([{ type: "idle" }]);

    // A reconnect: a new welcome, the window still untouched.
    welcome(host);
    expect(presenceMessages(host)).toEqual([{ type: "idle" }, { type: "idle" }]);
  });

  it("greets the active screen's server with activity while the window is in use", () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const { presence } = presenceOf(() => host);
    presence.watch(local.conn);
    presence.watch(host.conn);
    welcome(local);
    welcome(host);
    expect(presenceMessages(host)).toEqual([{ type: "activity" }]);
    expect(presenceMessages(local)).toEqual([{ type: "idle" }]);
  });

  it("greets a server that welcomed before it was watched", () => {
    const host = fakeServer("host:vm1");
    welcome(host);
    const { presence } = presenceOf(() => host);
    presence.watch(host.conn);
    expect(presenceMessages(host)).toEqual([{ type: "activity" }]);
  });

  it("sends nothing to servers without the capability", () => {
    const host = fakeServer("host:vm1");
    const { presence } = presenceOf(() => host);
    presence.watch(host.conn);
    welcome(host, ["managed-jobs-v1"]);
    presence.acted();
    presence.hidden([host.conn]);
    expect(presenceMessages(host)).toEqual([]);
  });

  it("sends actions to the active screen's server, at most every interval", () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    let active: Server = host;
    const { presence, state } = presenceOf(() => active);
    state.focused = false;
    for (const server of [local, host]) {
      presence.watch(server.conn);
      welcome(server);
    }
    presence.acted();
    state.now = ACTIVITY_INTERVAL_MS - 1;
    presence.acted();
    expect(presenceMessages(host)).toEqual([{ type: "idle" }, { type: "activity" }]);
    state.now = ACTIVITY_INTERVAL_MS;
    presence.acted();
    expect(presenceMessages(host)).toHaveLength(3);

    active = local;
    presence.acted();
    expect(presenceMessages(local)).toEqual([{ type: "idle" }, { type: "activity" }]);
  });

  it("makes every server idle when the window is hidden, and the next action counts at once", () => {
    const local = fakeServer("local");
    const host = fakeServer("host:vm1");
    const { presence, state } = presenceOf(() => host);
    for (const server of [local, host]) {
      presence.watch(server.conn);
      welcome(server);
    }
    presence.hidden([local.conn, host.conn]);
    expect(presenceMessages(local).at(-1)).toEqual({ type: "idle" });
    expect(presenceMessages(host).at(-1)).toEqual({ type: "idle" });

    state.now = 1;
    presence.acted();
    expect(presenceMessages(host).at(-1)).toEqual({ type: "activity" });
  });
});
