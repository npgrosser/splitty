/** Preferences choose an editor; they never grant permission to launch it. */
import type { PathKind } from "./editor_target.ts";
export type AppEditor = "vscode" | "zed";
export interface ExtensionRule { extension: string; editor: AppEditor }
export interface ProjectRule { connection: string; path: string; editor: AppEditor }
export interface EditorPreferences {
  file: AppEditor;
  folder: AppEditor;
  extensions: ExtensionRule[];
  projects: ProjectRule[];
}
export const editorDefaults: EditorPreferences = { file: "vscode", folder: "vscode", extensions: [], projects: [] };
export function isAppEditor(value: unknown): value is AppEditor { return value === "vscode" || value === "zed"; }
export function editorPreferences(value: unknown): EditorPreferences {
  const data = (value && typeof value === "object" ? value : {}) as Partial<EditorPreferences>;
  return {
    file: isAppEditor(data.file) ? data.file : "vscode",
    folder: isAppEditor(data.folder) ? data.folder : "vscode",
    extensions: Array.isArray(data.extensions) ? data.extensions.filter((r) => r && typeof r.extension === "string" && /^\.?[a-zA-Z0-9_-]{1,32}$/.test(r.extension) && isAppEditor(r.editor)).slice(0, 100) : [],
    projects: Array.isArray(data.projects) ? data.projects.filter((r) => r && typeof r.connection === "string" && (r.connection === "local" || (r.connection.startsWith("host:") && r.connection.length > 5 && r.connection.length <= 260)) && typeof r.path === "string" && r.path.startsWith("/") && r.path.length <= 4096 && !r.path.split("/").some((p) => p === "." || p === "..") && isAppEditor(r.editor)).slice(0, 100) : [],
  };
}
export function preferredEditor(value: unknown, connection: string, path: string, kind: PathKind, explicit?: AppEditor): AppEditor {
  if (isAppEditor(explicit)) return explicit;
  const prefs = editorPreferences(value);
  const project = prefs.projects.filter((r) => {
    const prefix = r.path.replace(/\/+$/, "") || "/";
    return r.connection === connection && (prefix === "/" || path === prefix || path.startsWith(prefix + "/"));
  }).sort((a, b) => b.path.replace(/\/+$/, "").length - a.path.replace(/\/+$/, "").length)[0];
  if (project) return project.editor;
  if (kind === "file") {
    const name = path.split("/").at(-1)!;
    const suffix = name.includes(".") ? name.split(".").at(-1)!.toLowerCase() : "";
    const extension = prefs.extensions.find((r) => r.extension.replace(/^\./, "").toLowerCase() === suffix);
    if (extension) return extension.editor;
  }
  return prefs[kind];
}
