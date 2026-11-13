import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { fakeServer } from "./testing/fake_server.ts";
import { seedSettingsForTest } from "./testing/settings.ts";
import { applyServerSettings, getSettings, type SplittySettings } from "./settings.ts";
import {
  attachNotifications, getNotifications, getUnreadCountForScreen, getUnreadForPane, getUnreadTitlesForScreen,
  markPaneRead, onNewNotification, setWatchedPane, type SplittyNotification,
} from "./notifications.ts";

function host() {
  const server = fakeServer("host:vm1", "vm1");
  server.conn.sessions.linkPane("terminal-1", "sess-1", "screen-a");
  const detach = attachNotifications(server.conn);
  return { ...server, detach };
}

let counter = 0;
const next = (prefix: string) => `${prefix}-${++counter}`;
const idle = (socket: ReturnType<typeof host>["socket"]) =>
  socket.message({
    type: "process.idle", id: next("idle"), sessionId: "sess-1", process: "claude",
    notice: { kind: "process.idle", severity: "info", title: "claude is waiting" },
  });

function setNotifications(changes: Partial<SplittySettings["notifications"]>): void {
  const current = getSettings();
  applyServerSettings({ ...current, notifications: { ...current.notifications, ...changes } });
}

/** The notifications of `conn` that are neither read nor dismissed, newest last. */
function unread(conn: string): SplittyNotification[] {
  return getNotifications().filter((n) => n.conn === conn && !n.read);
}

let toasts: string[] = [];
let stopToasts: () => void = () => {};
beforeEach(() => {
  seedSettingsForTest();
  toasts = [];
  stopToasts = onNewNotification((n) => toasts.push(n.title));
});
afterEach(() => {
  stopToasts();
  setWatchedPane(null);
  markPaneRead("host:vm1", "terminal-1");
  markPaneRead("host:vm1", "");
});

describe("unread notifications", () => {
  it("count what arrives for a pane the user does not look at, and read it with the pane", () => {
    const h = host();
    setWatchedPane(() => false);
    idle(h.socket);
    idle(h.socket);
    expect(getUnreadCountForScreen("host:vm1", "screen-a")).toBe(2);
    expect(getUnreadTitlesForScreen("host:vm1", "screen-a")).toEqual(["vm1: claude is waiting", "vm1: claude is waiting"]);
    expect(toasts).toEqual(["vm1: claude is waiting", "vm1: claude is waiting"]);
    markPaneRead("host:vm1", "terminal-1");
    expect(getUnreadCountForScreen("host:vm1", "screen-a")).toBe(0);
    h.detach();
  });

  // mux-protocol, Notifications in the Window: Watched pane.
  it("record what happens in the pane the user looks at as read, without a toast", () => {
    const h = host();
    setWatchedPane((conn, paneId) => conn === "host:vm1" && paneId === "terminal-1");
    idle(h.socket);
    expect(getUnreadCountForScreen("host:vm1", "screen-a")).toBe(0);
    expect(toasts).toEqual([]);
    h.detach();
  });
});

