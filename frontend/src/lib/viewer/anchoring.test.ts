import { describe, expect, it } from "vitest";
import {
  anchor,
  anchorer,
  CONTEXT_CHARS,
  SIMILAR_LINES,
  SIMILAR_MAX_QUOTE,
  SIMILAR_SCORE,
  lastLine,
  lineAt,
  lineStarts,
  segmentLines,
  sourceLines,
  sourceText,
  targetAt,
  type LineRange,
  type QuoteTarget,
} from "./anchoring.ts";

/** The target of the first occurrence of `quote` in `text`, as a selection of it would give. */
function selectionTarget(text: string, quote: string): QuoteTarget {
  const start = text.indexOf(quote);
  expect(start).toBeGreaterThanOrEqual(0);
  const target = targetAt(text, start, start + quote.length, sourceLines(text));
  expect(target).not.toBeNull();
  return target!;
}

function numberedLines(count: number, from = 1): string {
  return Array.from({ length: count }, (_, i) => `filler line ${from + i}`).join("\n");
}

describe("line numbers", () => {
  it("maps offsets to 1-based lines", () => {
    const text = "one\ntwo\n\nfour";
    const starts = lineStarts(text);
    expect(starts).toEqual([0, 4, 8, 9]);
    expect(lineAt(starts, 0)).toBe(1);
    expect(lineAt(starts, 3)).toBe(1); // the newline belongs to its line
    expect(lineAt(starts, 4)).toBe(2);
    expect(lineAt(starts, 8)).toBe(3);
    expect(lineAt(starts, 13)).toBe(4);
  });

  it("ends a passage on the line of its last character", () => {
    const lines = sourceLines("one\ntwo\nthree");
    expect(lines(0, 4)).toEqual({ startLine: 1, endLine: 1 }); // "one\n"
    expect(lines(4, 13)).toEqual({ startLine: 2, endLine: 3 });
  });
});

describe("targetAt", () => {
  it("keeps the quote, its context and its lines", () => {
    const text = "# Plan\n\n1. Build the parser.\n2. Retry three times on failure.\n3. Ship it.\n";
    const target = selectionTarget(text, "Retry three times");
    expect(target).toEqual({
      quote: "Retry three times",
      prefix: "# Plan\n\n1. Build the parser.\n2. ",
      suffix: " on failure.\n3. Ship it.\n",
      startLine: 4,
      endLine: 4,
    });
  });

  it("cuts the context to its length", () => {
    const text = `${"a".repeat(100)}QUOTE${"b".repeat(100)}`;
    const target = selectionTarget(text, "QUOTE");
    expect(target.prefix).toBe("a".repeat(CONTEXT_CHARS));
    expect(target.suffix).toBe("b".repeat(CONTEXT_CHARS));
  });

  it("leaves the whitespace at the ends of a selection out", () => {
    const text = "first\n  second line  \nthird";
    const start = text.indexOf("\n");
    const end = text.indexOf("third");
    const target = targetAt(text, start, end, sourceLines(text));
    expect(target?.quote).toBe("second line");
    expect(target?.startLine).toBe(2);
    expect(target?.endLine).toBe(2);
  });

  it("accepts a selection made backwards", () => {
    const text = "alpha beta gamma";
    expect(targetAt(text, 10, 6, sourceLines(text))?.quote).toBe("beta");
  });

  it("gives nothing for a selection of whitespace", () => {
    const text = "a   \n\n  b";
    expect(targetAt(text, 1, 7, sourceLines(text))).toBeNull();
  });

  it("does not split a surrogate pair at the edge of the context", () => {
    const text = `😀${"x".repeat(CONTEXT_CHARS - 1)}QUOTE${"y".repeat(CONTEXT_CHARS - 1)}😀 tail`;
    const target = selectionTarget(text, "QUOTE");
    expect(target.prefix).toBe("x".repeat(CONTEXT_CHARS - 1));
    expect(target.suffix).toBe("y".repeat(CONTEXT_CHARS - 1));
  });
});

