import { describe, expect, it, vi } from "vitest";

const { apiFetch } = vi.hoisted(() => ({ apiFetch: vi.fn() }));
vi.mock("./protocol/http.ts", () => ({ apiFetch }));

vi.stubGlobal("window", { location: { search: "" }, matchMedia: () => ({ matches: false, addEventListener: vi.fn() }) });
vi.mock("./window.ts", () => ({ windowAPI: {} }));
vi.mock("./settings.ts", () => ({ getSettings: vi.fn(), settings: vi.fn() }));

const { listThemeNames } = await import("./theme.ts");

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

describe("listThemeNames", () => {
  it("asks the route the server serves and returns the names", async () => {
    apiFetch.mockResolvedValueOnce(json(["dark", "light"]));
    expect(await listThemeNames()).toEqual(["dark", "light"]);
    expect(apiFetch).toHaveBeenCalledWith("/api/home/themes");
  });
  it("is empty when the server errs, is unreachable or answers another shape", async () => {
    apiFetch.mockResolvedValueOnce(json({ error: "x" }, 500));
    expect(await listThemeNames()).toEqual([]);
    apiFetch.mockRejectedValueOnce(new Error("offline"));
    expect(await listThemeNames()).toEqual([]);
    apiFetch.mockResolvedValueOnce(json({ themes: [] }));
    expect(await listThemeNames()).toEqual([]);
  });
});
