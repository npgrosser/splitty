import { describe, expect, it, vi } from "vitest";
import folders from "../../../testdata/editor-folders.json";
import cases from "../../../testdata/remote-editor.json";
import { localEditorTarget, remoteEditorTarget, sshPath, type Editor } from "./editor_target.ts";
import { localEditorTarget as electronLocalTarget, remoteEditorTarget as electronTarget } from "../../../frontend-electron/src/editor_target.ts";
import { openLocalEditor, openRemoteEditor } from "../../../frontend-electron/src/editor.ts";

for (const [name, build] of [["browser", remoteEditorTarget], ["electron", electronTarget]] as const) {
  describe(`${name} editor targets`, () => {
    it.each(cases.valid)("preserves $target folder $cwd", ({ target, cwd, uri }) => {
      expect(build(target, cwd).value?.uri).toBe(uri);
      const parsed = new URL(uri);
      expect(parsed.protocol).toBe("vscode:");
      expect(parsed.host).toBe("vscode-remote");
      expect(parsed.search).toBe("");
      expect(parsed.hash).toBe("");
      expect(decodeURIComponent(parsed.pathname)).toBe(`/ssh-remote+${target}${cwd}${cwd.endsWith("/") ? "" : "/"}`);
    });
    it.each(cases.invalid)("rejects $target folder $cwd", ({ target, cwd }) => {
      expect(build(target, cwd).value).toBeUndefined();
      expect(build(target, cwd).reason).toBeTruthy();
    });
    it("rejects malformed Unicode and oversized or non-string data without throwing", () => {
      for (const [target, cwd] of [["x".repeat(256), "/srv"], ["vm1", "/" + "界".repeat(1366)], ["vm1", "/\ud800"], [null, "/srv"], ["vm1", {}]]) {
        expect(build(target, cwd).value).toBeUndefined();
      }
    });
    it("gives actionable alias guidance for encoded-authority ambiguity", () => {
      expect(build("abc", "/srv").reason).toContain("non-hex character");
    });
  });
}

const own = { role: "main", origin: "http://127.0.0.1:9800", frameOrigin: "http://127.0.0.1:9800", topFrame: true };

describe("Electron editor command", () => {
  it.each([
    { role: "remote" }, { role: "popout" }, { role: undefined }, { topFrame: false },
    { frameOrigin: "http://127.0.0.1:9801" }, { frameOrigin: "https://evil.example" },
    { frameOrigin: undefined }, { origin: undefined },
  ])("rejects untrusted sender %j before opening", async (change) => {
    const open = vi.fn();
    await expect(openRemoteEditor({ ...own, ...change }, { target: "vm1", cwd: "/srv" }, open)).rejects.toThrow();
    expect(open).not.toHaveBeenCalled();
  });
  it.each(cases.invalid)("rejects invalid arguments $target $cwd natively", async (input) => {
    const open = vi.fn();
    await expect(openRemoteEditor(own, input, open)).rejects.toThrow();
    expect(open).not.toHaveBeenCalled();
  });
  it("constructs the URI itself and propagates OS refusal", async () => {
    const open = vi.fn().mockRejectedValue(new Error("No protocol handler"));
    await expect(openRemoteEditor(own, { target: "user@192.0.2.10", cwd: "/srv/My App", uri: "file:///tmp/evil", executable: "sh" }, open)).rejects.toThrow("No protocol handler");
    expect(open).toHaveBeenCalledWith("vscode://vscode-remote/ssh-remote+user%40192.0.2.10/srv/My%20App/");
  });
});

describe("local editor targets", () => {
  it("opens a plain folder, or one in WSL through its distribution", () => {
    expect(localEditorTarget("/home/me/a b").value?.uri).toBe("vscode://file/home/me/a%20b/");
    expect(localEditorTarget("/home/me", "Ubuntu").value?.uri).toBe("vscode://vscode-remote/wsl+Ubuntu/home/me/");
  });
  it("refuses relative or unsafe folders and odd distribution names", () => {
    expect(localEditorTarget("relative").reason).toMatch(/local working directory/);
    expect(localEditorTarget("/a/../b").value).toBeUndefined();
    expect(localEditorTarget("/srv", "Ubuntu/../x").value).toBeUndefined();
  });
});

