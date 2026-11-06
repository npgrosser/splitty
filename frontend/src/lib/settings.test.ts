import { afterEach, describe, expect, it, vi } from "vitest";
const { apiFetch } = vi.hoisted(() => ({ apiFetch: vi.fn() }));
vi.mock("./protocol/http.ts", () => ({ apiFetch }));
afterEach(() => { vi.resetModules(); vi.clearAllMocks(); });
const initial = { terminal: { xterm: { scrollSensitivity: 1 } }, drawer: { sidebarWidth: 200, pinned: false }, editors: { file: "vscode", folder: "vscode", extensions: [], projects: [] } };
const metadata = { defaults: initial, limits: { "terminal.xterm.scrollSensitivity": { min: 0.1, max: 10, step: 0.1 } } };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
describe("server-owned settings", () => {
  it("omits a profile from a new screen request when none was selected", async () => {
    const { createScreenOp } = await import("./mux/mux_client.ts");
    expect(createScreenOp("screen", undefined, "pane")).toEqual({
      type: "create_screen", screenId: "screen", name: undefined, newPaneId: "pane", shellProfileId: undefined,
    });
  });
  it("loads server defaults and limits and applies change events", async () => {
    apiFetch.mockImplementation(async (path: string) => path.endsWith("metadata") ? json(metadata) : json(initial));
    const settings = await import("./settings.ts"); await settings.loadSettings();
    expect(settings.defaults.drawer.sidebarWidth).toBe(200);
    expect(settings.numericLimit("terminal.xterm.scrollSensitivity", { min: 1, max: 10, step: 1 }).step).toBe(0.1);
    settings.applyServerSettings({ ...initial, drawer: { ...initial.drawer, sidebarWidth: 280 } } as never);
    expect(settings.getSettings().drawer.sidebarWidth).toBe(280);
  });
  it("applies only local connection settings events", async () => {
    apiFetch.mockImplementation(async (path: string) => path.endsWith("metadata") ? json(metadata) : json(initial));
    const settings = await import("./settings.ts");
    await settings.loadSettings();
    const { fakeServer } = await import("./testing/fake_server.ts");
    const local = fakeServer("local");
    const remote = fakeServer("host:example");
    remote.socket.message({ type: "settings.changed", settings: { ...initial, drawer: { ...initial.drawer, sidebarWidth: 300 } } });
    expect(settings.getSettings().drawer.sidebarWidth).toBe(200);
    local.socket.message({ type: "settings.changed", settings: { ...initial, drawer: { ...initial.drawer, sidebarWidth: 280 } } });
    expect(settings.getSettings().drawer.sidebarWidth).toBe(280);
    local.conn.dispose(); remote.conn.dispose();
  });
  it("uses old host values as reset values without metadata", async () => {
    apiFetch.mockImplementation(async (path: string) => path.endsWith("metadata") ? json({}, 404) : json({ ...initial, drawer: { sidebarWidth: 270, pinned: false } }));
    const settings = await import("./settings.ts"); await settings.loadSettings();
    expect(settings.defaults.drawer.sidebarWidth).toBe(270);
  });
  it("rolls back rejected optimistic updates with field errors", async () => {
    apiFetch.mockImplementation(async (path: string, init?: RequestInit) => path.endsWith("metadata") ? json(metadata) : init?.method === "PATCH" ? json({ field: "drawer.sidebarWidth", error: "outside allowed range" }, 400) : json(initial));
    const settings = await import("./settings.ts"); await settings.loadSettings();
    await settings.updateSetting("drawer", "sidebarWidth", 900);
    expect(settings.getSettings().drawer.sidebarWidth).toBe(200);
    expect(settings.getSettingsError()).toEqual({ field: "drawer.sidebarWidth", error: "outside allowed range" });
  });
  it("refreshes after another window changes settings during a pending write", async () => {
    let current = initial;
    let finishPatch!: (value: Response) => void;
    apiFetch.mockImplementation(async (path: string, init?: RequestInit) => {
      if (path.endsWith("metadata")) return json(metadata);
      if (init?.method === "PATCH") return new Promise<Response>((resolve) => { finishPatch = resolve; });
      return json(current);
    });
    const settings = await import("./settings.ts");
    await settings.loadSettings();
    const pending = settings.updateSetting("drawer", "sidebarWidth", 260);
    current = { ...initial, drawer: { ...initial.drawer, sidebarWidth: 280 } };
    settings.applyServerSettings(current as never);
    finishPatch(json({ ...initial, drawer: { ...initial.drawer, sidebarWidth: 260 } }));
    await pending;
    expect(settings.getSettings().drawer.sidebarWidth).toBe(280);
  });
});
