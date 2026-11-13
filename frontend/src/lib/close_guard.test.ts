import { afterEach, describe, expect, it, vi } from "vitest";
import type { DockviewApi } from "dockview-core";
import {
  guardedClose, guardedCloseMultiple, guardedReplace, hostScreensOpen, onCloseRequest, quitImpact,
  type CloseRequest,
} from "./close_guard.ts";
import type { CloseAssessment, CloseLoss } from "./protocol/types.ts";
import { ScreenBinding, registerScreenApi, unregisterScreenApi } from "./connection/screen_binding.ts";
import { addConnection, localConnection, removeConnection } from "./connection/registry.ts";
import type { ServerConnection } from "./connection/server_connection.ts";
import { fakeEmulator, fakeServer } from "./testing/fake_server.ts";
import { TerminalSession } from "./terminal/terminal_session.ts";

function loss(paneId: string, fields: Partial<CloseLoss>): CloseLoss {
  return { screenId: "screen-1", paneId, running: false, starting: false, scratchpad: false, annotations: 0, ...fields };
}

function assessment(entries: CloseLoss[]): CloseAssessment {
  return {
    confirm: entries.length > 0,
    running: entries.filter((e) => e.running).length,
    starting: entries.filter((e) => e.starting).length,
    scratchpads: entries.filter((e) => e.scratchpad).length,
    annotations: entries.reduce((n, e) => n + e.annotations, 0),
    entries,
  };
}

/** A host connection with a screen whose layout is `api`, assessing with `answer`. */
function screen(answer: ((...args: unknown[]) => Promise<CloseAssessment>) | null) {
  const { conn } = fakeServer("host:vm2", "vm2");
  const binding = new ScreenBinding("screen-key", conn, "screen-1");
  const api = { id: "dv-close" } as unknown as DockviewApi;
  registerScreenApi(api, binding);
  conn.mux.closeAssessments = answer !== null;
  const assess = vi.spyOn(conn.mux, "assessClose").mockImplementation(answer ?? (() => Promise.reject(Error("unsupported"))));
  const requests: CloseRequest[] = [];
  const off = onCloseRequest((request) => requests.push(request));
  const done = () => { off(); unregisterScreenApi(api); conn.dispose(); };
  return { conn, api, assess, requests, done };
}

afterEach(() => vi.restoreAllMocks());

// mux-protocol, Close Assessment: the window decides from the server's answer only.
describe("closing panes", () => {
  it("closes at once when the server says nothing is lost", async () => {
    const { api, assess, requests, done } = screen(() => Promise.resolve(assessment([])));
    const close = vi.fn();
    await guardedClose(api, "pane-1", close);
    expect(assess).toHaveBeenCalledWith({ kind: "panes", screenId: "screen-1", paneIds: ["pane-1"] });
    expect(close).toHaveBeenCalledTimes(1);
    expect(requests).toEqual([]);
    done();
  });

  it("names the running program, with the title its program set", async () => {
    const { conn, api, requests, done } = screen(() => Promise.resolve(assessment([loss("pane-1", { running: true, sessionId: "sess-c", title: "claude" })])));
    const session = new TerminalSession(conn.sessions, "sess-c", fakeEmulator());
    session.wire();
    session.handleTitle({ title: "claude", cwd: "/", cmdline: "claude", baseShell: false, oscTitle: "✳ Remote session UX" });
    const close = vi.fn();
    await guardedClose(api, "pane-1", close);
    expect(close).not.toHaveBeenCalled();
    expect(requests).toMatchObject([{ paneId: "pane-1", processName: "claude", applicationTitle: "✳ Remote session UX", unknown: false, action: "close" }]);
    requests[0].close();
    expect(close).toHaveBeenCalledTimes(1);
    conn.sessions.unregister(session);
    done();
  });

  // The web window used to treat a terminal that is still starting as idle.
  it("asks about a terminal that is still starting", async () => {
    const { api, requests, done } = screen(() => Promise.resolve(assessment([loss("pane-1", { starting: true })])));
    const close = vi.fn();
    await guardedClose(api, "pane-1", close);
    expect(close).not.toHaveBeenCalled();
    expect(requests).toMatchObject([{ processName: "", starting: 1 }]);
    done();
  });

  // session-scratchpad, Confirmation Before Text Is Lost; viewer-annotations, Confirmation Before Annotations Are Lost.
  it("closes the idle panes at once and asks once for the others", async () => {
    const { api, requests, done } = screen(() => Promise.resolve(assessment([
      loss("pane-1", { scratchpad: true }), loss("v1", { annotations: 3 }), loss("v2", { annotations: 2 }),
    ])));
    const one = vi.fn(), idle = vi.fn(), v1 = vi.fn(), v2 = vi.fn();
    await guardedCloseMultiple(api, [{ id: "pane-1", close: one }, { id: "pane-2", close: idle }, { id: "v1", close: v1 }, { id: "v2", close: v2 }]);
    expect(idle).toHaveBeenCalledTimes(1);
    expect([one, v1, v2].every((fn) => fn.mock.calls.length === 0)).toBe(true);
    expect(requests).toHaveLength(1);
    expect(requests[0]).toMatchObject({ processName: "", scratchpads: 1, annotations: 5 });
    requests[0].close();
    expect([one, v1, v2].every((fn) => fn.mock.calls.length === 1)).toBe(true);
    done();
  });

  it("counts several running programs in the name", async () => {
    const { api, requests, done } = screen(() => Promise.resolve(assessment([
      loss("pane-1", { running: true, title: "make" }), loss("pane-2", { running: true, title: "vim" }),
    ])));
    await guardedCloseMultiple(api, [{ id: "pane-1", close: vi.fn() }, { id: "pane-2", close: vi.fn() }]);
    expect(requests[0]).toMatchObject({ paneId: "pane-1", processName: "make (and 1 more)" });
    done();
  });

  // mux-protocol, Close Assessment: Older server.
  it("asks generically for every pane without the server's assessment", async () => {
    const { api, assess, requests, done } = screen(null);
    const close = vi.fn(), other = vi.fn();
    await guardedCloseMultiple(api, [{ id: "pane-1", close }, { id: "pane-2", close: other }]);
    expect(assess).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    expect(requests).toMatchObject([{ unknown: true, processName: "", scratchpads: 0, annotations: 0 }]);
    requests[0].close();
    expect(close).toHaveBeenCalledTimes(1);
    expect(other).toHaveBeenCalledTimes(1);
    done();
  });

  it("asks generically when the assessment fails", async () => {
    const { api, requests, done } = screen(() => Promise.reject(Error("Close assessment timed out")));
    const close = vi.fn();
    await guardedClose(api, "pane-1", close);
    expect(close).not.toHaveBeenCalled();
    expect(requests).toMatchObject([{ unknown: true }]);
    done();
  });
});

