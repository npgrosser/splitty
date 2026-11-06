import { describe, expect, it } from "vitest";
import { compareListPlaces, documentName, fenceFor, reviewFileName, reviewMarkdown, type ListPlace, type ReviewEntry } from "./review.ts";

describe("reviewMarkdown", () => {
  it("lists a question and a suggested change on a plan", () => {
    const entries: ReviewEntry[] = [
      {
        kind: "question",
        body: "Why not compute it on demand? The list has at most 20 entries.",
        passage: {
          quote: "Cache the session list and invalidate it\non every change.",
          lines: { start: 12, end: 14 },
          stale: false,
        },
      },
      {
        kind: "change",
        replacement: "retry with exponential backoff, at most five times",
        passage: { quote: "retry three times", lines: { start: 30, end: 30 }, stale: false },
      },
    ];
    expect(reviewMarkdown({ kind: "file", path: "/home/me/app/PLAN.md" }, entries)).toBe(
      [
        "# Review of PLAN.md",
        "",
        "Document: /home/me/app/PLAN.md",
        "",
        "## 1. Lines 12-14, question",
        "> Cache the session list and invalidate it",
        "> on every change.",
        "",
        "Why not compute it on demand? The list has at most 20 entries.",
        "",
        "## 2. Line 30, suggested change",
        "> retry three times",
        "",
        "Replace with:",
        "```",
        "retry with exponential backoff, at most five times",
        "```",
        "",
      ].join("\n"),
    );
  });

  it("names a terminal selection by its title and gives no path", () => {
    const review = reviewMarkdown({ kind: "text", title: "Selection from claude" }, [
      { kind: "comment", body: "This step is missing a test.", passage: { quote: "Updated the parser", lines: { start: 3, end: 3 }, stale: false } },
    ]);
    expect(review.startsWith("# Review of Selection from claude\n\n## 1. Line 3, comment\n")).toBe(true);
    expect(review).not.toContain("Document:");
  });

  it("notes a passage that was not found, and gives a rendered HTML passage no lines", () => {
    const review = reviewMarkdown({ kind: "file", path: "/srv/report.html" }, [
      { kind: "comment", body: "Still relevant?", passage: { quote: "Old wording", lines: { start: 50, end: 52 }, stale: true } },
      { kind: "delete", passage: { quote: "Obsolete table", lines: null, stale: false } },
    ]);
    expect(review).toContain("## 1. Lines 50-52, comment (passage not found in the current version)\n> Old wording\n\nStill relevant?");
    expect(review).toContain("## 2. Passage, delete\n> Obsolete table\n");
  });

  it("puts general comments last, questions marked, in their order", () => {
    const review = reviewMarkdown({ kind: "file", path: "/w/PLAN.md" }, [
      { kind: "comment", body: "Split step 4 into two steps." },
      { kind: "comment", body: "Fine.", passage: { quote: "Step 1", lines: { start: 1, end: 1 }, stale: false } },
      { kind: "question", body: "Is there a migration?\nIf so, where?" },
    ]);
    expect(review.endsWith(
      "## 1. Line 1, comment\n> Step 1\n\nFine.\n\n## General comments\n\n- Split step 4 into two steps.\n- Question: Is there a migration?\n  If so, where?\n",
    )).toBe(true);
  });

  it("fences a replacement that holds backticks so that it cannot close its fence", () => {
    const replacement = "use ```rust blocks``` and ``code``";
    const review = reviewMarkdown({ kind: "file", path: "/w/a.md" }, [
      { kind: "change", replacement, passage: { quote: "old", lines: { start: 2, end: 2 }, stale: false } },
    ]);
    expect(review).toContain(`Replace with:\n\`\`\`\`\n${replacement}\n\`\`\`\``);
  });

  it("keeps empty lines of a quote inside the block quote", () => {
    const review = reviewMarkdown({ kind: "file", path: "/w/a.md" }, [
      { kind: "comment", body: "Merge these.", passage: { quote: "first\r\n\r\nsecond  ", lines: { start: 4, end: 6 }, stale: false } },
    ]);
    expect(review).toContain("> first\n>\n> second\n\nMerge these.");
  });

  it("writes a remark of a suggested change before its replacement", () => {
    const review = reviewMarkdown({ kind: "file", path: "/w/a.md" }, [
      { kind: "change", body: "Clearer.", replacement: "new text\n", passage: { quote: "old text", lines: { start: 9, end: 9 }, stale: false } },
    ]);
    expect(review).toContain("> old text\n\nClearer.\n\nReplace with:\n```\nnew text\n```\n");
  });
});

describe("what the review lets through", () => {
  // A terminal without bracketed paste, or one whose program reads escape
  // sequences from a paste, would act on control characters; the review
  // holds none but its line breaks and tabs.
  it("replaces control and bidirectional formatting characters", () => {
    const review = reviewMarkdown({ kind: "file", path: "/w/a\u001b.md" }, [
      {
        kind: "change",
        body: "Run \u001b[31mthis\u0007\u009b and \u202eevil\u202c.",
        replacement: "keep\ttabs\u0000\n",
        passage: { quote: "old\u0003text\u2066", lines: { start: 1, end: 1 }, stale: false },
      },
      { kind: "comment", body: "General\u007f note." },
    ]);
    expect(review).not.toMatch(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/);
    expect(review).toContain("Document: /w/a\ufffd.md");
    expect(review).toContain("> old\ufffdtext\ufffd");
    expect(review).toContain("Run \ufffd[31mthis\ufffd\ufffd and \ufffdevil\ufffd.");
    expect(review).toContain("keep\ttabs\ufffd");
    expect(review).toContain("- General\ufffd note.");
  });

  // viewer-annotations, The Review (A quote with an escape sequence).
  it("keeps a quote from ending a bracketed paste", () => {
    const review = reviewMarkdown({ kind: "file", path: "/home/me/app/PLAN.md" }, [
      { kind: "comment", body: "Odd output.", passage: { quote: "done\u001b[201~\nrm -rf ~/app", lines: { start: 3, end: 4 }, stale: false } },
    ]);
    expect(review).not.toContain("\u001b");
    expect(review).toContain("> done\ufffd[201~\n> rm -rf ~/app");
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

  it("lists anchored passages in document order, then those not found, then general comments", () => {
    const places: ListPlace[] = [
      { state: "general", index: 0 },
      { state: "stale", startLine: 2, endLine: 2, index: 1 },
      anchored(2, 30, 900),
      anchored(3, 12, 300, "source", 14),
      { state: "general", index: 4 },
      anchored(5, 12, 250),
      { state: "stale", startLine: 1, endLine: 1, index: 6 },
    ];
    expect([...places].sort(compareListPlaces).map((place) => place.index)).toEqual([5, 3, 2, 6, 1, 0, 4]);
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

  it("makes a fence longer than any backtick run", () => {
    expect(fenceFor("plain")).toBe("```");
    expect(fenceFor("a ```` b")).toBe("`````");
  });
});
