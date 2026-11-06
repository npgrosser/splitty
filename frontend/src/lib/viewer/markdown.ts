/**
 * Markdown to HTML for viewer panes (`viewer-panes`, Showing A Document):
 * markdown-it with GFM tables, strikethrough, autolinks and task lists.
 *
 * Every block carries the source lines it came from as `data-source-line` and
 * `data-source-line-end` (1-based, inclusive), so a selection in the rendered
 * view maps back to the file. Headings get GitHub-style IDs for `#…` links.
 * Fenced code is emitted plain, with its language in `data-language`; the
 * renderer highlights it once the language is loaded.
 *
 * The output is not safe as it is: raw HTML of the document passes through,
 * and `render.ts` sanitizes everything before it reaches the page.
 */

import MarkdownIt from "markdown-it";
import type { StateBlock, StateCore, Token } from "markdown-it";

const md = new MarkdownIt({ html: true, linkify: true, typographer: false });
const { escapeHtml, unescapeAll } = md.utils;

function sourceAttributes(token: Token): string {
  if (!token.map) return "";
  return ` data-source-line="${token.map[0] + 1}" data-source-line-end="${Math.max(token.map[0] + 1, token.map[1])}"`;
}

/** A YAML front matter block at the top of a file shows as a code block. */
function frontMatter(state: StateBlock, startLine: number, endLine: number, silent: boolean): boolean {
  if (startLine !== 0) return false;
  const line = (n: number) => state.src.slice(state.bMarks[n] + state.tShift[n], state.eMarks[n]).trimEnd();
  if (line(0) !== "---" || state.tShift[0] !== 0) return false;
  let end = 1;
  while (end < endLine && line(end) !== "---" && line(end) !== "...") end++;
  if (end >= endLine) return false;
  if (silent) return true;
  const token = state.push("fence", "code", 0);
  token.info = "yaml";
  token.markup = "---";
  token.content = state.getLines(1, end, 0, true);
  token.map = [0, end + 1];
  state.line = end + 1;
  return true;
}

function sourceLines(state: StateCore): void {
  const lines = state.src.split("\n");
  for (const token of state.tokens) {
    if (!token.map || !token.block || token.nesting === -1) continue;
    const start = token.map[0];
    let end = Math.max(start + 1, token.map[1]);
    // The last item of a list, and the list, reach over the blank lines after
    // them; those are not part of a passage taken from the block.
    while (end > start + 1 && (lines[end - 1] ?? "").trim() === "") end--;
    token.attrSet("data-source-line", String(start + 1));
    token.attrSet("data-source-line-end", String(end));
  }
}

/** The text of an inline token, as a heading's ID is made from it. */
function inlineText(token: Token): string {
  return (token.children ?? [])
    .filter((child) => child.type === "text" || child.type === "code_inline")
    .map((child) => child.content)
    .join("");
}

/** GitHub's heading IDs: lower case, letters, digits, `-` and `_`, spaces as `-`. */
export function headingSlug(text: string): string {
  return text.trim().toLowerCase().replace(/[^\p{L}\p{M}\p{N}\s_-]/gu, "").replace(/\s/g, "-");
}

function headingIds(state: StateCore): void {
  const used = new Map<string, number>();
  const tokens = state.tokens;
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i].type !== "heading_open" || tokens[i + 1]?.type !== "inline") continue;
    const base = headingSlug(inlineText(tokens[i + 1])) || "section";
    const seen = used.get(base) ?? 0;
    used.set(base, seen + 1);
    tokens[i].attrSet("id", seen === 0 ? base : `${base}-${seen}`);
  }
}

const TASK = /^\[([ xX])\][ \t]+/;

/** `- [ ] item` and `- [x] item`: a list item with a box that shows its state. */
function taskLists(state: StateCore): void {
  const tokens = state.tokens;
  for (let i = 2; i < tokens.length; i++) {
    const inline = tokens[i];
    if (inline.type !== "inline" || tokens[i - 1].type !== "paragraph_open" || tokens[i - 2].type !== "list_item_open") continue;
    const first = inline.children?.[0];
    const match = first?.type === "text" ? TASK.exec(first.content) : null;
    if (!first || !match) continue;
    first.content = first.content.slice(match[0].length);
    const box = new state.Token("task_box", "span", 0);
    box.meta = { checked: match[1] !== " " };
    inline.children!.unshift(box);
    const item = tokens[i - 2];
    item.attrJoin("class", "task-list-item");
    for (let j = i - 3; j >= 0; j--) {
      const list = tokens[j];
      if ((list.type !== "bullet_list_open" && list.type !== "ordered_list_open") || list.level !== item.level - 1) continue;
      if (!String(list.attrGet("class") ?? "").split(" ").includes("contains-task-list")) list.attrJoin("class", "contains-task-list");
      break;
    }
  }
}

md.block.ruler.before("hr", "front_matter", frontMatter);
md.core.ruler.push("source_lines", sourceLines);
md.core.ruler.push("heading_ids", headingIds);
md.core.ruler.push("task_lists", taskLists);

md.renderer.rules.task_box = (tokens, idx) => {
  const checked = (tokens[idx].meta as { checked?: boolean } | null)?.checked === true;
  return `<span class="task-box" role="checkbox" aria-checked="${checked}" aria-disabled="true"></span>`;
};

md.renderer.rules.fence = (tokens, idx) => {
  const token = tokens[idx];
  const language = (token.info ? unescapeAll(token.info).trim() : "").split(/\s+/)[0] ?? "";
  const code = language ? ` class="language-${escapeHtml(language)}" data-language="${escapeHtml(language)}"` : "";
  return `<pre${sourceAttributes(token)}><code${code}>${escapeHtml(token.content)}</code></pre>\n`;
};

md.renderer.rules.code_block = (tokens, idx) => {
  const token = tokens[idx];
  return `<pre${sourceAttributes(token)}><code>${escapeHtml(token.content)}</code></pre>\n`;
};

/** The HTML of a Markdown document, not yet sanitized. */
export function markdownToHtml(text: string): string {
  return md.render(text);
}
