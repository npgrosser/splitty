import { describe, it, expect, vi, afterEach } from "vitest";

async function loadPlatform(search: string, globals: Record<string, unknown> = {}) {
  vi.resetModules();
  vi.stubGlobal("window", { location: { search }, matchMedia: () => ({ matches: false }), ...globals });
  return import("./platform.ts");
}
afterEach(() => vi.unstubAllGlobals());

describe("native platform assignment", () => {
  it("does not grant native UI capabilities from forged globals or host query parameters", async () => {
    const platform = await loadPlatform("?host=vm1&desktop=1", { __TAURI_INTERNALS__: {}, __SPLITTY_TAURI__: {} });
    expect(platform.shellKind).toBe("web");
    expect(platform.isRemoteSession).toBe(false);
    expect(platform.isDesktop).toBe(false);
  });
  it("uses the native assignment despite a conflicting query", async () => {
    const platform = await loadPlatform("?host=forged");
    platform.assignPlatform("tauri", "main");
    expect(platform.shellKind).toBe("tauri");
    expect(platform.isRemoteSession).toBe(false);
    expect(platform.isDesktop).toBe(true);
    expect(() => platform.assignPlatform("tauri", "remote")).toThrow();
  });
  it("keeps remote windows restricted even when no query names a host", async () => {
    const platform = await loadPlatform("", { __TAURI_INTERNALS__: {} });
    platform.assignPlatform("tauri", "remote");
    expect(platform.shellKind).toBe("web");
    expect(platform.isRemoteSession).toBe(true);
    expect(platform.isDesktop).toBe(false);
  });
});

it("can render server-side UI without browser globals or assigning native authority", async () => {
  vi.resetModules(); vi.unstubAllGlobals();
  const platform = await import("./platform.ts");
  expect(platform.shellKind).toBe("web");
  expect(platform.nativeShell).toBeUndefined();
  expect(platform.isDesktop).toBe(false);
  expect(platform.isRemoteSession).toBe(false);
  expect(platform.isPWA).toBe(false);
});
