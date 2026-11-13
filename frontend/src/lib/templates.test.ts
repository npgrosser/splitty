import { afterEach, describe, expect, it, vi } from "vitest";
const { apiFetch } = vi.hoisted(() => ({ apiFetch: vi.fn() }));
vi.mock("./protocol/http.ts", () => ({ apiFetch }));
import { saveTemplate } from "./templates.ts";
import type { Template } from "./protocol/types.ts";

afterEach(() => vi.clearAllMocks());
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
const template: Template = { name: "v1.2", panes: { shell: { command: "make" } }, layout: { slot: "shell" } };

// templates, Saved Template Names
describe("saving a template by name", () => {
  it("sends the name and lets the server name the file", async () => {
    apiFetch.mockResolvedValueOnce(json({ slug: "v1-2", replaced: false }));
    expect(await saveTemplate(template, false)).toEqual({ saved: true, slug: "v1-2", replaced: false });
    const [path, init] = apiFetch.mock.calls[0];
    expect(path).toBe("/api/templates");
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body)).toEqual({ template, overwrite: false });
  });

  it("reports the existing file on 409 and overwrites only when asked", async () => {
    apiFetch.mockResolvedValueOnce(json({ error: "template exists", slug: "v1-2" }, 409));
    expect(await saveTemplate(template, false)).toEqual({ saved: false, slug: "v1-2" });
    apiFetch.mockResolvedValueOnce(json({ slug: "v1-2", replaced: true }));
    expect(await saveTemplate(template, true)).toEqual({ saved: true, slug: "v1-2", replaced: true });
    expect(JSON.parse(apiFetch.mock.calls[1][1].body).overwrite).toBe(true);
  });

  it("throws the server's refusal", async () => {
    apiFetch.mockResolvedValueOnce(new Response("no letters or digits", { status: 400 }));
    await expect(saveTemplate({ ...template, name: "..." }, false)).rejects.toThrow("no letters or digits");
  });
});
