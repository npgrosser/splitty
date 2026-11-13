import { describe, expect, it } from "vitest";
import folders from "../../../testdata/editor-folders.json";
import cases from "../../../testdata/remote-editor.json";
import files from "../../../testdata/editor-files.json";
import { localEditorTarget, remoteEditorTarget, sshPath, type Editor, type PathKind } from "./editor_target.ts";

describe("remote editor targets", () => {
  it.each(cases.valid)("preserves $target folder $cwd", ({ target, cwd, uri }) => {
    expect(remoteEditorTarget(target, cwd).value?.uri).toBe(uri);
    const parsed = new URL(uri);
    expect(parsed.protocol).toBe("vscode:");
    expect(parsed.host).toBe("vscode-remote");
    expect(parsed.search).toBe("");
    expect(parsed.hash).toBe("");
    expect(decodeURIComponent(parsed.pathname)).toBe(`/ssh-remote+${target}${cwd}${cwd.endsWith("/") ? "" : "/"}`);
  });
  it.each(cases.invalid)("rejects $target folder $cwd", ({ target, cwd }) => {
    expect(remoteEditorTarget(target, cwd).value).toBeUndefined();
    expect(remoteEditorTarget(target, cwd).reason).toBeTruthy();
  });
  it("rejects malformed Unicode and oversized or non-string data without throwing", () => {
    for (const [target, cwd] of [["x".repeat(256), "/srv"], ["vm1", "/" + "界".repeat(1366)], ["vm1", "/\ud800"], [null, "/srv"], ["vm1", {}]]) {
      expect(remoteEditorTarget(target, cwd).value).toBeUndefined();
    }
  });
  it("gives actionable alias guidance for encoded-authority ambiguity", () => {
    expect(remoteEditorTarget("abc", "/srv").reason).toContain("non-hex character");
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

describe("local and Zed folder vectors", () => {
  function build(input: { editor: string; cwd: string; target?: string; wslDistro?: string }) {
    return input.target !== undefined
      ? remoteEditorTarget(input.target, input.cwd, input.editor as Editor)
      : localEditorTarget(input.cwd, input.wslDistro, input.editor as Editor);
  }
  it.each(folders.valid)("preserves $editor $cwd", (input) => expect(build(input).value?.uri).toBe(input.uri));
  it.each(folders.invalid)("rejects $editor $cwd", (input) => expect(build(input).value).toBeUndefined());
});

describe("file handoff", () => {
  function build(input: { editor: string; cwd: string; kind: string; target?: string; wslDistro?: string }) {
    return input.target !== undefined
      ? remoteEditorTarget(input.target, input.cwd, input.editor as Editor, input.kind as PathKind)
      : localEditorTarget(input.cwd, input.wslDistro, input.editor as Editor, input.kind as PathKind);
  }
  it.each(files.valid)("preserves $editor $cwd", (input) => expect(build(input).value?.uri).toBe(input.uri));
  it.each(files.invalid)("refuses $editor $cwd $kind", (input) => expect(build(input).value).toBeUndefined());
});