describe("server notices", () => {
  // mux-protocol, Classified Notification Events.
  it("take kind, severity and title from the notice", () => {
    const h = host();
    h.socket.message({
      type: "session.exited", id: next("crash"), sessionId: "sess-1", exitCode: 1, process: "node", userInitiated: false,
      notice: { kind: "session.crashed", severity: "error", title: "node failed", setting: "processCrashed" },
    });
    expect(getUnreadForPane("host:vm1", "terminal-1")).toMatchObject({
      kind: "session.crashed", severity: "error", title: "vm1: node failed", setting: "processCrashed",
    });
    h.detach();
  });

  // mux-protocol, Notifications in the Window: Own pane closed.
  it("add nothing for a suppressed notice", () => {
    const h = host();
    for (const suppressed of ["user-initiated", "program-status"]) {
      h.socket.message({
        type: "session.exited", id: next("closed"), sessionId: "sess-1", exitCode: 129, userInitiated: true,
        notice: { kind: "session.crashed", severity: "error", title: "zsh failed", setting: "processCrashed", suppressed },
      });
    }
    expect(unread("host:vm1")).toEqual([]);
    expect(toasts).toEqual([]);
    h.detach();
  });

  // program-status, Status Notifications in the Window: Web window.
  it("follow program status changes that carry a notice", () => {
    const h = host();
    h.socket.message({
      type: "program_status.changed", id: next("status"), sessionId: "sess-1", status: { state: "working", updatedAt: 1 },
    });
    expect(unread("host:vm1")).toEqual([]);
    h.socket.message({
      type: "program_status.changed", id: next("status"), sessionId: "sess-1", status: { state: "blocked", updatedAt: 2 }, notify: true,
      notice: { kind: "program-status", severity: "warning", title: "build: Waiting for permission", setting: "programStatus" },
    });
    expect(getUnreadForPane("host:vm1", "terminal-1")).toMatchObject({ severity: "warning", title: "vm1: build: Waiting for permission" });
    h.detach();
  });

  // mux-protocol, Notifications in the Window: Switch off, Switch turned off later.
  it("apply the window's switches, and a switch turned off removes what it governs", () => {
    const h = host();
    const finished = () => h.socket.message({
      type: "process.finished", id: next("finished"), sessionId: "sess-1", process: "make",
      notice: { kind: "process.finished", severity: "info", title: "make finished", setting: "processFinished" },
    });
    finished();
    idle(h.socket);
    expect(unread("host:vm1").map((n) => n.title)).toEqual(["vm1: make finished", "vm1: claude is waiting"]);
    setNotifications({ processFinished: false });
    expect(unread("host:vm1").map((n) => n.title)).toEqual(["vm1: claude is waiting"]);
    finished();
    expect(unread("host:vm1").map((n) => n.title)).toEqual(["vm1: claude is waiting"]);
    setNotifications({ processFinished: true });
    expect(unread("host:vm1").map((n) => n.title)).toEqual(["vm1: claude is waiting"]);

    setNotifications({ programStatus: false });
    h.socket.message({
      type: "program_status.changed", id: next("status"), sessionId: "sess-1", status: { state: "done", updatedAt: 3 }, notify: true,
      notice: { kind: "program-status", severity: "success", title: "Screen 1: Done", setting: "programStatus" },
    });
    expect(unread("host:vm1").map((n) => n.title)).toEqual(["vm1: claude is waiting"]);
    h.detach();
  });

  it("show no toast while toasts are off", () => {
    const h = host();
    setNotifications({ toasts: false });
    idle(h.socket);
    expect(unread("host:vm1")).toHaveLength(1);
    expect(toasts).toEqual([]);
    h.detach();
  });

  // mux-protocol, Notification Notices Capability: Older server.
  it("give an older server's events a generic notice, governed by enabled only", () => {
    const h = host();
    setNotifications({ processFinished: false, processCrashed: false });
    h.socket.message({ type: "process.finished", id: next("old"), sessionId: "sess-1", process: "cargo" });
    h.socket.message({ type: "session.exited", id: next("old"), sessionId: "sess-1", exitCode: 1, userInitiated: true });
    h.socket.message({ type: "bell", id: next("old"), sessionId: "sess-1" });
    h.socket.message({ type: "program_status.changed", id: next("old"), sessionId: "sess-1", status: { state: "blocked", updatedAt: 1 }, notify: true });
    expect(unread("host:vm1").map((n) => [n.severity, n.title])).toEqual([
      ["info", "vm1: Activity in cargo"],
      ["info", "vm1: Session activity"],
    ]);
    setNotifications({ enabled: false });
    expect(unread("host:vm1")).toEqual([]);
    h.detach();
  });

  // mux-protocol, Notification Notices Capability: Generic notification event.
  it("add nothing for a generic notification event, with or without a notice", () => {
    const h = host();
    h.socket.message({ type: "notification", id: next("generic"), sessionId: "sess-1", severity: "error", title: "build" });
    h.socket.message({ type: "notification", id: next("generic"), sessionId: "sess-1", notice: { kind: "process.idle", severity: "error", title: "build" } });
    expect(unread("host:vm1")).toEqual([]);
    h.detach();
  });
});
