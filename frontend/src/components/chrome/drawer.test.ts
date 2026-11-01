/// <reference types="node" />
import { afterAll, describe, expect, it, vi } from "vitest";
import { render } from "svelte/server";
import Drawer from "./Drawer.svelte";
import HostsPanel from "./HostsPanel.svelte";
import { portForwarding } from "../../stores/ports.svelte.ts";
import { hostGroups } from "../../lib/remote/on_hosts.ts";
import type { HostCatalog } from "../../lib/remote/host_directory.ts";
import type { Screen } from "../../stores/screens.svelte.ts";

const SCREENS: Screen[] = [
  { id: "local", name: "Shell", conn: "local", wireId: "local" },
  { id: "local-2", name: "Notes", conn: "local", wireId: "local-2" },
  { id: "remote", name: "Frontend", conn: "host:dev-vm", wireId: "remote" },
] as Screen[];
const HOST_ORDER: string[] = [];
const LIVE = [{ key: "host:dev-vm" as const, state: { phase: "ready" as const, progress: [], attempt: 0 }, available: [{ id: "api", name: "API", panes: 3 }] }];

vi.mock("../../stores/screens.svelte.ts", () => ({
  getScreens: () => SCREENS, getHostOrder: () => HOST_ORDER,
  getActiveScreenId: () => "remote", createScreen: vi.fn(), renameScreen: vi.fn(),
  setActiveScreen: vi.fn(), moveScreenInGroup: vi.fn(), subscribe: vi.fn(),
}));
vi.mock("../../stores/ui.svelte.ts", () => ({
  getDrawerState: () => "open", isDrawerPinned: () => true,
  setDrawerState: vi.fn(), setDrawerPinned: vi.fn(), setDrawerDragging: vi.fn(),
  setDrawerLiveWidth: vi.fn(), setRemoteScreenPickerOpen: vi.fn(), setHostPickerOpen: vi.fn(), showContextMenu: vi.fn(),
}));
vi.mock("../../lib/shortcuts.ts", () => ({ shortcutKey: () => "" }));
vi.mock("../../lib/remoteWindows.ts", () => ({ remoteWindows: { available: false } }));
vi.mock("./SettingsPanel.svelte", () => ({ default: () => {} }));
vi.mock("../../lib/connection/screen_binding.ts", () => ({
  sessionForPane: (_api: unknown, paneId: string) => ({ lastIsBaseShell: paneId.endsWith("idle") }),
}));
let localVersion: string | undefined;
vi.mock("../../stores/connections.svelte.ts", () => ({
  getLocalVersion: () => localVersion,
  hostViews: () => LIVE,
}));
vi.mock("../../stores/ports.svelte.ts", () => ({
  portForwarding: vi.fn(),
  hostPortLists: () => PORTS,
  reverseForwarding: vi.fn(),
  hostShareLists: () => [],
}));

const PORTS = [
  { target: "dev-vm", rows: [
    { port: 5173, process: "vite", forward: { target: "dev-vm", remote: 5173, local: 5173, state: "up", url: "http://localhost:5173" }, saved: { target: "dev-vm", remote: 5173, local: 5173, label: "Frontend", enabled: true } },
    { port: 8080, saved: { target: "dev-vm", remote: 8080, local: 18080, label: "API", enabled: true }, error: { code: "local_port_conflict", message: "Local port 18080 is in use.", hint: "Choose an unused port." } },
    { port: 35081, process: "python" },
  ] },
  { target: "saved-vm", rows: [{ port: 8000, saved: { target: "saved-vm", remote: 8000, local: 18000, label: "API", enabled: false } }] },
];
const CATALOG: HostCatalog[] = [{ target: "saved-vm", lastSeen: 1_790_000_000, screens: [{ id: "old", name: "Old work", panes: 2 }] }];

/** Panes as dockview lists them, for the screens that show any. */
function screenApis() {
  const panel = (id: string, title: string, group: string) => ({ id, title, group: { id: group } });
  return new Map([
    ["local", { panels: [panel("a", "✳ fresh", "g1"), panel("b", "✳ Remote session UX", "g1")] }],
    ["local-2", { panels: [panel("c-idle", "zsh: ~", "g1")] }],
    ["remote", { panels: [panel("d", "uv run pytest -n 4", "g1")] }],
  ]) as never;
}

function props(withPanes = false) {
  return {
    screenApis: withPanes ? screenApis() : new Map(), catalog: CATALOG, recentHosts: [],
    onCloseScreen: vi.fn(), onTerminateScreen: vi.fn(), onActivatePane: vi.fn(), onOpenHostScreen: vi.fn(),
    onConnectHost: vi.fn(), onBrowseHost: vi.fn(), onNewHostScreen: vi.fn(), onUpdateHost: vi.fn(),
  };
}