describe("anchor", () => {
  it("Lines inserted above the passage: finds the passage again at its new lines", () => {
    const before = `${numberedLines(4)}\nThe cache is rebuilt on every change.\n${numberedLines(3, 6)}`;
    const target = selectionTarget(before, "rebuilt on every change");
    expect(target.startLine).toBe(5);

    const after = `${numberedLines(10, 100)}\n${before}`;
    const result = anchor(after, target);
    expect(result.state).toBe("anchored");
    if (result.state !== "anchored") return;
    expect(after.slice(result.anchor.start, result.anchor.end)).toBe("rebuilt on every change");
    expect(result.anchor.startLine).toBe(15);
    expect(result.anchor.endLine).toBe(15);
  });

  it("marks an annotation stale when its passage is gone, and anchors it once the passage is back", () => {
    const text = "Step one.\nRetry three times.\nStep three.\n";
    const target = selectionTarget(text, "Retry three times");
    expect(anchor("Step one.\nStep three.\n", target)).toEqual({ state: "stale" });

    const again = anchor("Intro.\nStep one.\nRetry three times.\nStep three.\n", target);
    expect(again.state).toBe("anchored");
    if (again.state === "anchored") expect(again.anchor.startLine).toBe(3);
  });

  it("The same words twice: picks the occurrence with the recorded context over a nearer one", () => {
    const original = "intro\nThen deploy, run the tests again.\n";
    const target = selectionTarget(original, "the tests");
    expect(target.startLine).toBe(2);
    // Line 2 is nearest to the recorded line, but only line 23 has the recorded context.
    const changed = `intro\nthe tests\n${numberedLines(20)}\nThen deploy, run the tests again.\n`;
    const result = anchor(changed, target);
    expect(result.state).toBe("anchored");
    if (result.state !== "anchored") return;
    expect(result.anchor.startLine).toBe(23);
    expect(changed.slice(result.anchor.start - 4, result.anchor.end + 7)).toBe("run the tests again.");
  });

  it("among occurrences with equal context, picks the one nearest the recorded line", () => {
    const text = ["x", "TODO", "x", "x", "x", "x", "TODO", "x"].join("\n");
    const target: QuoteTarget = { quote: "TODO", prefix: "", suffix: "", startLine: 6, endLine: 6 };
    const result = anchor(text, target);
    expect(result.state === "anchored" && result.anchor.startLine).toBe(7);
  });

  it("finds a reflowed paragraph with whitespace treated as one space", () => {
    const original = "Intro.\nThe session list is cached and\ninvalidated on every change.\nEnd.";
    const target = selectionTarget(original, "cached and\ninvalidated on every");
    const reflowed = "Intro.\nThe session list is cached  and invalidated\n   on every change.\nEnd.";
    const result = anchor(reflowed, target);
    expect(result.state).toBe("anchored");
    if (result.state !== "anchored") return;
    expect(reflowed.slice(result.anchor.start, result.anchor.end)).toBe("cached  and invalidated\n   on every");
    expect(result.anchor.startLine).toBe(2);
    expect(result.anchor.endLine).toBe(3);
  });

  it("is stale for an empty quote", () => {
    expect(anchor("anything", { quote: "", prefix: "", suffix: "", startLine: 1, endLine: 1 })).toEqual({ state: "stale" });
  });

  it("names the lines a caller's line map gives, as for a rendered view", () => {
    // Rendered text of two blocks: "Title" from source lines 1-1, "Body text" from 3-5.
    const text = "Title\nBody text";
    const lines: LineRange = (start, end) => ({
      startLine: start < 6 ? 1 : 3,
      endLine: end <= 6 ? 1 : 5,
    });
    const target: QuoteTarget = { quote: "Body", prefix: "Title\n", suffix: " text", startLine: 3, endLine: 5 };
    const result = anchor(text, target, lines);
    expect(result).toEqual({ state: "anchored", anchor: { start: 6, end: 10, startLine: 3, endLine: 5 } });
  });

  it("anchors many annotations against one version", () => {
    const text = "alpha\nbeta\ngamma\n";
    const anchorAll = anchorer(text);
    const states = ["alpha", "gamma", "delta"].map((quote) =>
      anchorAll({ quote, prefix: "", suffix: "", startLine: 1, endLine: 1 }).state,
    );
    expect(states).toEqual(["anchored", "anchored", "stale"]);
  });
});

