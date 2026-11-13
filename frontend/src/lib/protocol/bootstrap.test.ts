import { afterEach, expect, it, vi } from "vitest";
import type { TauriNativeBridge } from "./tauri_channel.ts";

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });
async function page(protocol: string, hostname: string, native?: Partial<TauriNativeBridge>) {
  vi.resetModules();
  const fetch = vi.fn(); vi.stubGlobal("fetch", fetch);
  vi.stubGlobal("window", { location: { protocol, hostname, port: "", search: "?host=forged&bootstrap=untrusted" },
    matchMedia: () => ({ matches: false }), addEventListener: () => {},
    __SPLITTY_TAURI__: { native, ready: Promise.resolve({ role: "main" }) }, __TAURI_INTERNALS__: {} });
  return { fetch, bootstrap: (await import("./bootstrap.ts")).bootstrapConnection };
}
it("keeps an HTTP browser on its gateway despite forged shell globals", async () => {
  const open = vi.fn(), p = await page("http:", "127.0.0.1", { open });
  const finish = await p.bootstrap(); finish();
  expect(open).not.toHaveBeenCalled(); expect(p.fetch).not.toHaveBeenCalled();
  expect((await import("./connection.ts")).rootAssignment).toBeUndefined();
  expect((await import("../platform.ts")).shellKind).toBe("web");
});
it("latches packaged native mode before a failed assignment without HTTP fallback", async () => {
  const p = await page("tauri:", "localhost", { open: async () => { throw Error("not assigned"); }, cancelOpen: vi.fn() });
  await expect(p.bootstrap()).rejects.toThrow("not assigned");
  const http = await import("./http.ts");
  await expect(http.apiFetch("/api/home/settings")).rejects.toThrow();
  await expect(http.exchangeBootstrap("untrusted")).rejects.toThrow("unavailable");
  await expect(http.fetchWsTicket()).rejects.toThrow("unavailable");
  expect(p.fetch).not.toHaveBeenCalled();
});
it("separates startup reads from application welcome and uses the assigned role", async () => {
  let sequence = 0;
  const native: TauriNativeBridge = {
    state: vi.fn(),
    open: vi.fn(async () => ({ handle: `client-${++sequence}`, document: "doc", instance: "instance", generation: 1, role: "main" as const })),
    cancelOpen: vi.fn(), listen: vi.fn(() => () => {}), start: vi.fn(async () => {}), close: vi.fn(),
    write: vi.fn(async () => {}), consumed: vi.fn(async () => {}),
  };
  const p = await page("tauri:", "localhost", native), finish = await p.bootstrap();
  const root = await import("./connection.ts");
  expect(root.rootAssignment?.role).toBe("main");
  expect((await import("../platform.ts")).isRemoteSession).toBe(false);
  expect((await import("../platform.ts")).shellKind).toBe("tauri");
  expect(native.start).toHaveBeenCalledTimes(1);
  finish(); expect(native.close).toHaveBeenCalledWith("client-1");
  expect(native.open).toHaveBeenCalledTimes(1);
  await root.socket.connect(); expect(native.open).toHaveBeenCalledTimes(2);
  root.socket.dispose(); expect(p.fetch).not.toHaveBeenCalled();
});
it("reports Tauri recovery expiry and retries the same assigned document on a native retry event", async () => {
  vi.useFakeTimers(); vi.resetModules();
  const callbacks = new Map<string, () => void>();
  const assigned = { handle: "client", document: "doc", instance: "instance", generation: 1, role: "remote" as const, target: "vm1" };
  const native = {
    state: vi.fn(), cancelOpen: vi.fn(), listen: vi.fn(() => () => {}),
    start: vi.fn(async () => {}), write: vi.fn(async () => {}), consumed: vi.fn(async () => {}), close: vi.fn(),
    open: vi.fn(async () => assigned),
  };
  const fetch = vi.fn(); vi.stubGlobal("fetch", fetch);
  vi.stubGlobal("window", { location: { protocol: "tauri:", hostname: "localhost", port: "", search: "" },
    matchMedia: () => ({ matches: false }), addEventListener: (event: string, callback: () => void) => callbacks.set(event, callback),
    __SPLITTY_TAURI__: { native, ready: Promise.resolve({ role: "remote", target: "vm1" }) } });
  const { bootstrapConnection } = await import("./bootstrap.ts");
  const finish = await bootstrapConnection(); finish();
  const { socket } = await import("./connection.ts");
  const { NativeConnectionError } = await import("./native_socket.ts");
  const connection = socket as import("./native_connection.ts").NativeConnection;
  await connection.connect();
  native.open.mockRejectedValue(new NativeConnectionError("SSH unavailable", "backendLost", true));
  connection.revoke();
  connection.onError?.(new NativeConnectionError("SSH lost", "backendLost", true));
  await vi.advanceTimersByTimeAsync(620_000);
  expect(connection.connectionState).toBe("disconnected");
  expect(native.state).toHaveBeenLastCalledWith("disconnected");
  native.open.mockResolvedValue({ ...assigned, handle: "replacement", generation: 2 });
  callbacks.get("splitty-native-retry")!();
  for (let index = 0; index < 20; index++) await Promise.resolve();
  expect(connection.connectionState).toBe("connected");
  expect(native.state).toHaveBeenLastCalledWith("connected");
  expect(connection.assignment?.document).toBe("doc");
  expect(window.location.protocol).toBe("tauri:");
  expect(fetch).not.toHaveBeenCalled();
  connection.dispose(); vi.useRealTimers();
});