function panelProps() {
  const groups = hostGroups(SCREENS, LIVE, [], CATALOG, PORTS);
  return {
    groups, portLists: PORTS as never, portTargets: groups.map((g) => g.target), focus: null, activeScreenId: "remote",
    onShowScreen: vi.fn(), onOpenHostScreen: vi.fn(), onConnectHost: vi.fn(), onBrowseHost: vi.fn(),
    onNewHostScreen: vi.fn(), onConnectNew: vi.fn(), onOpenWindow: vi.fn(), onUpdateHost: vi.fn(),
    onSetHostAlias: vi.fn(), onForgetHost: vi.fn(),
  };
}

describe("screen list", () => {
  it("lists remote screens like local ones, under one caption per host with open screens", () => {
    const callbacks = props();
    const { body } = render(Drawer, { props: callbacks });
    expect(body).toMatch(/<button[^>]*aria-current="true"[^>]*>Frontend<\/button>/);
    expect(body).toContain('aria-label="Screens on dev-vm"');
    expect(body).toContain('aria-label="New screen on dev-vm"');
    // A host without open screens, and every port control, stay in the Hosts panel.
    expect(body).not.toContain("saved-vm");
    expect(body).not.toContain("Resume");
    expect(body).toContain("New screen");
    expect(callbacks.onBrowseHost).not.toHaveBeenCalled();
    expect(callbacks.onConnectHost).not.toHaveBeenCalled();
    expect(portForwarding).not.toHaveBeenCalled();
  });

  it("shows the host groups in the order the user arranged, below the local screens (Host group moved)", () => {
    SCREENS.push({ id: "prod", name: "Deploy", conn: "host:prod-vm", wireId: "prod" } as Screen);
    HOST_ORDER.push("prod-vm");
    try {
      const { body } = render(Drawer, { props: props() });
      const local = body.indexOf('aria-label="Local screens"');
      const prod = body.indexOf('aria-label="Screens on prod-vm"');
      const dev = body.indexOf('aria-label="Screens on dev-vm"');
      expect(local).toBeGreaterThanOrEqual(0);
      expect(prod).toBeGreaterThan(local);
      expect(dev).toBeGreaterThan(prod);
    } finally {
      SCREENS.pop();
      HOST_ORDER.length = 0;
    }
  });

  it("offers a host's ports from its caption, counting the forwarded ones", () => {
    const { body } = render(Drawer, { props: props() });
    expect(body).toMatch(/<button[^>]*class="drawer-host-ports[^"]*\bforwarded"[^>]*aria-label="Ports on dev-vm"[^>]*>[\s\S]*?<span[^>]*>1<\/span>/);
  });
});

describe("Hosts panel", () => {
  it("shows a display name with its real SSH address and a menu even on folded hosts", () => {
    const callbacks = panelProps();
    callbacks.groups[1] = { ...callbacks.groups[1], alias: "Home Lab" };
    const { body } = render(HostsPanel, { props: callbacks });
    expect(body).toContain("Home Lab");
    expect(body).toMatch(/class="address[^>]*>saved-vm<\/span>/);
    expect(body).toContain('aria-label="More for host Home Lab"');
    expect(body).toContain('aria-label="More for host dev-vm"');
    expect(callbacks.onSetHostAlias).not.toHaveBeenCalled();
    expect(callbacks.onForgetHost).not.toHaveBeenCalled();
  });

  it("unfolds hosts with open screens and keeps the others folded without connecting them", () => {
    const callbacks = panelProps();
    const { body } = render(HostsPanel, { props: callbacks });
    expect(body).toMatch(/<button[^>]*class="host-head[^"]*"[^>]*aria-expanded="true"[^>]*title="dev-vm/);
    expect(body).toMatch(/<button[^>]*class="host-head[^"]*"[^>]*aria-expanded="false"[^>]*title="saved-vm/);
    expect(body).toContain("Not connected · 1 last seen");
    expect(body).toContain("In window");
    expect(body).toContain("API");
    expect(body).toContain('aria-label="Ports on dev-vm"');
    expect(body).not.toContain('aria-label="Ports on saved-vm"');
    expect(body).toContain("New window");
    expect(callbacks.onBrowseHost).not.toHaveBeenCalled();
    expect(callbacks.onConnectHost).not.toHaveBeenCalled();
  });

  it("offers an update for a connected host of another version, and only then", () => {
    const daemon = { version: "0.1.1", protocol: 1 };
    const withVersion = () => {
      const p = panelProps();
      p.groups[0] = { ...p.groups[0], state: { phase: "ready", progress: [], attempt: 0, ready: { daemon } } as never };
      return p;
    };
    localVersion = "0.1.3";
    expect(render(HostsPanel, { props: withVersion() }).body).toContain("Runs Splitty 0.1.1; this app is 0.1.3.");
    localVersion = "0.1.1";
    expect(render(HostsPanel, { props: withVersion() }).body).not.toContain("Update…");
    localVersion = undefined;
    expect(render(HostsPanel, { props: panelProps() }).body).not.toContain("Update…");
  });

  it("offers a remote window only where the shell can open one", () => {
    const { body } = render(HostsPanel, { props: { ...panelProps(), onOpenWindow: undefined } });
    expect(body).not.toContain("New window");
  });
});

