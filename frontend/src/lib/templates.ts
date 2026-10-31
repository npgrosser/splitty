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

export async function saveTemplate(slug: string, template: Template): Promise<void> {
  const res = await apiFetch(`/api/templates/${encodeURIComponent(slug)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(template),
  });
  if (!res.ok) {
    const message = await res.text();
    throw new Error(message || "Failed to save template");
  }
}
