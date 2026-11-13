import { afterEach, expect, it, vi } from "vitest";
import type { NativeRequest } from "./native_api.ts";
import type { Template } from "./types.ts";

vi.mock("../platform.ts", () => ({ shellKind: "web", nativeShell: undefined, getDesktopPlatform: () => "linux" }));
vi.mock("../window.ts", () => ({ windowAPI: { setTitleBarStyle: vi.fn(), setBackgroundColor: vi.fn() } }));
afterEach(() => { vi.resetModules(); vi.unstubAllGlobals(); });

it.each(["browser", "native"])("preserves feature data and domain errors through the %s adapter after an origin change", async mode => {
  vi.resetModules();
  const storage = { getItem: vi.fn(() => { throw Error("previous origin unavailable"); }), setItem: vi.fn() };
  vi.stubGlobal("localStorage", storage);
  vi.stubGlobal("window", { matchMedia: () => ({ matches: true, addEventListener: vi.fn() }), location: { search: "" } });
  vi.stubGlobal("document", { documentElement: { style: { setProperty: vi.fn(), removeProperty: vi.fn() }, dataset: {}, setAttribute: vi.fn(), removeAttribute: vi.fn() } });
  let settings: Record<string, Record<string, unknown>> = { drawer: { pinned: false }, appearance: { theme: "saved-theme" } };
  let profiles = { defaultProfileId: "custom", profiles: [{ id: "custom", name: "My shell", command: "/bin/sh", args: [], enabled: true, target: "local" as const, source: "custom" as const, available: true, experimental: false }] };
  const templates = new Map<string, Template>();
  let order: string[] = ["vm1"];
  const operations: string[] = [];
  const answer = (body: unknown, status = 200) => ({ status, contentType: "json" as const, body: JSON.stringify(body) });
  const domain = ({ operation, args }: NativeRequest) => {
    operations.push(operation);
    switch (operation) {
      case "home.get": return answer(settings);
      case "home.update":
        for (const [section, value] of Object.entries(args.body as typeof settings)) settings[section] = { ...settings[section], ...value };
        return answer(settings);
      case "theme.get": return answer({ name: args.name ?? "saved-theme", variant: "dark", ui: { "surface-base": "#112233" }, terminal: { bg: "#112233" } });
      case "shellProfiles.get": return answer(profiles);
      case "shellProfiles.update":
        if (!(args.body as typeof profiles).defaultProfileId) return answer({ error: "Choose a default shell" }, 422);
        profiles = args.body as typeof profiles; return answer(profiles);
      case "template.create": {
        const { template, overwrite } = args.body as { template: Template; overwrite: boolean };
        const slug = template.name.toLowerCase().replace(/[^a-z0-9]+/g, "-");
        const replaced = templates.has(slug);
        if (replaced && !overwrite) return answer({ error: "template exists", slug }, 409);
        templates.set(slug, template); return answer({ slug, replaced });
      }
      case "template.get": return templates.has(String(args.slug)) ? answer(templates.get(String(args.slug))) : { status: 404, contentType: "text" as const, body: "Template missing" };
      case "template.list": return answer([...templates].map(([slug, template]) => ({ slug, displayName: template.name })));
      case "template.delete": templates.delete(String(args.slug)); return { status: 204, contentType: "empty" as const, body: "" };
      case "hostOrder.get": return answer({ version: 1, order });
      case "hostOrder.update": order = (args.body as { order: string[] }).order; return answer({ version: 1, order });
      default: throw Error(`Unexpected domain operation ${operation}`);
    }
  };
  const browserFetch = vi.fn(async (path: string, init?: RequestInit) => {
    expect(new Headers(init?.headers).get("X-Splitty-Gateway")).toBe("fixture-gateway");
    const url = new URL(path, "http://gateway.invalid");
    const method = init?.method ?? "GET";
    const routes: Record<string, string> = { "/api/home/settings": method === "PATCH" ? "home.update" : "home.get", "/api/home/theme": "theme.get", "/api/shell-profiles": method === "PUT" ? "shellProfiles.update" : "shellProfiles.get", "/api/remote/host-order": method === "PUT" ? "hostOrder.update" : "hostOrder.get" };
    const operation = url.pathname.startsWith("/api/templates/") ? (({ GET: "template.get", POST: "template.save", DELETE: "template.delete" } as Record<string, string>)[method]) : url.pathname === "/api/templates" ? (method === "POST" ? "template.create" : "template.list") : routes[url.pathname];
    if (!operation) throw Error("Unexpected browser route");
    const reply = domain({ operation, args: { ...(init?.body ? { body: JSON.parse(String(init.body)) } : {}), ...(url.searchParams.has("name") ? { name: url.searchParams.get("name") } : {}), ...(url.pathname.startsWith("/api/templates/") ? { slug: decodeURIComponent(url.pathname.slice("/api/templates/".length)) } : {}) } });
    return new Response(reply.contentType === "empty" ? null : reply.body, { status: reply.status, headers: { "Content-Type": reply.contentType === "json" ? "application/json" : "text/plain" } });
  });
  vi.stubGlobal("fetch", browserFetch);
  const assign = async () => {
    const http = await import("./http.ts");
    if (mode === "native") http.assignNativeApi({ request: async request => ({ id: 1, ...domain(request) }) });
    else http.assignBrowserGateway("fixture-gateway");
  };
  await assign();
  let home = await import("../settings.ts");
  await home.loadSettings();
  expect(home.getSettings().appearance.theme).toBe("saved-theme");
  await home.updateSetting("drawer", "pinned", true);
  const shell = await import("../shell_profiles.ts");
  await shell.loadShellProfiles();
  await shell.saveShellProfiles({ ...profiles, profiles: profiles.profiles.map(profile => ({ ...profile, name: "Updated shell" })) });
  await expect(shell.saveShellProfiles({ defaultProfileId: "", profiles: [] })).rejects.toThrow("Choose a default shell");
  const theme = await import("../theme.ts");
  expect((await theme.reloadTheme("chosen-theme")).name).toBe("chosen-theme");
  const template = await import("../templates.ts");
  const value: Template = { name: "Saved workspace", panes: { pane: { command: "echo hi" } }, layout: { slot: ["pane"] } };
  expect(await template.saveTemplate(value, false)).toEqual({ saved: true, slug: "saved-workspace", replaced: false });
  expect(await template.saveTemplate(value, false)).toEqual({ saved: false, slug: "saved-workspace" });
  expect(await template.saveTemplate(value, true)).toEqual({ saved: true, slug: "saved-workspace", replaced: true });
  expect(await template.getTemplate("saved-workspace")).toEqual(value);
  expect(await template.listTemplates()).toEqual([{ slug: "saved-workspace", displayName: value.name }]);
  await template.deleteTemplate("saved-workspace");
  await expect(template.getTemplate("saved-workspace")).rejects.toThrow("Template missing");
  const { HostOrderStore } = await import("../remote/host_order.ts");
  const { apiFetch } = await import("./http.ts");
  const changed = vi.fn(); const hostOrder = new HostOrderStore({ fetch: apiFetch, changed });
  await hostOrder.save(["vm2", "vm1"]); await hostOrder.load();
  expect(changed).toHaveBeenLastCalledWith(["vm2", "vm1"]);
  // Reload the UI modules against the same service state, with no previous
  // origin's localStorage available and no mutation replay.
  const mutations = operations.filter(operation => ["home.update", "shellProfiles.update", "template.create", "template.delete", "hostOrder.update"].includes(operation));
  vi.resetModules(); await assign(); home = await import("../settings.ts");
  await home.loadSettings(); expect(home.getSettings().drawer.pinned).toBe(true);
  expect((await (await import("../shell_profiles.ts")).loadShellProfiles()).profiles[0].name).toBe("Updated shell");
  expect(operations.filter(operation => ["home.update", "shellProfiles.update", "template.create", "template.delete", "hostOrder.update"].includes(operation))).toEqual(mutations);
  expect(storage.getItem).not.toHaveBeenCalled(); expect(storage.setItem).not.toHaveBeenCalled();
  if (mode === "native") expect(browserFetch).not.toHaveBeenCalled();
});
