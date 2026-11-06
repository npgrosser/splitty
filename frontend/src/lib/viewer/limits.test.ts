import { describe, expect, it } from "vitest";
import { MAX_PATH_BYTES, isViewerCwd, isViewerPath, isViewerTitle, viewerTitle, wellFormed } from "./limits.ts";

// viewer-panes, Opening A Viewer: the window checks a path by the server's
// rule, so it never sends an op the server refuses.
describe("isViewerPath", () => {
  it("takes absolute paths", () => {
    for (const path of ["/", "/home/me/app/PLAN.md", "/home/me/my notes/plan v2.md", "/home/me/.config/app.toml", "/home/me/a..b/c.", "/home/me//PLAN.md", "/tmp/plän.md"]) {
      expect(isViewerPath(path), path).toBe(true);
    }
  });

  it("refuses relative paths and paths with a dot segment", () => {
    // A relative path; A path with a dot segment.
    for (const path of ["", "docs/plan.md", "./plan.md", "/home/me/app/../secrets.txt", "/home/me/./PLAN.md", "/home/me/..", "/home/me/."]) {
      expect(isViewerPath(path), path).toBe(false);
    }
  });

  it("refuses a leading double slash and backslashes", () => {
    expect(isViewerPath("//server/share/file.md")).toBe(false);
    expect(isViewerPath("/home/me\\PLAN.md")).toBe(false);
  });

  it("refuses control characters and invisible formatting characters", () => {
    for (const char of ["\u0000", "\n", "\u001b", "\u007f", "\u0085", "\u009f", "​", "‏", "‪", "‮", "⁠", "⁯", "﻿"]) {
      expect(isViewerPath(`/home/me/a${char}b.md`), JSON.stringify(char)).toBe(false);
    }
    expect(isViewerPath("/home/me/\ud800.md")).toBe(false);
    expect(isViewerPath("/home/me/ .md")).toBe(true);
  });

  it("takes at most 4,096 bytes of UTF-8", () => {
    expect(isViewerPath(`/${"a".repeat(MAX_PATH_BYTES - 1)}`)).toBe(true);
    expect(isViewerPath(`/${"a".repeat(MAX_PATH_BYTES)}`)).toBe(false);
    // Two bytes each.
    expect(isViewerPath(`/${"ä".repeat(2048)}`)).toBe(false);
  });
});

describe("isViewerCwd", () => {
  it("takes an absolute directory", () => {
    expect(isViewerCwd("/home/me/app")).toBe(true);
    expect(isViewerCwd("home/me")).toBe(false);
    expect(isViewerCwd("")).toBe(false);
  });
});

describe("titles", () => {
  it("takes 1 to 200 characters without controls", () => {
    expect(isViewerTitle("Selection from claude")).toBe(true);
    expect(isViewerTitle("")).toBe(false);
    expect(isViewerTitle("😀".repeat(200))).toBe(true);
    expect(isViewerTitle("x".repeat(201))).toBe(false);
    expect(isViewerTitle("a\tb")).toBe(false);
  });

  it("makes a title the server takes from a pane title", () => {
    expect(viewerTitle("Selection from \u001b]0;claude\u0007")).toBe("Selection from  ]0;claude");
    expect([...viewerTitle(`Selection from ${"😀".repeat(300)}`)]).toHaveLength(200);
    expect(isViewerTitle(viewerTitle(`Selection from ${"x\n".repeat(300)}`))).toBe(true);
  });

  it("replaces lone surrogates", () => {
    expect(wellFormed("a\ud800b\udc00c😀")).toBe("a�b�c😀");
  });
});
