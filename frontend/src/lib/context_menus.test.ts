import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DockviewApi, DockviewPanelApi } from "dockview-core";
import { ScreenBinding, registerScreenApi, unregisterScreenApi } from "./connection/screen_binding.ts";
import type { ConnKey } from "./connection/server_connection.ts";
import { fakeEmulator, fakeServer } from "./testing/fake_server.ts";
import { TerminalSession } from "./terminal/terminal_session.ts";

const state = vi.hoisted(() => ({ shellKind: "web", isRemoteSession: false }));
const notice = vi.hoisted(() => ({ info: vi.fn(), error: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: notice }));
vi.mock("./platform.ts", () => ({ get shellKind() { return state.shellKind; }, get isRemoteSession() { return state.isRemoteSession; } }));
vi.mock("./shortcuts.ts", () => ({ shortcutKey: () => "" }));
vi.mock("./panes.ts", () => ({ addTerminalPane: vi.fn() }));
vi.mock("./close_guard.ts", () => ({ guardedClose: vi.fn(), guardedCloseMultiple: vi.fn() }));
vi.mock("../stores/ui.svelte.ts", () => ({ toggleDrawer: vi.fn(), getDrawerState: () => "closed" }));
vi.mock("./shell_profiles.ts", () => ({ getShellProfiles: vi.fn(), shellProfileIcon: vi.fn() }));
vi.mock("./shell_profile_model.ts", () => ({ visibleShellProfiles: vi.fn() }));
vi.mock("./window.ts", () => ({ openExternalTarget: vi.fn() }));

import { moveToScreenItem, paneMenuItems, tabMenuItems } from "./context_menus.ts";
import { subscribeUICommands, type UICommand } from "./client_commands.ts";
import type { Screen } from "../stores/screens.svelte.ts";
import { setLocalWslDistro } from "../stores/connections.svelte.ts";
import { registerScratchpad } from "./scratchpad.ts";

const paneId = "terminal-same";
const APIs: DockviewApi[] = [];
const servers: ReturnType<typeof fakeServer>[] = [];
const assign = vi.fn();
const invoke = vi.fn();

function screen(key: ConnKey, cwd = "/srv/project", id = `dv-${APIs.length}`) {
  const params: { jobId?: string } = {};
  const api = { id, getPanel: (pane: string) => (pane === paneId ? { params } : undefined) } as unknown as DockviewApi;
  const server = fakeServer(key, "untrusted-label");
  const binding = new ScreenBinding(`screen-${id}`, server.conn, "screen-same");
  const session = new TerminalSession(server.conn.sessions, "session-same", fakeEmulator());
  session.wire();
  session.lastCwd = cwd;
  binding.linkPane(paneId, session.id);
  registerScreenApi(api, binding);
  APIs.push(api);
  servers.push(server);
  return { api, binding, session, params, ...server };
}

function menu(kind: "pane" | "tab", api: DockviewApi) {
  return kind === "pane" ? paneMenuItems(paneId, api) : tabMenuItems(paneId, {
    group: { panels: [{ id: paneId }] },
  } as DockviewPanelApi, api);
}
function editor(kind: "pane" | "tab", api: DockviewApi, name = "VS Code") {
  return menu(kind, api).find((item) => item.label === "Open Folder in")?.children?.find((item) => item.label === name);
}

beforeEach(() => {
  vi.clearAllMocks();
  state.shellKind = "web";
  state.isRemoteSession = false;
  assign.mockReset();
  invoke.mockReset().mockResolvedValue(undefined);
  vi.stubGlobal("window", {
    location: { assign, search: "" },
    __TAURI_INTERNALS__: { invoke }, __SPLITTY_ELECTRON__: { invoke },
  });
});
afterEach(() => {
  for (const api of APIs.splice(0)) unregisterScreenApi(api);
  for (const server of servers.splice(0)) server.conn.dispose();
  vi.unstubAllGlobals();
});