describe("a changed passage", () => {
  const plan = [
    "# Plan",
    "",
    "1. Read the configuration from the file.",
    "2. Cache the session list and invalidate it on every change.",
    "3. Retry three times before giving up.",
    "4. Write the result to the log.",
    "",
  ].join("\n");
  const quote = "Cache the session list and invalidate it on every change.";

  it("The passage reworded: anchors at the passage the quote became, as changed", () => {
    const target = selectionTarget(plan, quote);
    const reworded = plan
      .replace("# Plan\n", "# Plan\n\nA short introduction.\n")
      .replace(quote, "Cache the list of sessions and invalidate it on each change.");
    const result = anchor(reworded, target);
    expect(result.state).toBe("changed");
    if (result.state !== "changed") return;
    expect(reworded.slice(result.anchor.start, result.anchor.end)).toBe("Cache the list of sessions and invalidate it on each change.");
    expect(result.anchor.startLine).toBe(6);
    expect(result.anchor.endLine).toBe(6);
    expect(result.score).toBeGreaterThanOrEqual(SIMILAR_SCORE);
    expect(result.score).toBeLessThan(1);
    // The target keeps its recorded quote and lines.
    expect(target.quote).toBe(quote);
    expect(target.startLine).toBe(4);
  });

  it("folds case and collapses whitespace before comparing", () => {
    const target = selectionTarget(plan, quote);
    const shouted = plan.replace(quote, "CACHE the session  list and\ninvalidate it on any change.");
    const result = anchor(shouted, target);
    expect(result.state).toBe("changed");
    if (result.state !== "changed") return;
    expect(shouted.slice(result.anchor.start, result.anchor.end)).toBe("CACHE the session  list and\ninvalidate it on any change.");
    expect(result.anchor).toMatchObject({ startLine: 4, endLine: 5 });
  });

  it("The passage removed: stays not found when nothing similar is near", () => {
    const target = selectionTarget(plan, quote);
    const rewritten = plan.replace(`2. ${quote}\n`, "2. Compute the list on demand.\n");
    expect(anchor(rewritten, target)).toEqual({ state: "stale" });
  });

  it("anchors without a change again once a later version holds the quote", () => {
    const target = selectionTarget(plan, quote);
    const reworded = plan.replace(quote, "Cache the list of sessions and invalidate it on each change.");
    expect(anchor(reworded, target).state).toBe("changed");
    expect(anchor(plan, target).state).toBe("anchored");
  });

  it("looks only near the recorded lines", () => {
    const target = selectionTarget(plan, quote);
    const reworded = "Cache the list of sessions and invalidate it on each change.";
    const far = `${numberedLines(SIMILAR_LINES + 10)}\n${reworded}\n`;
    expect(anchor(far, target)).toEqual({ state: "stale" });
    const near = `${numberedLines(SIMILAR_LINES - 10)}\n${reworded}\n`;
    expect(anchor(near, target).state).toBe("changed");
  });

  it("prefers the similar passage nearest the recorded lines among equal ones", () => {
    const target: QuoteTarget = { quote: "retry three times before giving up", prefix: "", suffix: "", startLine: 20, endLine: 20 };
    const lines = numberedLines(30).split("\n");
    lines[4] = "Retry four times before giving up";
    lines[22] = "Retry four times before giving up";
    const text = lines.join("\n");
    const result = anchor(text, target);
    expect(result.state).toBe("changed");
    if (result.state === "changed") expect(result.anchor.startLine).toBe(23);
  });

  it("skips the search for a quote of more than 4,000 characters", () => {
    const long = "word ".repeat(SIMILAR_MAX_QUOTE / 5 + 1).trim();
    const target: QuoteTarget = { quote: long, prefix: "", suffix: "", startLine: 1, endLine: 1 };
    expect(anchor(`${long.slice(0, -10)} other`, target)).toEqual({ state: "stale" });
    const fits = long.slice(0, 3_000);
    expect(anchor(`${fits.slice(0, -10)} other`, { ...target, quote: fits }).state).toBe("changed");
  });

  it("starts and ends a passage at any character of a script without spaces", () => {
    const target: QuoteTarget = { quote: "設定ファイルを読み込んでから接続する", prefix: "", suffix: "", startLine: 1, endLine: 1 };
    const text = "最初に設定ファイルを読み込んで接続する。次に進む。";
    const result = anchor(text, target);
    expect(result.state).toBe("changed");
    if (result.state === "changed") expect(text.slice(result.anchor.start, result.anchor.end)).toBe("設定ファイルを読み込んで接続する");
  });

  it("does not stall on a large document or a long line", () => {
    const words = ["cache", "session", "list", "retry", "change", "window", "server", "review", "thread", "passage"];
    let seed = 7;
    const next = () => {
      seed = (Math.imul(seed, 1103515245) + 12345) | 0;
      return (seed >>> 8) & 0xffff;
    };
    const prose = (chars: number, width: number) => {
      const out: string[] = [];
      let length = 0;
      let column = 0;
      while (length < chars) {
        const word = `${words[next() % words.length]}${next() % 100}`;
        const gap = column + word.length > width ? "\n" : " ";
        column = gap === "\n" ? 0 : column + word.length + 1;
        out.push(word, gap);
        length += word.length + 1;
      }
      return out.join("");
    };
    const large = prose(8 * 1024 * 1024, 80);
    const line = prose(2 * 1024 * 1024, Number.MAX_SAFE_INTEGER);
    for (const text of [large, line]) {
      const anchorAll = anchorer(text);
      const started = performance.now();
      for (let i = 0; i < 20; i++) {
        anchorAll({ quote: prose(SIMILAR_MAX_QUOTE - 100 - i, Number.MAX_SAFE_INTEGER).trim(), prefix: "", suffix: "", startLine: 1 + i * 500, endLine: 1 + i * 500 });
      }
      expect(performance.now() - started).toBeLessThan(5_000);
    }
  });
});

