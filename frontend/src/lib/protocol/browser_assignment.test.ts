import { afterEach, expect, it, vi } from "vitest";

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });
async function page() {
  vi.resetModules();
  vi.stubGlobal("window", { location: { protocol: "http:", search: "?host=forged&role=main" },
    matchMedia: () => ({ matches: false }), __SPLITTY_ELECTRON__: {}, __TAURI_INTERNALS__: {} });
  const fetch = vi.fn(); vi.stubGlobal("fetch", fetch);
  return { fetch, bootstrap: (await import("./browser_assignment.ts")).bootstrapBrowser };
}
const gateway = "splitty-session-" + "a".repeat(32);
const response = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });

it("assigns a remote root from the gateway and restricts commands despite its local UI key", async () => {
  const p = await page(); p.fetch.mockResolvedValue(response({ gateway, role: "remote", target: "me@192.0.2.1" }));
  const login = vi.fn(); await p.bootstrap(login);
  expect(login).not.toHaveBeenCalled();
  expect(p.fetch).toHaveBeenCalledExactlyOnceWith("/api/client-assignment", {
    credentials: "same-origin", cache: "no-store", signal: expect.any(AbortSignal),
  });
  const platform = await import("../platform.ts");
  expect(platform.isRemoteSession).toBe(true); expect(platform.shellKind).toBe("web");
  const { localConnection } = await import("../connection/registry.ts");
  const root = localConnection();
  expect(root.key).toBe("local"); expect(root.kind).toBe("remote"); expect(root.label).toBe("me@192.0.2.1");
  const { commandAllowed } = await import("../connection/routing.ts");
  expect(commandAllowed(root.kind, { type: "pane.paste" }, { allowClipboardWrite: true })).toBe(false);
  expect(() => (platform.assignPlatform("web", "main"))).toThrow(); root.dispose();
});

it("keeps an assigned local browser local despite the host query and shell globals", async () => {
  const p = await page(); p.fetch.mockResolvedValue(response({ gateway, role: "main" }));
  await p.bootstrap(vi.fn());
  const platform = await import("../platform.ts");
  expect(platform.isRemoteSession).toBe(false); expect(platform.isDesktop).toBe(false);
  expect((await import("./connection.ts")).rootAssignment).toBeUndefined();
  expect((await import("./connection.ts")).nativeHostOpener).toBeUndefined();
});

it("waits for authentication and assignment before releasing workspace startup", async () => {
  const p = await page();
  p.fetch.mockResolvedValueOnce(response({}, 401))
    .mockResolvedValueOnce(response({}, 401)) // wrong token
    .mockResolvedValueOnce(response({})) // accepted token
    .mockResolvedValueOnce(response({ gateway, role: "remote", target: "192.0.2.2" }));
  await p.bootstrap(async submit => {
    const connection = await import("./connection.ts");
    expect(connection.browserAssignment).toBeUndefined();
    expect(await submit("wrong")).toBe(false);
    expect(connection.browserAssignment).toBeUndefined();
    expect(await submit("accepted")).toBe(true);
  });
  expect((await import("./connection.ts")).browserAssignment).toEqual({ gateway, role: "remote", target: "192.0.2.2" });
  expect(p.fetch.mock.calls.map(call => call[0])).toEqual([
    "/api/client-assignment", "/api/session", "/api/session", "/api/client-assignment",
  ]);
});

it.each([
  { gateway, role: "setup" }, { gateway, role: "main", target: "192.0.2.1" }, { gateway, role: "remote" },
  { gateway, role: "remote", target: "bad\nhost" }, { gateway, role: "remote", target: "x".repeat(1025) }, null,
])("rejects malformed assignment without falling back to a local root: %j", async value => {
  const p = await page(); p.fetch.mockResolvedValue(response(value));
  const login = vi.fn(); await expect(p.bootstrap(login)).rejects.toThrow("Invalid browser gateway assignment");
  expect(login).not.toHaveBeenCalled();
  expect((await import("./connection.ts")).browserAssignment).toBeUndefined();
});

it.each([403, 404, 503])("does not turn a gateway %s into a token prompt or local assignment", async status => {
  const p = await page(); p.fetch.mockResolvedValue(response({}, status));
  const login = vi.fn(); await expect(p.bootstrap(login)).rejects.toThrow("could not assign");
  expect(login).not.toHaveBeenCalled();
  expect((await import("./connection.ts")).browserAssignment).toBeUndefined();
});

it("never contacts a browser gateway for an assigned native document", async () => {
  const p = await page();
  const { NativeConnection } = await import("./native_connection.ts");
  const assignment = { handle: "root", document: "doc", instance: "instance", generation: 1, role: "main" as const };
  const root = new NativeConnection(async () => { throw Error("unused"); }, assignment);
  (await import("./connection.ts")).assignRootConnection(root, assignment);
  const login = vi.fn(); await p.bootstrap(login);
  expect(p.fetch).not.toHaveBeenCalled(); expect(login).not.toHaveBeenCalled(); root.dispose();
});

it("pins API mutations and reconnect tickets to the assigned gateway even if another tab changes its cookie", async () => {
  const p = await page(); p.fetch.mockResolvedValueOnce(response({ gateway, role: "main" }));
  const reload = vi.fn(); window.location.reload = reload;
  await p.bootstrap(vi.fn());
  const { apiFetch, fetchWsTicket } = await import("./http.ts");
  p.fetch.mockResolvedValueOnce(response({ ticket: "first" }));
  expect(await fetchWsTicket()).toBe("first");
  expect(new Headers(p.fetch.mock.calls[1]![1].headers).get("x-splitty-gateway")).toBe(gateway);
  p.fetch.mockResolvedValueOnce(response({ saved: true }));
  await apiFetch("/api/home/settings", { method: "PUT", headers: { "Content-Type": "application/json", "X-Splitty-Gateway": "forged" }, body: "{}" });
  expect(new Headers(p.fetch.mock.calls[2]![1].headers).get("x-splitty-gateway")).toBe(gateway);
  p.fetch.mockResolvedValueOnce(new Response("changed", { status: 409, headers: { "x-splitty-gateway-changed": "1" } }));
  expect(await fetchWsTicket()).toBeNull();
  expect(reload).toHaveBeenCalledOnce();
  await expect(apiFetch("/api/home/settings", { method: "PUT", body: "{}" })).rejects.toThrow("revoked");
  expect(await fetchWsTicket()).toBeNull();
  expect(p.fetch).toHaveBeenCalledTimes(4); expect(reload).toHaveBeenCalledOnce();
});

it("discards already pending API replies after gateway revocation", async () => {
  const p = await page(); p.fetch.mockResolvedValueOnce(response({ gateway, role: "main" }));
  window.location.reload = vi.fn(); await p.bootstrap(vi.fn());
  const { apiFetch, fetchWsTicket } = await import("./http.ts");
  let deliver!: (response: Response) => void;
  p.fetch.mockImplementationOnce(() => new Promise(resolve => { deliver = resolve; }));
  const pending = apiFetch("/api/home/settings");
  p.fetch.mockResolvedValueOnce(new Response("changed", { status: 409, headers: { "x-splitty-gateway-changed": "1" } }));
  await fetchWsTicket(); deliver(response({ late: true }));
  await expect(pending).rejects.toThrow("revoked");
});
