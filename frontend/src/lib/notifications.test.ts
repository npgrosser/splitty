import { afterEach, describe, expect, it } from "vitest";
import { fakeServer } from "./testing/fake_server.ts";
import {
  attachNotifications, getUnreadCountForScreen, getUnreadTitlesForScreen, markPaneRead, setWatchedPane,
} from "./notifications.ts";

function host() {
  const server = fakeServer("host:vm1", "vm1");
  server.conn.sessions.linkPane("terminal-1", "sess-1", "screen-a");
  const detach = attachNotifications(server.conn);
  return { ...server, detach };
}

let counter = 0;
const idle = (socket: ReturnType<typeof host>["socket"]) =>
  socket.message({ type: "process.idle", id: `idle-${++counter}`, sessionId: "sess-1", process: "claude" });

afterEach(() => setWatchedPane(null));

describe("unread notifications", () => {
  it("count what arrives for a pane the user does not look at, and read it with the pane", () => {
    const h = host();
    setWatchedPane(() => false);
    idle(h.socket);
    idle(h.socket);
    expect(getUnreadCountForScreen("host:vm1", "screen-a")).toBe(2);
    expect(getUnreadTitlesForScreen("host:vm1", "screen-a")).toEqual(["vm1: claude is waiting", "vm1: claude is waiting"]);
    markPaneRead("host:vm1", "terminal-1");
    expect(getUnreadCountForScreen("host:vm1", "screen-a")).toBe(0);
    h.detach();
  });

  it("do not count what happens in the pane the user looks at", () => {
    const h = host();
    setWatchedPane((conn, paneId) => conn === "host:vm1" && paneId === "terminal-1");
    idle(h.socket);
    expect(getUnreadCountForScreen("host:vm1", "screen-a")).toBe(0);
    h.detach();
  });
});
