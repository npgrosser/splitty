import { describe, expect, it } from "vitest";
import { headingSlug, markdownToHtml } from "./markdown.ts";

// viewer-panes, Showing A Document: the Markdown pipeline before sanitizing.
describe("markdownToHtml", () => {
  it("marks every block with the source lines it came from", () => {
    const html = markdownToHtml("# Plan\n\nFirst paragraph\nstill first.\n\n- one\n- two\n");
    expect(html).toContain(`<h1 data-source-line="1" data-source-line-end="1" id="plan">Plan</h1>`);
    expect(html).toContain(`<p data-source-line="3" data-source-line-end="4">First paragraph`);
    expect(html).toContain(`<ul data-source-line="6" data-source-line-end="7">`);
    expect(html).toContain(`<li data-source-line="7" data-source-line-end="7">two</li>`);
  });

  it("leaves the blank lines after the last item of a list out of its lines", () => {
    const html = markdownToHtml("1. one\n2. two\n\nClosing words.\n");
    expect(html).toContain(`<ol data-source-line="1" data-source-line-end="2">`);
    expect(html).toContain(`<li data-source-line="2" data-source-line-end="2">two</li>`);
    expect(html).toContain(`<p data-source-line="4" data-source-line-end="4">Closing words.</p>`);
  });

  it("marks code blocks with their lines and language, unhighlighted", () => {
    const html = markdownToHtml("Text\n\n```rust title=x\nfn main() {}\n```\n\n    indented\n");
    expect(html).toContain(`<pre data-source-line="3" data-source-line-end="5"><code class="language-rust" data-language="rust">fn main() {}\n</code></pre>`);
    expect(html).toContain(`<pre data-source-line="7" data-source-line-end="7"><code>indented\n</code></pre>`);
  });

  it("escapes a fence's language and content", () => {
    const html = markdownToHtml("```x\"onload=\"y\n<script>alert(1)</script>\n```\n");
    expect(html).toContain(`class="language-x&quot;onload=&quot;y"`);
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
  });

  it("gives headings GitHub-style IDs, unique within the document", () => {
    const html = markdownToHtml("# Step 1: Set up `cargo`!\n\n## Step 1: Set up `cargo`!\n\n## Ünïcode Überschrift\n\n## ???\n");
    expect(html).toContain(`id="step-1-set-up-cargo"`);
    expect(html).toContain(`id="step-1-set-up-cargo-1"`);
    expect(html).toContain(`id="ünïcode-überschrift"`);
    expect(html).toContain(`id="section"`);
  });

  it("shows task list items with boxes that cannot be changed", () => {
    const html = markdownToHtml("- [ ] open\n- [x] done\n- plain\n");
    expect(html).toContain(`<ul data-source-line="1" data-source-line-end="3" class="contains-task-list">`);
    expect(html).toContain(`<li data-source-line="1" data-source-line-end="1" class="task-list-item"><span class="task-box" role="checkbox" aria-checked="false" aria-disabled="true"></span>open</li>`);
    expect(html).toContain(`<span class="task-box" role="checkbox" aria-checked="true" aria-disabled="true"></span>done`);
    expect(html).toContain(`<li data-source-line="3" data-source-line-end="3">plain</li>`);
    expect(html).not.toContain("<input");
  });

  it("renders GFM tables, strikethrough and autolinks", () => {
    const html = markdownToHtml("| a | b |\n|---|:-:|\n| 1 | 2 |\n\n~~gone~~ https://example.com\n");
    expect(html).toContain(`<table data-source-line="1" data-source-line-end="3">`);
    expect(html).toContain(`<th style="text-align:center">b</th>`);
    expect(html).toContain("<s>gone</s>");
    expect(html).toContain(`<a href="https://example.com">https://example.com</a>`);
  });

  it("shows YAML front matter as a code block", () => {
    const html = markdownToHtml("---\ntitle: Plan\n---\n# Plan\n");
    expect(html).toContain(`<pre data-source-line="1" data-source-line-end="3"><code class="language-yaml" data-language="yaml">title: Plan\n</code></pre>`);
    expect(html).toContain(`<h1 data-source-line="4" data-source-line-end="4" id="plan">`);
  });

  it("passes raw HTML on, for the sanitizer", () => {
    expect(markdownToHtml("<div onclick=\"x()\">a</div>\n")).toContain(`<div onclick="x()">a</div>`);
  });
});

describe("headingSlug", () => {
  it("keeps letters, digits, spaces as dashes, - and _", () => {
    expect(headingSlug("  Hello, World_2 - Again  ")).toBe("hello-world_2---again");
  });
});
