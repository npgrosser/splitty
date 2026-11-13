import { apiFetch } from "./protocol/http.ts";
import type { Template, TemplateSummary } from "./protocol/types.ts";

export async function listTemplates(): Promise<TemplateSummary[]> {
  const res = await apiFetch("/api/templates");
  if (!res.ok) throw new Error("Failed to fetch templates");
  return res.json();
}

export async function getTemplate(slug: string): Promise<Template> {
  const res = await apiFetch(`/api/templates/${encodeURIComponent(slug)}`);
  if (!res.ok) {
    const message = await res.text();
    throw new Error(message || `Failed to fetch template ${slug}`);
  }
  return res.json();
}

export async function deleteTemplate(slug: string): Promise<void> {
  const res = await apiFetch(`/api/templates/${encodeURIComponent(slug)}`, {
    method: "DELETE",
  });
  if (!res.ok) {
    const message = await res.text();
    throw new Error(message || "Failed to delete template");
  }
}

/** Where a save by name went, or the file of that name that already exists. */
export type TemplateSaveResult =
  | { saved: true; slug: string; replaced: boolean }
  | { saved: false; slug: string };

/**
 * Save `template` under its name (`templates`, Saved Template Names): the
 * server names the file and, without `overwrite`, saves only when no file of
 * that name exists.
 */
export async function saveTemplate(template: Template, overwrite: boolean): Promise<TemplateSaveResult> {
  const res = await apiFetch("/api/templates", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ template, overwrite }),
  });
  if (res.status === 409) {
    const body = await res.json().catch(() => ({}));
    return { saved: false, slug: typeof body?.slug === "string" ? body.slug : "" };
  }
  if (!res.ok) {
    const message = await res.text();
    throw new Error(message || "Failed to save template");
  }
  const body = await res.json();
  return { saved: true, slug: String(body.slug ?? ""), replaced: body.replaced === true };
}