describe("SSH paths", () => {
  it("joins target and folder as scp, rsync and sshfs take them", () => {
    expect(sshPath("me@vm1", "/srv/My App")).toBe("me@vm1:/srv/My App");
    expect(sshPath("vm1", "/")).toBe("vm1:/");
    expect(sshPath("me@vm1:2222", "/srv")).toBe("ssh://me@vm1:2222/srv");
  });
  it("has none without an absolute folder or with control characters", () => {
    expect(sshPath("vm1", "")).toBeUndefined();
    expect(sshPath("vm1", "srv")).toBeUndefined();
    expect(sshPath("vm1", "/a\nb")).toBeUndefined();
  });
});

for (const [name, remote, local] of [["browser", remoteEditorTarget, localEditorTarget], ["electron", electronTarget, electronLocalTarget]] as const) {
  describe(`${name} local and Zed folder vectors`, () => {
    function build(input: { editor: string; cwd: string; target?: string; wslDistro?: string }) {
      return input.target !== undefined ? remote(input.target, input.cwd, input.editor as Editor) : local(input.cwd, input.wslDistro, input.editor as Editor);
    }
    it.each(folders.valid)("preserves $editor $cwd", (input) => expect(build(input).value?.uri).toBe(input.uri));
    it.each(folders.invalid)("rejects $editor $cwd", (input) => expect(build(input).value).toBeUndefined());
  });
}

describe("Electron local editor isolation", () => {
  it.each([{ role: "remote" }, { role: "popout" }, { topFrame: false }, { frameOrigin: "https://evil.example" }, { origin: undefined }])("rejects untrusted sender %j", async (change) => {
    const open = vi.fn();
    await expect(openLocalEditor({ ...own, ...change }, { editor: "zed", cwd: "/srv" }, open)).rejects.toThrow();
    expect(open).not.toHaveBeenCalled();
  });
  it.each(folders.invalid)("validates $editor $cwd before OS dispatch", async (input) => {
    const open = vi.fn();
    await expect((input.target !== undefined ? openRemoteEditor : openLocalEditor)(own, input, open)).rejects.toThrow();
    expect(open).not.toHaveBeenCalled();
  });
  it.each(folders.valid)("constructs $editor $cwd natively", async (input) => {
    const open = vi.fn().mockResolvedValue(undefined);
    await (input.target !== undefined ? openRemoteEditor : openLocalEditor)(own, { ...input, uri: "file:///evil", executable: "sh" }, open);
    expect(open).toHaveBeenCalledExactlyOnceWith(input.uri);
  });
  it("propagates local dispatch refusal and rejects malformed distribution types", async () => {
    const open = vi.fn().mockRejectedValue(new Error("No handler"));
    await expect(openLocalEditor(own, { editor: "zed", cwd: "/srv" }, open)).rejects.toThrow("No handler");
    open.mockClear();
    await expect(openLocalEditor(own, { editor: "vscode", cwd: "/srv", wslDistro: {} }, open)).rejects.toThrow();
    expect(open).not.toHaveBeenCalled();
  });
});

import files from "../../../testdata/editor-files.json";
import type { PathKind } from "./editor_target.ts";
for (const [name, remote, local] of [["browser", remoteEditorTarget, localEditorTarget], ["electron", electronTarget, electronLocalTarget]] as const) {
  describe(`${name} file handoff`, () => {
    function build(input: { editor: string; cwd: string; kind: string; target?: string; wslDistro?: string }) {
      return input.target !== undefined ? remote(input.target, input.cwd, input.editor as Editor, input.kind as PathKind) : local(input.cwd, input.wslDistro, input.editor as Editor, input.kind as PathKind);
    }
    it.each(files.valid)("preserves $editor $cwd", (input) => expect(build(input).value?.uri).toBe(input.uri));
    it.each(files.invalid)("refuses $editor $cwd $kind", (input) => expect(build(input).value).toBeUndefined());
  });
}
it.each(files.valid)("Electron dispatches $editor file $cwd", async (input) => {
  const open = vi.fn().mockResolvedValue(undefined);
  await (input.target !== undefined ? openRemoteEditor : openLocalEditor)(own, input, open);
  expect(open).toHaveBeenCalledExactlyOnceWith(input.uri);
});
it.each(files.invalid)("Electron rejects file input $cwd $kind", async (input) => {
  const open = vi.fn();
  await expect((input.target !== undefined ? openRemoteEditor : openLocalEditor)(own, input, open)).rejects.toThrow();
  expect(open).not.toHaveBeenCalled();
});