// Optional review artifact from these same fixtures; no browser, server, or network.
// SPLITTY_UX_EVIDENCE=/tmp/sidebar.html pnpm test src/components/chrome/drawer.test.ts
afterAll(async () => {
  const output = process.env.SPLITTY_UX_EVIDENCE;
  if (!output) return;
  const { readFileSync, writeFileSync } = await import("node:fs");
  const { resolve } = await import("node:path");
  const { compile } = await import("svelte/compiler");
  const css = ["Drawer", "HostsPanel", "HostPorts", "PortRow", "ManualPortForm"].map((name) => {
    const filename = resolve(`src/components/chrome/${name}.svelte`);
    return compile(readFileSync(filename, "utf8"), { filename, generate: "server", dev: true }).css?.code ?? "";
  }).join("\n");
  const themes = {
    dark: "--splitty-surface-base:#0f0f0f;--splitty-surface-raised:#1a1a1a;--splitty-text-primary:#c8c8c8;--splitty-text-secondary:#8a8a8a;--splitty-text-muted:#5c5c5c;--splitty-border-subtle:#242424;--splitty-border-default:#2e2e2e;--splitty-hover-bg:rgba(255,255,255,.05);--splitty-active-bg:rgba(255,255,255,.09);--splitty-accent:#63a0ff;--splitty-success:#98c379;--splitty-warning:#e5c07b;--splitty-danger:#e06c75;--splitty-danger-hover:#e06c7529;--splitty-scrollbar:rgba(255,255,255,.08)",
    light: "--splitty-surface-base:#ffffff;--splitty-surface-raised:#f0f0f0;--splitty-text-primary:#252525;--splitty-text-secondary:#555555;--splitty-text-muted:#8a8a8a;--splitty-border-subtle:#e0e0e0;--splitty-border-default:#d4d4d4;--splitty-hover-bg:rgba(0,0,0,.04);--splitty-active-bg:rgba(0,0,0,.07);--splitty-accent:#3589d6;--splitty-success:#588c39;--splitty-warning:#ad7a35;--splitty-danger:#c24a53;--splitty-danger-hover:#c24a5329;--splitty-scrollbar:rgba(0,0,0,.1)",
  };
  const sections = Object.entries(themes).map(([name, vars]) => {
    const drawer = render(Drawer, { props: props(true), idPrefix: `${name}-drawer-` }).body;
    // The preview shows dev-vm on an older Splitty, with its Update button.
    localVersion = "0.1.3";
    const preview = panelProps();
    preview.groups[0] = { ...preview.groups[0], state: { phase: "ready", progress: [], attempt: 0, ready: { daemon: { version: "0.1.2", protocol: 1 } } } as never };
    const panel = render(HostsPanel, { props: preview, idPrefix: `${name}-panel-` }).body;
    localVersion = undefined;
    return `<article style="${vars}"><h2>${name}</h2><div class="row"><div class="preview">${drawer}</div><div class="panel">${panel}</div></div></article>`;
  }).join("\n");
  writeFileSync(output, `<!doctype html><html lang="en"><meta charset="utf-8"><title>Sidebar preview</title>
<style>
*{box-sizing:border-box}body{margin:24px;background:#777;font:12px system-ui}h2{font-size:12px;margin:12px 0;color:#fff}article{margin-bottom:24px}.row{display:flex;align-items:start;gap:24px}
.preview{width:240px;height:640px;position:relative;background:var(--splitty-surface-base)}.preview .drawer-panel,.preview .drawer-resize-handle,.preview .drawer-backdrop{display:none}.preview .drawer{position:absolute;inset:0;transform:none}.preview .drawer-sidebar{height:100%}
.panel{width:340px;padding:20px;background:var(--splitty-surface-raised)}
${css}
</style><main>${sections}</main></html>`);
});
