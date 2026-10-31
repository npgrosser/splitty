import { describe, it, expect } from "vitest";
import { preferredEditor, editorPreferences } from "./editor_preferences.ts";

describe("local editor selection", () => {
  const settings = { folder: "zed", file: "vscode", extensions: [{ extension: ".rs", editor: "zed" }], projects: [
    { connection: "host:vm1", path: "/srv", editor: "zed" },
    { connection: "host:vm1", path: "/srv/app", editor: "vscode" },
  ] };
  it("uses explicit, longest project, extension, then kind default", () => {
    expect(preferredEditor(settings, "host:vm1", "/srv/app/file.rs", "file", "zed")).toBe("zed");
    expect(preferredEditor(settings, "host:vm1", "/srv/app/file.rs", "file")).toBe("vscode");
    expect(preferredEditor(settings, "host:vm1", "/srv/other/file.md", "file")).toBe("zed");
    expect(preferredEditor(settings, "host:vm2", "/srv/app/file.RS", "file")).toBe("zed");
    expect(preferredEditor(settings, "local", "/srv/app/file.md", "file")).toBe("vscode");
    expect(preferredEditor(settings, "local", "/srv/app", "folder")).toBe("zed");
  });
  it("matches directory boundaries and exact hosts", () => {
    expect(preferredEditor(settings, "host:vm1", "/srv/app-other/x.md", "file")).toBe("zed");
    expect(preferredEditor(settings, "host:vm1", "/srv-other/x.md", "file")).toBe("vscode");
    expect(preferredEditor(settings, "host:user@vm1", "/srv/x.md", "file")).toBe("vscode");
    expect(preferredEditor({ projects: [{ connection: "local", path: "/", editor: "zed" }] }, "local", "/a", "file")).toBe("zed");
  });
  it("ignores executable-like and malformed settings", () => {
    for (const value of [null, "sh", { file: "sh", folder: {}, extensions: [null, { extension: "rs", editor: "bash" }], projects: [null, { connection: "host:vm1", path: "/", editor: "sh" }] }]) {
      expect(preferredEditor(value, "vm1", "/x.rs", "file")).toBe("vscode");
    }
    expect(editorPreferences({ extensions: Array(200).fill({ extension: "rs", editor: "zed" }) }).extensions).toHaveLength(100);
  });
});
