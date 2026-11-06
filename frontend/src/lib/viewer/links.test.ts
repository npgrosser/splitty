import { describe, expect, it } from "vitest";
import { classifyLink, resolveDocumentLink } from "./links.ts";

const PLAN = "/home/me/app/docs/PLAN.md";

// access-control, Rendered Documents Are Inert: what a link of a rendered
// document does.
describe("classifyLink", () => {
  it("opens http and https links through the external opener", () => {
    expect(classifyLink("https://example.com/a b", PLAN)).toEqual({ kind: "external", url: "https://example.com/a%20b" });
    expect(classifyLink(" HTTP://example.com ", PLAN)).toEqual({ kind: "external", url: "http://example.com/" });
    expect(classifyLink("https://", PLAN)).toEqual({ kind: "none" });
  });

  it("scrolls to an anchor", () => {
    expect(classifyLink("#step-2", PLAN)).toEqual({ kind: "fragment", id: "step-2" });
    expect(classifyLink("#caf%C3%A9", PLAN)).toEqual({ kind: "fragment", id: "café" });
    expect(classifyLink("#", PLAN)).toEqual({ kind: "fragment", id: "" });
  });

  it("does nothing for any other link", () => {
    for (const href of ["javascript:alert(1)", "JaVaScRiPt:alert(1)", "data:text/html,x", "file:///etc/passwd", "mailto:me@example.com", "vbscript:x", "//example.com/x", "\\\\server\\share", "C:/Users/me/a.md", "", "?query"]) {
      expect(classifyLink(href, PLAN), href).toEqual({ kind: "none" });
    }
  });

  it("opens a relative link as a file next to the document", () => {
    expect(classifyLink("TASKS.md", PLAN)).toEqual({ kind: "file", path: "/home/me/app/docs/TASKS.md" });
    expect(classifyLink("../README.md#usage", PLAN)).toEqual({ kind: "file", path: "/home/me/app/README.md" });
    expect(classifyLink("/home/me/notes.md?x=1", PLAN)).toEqual({ kind: "file", path: "/home/me/notes.md" });
  });

  it("refuses a relative link to no path a viewer can show", () => {
    expect(classifyLink("../../../../../etc/passwd", PLAN)).toEqual({ kind: "refused" });
    expect(classifyLink("a\\b.md", PLAN)).toEqual({ kind: "refused" });
    expect(classifyLink("TASKS.md", undefined)).toEqual({ kind: "refused" });
  });
});

describe("resolveDocumentLink", () => {
  it("resolves against the document's directory", () => {
    expect(resolveDocumentLink(PLAN, "TASKS.md")).toBe("/home/me/app/docs/TASKS.md");
    expect(resolveDocumentLink(PLAN, "./img/a.png")).toBe("/home/me/app/docs/img/a.png");
    expect(resolveDocumentLink(PLAN, "../src/main.rs")).toBe("/home/me/app/src/main.rs");
    expect(resolveDocumentLink(PLAN, "/etc/hosts")).toBe("/etc/hosts");
  });

  it("drops dot segments, resolves .. and collapses duplicate slashes", () => {
    expect(resolveDocumentLink(PLAN, "a/./b/../c.md")).toBe("/home/me/app/docs/a/c.md");
    expect(resolveDocumentLink(PLAN, "a//b///c.md")).toBe("/home/me/app/docs/a/b/c.md");
    expect(resolveDocumentLink("/home//me/PLAN.md", "x.md")).toBe("/home/me/x.md");
    expect(resolveDocumentLink(PLAN, "../../../../x.md")).toBe("/x.md");
  });

  it("decodes the link's escapes, never the document's path", () => {
    expect(resolveDocumentLink(PLAN, "my%20notes.md")).toBe("/home/me/app/docs/my notes.md");
    expect(resolveDocumentLink("/home/me/100%25/PLAN.md", "x%25.md")).toBe("/home/me/100%25/x%.md");
    // An encoded dot segment is still one.
    expect(resolveDocumentLink(PLAN, "%2e%2E/x.md")).toBe("/home/me/app/x.md");
  });

  it("opens nothing for a link that climbs above the root", () => {
    expect(resolveDocumentLink(PLAN, "../../../../../x.md")).toBeUndefined();
    expect(resolveDocumentLink(PLAN, "/../x.md")).toBeUndefined();
  });

  it("opens nothing for a link whose path breaks the server's rule", () => {
    expect(resolveDocumentLink(PLAN, "a\\..\\b.md")).toBeUndefined();
    expect(resolveDocumentLink(PLAN, "a%0Ab.md")).toBeUndefined();
    expect(resolveDocumentLink(PLAN, "a%E2%80%AEb.md")).toBeUndefined();
    expect(resolveDocumentLink(PLAN, "a%2Fb.md")).toBeUndefined();
    expect(resolveDocumentLink(PLAN, "a%ZZ.md")).toBeUndefined();
    expect(resolveDocumentLink(PLAN, "x".repeat(5000))).toBeUndefined();
    expect(resolveDocumentLink(PLAN, "")).toBeUndefined();
  });
});
