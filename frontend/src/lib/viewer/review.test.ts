import { describe, expect, it } from "vitest";
import { compareListPlaces, documentName, reviewFileName, safeText, type ListPlace } from "./review.ts";

// The review's text is the server's (`splitty-core` `mux::review` tests).

describe("safeText", () => {
  it("shows control and bidirectional formatting characters as U+FFFD and keeps tabs and line breaks", () => {
    expect(safeText("Run \u001b[31mthis\u0007\u009b and \u202eevil\u202c.\tok\r\nnext")).toBe("Run \ufffd[31mthis\ufffd\ufffd and \ufffdevil\ufffd.\tok\nnext");
  });
});

describe("reviewFileName", () => {
  it("names the file after the document", () => {
    expect(reviewFileName({ kind: "file", path: "/home/me/app/PLAN.md" })).toBe("review-PLAN.md");
    expect(reviewFileName({ kind: "file", path: "/home/me/app/src/main.rs" })).toBe("review-main.rs.md");
    expect(reviewFileName({ kind: "file", path: "/home/me/notes.markdown" })).toBe("review-notes.md");
    expect(reviewFileName({ kind: "text", title: "Selection from claude" })).toBe("review-Selection-from-claude.md");
  });

  it("keeps no separators, dots at the start or characters a file system refuses", () => {
    expect(reviewFileName({ kind: "text", title: "../../etc/passwd" })).toBe("review-etc-passwd.md");
    expect(reviewFileName({ kind: "text", title: "a<b>:c\"d|e?*f" })).toBe("review-a-b-c-d-e-f.md");
    expect(reviewFileName({ kind: "text", title: "Überblick äöü" })).toBe("review-Überblick-äöü.md");
    expect(reviewFileName({ kind: "text", title: "***" })).toBe("review-document.md");
    expect(reviewFileName({ kind: "text", title: "x".repeat(300) })).toHaveLength("review-.md".length + 96);
  });
});

describe("compareListPlaces", () => {
  const anchored = (index: number, startLine: number, offset: number, view: "source" | "rendered" = "source", endLine = startLine): ListPlace =>
    ({ state: "anchored", view, offset, startLine, endLine, index });

  // viewer-annotations, Annotations In The Pane: General comments first.
  it("lists general comments first, then anchored passages in document order, then those not found", () => {
    const places: ListPlace[] = [
      { state: "general", index: 0 },
      { state: "stale", startLine: 2, endLine: 2, index: 1 },
      anchored(2, 30, 900),
      anchored(3, 12, 300, "source", 14),
      { state: "general", index: 4 },
      anchored(5, 12, 250),
      { state: "stale", startLine: 1, endLine: 1, index: 6 },
    ];
    expect([...places].sort(compareListPlaces).map((place) => place.index)).toEqual([0, 4, 5, 3, 2, 6, 1]);
  });

  it("orders passages of both views on one line consistently", () => {
    const places = [anchored(0, 5, 100), anchored(1, 5, 10, "rendered", 6), anchored(2, 5, 50, "source", 7)];
    const once = [...places].sort(compareListPlaces).map((place) => place.index);
    const reversed = [...places].reverse().sort(compareListPlaces).map((place) => place.index);
    expect(once).toEqual([2, 0, 1]);
    expect(reversed).toEqual(once);
  });
});

describe("helpers", () => {
  it("names a document by its last path segment", () => {
    expect(documentName("/home/me/app/PLAN.md")).toBe("PLAN.md");
    expect(documentName("C:\\work\\notes.txt")).toBe("notes.txt");
    expect(documentName("README")).toBe("README");
  });
});