describe("source text", () => {
  it("takes every line break as a line feed", () => {
    expect(sourceText("one\r\ntwo\rthree\n")).toBe("one\ntwo\nthree\n");
    const plain = "no carriage returns\n";
    expect(sourceText(plain)).toBe(plain);
  });

  it("counts the lines as the source view numbers them, without a line after the final break", () => {
    expect(lastLine("")).toBe(1);
    expect(lastLine("one")).toBe(1);
    expect(lastLine("one\n")).toBe(1);
    expect(lastLine("one\ntwo")).toBe(2);
    expect(lastLine("one\n\n")).toBe(2);
    expect(lastLine("\n\n\n")).toBe(3);
  });
});

describe("segmentLines", () => {
  // Rendered text "Title\nFirst item\nSecond item" of a heading on line 1 and
  // a list on lines 3-4, its line breaks between blocks in no block.
  const lines = segmentLines([
    { start: 0, lines: { start: 1, end: 1 } },
    { start: 5, lines: null },
    { start: 6, lines: { start: 3, end: 3 } },
    { start: 16, lines: null },
    { start: 17, lines: { start: 4, end: 4 } },
  ], 4);

  it("names the lines of the blocks a passage starts and ends in", () => {
    expect(lines(0, 5)).toEqual({ startLine: 1, endLine: 1 });
    expect(lines(8, 27)).toEqual({ startLine: 3, endLine: 4 });
    expect(lines(0, 27)).toEqual({ startLine: 1, endLine: 4 });
  });

  it("takes the next block's lines at a passage's start and the previous one's at its end", () => {
    // Starts on the break after the title, ends on the break after the first item.
    expect(lines(5, 17)).toEqual({ startLine: 3, endLine: 3 });
    expect(lines(2, 6)).toEqual({ startLine: 1, endLine: 1 });
  });

  it("keeps lines within the source's", () => {
    const past = segmentLines([{ start: 0, lines: { start: 7, end: 9 } }], 5);
    expect(past(0, 3)).toEqual({ startLine: 5, endLine: 5 });
  });

  it("spans the whole source without any lines, as for an HTML page", () => {
    expect(segmentLines([], 12)(0, 4)).toEqual({ startLine: 1, endLine: 12 });
    expect(segmentLines([{ start: 0, lines: null }], 3)(0, 2)).toEqual({ startLine: 1, endLine: 3 });
  });
});
