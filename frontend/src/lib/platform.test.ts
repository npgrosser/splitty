import { describe, it, expect, vi, afterEach } from "vitest";

/** Load `platform.ts` as a page with these globals and this query string would. */
async function loadPlatform(search: string, globals: Record<string, unknown> = {}) {
  vi.resetModules();
  vi.stubGlobal("window", {
    location: { search },
    matchMedia: () => ({ matches: false }),
    ...globals,
  });
  return import("./platform.ts");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("shellKind", () => {
  it("names the shell whose bridge the page has", async () => {
    expect((await loadPlatform("", { __TAURI_INTERNALS__: {} })).shellKind).toBe("tauri");
    expect((await loadPlatform("", { __SPLITTY_ELECTRON__: {} })).shellKind).toBe("electron");
    expect((await loadPlatform("")).shellKind).toBe("web");
  });

  /// desktop-remote: the session URL of a remote window carries `host`. Tauri
  /// injects its bridge into that window as well, but the page there is the
  /// host's and has no native rights, so it behaves as in a browser.
  it("is web in a remote session, even with Tauri's bridge injected", async () => {
    const platform = await loadPlatform("?bootstrap=abc&host=vm1", { __TAURI_INTERNALS__: {} });

    expect(platform.isRemoteSession).toBe(true);
    expect(platform.shellKind).toBe("web");
    expect(platform.isDesktop).toBe(false);
  });

  it("stays web after the bootstrap code left the URL", async () => {
    const platform = await loadPlatform("?host=user%40vm1%3A2222", { __TAURI_INTERNALS__: {} });

    expect(platform.shellKind).toBe("web");
  });
});
