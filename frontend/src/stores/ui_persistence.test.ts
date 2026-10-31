import { afterEach, describe, expect, it, vi } from "vitest";

const { apiFetch } = vi.hoisted(() => ({ apiFetch: vi.fn() }));
vi.mock("../lib/protocol/http.ts", () => ({ apiFetch }));

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); vi.clearAllMocks(); });

describe("drawer pin persistence", () => {
  it("restores pin and unpin from app settings with no previous browser storage", async () => {
    let stored = { drawer: { pinned: false, sidebarWidth: 280 } };
    const storage = { getItem: vi.fn(() => { throw new Error("old origin unavailable"); }), setItem: vi.fn() };
    vi.stubGlobal("localStorage", storage);
    apiFetch.mockImplementation(async (path: string, init?: RequestInit) => {
      expect(path).toBe("/api/home/settings");
      if (init?.method === "PATCH") {
        const patch = JSON.parse(String(init.body));
        stored = { drawer: { ...stored.drawer, ...patch.drawer } };
      }
      return new Response(JSON.stringify(stored));
    });

    for (const pinned of [true, false]) {
      vi.resetModules();
      const settings = await import("../lib/settings.ts");
      await settings.loadSettings();
      const ui = await import("./ui.svelte.ts");
      ui.setDrawerPinned(pinned);
      expect(ui.isDrawerPinned()).toBe(pinned);
      await vi.waitFor(() => expect(stored.drawer.pinned).toBe(pinned));

      // A new frontend instance has no access to the old origin's storage.
      vi.resetModules();
      const restartedSettings = await import("../lib/settings.ts");
      await restartedSettings.loadSettings();
      const restartedUi = await import("./ui.svelte.ts");
      expect(restartedUi.isDrawerPinned()).toBe(pinned);
      expect(restartedSettings.getSettings().drawer.sidebarWidth).toBe(280);
    }
    expect(storage.getItem).not.toHaveBeenCalled();
    expect(storage.setItem).not.toHaveBeenCalled();
  });
});
