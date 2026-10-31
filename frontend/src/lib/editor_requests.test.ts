import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeServer, fakeEmulator } from "./testing/fake_server.ts";
import { TerminalSession } from "./terminal/terminal_session.ts";
import { watchEditorRequests } from "./editor_requests.ts";

const state = vi.hoisted(() => ({ remote: false, shell: "tauri", contextReady: true, distro: undefined as string | undefined }));
const notice = vi.hoisted(() => ({ show: vi.fn(), dismiss: vi.fn(), error: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: Object.assign(notice.show, { dismiss: notice.dismiss, error: notice.error }) }));
vi.mock("./platform.ts", () => ({ get isRemoteSession() { return state.remote; }, get shellKind() { return state.shell; } }));
vi.mock("../stores/connections.svelte.ts", () => ({ getLocalWslDistro: () => state.distro, hasLocalEditorContext: () => state.contextReady }));
vi.mock("./settings.ts", () => ({ getSettings: () => ({ editors: { file: "vscode", folder: "vscode", extensions: [{ extension: "rs", editor: "zed" }] } }) }));

const invoke = vi.fn();
const assign = vi.fn();
const cleanups: (() => void)[] = [];
const id = "a".repeat(32);
function fixture(key: "local" | `host:${string}` = "host:vm1") {
  const server = fakeServer(key);
  const session = new TerminalSession(server.conn.sessions, "session", fakeEmulator());
  session.wire();
  const cleanup = watchEditorRequests(server.conn);
  cleanups.push(cleanup, () => server.conn.dispose());
  server.socket.message({ type: "mux.welcome", capabilities: ["editor-open-v1"] });
  return { ...server, session, cleanup };
}
function request(overrides = {}) {
  return { type: "editor.request", requestId: id, sessionId: "session", path: "/srv/file.rs", kind: "file", expiresAt: Date.now() + 30_000, ...overrides };
}
function prompt() { return notice.show.mock.calls.at(-1)![1]; }
function outcome(server: ReturnType<typeof fixture>) { return server.socket.sentOfType("editor.result").at(-1)?.outcome; }

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date", "performance", "setTimeout", "clearTimeout"] });
  vi.clearAllMocks(); state.remote = false; state.shell = "tauri"; state.contextReady = true; state.distro = undefined;
  invoke.mockResolvedValue(undefined); notice.show.mockReturnValue("toast");
  vi.stubGlobal("window", { __TAURI_INTERNALS__: { invoke }, __SPLITTY_ELECTRON__: { invoke }, location: { assign } });
});
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("confirmed editor requests", () => {
  it("registers capability but launches only on a local click, once", async () => {
    const server = fixture();
    expect(server.socket.sentOfType("editor.register")).toHaveLength(1);
    server.socket.message(request());
    expect(invoke).not.toHaveBeenCalled();
    expect(notice.show.mock.calls[0][0]).toContain("Zed");
    expect(prompt().description).toBe("vm1 — /srv/file.rs");
    prompt().action.onClick(); prompt().action.onClick();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("open_remote_editor", { editor: "zed", target: "vm1", cwd: "/srv/file.rs", kind: "file" });
    await Promise.resolve();
    expect(outcome(server)).toBe("dispatched");
    server.socket.message(request());
    expect(notice.show).toHaveBeenCalledTimes(1);
  });
  it.each(["web", "electron"])("dispatches immediately during %s confirmation", async (shell) => {
    state.shell = shell; const server = fixture("local"); server.socket.message(request({ editor: "vscode" }));
    prompt().action.onClick();
    if (shell === "web") expect(assign).toHaveBeenCalledExactlyOnceWith("vscode://file/srv/file.rs");
    else expect(invoke).toHaveBeenCalledExactlyOnceWith("editor:open-local", { editor: "vscode", cwd: "/srv/file.rs", kind: "file", wslDistro: undefined });
    await Promise.resolve(); expect(outcome(server)).toBe("dispatched");
  });
  it("refuses host-owned pages and servers without capability", () => {
    state.remote = true; const remote = fixture(); remote.socket.message(request());
    expect(remote.socket.sentOfType("editor.register")).toHaveLength(0);
    state.remote = false; const old = fixture(); old.socket.message({ type: "mux.welcome", capabilities: [] }); old.socket.message(request());
    expect(notice.show).not.toHaveBeenCalled(); expect(invoke).not.toHaveBeenCalled();
  });
  it.each(["deny", "dismiss", "cancel", "expire", "closed", "disconnect", "replace", "dispose"])("invalidates cached action on %s", async (action) => {
    const server = fixture(); server.socket.message(request()); const stale = prompt().action.onClick;
    switch (action) {
      case "deny": prompt().cancel.onClick(); break;
      case "dismiss": prompt().onDismiss(); break;
      case "cancel": server.socket.message({ type: "editor.cancelled", requestId: id }); break;
      case "expire": await vi.advanceTimersByTimeAsync(25_001); break;
      case "closed": server.socket.message({ type: "closed", sessionId: "session" }); break;
      case "disconnect": server.socket.drop(); server.socket.message({ type: "mux.welcome", capabilities: ["editor-open-v1"] }); break;
      case "replace": server.conn.sessions.register(new TerminalSession(server.conn.sessions, "session", fakeEmulator())); break;
      case "dispose": server.cleanup(); break;
    }
    stale(); expect(invoke).not.toHaveBeenCalled(); expect(assign).not.toHaveBeenCalled();
  });
  it("uses only the connection host and refuses missing sessions despite ID collisions elsewhere", () => {
    fixture("host:other");
    const server = fixture("host:me@vm1");
    server.socket.message(request({ target: "evil", uri: "file:///etc/passwd", editor: "vscode" }));
    prompt().action.onClick();
    expect(invoke.mock.calls[0][1].target).toBe("me@vm1");
    invoke.mockClear(); vi.advanceTimersByTime(2001);
    server.conn.sessions.unregister(server.session);
    server.socket.message(request({ requestId: "b".repeat(32) }));
    expect(outcome(server)).toBe("disconnected"); expect(invoke).not.toHaveBeenCalled();
  });
  it.each([{ path: null }, { path: "/x\u202ey" }, { path: "/a/../b" }, { path: "file:///tmp/x" }, { path: "/a:12" }, { editor: "sh" }, { kind: "url" }, { expiresAt: 0 }])("refuses invalid request %j", (change) => {
    const server = fixture(); server.socket.message(request(change));
    expect(notice.show).not.toHaveBeenCalled(); expect(invoke).not.toHaveBeenCalled();
    expect(["failed", "expired"]).toContain(outcome(server));
  });
  it("bounds prompt flooding per connection and per window", () => {
    const servers = [fixture("host:vm1"), fixture("host:vm2"), fixture("host:vm3"), fixture("host:vm4")];
    for (const s of servers) s.socket.message(request());
    expect(notice.show).toHaveBeenCalledTimes(3);
    expect(outcome(servers[3])).toBe("denied");
    servers[0].socket.message(request({ requestId: "b".repeat(32) }));
    expect(notice.show).toHaveBeenCalledTimes(3);
  });
  it("reports OS refusal and retains WSL restrictions", async () => {
    invoke.mockRejectedValue(new Error("no handler"));
    const server = fixture(); server.socket.message(request()); prompt().action.onClick();
    await Promise.resolve(); expect(outcome(server)).toBe("failed");
    state.distro = "Ubuntu"; const local = fixture("local"); local.socket.message(request());
    expect(outcome(local)).toBe("failed");
  });
});

it("waits for local WSL context and rejects a changed distribution before launch", () => {
  state.contextReady = false;
  const server = fixture("local"); server.socket.message(request({ editor: "vscode" }));
  expect(notice.show).not.toHaveBeenCalled(); expect(outcome(server)).toBe("failed");
  state.contextReady = true; state.distro = "Ubuntu"; vi.advanceTimersByTime(2001);
  server.socket.message(request({ requestId: "b".repeat(32), editor: "vscode" }));
  const stale = prompt().action.onClick; state.distro = "Debian"; stale();
  expect(invoke).not.toHaveBeenCalled(); expect(outcome(server)).toBe("disconnected");
});
