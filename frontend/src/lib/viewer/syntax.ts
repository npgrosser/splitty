/**
 * Syntax highlighting of viewer panes, shared by the source view and the code
 * blocks of rendered Markdown (`viewer-panes`, Showing A Document).
 *
 * Highlighted text gets `sv-*` classes; their colors are custom properties set
 * on the pane from the terminal palette (`syntax_colors.ts`), so a document
 * reads like the terminal next to it. Languages come from
 * `@codemirror/language-data`, which loads each one on demand as a separate
 * chunk.
 */

import { LanguageDescription, type LanguageSupport } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { highlightTree, tagHighlighter, tags as t } from "@lezer/highlight";

export const highlighter = tagHighlighter([
  { tag: t.keyword, class: "sv-keyword" },
  { tag: [t.string, t.regexp, t.character, t.attributeValue, t.escape], class: "sv-string" },
  { tag: [t.number, t.bool, t.null, t.atom, t.unit], class: "sv-number" },
  { tag: t.comment, class: "sv-comment" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName, t.labelName], class: "sv-function" },
  { tag: [t.typeName, t.className, t.namespace, t.tagName, t.self], class: "sv-type" },
  { tag: [t.propertyName, t.attributeName], class: "sv-property" },
  { tag: [t.operator, t.derefOperator], class: "sv-operator" },
  { tag: [t.meta, t.processingInstruction, t.annotation], class: "sv-meta" },
  { tag: t.heading, class: "sv-heading" },
  { tag: [t.link, t.url], class: "sv-link" },
  { tag: t.emphasis, class: "sv-emphasis" },
  { tag: t.strong, class: "sv-strong" },
  { tag: t.strikethrough, class: "sv-strike" },
  { tag: [t.inserted], class: "sv-inserted" },
  { tag: [t.deleted, t.invalid], class: "sv-deleted" },
]);

/** The rules for the `sv-*` classes; the colors are the pane's custom properties. */
export const SYNTAX_CSS = [
  ".sv-keyword{color:var(--sv-keyword)}",
  ".sv-string{color:var(--sv-string)}",
  ".sv-number{color:var(--sv-number)}",
  ".sv-comment{color:var(--sv-comment);font-style:italic}",
  ".sv-function{color:var(--sv-function)}",
  ".sv-type{color:var(--sv-type)}",
  ".sv-property{color:var(--sv-property)}",
  ".sv-operator{color:var(--sv-operator)}",
  ".sv-meta{color:var(--sv-comment)}",
  ".sv-heading{color:var(--sv-function);font-weight:600}",
  ".sv-link{color:var(--sv-string);text-decoration:underline}",
  ".sv-emphasis{font-style:italic}",
  ".sv-strong{font-weight:600}",
  ".sv-strike{text-decoration:line-through}",
  ".sv-inserted{color:var(--sv-string)}",
  ".sv-deleted{color:var(--sv-deleted)}",
].join("\n");

/** The language of a file, by its name (`PLAN.md`, `Makefile`, `main.rs`). */
export function languageForFile(fileName: string): LanguageDescription | null {
  return LanguageDescription.matchFilename(languages, fileName);
}

/**
 * The language a fenced code block names (`ts`, `rust`, `Python`): a
 * language's name or alias, or else an extension.
 */
export function languageForName(name: string): LanguageDescription | null {
  const word = name.trim().split(/\s+/)[0]?.replace(/^\{?\.?/, "").replace(/\}$/, "") ?? "";
  if (!word || word.length > 64) return null;
  return LanguageDescription.matchLanguageName(languages, word, false)
    ?? (/^[\w+#-]+$/.test(word) ? LanguageDescription.matchFilename(languages, `file.${word}`) : null);
}

/** Load a language once; null when its chunk cannot be loaded. */
export async function loadLanguage(language: LanguageDescription): Promise<LanguageSupport | null> {
  try {
    return language.support ?? await language.load();
  } catch (err) {
    console.warn(`viewer: language ${language.name} did not load`, err);
    return null;
  }
}

/** Code longer than this stays plain in rendered Markdown. */
export const MAX_HIGHLIGHT_CHARS = 200_000;

/**
 * `text` as highlighted nodes of `doc`: text nodes and `span`s with `sv-*`
 * classes, never markup from the text itself.
 */
export function highlightedNodes(doc: Document, text: string, support: LanguageSupport): Node[] {
  const tree = support.language.parser.parse(text);
  const nodes: Node[] = [];
  let at = 0;
  highlightTree(tree, highlighter, (from, to, classes) => {
    if (from > at) nodes.push(doc.createTextNode(text.slice(at, from)));
    const span = doc.createElement("span");
    span.className = classes;
    span.textContent = text.slice(from, to);
    nodes.push(span);
    at = to;
  });
  if (at < text.length) nodes.push(doc.createTextNode(text.slice(at)));
  return nodes;
}