for (const kind of ["pane", "tab"] as const) describe(`${kind} remote editor action`, () => {
  it("uses the selected connection despite colliding pane/session ids and opens only on click", () => {
    screen("local", "/local");
    screen("host:other", "/other");
    const selected = screen("host:user@192.0.2.10", "/srv/My App");
    const items = menu(kind, selected.api);
    const index = items.findIndex((item) => item.label === "Open Folder in");
    expect(items[index - 2].label).toBe("Copy Working Directory");
    expect(items[index - 1]).toMatchObject({ label: "Copy SSH Path", detail: "user@192.0.2.10:/srv/My App" });
    expect(items[index].detail).toBe("/srv/My App");
    expect(assign).not.toHaveBeenCalled();
    selected.socket.message({ type: "command", command: { type: "open_remote_editor", target: "evil", cwd: "/bad" } });
    expect(assign).not.toHaveBeenCalled();
    items[index].children![0].action();
    // Immediate, before a microtask or ticket lookup could lose user activation.
    expect(assign).toHaveBeenCalledExactlyOnceWith("vscode://vscode-remote/ssh-remote+user%40192.0.2.10/srv/My%20App/");
    expect(invoke).not.toHaveBeenCalled();
    expect(notice.info).not.toHaveBeenCalled();
    expect(selected.socket.sent).toEqual([]);
  });
  it("offers local desktop folders too, and never trusts the host query on remote pages", () => {
    const local = screen("local");
    expect(editor(kind, local.api)?.disabled).toBe(false);
    state.shellKind = "tauri";
    expect(editor(kind, local.api)?.disabled).toBe(false);
    state.shellKind = "web";
    state.isRemoteSession = true;
    window.location.search = "?host=user@192.0.2.10";
    expect(editor(kind, local.api)).toBeUndefined();
    expect(editor(kind, screen("host:vm1").api)).toBeUndefined();
    expect(assign).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });
  it.each(["", "relative/path", "/bad\npath"])("disables cwd %j with useful explanation", (cwd) => {
    const item = editor(kind, screen("host:vm1", cwd).api)!;
    expect(item.disabled).toBe(true);
    expect(item.detail).toMatch(/working directory/);
    item.action();
    expect(assign).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });
  it.each(["vm1:2222", "user@[::1]", "abc"])("explains unsupported target %s", (target) => {
    const item = editor(kind, screen(`host:${target}`).api)!;
    expect(item.disabled).toBe(true);
    expect(item.detail).toMatch(/alias/);
    item.action();
    expect(assign).not.toHaveBeenCalled();
  });
  it("uses current cwd, but rejects a session lost while the menu was open", () => {
    const selected = screen("host:vm1");
    const item = editor(kind, selected.api)!;
    selected.session.lastCwd = "/new #?";
    item.action();
    expect(assign).toHaveBeenLastCalledWith("vscode://vscode-remote/ssh-remote+vm1/new%20%23%3F/");
    selected.binding.unlinkPane(paneId);
    item.action();
    expect(assign).toHaveBeenCalledTimes(1);
    expect(notice.error).toHaveBeenCalled();
  });
  it("refuses removed/replaced bindings and replacement sessions with the same id", () => {
    const selected = screen("host:vm1");
    const item = editor(kind, selected.api)!;
    const replacement = new TerminalSession(selected.conn.sessions, "session-same", fakeEmulator());
    replacement.lastCwd = "/replacement";
    replacement.wire();
    item.action();
    expect(assign).not.toHaveBeenCalled();
    const next = editor(kind, selected.api)!;
    screen("host:other", "/other", selected.api.id);
    next.action();
    expect(assign).not.toHaveBeenCalled();
    unregisterScreenApi(selected.api);
    next.action();
    expect(assign).not.toHaveBeenCalled();
  });
  it("bounds path display without changing the URI data", () => {
    const selected = screen("host:vm1", "/" + "a".repeat(200));
    const item = editor(kind, selected.api)!;
    expect(menu(kind, selected.api).find((item) => item.label === "Open Folder in")!.detail!.length).toBeLessThanOrEqual(121);
    item.action();
    expect(assign.mock.calls[0][0]).toContain("a".repeat(200));
  });
});

it("reports browser refusal with setup help and no native fallback", () => {
  assign.mockImplementation(() => { throw new Error("OS refused"); });
  editor("pane", screen("host:vm1").api)!.action();
  expect(notice.error.mock.calls[0][0]).toBe("Could not open VS Code");
  expect(notice.error.mock.calls[0][1].description).toContain("Remote - SSH");
  expect(notice.info).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
});

it.each(["tauri", "electron"])("uses only the %s dedicated native command and reports rejection", async (shell) => {
  state.shellKind = shell;
  const selected = screen("host:user@192.0.2.10", "/a #?");
  const item = editor("tab", selected.api)!;
  invoke.mockRejectedValue(new Error("No handler"));
  item.action();
  await vi.waitFor(() => expect(notice.error).toHaveBeenCalled());
  expect(invoke).toHaveBeenCalledExactlyOnceWith(shell === "tauri" ? "open_remote_editor" : "editor:open-remote", { editor: "vscode", target: "user@192.0.2.10", cwd: "/a #?" });
  expect(assign).not.toHaveBeenCalled();
  expect(notice.info).not.toHaveBeenCalled();
});