// templates, Confirmed Screen Replacement.
describe("replacing a screen by a template", () => {
  it("applies at once when the replacement ends nothing, asks otherwise", async () => {
    const quiet = screen(() => Promise.resolve(assessment([])));
    const apply = vi.fn();
    expect(await guardedReplace(quiet.conn, "screen-1", apply)).toBe(true);
    expect(quiet.assess).toHaveBeenCalledWith({ kind: "screens", screenIds: ["screen-1"] });
    expect(apply).toHaveBeenCalledTimes(1);
    quiet.done();

    const busy = screen(() => Promise.resolve(assessment([loss("pane-1", { running: true, title: "vim" })])));
    const replace = vi.fn();
    expect(await guardedReplace(busy.conn, "screen-1", replace)).toBe(true);
    expect(replace).not.toHaveBeenCalled();
    expect(busy.requests).toMatchObject([{ action: "replace", processName: "vim" }]);
    busy.requests[0].close();
    expect(replace).toHaveBeenCalledTimes(1);
    busy.done();
  });

  it("asks generically on an older host", async () => {
    const older = screen(null);
    const apply = vi.fn();
    expect(await guardedReplace(older.conn, "screen-1", apply)).toBe(true);
    expect(apply).not.toHaveBeenCalled();
    expect(older.requests).toMatchObject([{ action: "replace", unknown: true }]);
    older.done();
  });
});

describe("the quit guard", () => {
  function local(answer: CloseAssessment | Error | null): ServerConnection {
    const conn = localConnection();
    conn.mux.closeAssessments = answer !== null;
    vi.spyOn(conn.mux, "assessClose").mockImplementation((scope) => {
      expect(scope).toEqual({ kind: "workspace" });
      return answer instanceof Error ? Promise.reject(answer) : Promise.resolve(answer ?? assessment([]));
    });
    return conn;
  }

  // session-scratchpad and viewer-annotations, Quit an embedded window.
  it("lists what the local server's end would end", async () => {
    local(assessment([
      loss("p1", { running: true, title: "make" }), loss("p2", { starting: true }), loss("p3", { scratchpad: true }), loss("v1", { annotations: 2 }),
    ]));
    expect(await quitImpact()).toEqual({ processes: [{ name: "make", title: undefined }], starting: 1, scratchpads: 1, annotations: 2, unknown: false });
  });

  it("asks nothing when nothing would be lost or a daemon keeps the sessions", async () => {
    local(assessment([]));
    expect(await quitImpact()).toBeNull();
    local(assessment([loss("p1", { running: true })]));
    localConnection().mux.serverMode = "daemon";
    expect(await quitImpact()).toBeNull();
    localConnection().mux.serverMode = "embedded";
  });

  it("asks generically without an assessment", async () => {
    local(null);
    expect(await quitImpact()).toMatchObject({ unknown: true, processes: [] });
    local(Error("Connection ended during the close assessment"));
    expect(await quitImpact()).toMatchObject({ unknown: true });
  });

  // remote-screens: quitting only collapses remote screens, their processes keep running.
  it("tells whether host screens are open", () => {
    expect(hostScreensOpen()).toBe(false);
    const { conn: host } = fakeServer("host:vm1", "vm1");
    addConnection(host);
    expect(hostScreensOpen()).toBe(true);
    removeConnection("host:vm1");
  });
});
