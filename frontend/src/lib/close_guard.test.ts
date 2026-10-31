import { describe, expect, it, vi } from "vitest";
import type { DockviewApi } from "dockview-core";
import {
  guardedClose, guardedCloseMultiple, hostScreensOpen, onCloseRequest, runningOnQuit, scratchpadsOnQuit,
  type CloseRequest,
} from "./close_guard.ts";
import { ScreenBinding, registerScreenApi, unregisterScreenApi } from "./connection/screen_binding.ts";
import { addConnection, localConnection, removeConnection } from "./connection/registry.ts";
import { fakeEmulator, fakeServer } from "./testing/fake_server.ts";
import { TerminalSession } from "./terminal/terminal_session.ts";

function running(session: TerminalSession, title: string): TerminalSession {
  session.wire();
  session.handleTitle({ title, cwd: "/", cmdline: title, baseShell: false, oscTitle: "" });
  return session;
}

describe("the quit guard", () => {
  // remote-screens: quitting only collapses remote screens, their processes keep running.
  it("counts only the local server's running processes", () => {
    expect(hostScreensOpen()).toBe(false);
    const { conn: host } = fakeServer("host:vm1", "vm1");
    addConnection(host);
    const remote = running(new TerminalSession(host.sessions, "sess-r", fakeEmulator()), "claude");
    expect(runningOnQuit()).toEqual([]);
    expect(hostScreensOpen()).toBe(true);

    const local = running(new TerminalSession(localConnection().sessions, "sess-l", fakeEmulator()), "make");
    expect(runningOnQuit()).toEqual([{ name: "make", title: undefined }]);

    localConnection().sessions.unregister(local);
    host.sessions.unregister(remote);
    removeConnection("host:vm1");
  });

  it("lists each running process with the title its program set", () => {
    const local = new TerminalSession(localConnection().sessions, "sess-c", fakeEmulator());
    local.wire();
    local.handleTitle({ title: "claude", cwd: "/", cmdline: "claude", baseShell: false, oscTitle: "✳ Remote session UX" });
    const make = running(new TerminalSession(localConnection().sessions, "sess-m", fakeEmulator()), "make");
    expect(runningOnQuit()).toEqual([{ name: "claude", title: "✳ Remote session UX" }, { name: "make", title: undefined }]);
    localConnection().sessions.unregister(local);
    localConnection().sessions.unregister(make);
  });

  it("asks nothing when a daemon keeps the sessions after the window goes", () => {
    const make = running(new TerminalSession(localConnection().sessions, "sess-d", fakeEmulator()), "make");
    localConnection().mux.serverMode = "daemon";
    expect(runningOnQuit()).toEqual([]);
    localConnection().mux.serverMode = "embedded";
    localConnection().sessions.unregister(make);
  });
});

describe("scratchpad text that closing would lose", () => {
  function idleShell(session: TerminalSession): TerminalSession {
    session.wire();
    session.handleTitle({ title: "zsh", cwd: "/", cmdline: "zsh", baseShell: true, oscTitle: "" });
    return session;
  }

  // session-scratchpad, Confirmation Before Text Is Lost (Close a pane with text)
  it("asks before closing an idle pane whose scratchpad holds text, once for several", () => {
    const { conn } = fakeServer("host:vm2", "vm2");
    const binding = new ScreenBinding("screen-key", conn, "screen-1");
    const api = { id: "dv-close" } as unknown as DockviewApi;
    registerScreenApi(api, binding);
    const shell = idleShell(new TerminalSession(conn.sessions, "sess-i", fakeEmulator()));
    binding.linkPane("pane-1", shell.id);
    const requests: CloseRequest[] = [];
    const off = onCloseRequest((request) => requests.push(request));
    const close = vi.fn();

    guardedClose(api, "pane-1", close);
    expect(close).toHaveBeenCalledTimes(1);
    expect(requests).toEqual([]);

    conn.scratchpads.edit("screen-1", shell.id, "send this after the build");
    conn.scratchpads.flush(shell.id);
    guardedClose(api, "pane-1", close);
    expect(close).toHaveBeenCalledTimes(1);
    expect(requests).toMatchObject([{ paneId: "pane-1", processName: "", scratchpads: 1 }]);
    requests[0].close();
    expect(close).toHaveBeenCalledTimes(2);

    const other = vi.fn();
    guardedCloseMultiple(api, [{ id: "pane-1", close }, { id: "pane-2", close: other }]);
    expect(other).toHaveBeenCalledTimes(1);
    expect(requests[1]).toMatchObject({ processName: "", scratchpads: 1 });

    off();
    unregisterScreenApi(api);
    conn.sessions.unregister(shell);
    conn.dispose();
  });

  // session-scratchpad, Confirmation Before Text Is Lost (Quit an embedded window)
  it("counts the local sessions' scratchpad text on quit, unless a daemon keeps it", () => {
    const local = localConnection();
    const shell = idleShell(new TerminalSession(local.sessions, "sess-q", fakeEmulator()));
    expect(scratchpadsOnQuit()).toBe(0);
    local.scratchpads.edit("screen-1", shell.id, "notes");
    local.scratchpads.flush(shell.id);
    expect(scratchpadsOnQuit()).toBe(1);
    expect(runningOnQuit()).toEqual([]);
    local.mux.serverMode = "daemon";
    expect(scratchpadsOnQuit()).toBe(0);
    local.mux.serverMode = "embedded";
    local.sessions.unregister(shell);
    expect(scratchpadsOnQuit()).toBe(0);
  });
});