for (const kind of ["pane", "tab"] as const) describe(`${kind} local editor and SSH path actions`, () => {
  afterEach(() => { setLocalWslDistro(undefined); });

  it("opens a local folder as a file URI, or through the server's WSL distribution", () => {
    const local = screen("local", "/home/me/My App");
    editor(kind, local.api)!.action();
    expect(assign).toHaveBeenLastCalledWith("vscode://file/home/me/My%20App/");
    setLocalWslDistro("Ubuntu-24.04");
    editor(kind, local.api)!.action();
    expect(assign).toHaveBeenLastCalledWith("vscode://vscode-remote/wsl+Ubuntu-24.04/home/me/My%20App/");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("copies a remote folder as target:path, and as an ssh URL when the target has a port", () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const copy = (api: DockviewApi) => menu(kind, api).find((item) => item.label === "Copy SSH Path");
    copy(screen("host:me@vm1", "/srv/app").api)!.action();
    expect(writeText).toHaveBeenLastCalledWith("me@vm1:/srv/app");
    copy(screen("host:me@vm1:2222", "/srv/app").api)!.action();
    expect(writeText).toHaveBeenLastCalledWith("ssh://me@vm1:2222/srv/app");
    expect(copy(screen("local").api)).toBeUndefined();
    expect(copy(screen("host:vm1", "").api)).toBeUndefined();
  });
});

for (const kind of ["pane", "tab"] as const) describe(`${kind} editor submenu`, () => {
  afterEach(() => setLocalWslDistro(undefined));

  it("offers both editors and, for a host, an SFTP client without opening anything on the parent action", () => {
    const selected = screen("host:vm1", "/srv/a #?%/project:123");
    const parent = menu(kind, selected.api).find((item) => item.label === "Open Folder in")!;
    expect(parent.children?.map((item) => item.label)).toEqual(["VS Code", "Zed", "SFTP Client"]);
    expect(menu(kind, screen("local").api).find((item) => item.label === "Open Folder in")!.children?.map((item) => item.label))
      .toEqual(["VS Code", "Zed"]);
    parent.action();
    expect(assign).not.toHaveBeenCalled();
    editor(kind, selected.api, "Zed")!.action();
    expect(assign).toHaveBeenCalledExactlyOnceWith("zed://ssh/vm1/srv/a%20%23%3F%25/project%3A123/");
    expect(notice.info).not.toHaveBeenCalled();
  });

  it("opens local Zed folders and disables WSL without opening a Windows path", () => {
    const local = screen("local", "/home/me/My App");
    editor(kind, local.api, "Zed")!.action();
    expect(assign).toHaveBeenLastCalledWith("zed://file/home/me/My%20App/");
    setLocalWslDistro("Ubuntu");
    const zed = editor(kind, local.api, "Zed")!;
    expect(zed.disabled).toBe(true);
    expect(zed.detail).toContain("WSL");
    zed.action();
    expect(assign).toHaveBeenCalledTimes(1);
  });

  it.each(["tauri", "electron"])("dispatches local folders only via %s native IPC", async (shell) => {
    state.shellKind = shell;
    const local = screen("local", "/srv/app");
    for (const name of ["VS Code", "Zed"]) editor(kind, local.api, name)!.action();
    expect(invoke).toHaveBeenNthCalledWith(1, shell === "tauri" ? "open_local_editor" : "editor:open-local", { editor: "vscode", cwd: "/srv/app", wslDistro: undefined });
    expect(invoke).toHaveBeenNthCalledWith(2, shell === "tauri" ? "open_local_editor" : "editor:open-local", { editor: "zed", cwd: "/srv/app", wslDistro: undefined });
    setLocalWslDistro("Ubuntu");
    editor(kind, local.api)!.action();
    expect(invoke).toHaveBeenLastCalledWith(shell === "tauri" ? "open_local_editor" : "editor:open-local", { editor: "vscode", cwd: "/srv/app", wslDistro: "Ubuntu" });
    expect(assign).not.toHaveBeenCalled();
    await Promise.resolve();
    expect(notice.info).not.toHaveBeenCalled();
  });

  it("opens a host's folder in the SFTP client with its port or IPv6 address as given", () => {
    for (const [target, uri] of [
      ["me@vm1:2222", "sftp://me@vm1:2222/srv/My%20App/"],
      ["user@[2001:db8::1]", "sftp://user@[2001:db8::1]/srv/My%20App/"],
      ["vm1", "sftp://vm1/srv/My%20App/"],
    ]) {
      const item = editor(kind, screen(`host:${target}`, "/srv/My App").api, "SFTP Client")!;
      expect(item.disabled).toBe(false);
      item.action();
      expect(assign).toHaveBeenLastCalledWith(uri);
    }
    expect(notice.info).not.toHaveBeenCalled();
  });

  it.each(["tauri", "electron"])("opens the SFTP client only via %s native IPC", (shell) => {
    state.shellKind = shell;
    editor(kind, screen("host:me@vm1:2222", "/srv").api, "SFTP Client")!.action();
    expect(invoke).toHaveBeenCalledExactlyOnceWith(shell === "tauri" ? "open_remote_editor" : "editor:open-remote", { editor: "sftp", target: "me@vm1:2222", cwd: "/srv" });
    expect(assign).not.toHaveBeenCalled();
  });

  it.each(["local", "host:user@vm1"] as const)("rechecks Zed's directory and session for %s", (key) => {
    const selected = screen(key);
    const item = editor(kind, selected.api, "Zed")!;
    selected.session.lastCwd = "/new #?";
    item.action();
    expect(assign).toHaveBeenLastCalledWith(key === "local" ? "zed://file/new%20%23%3F/" : "zed://ssh/user@vm1/new%20%23%3F/");
    selected.binding.unlinkPane(paneId);
    item.action();
    expect(assign).toHaveBeenCalledTimes(1);
    expect(notice.error).toHaveBeenCalled();
  });
});

// workspace-model, Moving a Pane to Another Screen: only within one server.
describe("Move to Screen", () => {
  const labels = (item: ReturnType<typeof moveToScreenItem>) =>
    item?.children?.map((child) => (child.separator ? "---" : child.label));

  it("offers the same server's other screens and a new one, and moves only on click", () => {
    const selected = screen("host:vm1");
    const screens: Screen[] = [
      { id: "screen-local", name: "Default", conn: "local", wireId: "screen-local" },
      { id: selected.binding.key, name: "api", conn: "host:vm1", wireId: "screen-same" },
      { id: "rs-logs", name: "logs", conn: "host:vm1", wireId: "s2" },
      { id: "rs-db", name: "db", conn: "host:vm2", wireId: "s3" },
    ];
    const commands: UICommand[] = [];
    const unsubscribe = subscribeUICommands((command) => commands.push(command));
    const item = moveToScreenItem(selected.api, paneId, screens);
    expect(labels(item)).toEqual(["logs", "---", "New Screen"]);
    expect(commands).toEqual([]);

    item!.children![0].action();
    item!.children![2].action();
    unsubscribe();
    expect(commands).toEqual([
      { type: "pane.move-to-screen", screenKey: selected.binding.key, paneId, target: "rs-logs" },
      { type: "pane.move-to-screen", screenKey: selected.binding.key, paneId, target: null },
    ]);
    expect(selected.socket.sent).toEqual([]);
  });

  it("offers only a new screen when the server has no other", () => {
    const selected = screen("local");
    const screens: Screen[] = [{ id: selected.binding.key, name: "Default", conn: "local", wireId: "screen-same" }];
    expect(labels(moveToScreenItem(selected.api, paneId, screens))).toEqual(["New Screen"]);
  });

  for (const kind of ["pane", "tab"] as const) {
    it(`is in the ${kind} menu, but not for a managed job's pane`, () => {
      const selected = screen("local");
      expect(menu(kind, selected.api).some((item) => item.label === "Move to Screen")).toBe(true);
      selected.params.jobId = "job-1";
      expect(menu(kind, selected.api).some((item) => item.label === "Move to Screen")).toBe(false);
    });
  }
});

// session-scratchpad, Scratchpad In The Pane: the pane's menu toggles its scratchpad.
describe("pane scratchpad entry", () => {
  it("shows or hides a pane's scratchpad, and is absent where the pane has none", () => {
    const { api } = screen("local");
    expect(menu("pane", api).some((item) => item.label.endsWith("Scratchpad"))).toBe(false);

    let open = false;
    const toggle = vi.fn(() => { open = !open; });
    const unregister = registerScratchpad(paneId, { toggle, isOpen: () => open });
    const show = menu("pane", api).find((item) => item.label === "Show Scratchpad");
    expect(show).toBeDefined();
    show?.action();
    expect(toggle).toHaveBeenCalledTimes(1);
    expect(menu("pane", api).some((item) => item.label === "Hide Scratchpad")).toBe(true);
    unregister();
    expect(menu("pane", api).some((item) => item.label.endsWith("Scratchpad"))).toBe(false);
  });
});
