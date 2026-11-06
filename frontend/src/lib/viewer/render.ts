/**
 * Rendered Markdown and HTML in a viewer pane (`viewer-panes`, Showing A
 * Document; `access-control`, Rendered Documents Are Inert).
 *
 * A document is sanitized (`sanitize.ts`) and placed into a shadow root. Its
 * host is the pane's scroll container and sets `contain: layout paint`,
 * `overflow: auto` and `isolation: isolate`: whatever the document positions,
 * fixed elements included, stays inside the pane and is clipped to it. An
 * HTML document's own style sheets apply only inside the shadow root, on a
 * light page as most HTML expects; its `html` and `body` rules apply to the
 * wrappers that stand in for them.
 *
 * Nothing in a document navigates: its links carry no `href`, and a click is
 * handled here as `links.ts` decides: `http:` and `https:` go to the system
 * browser, a relative link opens that file in a viewer pane, `#…` scrolls
 * within the document, and anything else does nothing. The link under the
 * pointer, or else the one with the keyboard focus, is reported as `links.ts`
 * classifies it, so that the pane can show where it leads before it is
 * followed: a document can make a link look like anything, or cover the pane.
 *
 * Code blocks are shown plain at once and highlighted when their language has
 * loaded.
 *
 * Annotations (`viewer-annotations`) anchor in the document's text: the text
 * of its content in DOM order (`RenderedText`), the same whether the document
 * is shown or rendered only to be read, as when its source is shown. A
 * Markdown passage takes its source lines from the blocks it lies in; an HTML
 * page names none, and its passages span the whole document. A selection is
 * read across the shadow boundary (`shadowSelection`). Passages are marked
 * with the CSS Custom Highlight API, which leaves the sanitized DOM as it is;
 * where the API is missing, and for the passages of the other view, the
 * covering blocks are marked instead. The marks under the pointer
 * (`marksAt`) are the passages whose ranges lie under it, by their client
 * rectangles or, where the engine lays nothing out, by the caret position,
 * and the marks of the blocks that hold it (D4 of viewer-clickable-marks).
 */

import { segmentLines, type LineRange, type LineSegment } from "./anchoring.ts";
import { classifyLink, type DocumentLink } from "./links.ts";
import { markdownToHtml } from "./markdown.ts";
import { documentStyles, linkTarget, sanitizeHtml, sanitizeStyleSheet } from "./sanitize.ts";
import { MAX_HIGHLIGHT_CHARS, SYNTAX_CSS, highlightedNodes, languageForName, loadLanguage } from "./syntax.ts";

/** The host's own style: the containment the document cannot leave. */
export const HOST_STYLE = "display:block;box-sizing:border-box;height:100%;overflow:auto;contain:layout paint;isolation:isolate;position:relative;outline:none";

export interface RenderOptions {
  /** The file the document is, for relative links. */
  readonly documentPath: string;
  openExternal(url: string): void;
  openFile(path: string): void;
  /** A relative link names no path a viewer pane can show; it opens nothing. */
  refuseLink(): void;
  /**
   * The link under the pointer, or else the one with the keyboard focus, as a
   * click would follow it; undefined once there is none.
   */
  pointLink?(link: DocumentLink | undefined): void;
}

/** A selection in a rendered document and the source lines it covers, where they are known. */
export interface RenderedSelection {
  readonly text: string;
  readonly startLine?: number;
  readonly endLine?: number;
}

/** How a passage is marked: open, found as what its quote most likely became, or of a resolved annotation. */
export type MarkStyle = "changed" | "resolved";

/**
 * A passage to mark in a rendered document's text; `active` is the one the
 * user chose, `hover` the one under the pointer, `id` its annotation's.
 */
export interface RenderedMark {
  readonly id?: string;
  readonly start: number;
  readonly end: number;
  readonly active?: boolean;
  readonly hover?: boolean;
  readonly style?: MarkStyle;
}

/** Source lines whose blocks to mark: the passages of the other view. */
export interface BlockMark {
  readonly id?: string;
  readonly startLine: number;
  readonly endLine: number;
  readonly active?: boolean;
  readonly hover?: boolean;
  readonly style?: MarkStyle;
}

/** The names of the highlights in the CSS Custom Highlight API registry, shared by the panes of a window. */
export const HIGHLIGHT = "splitty-annotation";
export const ACTIVE_HIGHLIGHT = "splitty-annotation-active";
export const HOVER_HIGHLIGHT = "splitty-annotation-hover";
export const CHANGED_HIGHLIGHT = "splitty-annotation-changed";
export const RESOLVED_HIGHLIGHT = "splitty-annotation-resolved";
/**
 * The attribute that marks a block, with the mark's style: empty, `active`,
 * `hover`, `changed` or `resolved`; a document cannot set `data-splitty-*` (`sanitize.ts`).
 */
const BLOCK_MARK = "data-splitty-mark";

/**
 * Where marks overlap, the higher one shows: the chosen passage over the one
 * under the pointer, over an open one, over a changed one, over a resolved one.
 */
type MarkRank = 0 | 1 | 2 | 3 | 4;
const RANK_VALUES = ["resolved", "changed", "", "hover", "active"] as const;
const RANK_HIGHLIGHTS = [RESOLVED_HIGHLIGHT, CHANGED_HIGHLIGHT, HIGHLIGHT, HOVER_HIGHLIGHT, ACTIVE_HIGHLIGHT] as const;

function rankOf(mark: { readonly active?: boolean; readonly hover?: boolean; readonly style?: MarkStyle }): MarkRank {
  if (mark.active) return 4;
  if (mark.hover) return 3;
  return mark.style === "resolved" ? 0 : mark.style === "changed" ? 1 : 2;
}

const MONO = `"SplittyMono", "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`;

const BASE_CSS = `
.markdown-body{box-sizing:border-box;max-width:960px;margin:0 auto;padding:16px 28px 40px;color:var(--splitty-text-primary,#c8c8c8);font:14px/1.6 var(--splitty-font-family,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif);overflow-wrap:break-word}
.markdown-body>:first-child{margin-top:0}
.markdown-body h1,.markdown-body h2,.markdown-body h3,.markdown-body h4,.markdown-body h5,.markdown-body h6{margin:1.4em 0 .6em;line-height:1.25;font-weight:600}
.markdown-body h1{font-size:1.9em;padding-bottom:.3em;border-bottom:1px solid var(--splitty-border-default,#2a2a2a)}
.markdown-body h2{font-size:1.45em;padding-bottom:.3em;border-bottom:1px solid var(--splitty-border-default,#2a2a2a)}
.markdown-body h3{font-size:1.2em}
.markdown-body h4{font-size:1em}
.markdown-body h5,.markdown-body h6{font-size:.9em;color:var(--splitty-text-secondary,#8a8a8a)}
.markdown-body p,.markdown-body ul,.markdown-body ol,.markdown-body blockquote,.markdown-body pre,.markdown-body table{margin:0 0 1em}
.markdown-body ul,.markdown-body ol{padding-left:2em}
.markdown-body li+li{margin-top:.2em}
.markdown-body blockquote{padding:0 1em;color:var(--splitty-text-secondary,#8a8a8a);border-left:3px solid var(--splitty-border-default,#2a2a2a)}
.markdown-body code{font-family:${MONO};font-size:.88em;padding:.1em .35em;border-radius:4px;background:var(--splitty-surface-raised,#1a1a1a)}
.markdown-body pre{padding:12px 14px;overflow:auto;border-radius:6px;background:var(--splitty-surface-raised,#1a1a1a);line-height:1.5}
.markdown-body pre code{padding:0;font-size:12.5px;background:none;white-space:pre}
.markdown-body table{display:block;width:max-content;max-width:100%;overflow:auto;border-collapse:collapse}
.markdown-body th,.markdown-body td{padding:5px 12px;border:1px solid var(--splitty-border-default,#2a2a2a)}
.markdown-body th{font-weight:600}
.markdown-body tr:nth-child(2n){background:var(--splitty-hover-bg,rgba(255,255,255,.04))}
.markdown-body hr{height:0;margin:1.5em 0;border:0;border-top:1px solid var(--splitty-border-default,#2a2a2a)}
.markdown-body img{max-width:100%}
.markdown-body [data-splitty-href]{color:var(--splitty-accent,#63a0ff);text-decoration:underline;text-underline-offset:2px;cursor:pointer}
.markdown-body .contains-task-list{padding-left:1.4em;list-style:none}
.markdown-body .task-list-item{position:relative}
.task-box{display:inline-block;box-sizing:border-box;width:.95em;height:.95em;margin:0 .45em 0 -1.35em;vertical-align:-.12em;border:1px solid var(--splitty-text-secondary,#8a8a8a);border-radius:3px}
.task-box[aria-checked="true"]{border-color:var(--splitty-accent,#63a0ff);background:var(--splitty-accent,#63a0ff)}
.task-box[aria-checked="true"]::after{content:"";display:block;width:.25em;height:.5em;margin:.06em auto 0;border:solid var(--splitty-surface-base,#0f0f0f);border-width:0 .12em .12em 0;transform:rotate(45deg)}
.missing-image{display:inline-block;padding:1px 6px;font-size:.9em;border:1px dashed currentColor;border-radius:4px;opacity:.7}
[data-splitty-html]{all:initial;display:block;box-sizing:border-box;min-height:100%;background:#fff;color:#000;color-scheme:light}
[data-splitty-body]{display:block;margin:8px}
[data-splitty-html] [data-splitty-href]{color:#0645ad;text-decoration:underline;cursor:pointer}
[data-splitty-href]:focus-visible{outline:2px solid var(--splitty-accent,#63a0ff);outline-offset:2px}
::highlight(${HIGHLIGHT}){background-color:var(--splitty-annotation-bg,rgba(99,160,255,.24))}
::highlight(${CHANGED_HIGHLIGHT}){background-color:var(--splitty-annotation-changed-bg,rgba(229,192,123,.24));text-decoration:underline dotted var(--splitty-warning,#e5c07b)}
::highlight(${RESOLVED_HIGHLIGHT}){background-color:var(--splitty-annotation-resolved-bg,rgba(138,138,138,.18))}
::highlight(${HOVER_HIGHLIGHT}){background-color:var(--splitty-annotation-hover-bg,rgba(99,160,255,.36))}
::highlight(${ACTIVE_HIGHLIGHT}){background-color:var(--splitty-annotation-active-bg,rgba(99,160,255,.5))}
[${BLOCK_MARK}]{background-color:var(--splitty-annotation-block-bg,rgba(99,160,255,.12));box-shadow:-6px 0 0 -3px var(--splitty-accent,#63a0ff);border-radius:2px}
[${BLOCK_MARK}="changed"]{background-color:var(--splitty-annotation-changed-bg,rgba(229,192,123,.16));box-shadow:-6px 0 0 -3px var(--splitty-warning,#e5c07b)}
[${BLOCK_MARK}="resolved"]{background-color:var(--splitty-annotation-resolved-bg,rgba(138,138,138,.12));box-shadow:-6px 0 0 -3px var(--splitty-text-muted,#6a6a6a)}
[${BLOCK_MARK}="hover"]{background-color:var(--splitty-annotation-hover-bg,rgba(99,160,255,.22))}
[${BLOCK_MARK}="active"]{background-color:var(--splitty-annotation-active-bg,rgba(99,160,255,.3))}
${SYNTAX_CSS}
`;

function elementOf(target: EventTarget | null): Element | null {
  const node = target as Node | null;
  if (!node) return null;
  return node.nodeType === 1 ? node as Element : node.parentElement;
}

/** The link of the document that `target` is in, if any. */
function linkOf(target: EventTarget | null): Element | null {
  return elementOf(target)?.closest("[data-splitty-href]") ?? null;
}

function adoptable(root: ShadowRoot): boolean {
  return Array.isArray((root as ShadowRoot & { adoptedStyleSheets?: unknown }).adoptedStyleSheets);
}

function sourceLine(node: Node | null, attribute: "data-source-line" | "data-source-line-end"): number | undefined {
  const value = elementOf(node)?.closest(`[${attribute}]`)?.getAttribute(attribute);
  const line = value ? Number(value) : NaN;
  return Number.isSafeInteger(line) && line > 0 ? line : undefined;
}

function lineAttribute(element: Element, attribute: string): number | undefined {
  const line = Number(element.getAttribute(attribute));
  return Number.isSafeInteger(line) && line > 0 ? line : undefined;
}

/** The source lines a block names, or null. */
function blockLines(element: Element): { start: number; end: number } | null {
  const start = lineAttribute(element, "data-source-line");
  if (start === undefined) return null;
  return { start, end: Math.max(start, lineAttribute(element, "data-source-line-end") ?? start) };
}

const SHOW_TEXT = 0x4;
const TEXT_NODE = 3;

/** The parts of a boundary range that the selection APIs share. */
export interface BoundaryRange {
  readonly startContainer: Node;
  readonly startOffset: number;
  readonly endContainer: Node;
  readonly endOffset: number;
  readonly collapsed: boolean;
}

type ComposedSelection = Selection & { getComposedRanges?: (...args: unknown[]) => BoundaryRange[] };

/**
 * The selection within `root`, read across its shadow boundary: with
 * `Selection.getComposedRanges` (the options form, then the older form that
 * takes shadow roots as arguments), else `ShadowRoot.getSelection`, else the
 * document's selection where an engine exposes shadow ranges there. Null
 * without a selection, or for one that leaves the root.
 */
export function shadowSelection(root: ShadowRoot): BoundaryRange | null {
  const inside = (range: BoundaryRange | undefined): range is BoundaryRange =>
    !!range && !range.collapsed && root.contains(range.startContainer) && root.contains(range.endContainer);
  const selection = root.ownerDocument.getSelection() as ComposedSelection | null;
  if (selection && typeof selection.getComposedRanges === "function") {
    for (const args of [[{ shadowRoots: [root] }], [root]]) {
      try {
        const [range] = selection.getComposedRanges(...args);
        if (inside(range)) return range;
      } catch {
        // An engine that knows only the other form.
      }
    }
  }
  const own = (root as ShadowRoot & { getSelection?: () => Selection | null }).getSelection?.();
  for (const candidate of [own, selection]) {
    if (!candidate || candidate.rangeCount === 0) continue;
    const range = candidate.getRangeAt(0);
    if (inside(range)) return range;
  }
  return null;
}

/**
 * The text of a rendered document's content as annotations anchor in it:
 * its text nodes in DOM order, and the source lines of each run.
 */
export class RenderedText {
  readonly text: string;
  readonly lines: LineRange;
  private readonly nodes: Text[] = [];
  private readonly starts: number[] = [];
  private readonly index = new Map<Text, number>();

  /**
   * `content` is the element that holds the document; `last` its source's
   * last line. A Markdown document's blocks name their lines; an HTML page
   * names none, and a passage of it spans lines 1 to `last`.
   */
  constructor(private readonly content: Element, format: "markdown" | "html", last: number) {
    const parts: string[] = [];
    const segments: LineSegment[] = [];
    const lineCache = new Map<Element, { start: number; end: number } | null>();
    const linesOf = (node: Text) => {
      const parent = node.parentElement;
      if (!parent) return null;
      const block = parent.closest("[data-source-line]");
      if (!block || !content.contains(block)) return null;
      let lines = lineCache.get(block);
      if (lines === undefined) {
        lines = blockLines(block);
        lineCache.set(block, lines);
      }
      return lines;
    };
    const walker = content.ownerDocument.createTreeWalker(content, SHOW_TEXT);
    let length = 0;
    let lastLines: { start: number; end: number } | null | undefined;
    for (let node = walker.nextNode() as Text | null; node; node = walker.nextNode() as Text | null) {
      if (node.data.length === 0 || node.parentElement?.closest("style")) continue;
      this.index.set(node, this.nodes.length);
      this.nodes.push(node);
      this.starts.push(length);
      parts.push(node.data);
      if (format === "markdown") {
        const lines = linesOf(node);
        if (lines !== lastLines) segments.push({ start: length, lines });
        lastLines = lines;
      }
      length += node.data.length;
    }
    this.text = parts.join("");
    const whole = Math.max(1, last);
    this.lines = format === "markdown" ? segmentLines(segments, whole) : () => ({ startLine: 1, endLine: whole });
  }

  /** The offset in the text of a boundary point, or undefined for a point outside the content. */
  offsetOf(container: Node, offset: number): number | undefined {
    if (container.nodeType === TEXT_NODE) {
      const at = this.index.get(container as Text);
      if (at !== undefined) return this.starts[at] + Math.max(0, Math.min(offset, (container as Text).data.length));
    }
    if (!this.content.contains(container)) return undefined;
    // A point between nodes: where the first text node after it starts.
    const probe = this.content.ownerDocument.createRange();
    probe.setStart(container, offset);
    let low = 0;
    let high = this.nodes.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (probe.comparePoint(this.nodes[middle], 0) < 0) low = middle + 1;
      else high = middle;
    }
    return low < this.nodes.length ? this.starts[low] : this.text.length;
  }

  /** The offsets of a range within the content, or undefined when it leaves it. */
  offsetsOf(range: BoundaryRange): { start: number; end: number } | undefined {
    const start = this.offsetOf(range.startContainer, range.startOffset);
    const end = this.offsetOf(range.endContainer, range.endOffset);
    if (start === undefined || end === undefined) return undefined;
    return { start: Math.min(start, end), end: Math.max(start, end) };
  }

  /** A DOM range of the passage `[start, end)`, or null for an empty one. */
  range(start: number, end: number): Range | null {
    if (end <= start || this.nodes.length === 0) return null;
    const first = this.nodeAt(start, false);
    const last = this.nodeAt(end, true);
    const range = this.content.ownerDocument.createRange();
    range.setStart(this.nodes[first], Math.max(0, start - this.starts[first]));
    range.setEnd(this.nodes[last], Math.min(this.nodes[last].data.length, end - this.starts[last]));
    return range;
  }

  /** The elements that hold the passage `[start, end)`: the innermost blocks of its text. */
  blocksOf(start: number, end: number): Element[] {
    if (end <= start || this.nodes.length === 0) return [];
    const blocks = new Set<Element>();
    for (let i = this.nodeAt(start, false); i <= this.nodeAt(end, true); i++) {
      const parent = this.nodes[i].parentElement;
      const block = parent?.closest("p, li, h1, h2, h3, h4, h5, h6, pre, blockquote, td, th, dt, dd, figcaption, caption") ?? parent;
      if (block && this.content.contains(block) && block !== this.content) blocks.add(block);
    }
    return [...blocks];
  }

  /**
   * The text node that holds `offset`: at a boundary between two nodes, the
   * later one for a passage's start and the earlier one for its end.
   */
  private nodeAt(offset: number, end: boolean): number {
    let low = 0;
    let high = this.nodes.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      const start = this.starts[middle];
      if (start < offset || (!end && start === offset)) low = middle;
      else high = middle - 1;
    }
    return low;
  }
}

/** The Custom Highlight API of a window, where it has one. */
interface HighlightApi {
  readonly registry: Map<string, Set<AbstractRange>>;
  create(): Set<AbstractRange>;
}

function highlightApi(doc: Document): HighlightApi | undefined {
  const view = doc.defaultView as (Window & { Highlight?: new (...ranges: AbstractRange[]) => Set<AbstractRange>; CSS?: { highlights?: Map<string, Set<AbstractRange>> } }) | null;
  const Highlight = view?.Highlight;
  const registry = view?.CSS?.highlights;
  if (typeof Highlight !== "function" || !registry) return undefined;
  return { registry, create: () => new Highlight() };
}

/** The window's shared highlight `name`, created on first use. */
function sharedHighlight(api: HighlightApi, rank: MarkRank): Set<AbstractRange> {
  const name = RANK_HIGHLIGHTS[rank];
  let highlight = api.registry.get(name);
  if (!highlight) {
    highlight = api.create();
    // Overlapping highlights paint by priority, not by the order they were registered in.
    (highlight as { priority?: number }).priority = rank;
    api.registry.set(name, highlight);
  }
  return highlight;
}

/** The content of a document as `render` shows it, for its text only: nothing of it reaches a live document. */
export function renderedText(doc: Document, format: "markdown" | "html", text: string, last: number): RenderedText {
  const content = doc.createElement("div");
  content.append(sanitizeHtml(format === "markdown" ? markdownToHtml(text) : text));
  return new RenderedText(content, format, last);
}

/** A rendered document in its shadow root. */
export class RenderedDocument {
  /** The scroll container and shadow host. */
  readonly host: HTMLElement;
  private readonly root: ShadowRoot;
  private generation = 0;
  /** The link under the pointer, the one with the keyboard focus, and the one last reported. */
  private pointed: Element | null = null;
  private focused: Element | null = null;
  private reported: Element | null = null;
  private format: "markdown" | "html" = "markdown";
  /** The element that holds the document's content, and its last source line. */
  private content: Element | null = null;
  private last = 1;
  private model: RenderedText | null = null;
  private marks: readonly RenderedMark[] = [];
  private blockMarks: readonly BlockMark[] = [];
  /** The ranges this document added to the window's highlights, and the blocks it marked. */
  private highlighted: { readonly highlight: Set<AbstractRange>; readonly range: AbstractRange }[] = [];
  private markedBlocks: Element[] = [];
  /** The ranges of the marked passages, and the annotations of each marked block, for `marksAt`. */
  private passageRanges: { readonly mark: RenderedMark; readonly range: Range }[] = [];
  private blockOwners = new Map<Element, string[]>();

  constructor(doc: Document, private readonly options: RenderOptions) {
    this.host = doc.createElement("div");
    this.host.className = "viewer-rendered";
    this.host.setAttribute("style", HOST_STYLE);
    // Focusable for keyboard scrolling, never in the tab order.
    this.host.tabIndex = -1;
    this.root = this.host.attachShadow({ mode: "open" });
    this.root.addEventListener("click", this.onClick);
    this.root.addEventListener("auxclick", this.onAuxClick);
    this.root.addEventListener("keydown", this.onKeyDown);
    this.root.addEventListener("focusin", this.onFocusIn);
    this.root.addEventListener("focusout", this.onFocusOut);
    // On the host, which also sees the pointer over itself: its scroll bars
    // and the room the document leaves around its content.
    this.host.addEventListener("pointerover", this.onPointerOver);
    this.host.addEventListener("pointerleave", this.onPointerLeave);
  }

  /**
   * Show `text` as `format`, replacing what was shown. `last` is the
   * source's last line, which the passages of an HTML page span to.
   */
  render(format: "markdown" | "html", text: string, last = 1): void {
    const generation = ++this.generation;
    const doc = this.host.ownerDocument;
    const base = doc.createElement("style");
    base.textContent = BASE_CSS;
    const nodes: Node[] = [base];
    let sheet: CSSStyleSheet | null = null;
    // Marks name passages of the text shown so far.
    this.clearMarks();
    this.marks = [];
    this.blockMarks = [];
    this.format = format;
    this.last = last;
    this.model = null;
    if (format === "markdown") {
      const body = doc.createElement("div");
      body.className = "markdown-body";
      body.append(sanitizeHtml(markdownToHtml(text)));
      nodes.push(body);
      this.content = body;
    } else {
      const styles = documentStyles(text);
      sheet = sanitizeStyleSheet(styles.css, doc);
      const page = doc.createElement("div");
      page.setAttribute("data-splitty-html", "");
      const body = doc.createElement("div");
      body.setAttribute("data-splitty-body", "");
      if (styles.bodyClass) body.className = styles.bodyClass;
      body.append(sanitizeHtml(text));
      page.append(body);
      this.content = body;
      if (sheet && !adoptable(this.root)) {
        // Without constructable sheets in shadow roots: the sanitized rules as
        // the engine itself serializes them.
        const style = doc.createElement("style");
        style.textContent = [...sheet.cssRules].map((rule) => rule.cssText).join("\n");
        nodes.push(style);
      }
      nodes.push(page);
    }
    if (adoptable(this.root)) this.root.adoptedStyleSheets = sheet ? [sheet] : [];
    this.root.replaceChildren(...nodes);
    // The links of the old content are gone.
    this.pointed = null;
    this.focused = null;
    this.reportLink();
    void this.highlight(generation);
  }

  /** The shadow root, for tests and for marks a later change places in it. */
  get shadow(): ShadowRoot {
    return this.root;
  }

  /** The document's text as annotations anchor in it; null before the first render. */
  textModel(): RenderedText | null {
    if (!this.content) return null;
    this.model ??= new RenderedText(this.content, this.format, this.last);
    return this.model;
  }

  /** The selected passage as offsets into `text()`, with the range it covers; undefined without one. */
  selectedPassage(): { start: number; end: number; range: BoundaryRange } | undefined {
    const range = shadowSelection(this.root);
    const model = this.textModel();
    if (!range || !model) return undefined;
    const offsets = model.offsetsOf(range);
    return offsets && offsets.end > offsets.start ? { ...offsets, range } : undefined;
  }

  /**
   * Mark passages of the text and the blocks of source lines; replaces the
   * marks set before. Kept over the highlighting of code blocks.
   */
  setMarks(marks: readonly RenderedMark[], blocks: readonly BlockMark[] = []): void {
    this.marks = marks;
    this.blockMarks = blocks;
    this.applyMarks();
  }

  /**
   * The IDs of the marks at the point `x`, `y` of the window, where `target`
   * is the element under it: the passages there and the marked blocks that
   * hold it. A link is no mark's: a click on it follows the link.
   */
  marksAt(target: EventTarget | null, x: number, y: number): string[] {
    const element = elementOf(target);
    if (!element || !this.root.contains(element) || linkOf(element)) return [];
    const ids = new Set<string>();
    for (let at: Element | null = element; at && this.root.contains(at); at = at.parentElement) {
      for (const id of this.blockOwners.get(at) ?? []) ids.add(id);
    }
    let caret: number | null | undefined = null;
    for (const { mark, range } of this.passageRanges) {
      if (!mark.id || ids.has(mark.id)) continue;
      const rects = typeof range.getClientRects === "function"
        ? [...range.getClientRects()].filter((rect) => rect.width > 0 || rect.height > 0)
        : [];
      if (rects.length === 0 && caret === null) caret = this.caretOffset(x, y);
      const under = rects.length > 0
        ? rects.some((rect) => x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom)
        : typeof caret === "number" && mark.start <= caret && caret <= mark.end;
      if (under) ids.add(mark.id);
    }
    return [...ids];
  }

  /** Where the engine's caret would go at the point `x`, `y`, as an offset into the text; undefined outside it. */
  private caretOffset(x: number, y: number): number | undefined {
    const doc = this.host.ownerDocument as Document & {
      caretPositionFromPoint?: (x: number, y: number, options?: { shadowRoots?: ShadowRoot[] }) => { offsetNode: Node; offset: number } | null;
      caretRangeFromPoint?: (x: number, y: number) => Range | null;
    };
    let node: Node | undefined;
    let offset = 0;
    try {
      const position = doc.caretPositionFromPoint?.(x, y, { shadowRoots: [this.root] });
      if (position) ({ offsetNode: node, offset } = position);
      else {
        const range = doc.caretRangeFromPoint?.(x, y);
        if (range) ({ startContainer: node, startOffset: offset } = range);
      }
    } catch {
      return undefined;
    }
    if (!node || !this.root.contains(node)) return undefined;
    return this.textModel()?.offsetOf(node, offset);
  }

  /** Where the passage `[start, end)` is on the screen, or undefined where the engine cannot tell. */
  passageRect(start: number, end: number): DOMRect | undefined {
    const range = this.textModel()?.range(start, end);
    return range && typeof range.getBoundingClientRect === "function" ? range.getBoundingClientRect() : undefined;
  }

  /** Bring the passage `[start, end)` into view, near the top third. */
  scrollToPassage(start: number, end: number): void {
    const rect = this.passageRect(start, end);
    const block = rect ? undefined : this.textModel()?.blocksOf(start, end)[0];
    this.scrollToRect(rect ?? block?.getBoundingClientRect());
  }

  /** Bring the first block of the source lines into view. */
  scrollToLines(startLine: number, endLine: number): void {
    this.scrollToRect(this.blocksFor(startLine, endLine)[0]?.getBoundingClientRect());
  }

  selection(): RenderedSelection | undefined {
    const own = (this.root as ShadowRoot & { getSelection?: () => Selection | null }).getSelection?.();
    const selection = own ?? this.host.ownerDocument.getSelection();
    const text = selection?.toString() ?? "";
    if (!selection || !text) return undefined;
    // Either end may come first: a selection made backwards starts at its focus.
    const ends = [selection.anchorNode, selection.focusNode];
    const starts = ends.map((node) => sourceLine(node, "data-source-line")).filter((line) => line !== undefined);
    const stops = ends.map((node) => sourceLine(node, "data-source-line-end")).filter((line) => line !== undefined);
    return {
      text,
      ...(starts.length ? { startLine: Math.min(...starts) } : {}),
      ...(stops.length ? { endLine: Math.max(...stops) } : {}),
    };
  }

  /** How far down the document is scrolled, 0 to 1. */
  scrollFraction(): number {
    const max = this.host.scrollHeight - this.host.clientHeight;
    return max > 0 ? this.host.scrollTop / max : 0;
  }

  scrollToFraction(fraction: number): void {
    const max = this.host.scrollHeight - this.host.clientHeight;
    if (max > 0) this.host.scrollTop = Math.round(fraction * max);
  }

  focus(): void {
    this.host.focus({ preventScroll: true });
  }

  destroy(): void {
    this.generation++;
    this.clearMarks();
    this.marks = [];
    this.blockMarks = [];
    this.root.removeEventListener("click", this.onClick);
    this.root.removeEventListener("auxclick", this.onAuxClick);
    this.root.removeEventListener("keydown", this.onKeyDown);
    this.root.removeEventListener("focusin", this.onFocusIn);
    this.root.removeEventListener("focusout", this.onFocusOut);
    this.host.removeEventListener("pointerover", this.onPointerOver);
    this.host.removeEventListener("pointerleave", this.onPointerLeave);
    this.pointed = this.focused = this.reported = null;
    this.root.replaceChildren();
    this.host.remove();
  }

  private follow(href: string): void {
    const link = classifyLink(href, this.options.documentPath);
    if (link.kind === "fragment") this.scrollToId(link.id);
    else if (link.kind === "external") this.options.openExternal(link.url);
    else if (link.kind === "file") this.options.openFile(link.path);
    else if (link.kind === "refused") this.options.refuseLink();
  }

  private scrollToId(id: string): void {
    if (!id) {
      this.host.scrollTop = 0;
      return;
    }
    const target = this.root.getElementById(id)
      ?? [...this.root.querySelectorAll("[name]")].find((element) => element.getAttribute("name") === id);
    if (!target) return;
    const offset = target.getBoundingClientRect().top - this.host.getBoundingClientRect().top;
    this.host.scrollTop = Math.max(0, this.host.scrollTop + offset - 8);
  }

  // A shadow root's listener map knows no mouse or key events, so these take
  // any event.
  private readonly onClick = (event: Event): void => {
    const element = elementOf(event.target);
    const href = linkTarget(element);
    if (href === undefined) {
      if (element?.closest("a")) event.preventDefault();
      return;
    }
    event.preventDefault();
    if ((event as MouseEvent).button === 0) this.follow(href);
  };

  private readonly onAuxClick = (event: Event): void => {
    if (elementOf(event.target)?.closest("a, [data-splitty-href]")) event.preventDefault();
  };

  private readonly onKeyDown = (event: Event): void => {
    if ((event as KeyboardEvent).key !== "Enter") return;
    const href = linkTarget(elementOf(event.target));
    if (href === undefined) return;
    event.preventDefault();
    this.follow(href);
  };

  // Moving over any element of the document, or over the host itself, says
  // which link, if any, is under the pointer; leaving the host leaves them all.
  // The event's target at the host is the host; its path starts at the element
  // under the pointer, as the shadow root is open.
  private readonly onPointerOver = (event: Event): void => {
    this.pointed = linkOf(event.composedPath()[0] ?? null);
    this.reportLink();
  };

  private readonly onPointerLeave = (): void => {
    this.pointed = null;
    this.reportLink();
  };

  private readonly onFocusIn = (event: Event): void => {
    this.focused = linkOf(event.target);
    this.reportLink();
  };

  // Focus that moves on to another link arrives with the next `focusin`.
  private readonly onFocusOut = (): void => {
    this.focused = null;
    this.reportLink();
  };

  /** Report the pointed link, or else the focused one, when that changed. */
  private reportLink(): void {
    const link = this.pointed ?? this.focused;
    if (link === this.reported) return;
    this.reported = link;
    const href = link?.getAttribute("data-splitty-href");
    this.options.pointLink?.(href === null || href === undefined ? undefined : classifyLink(href, this.options.documentPath));
  }

  private async highlight(generation: number): Promise<void> {
    for (const code of [...this.root.querySelectorAll<HTMLElement>("pre > code[data-language]")]) {
      const text = code.textContent ?? "";
      const language = text.length <= MAX_HIGHLIGHT_CHARS ? languageForName(code.dataset.language ?? "") : null;
      if (!language) continue;
      const support = await loadLanguage(language);
      if (generation !== this.generation) return;
      if (!support || !code.isConnected || code.textContent !== text) continue;
      code.replaceChildren(...highlightedNodes(code.ownerDocument, text, support));
      // The text is the same, its nodes are new: the marks follow them.
      this.model = null;
      if (this.marks.length > 0 || this.blockMarks.length > 0) this.applyMarks();
    }
  }

  private scrollToRect(rect: DOMRect | undefined): void {
    if (!rect) return;
    const top = rect.top - this.host.getBoundingClientRect().top;
    this.host.scrollTop = Math.max(0, this.host.scrollTop + top - this.host.clientHeight / 3);
  }

  /** The innermost blocks that name lines within `[startLine, endLine]`. */
  private blocksFor(startLine: number, endLine: number): Element[] {
    if (!this.content || this.format !== "markdown") return [];
    const hits = [...this.content.querySelectorAll("[data-source-line]")].filter((block) => {
      const lines = blockLines(block);
      return !!lines && lines.end >= startLine && lines.start <= endLine;
    });
    // In document order, a block that holds another hit is followed by one.
    return hits.filter((block, i) => !(i + 1 < hits.length && block.contains(hits[i + 1])));
  }

  private applyMarks(): void {
    this.clearMarks();
    const model = this.textModel();
    if (!model) return;
    const api = highlightApi(this.host.ownerDocument);
    const blocks = new Map<Element, MarkRank>();
    const markBlock = (block: Element, mark: RenderedMark | BlockMark) => {
      blocks.set(block, Math.max(blocks.get(block) ?? 0, rankOf(mark)) as MarkRank);
      if (mark.id) this.blockOwners.set(block, [...(this.blockOwners.get(block) ?? []), mark.id]);
    };
    for (const mark of this.marks) {
      if (!api) {
        for (const block of model.blocksOf(mark.start, mark.end)) markBlock(block, mark);
        continue;
      }
      const range = model.range(mark.start, mark.end);
      if (!range) continue;
      const highlight = sharedHighlight(api, rankOf(mark));
      highlight.add(range);
      this.highlighted.push({ highlight, range });
      this.passageRanges.push({ mark, range });
    }
    for (const mark of this.blockMarks) {
      for (const block of this.blocksFor(mark.startLine, mark.endLine)) markBlock(block, mark);
    }
    for (const [block, rank] of blocks) {
      block.setAttribute(BLOCK_MARK, RANK_VALUES[rank]);
      this.markedBlocks.push(block);
    }
  }

  private clearMarks(): void {
    for (const { highlight, range } of this.highlighted) highlight.delete(range);
    this.highlighted = [];
    for (const block of this.markedBlocks) block.removeAttribute(BLOCK_MARK);
    this.markedBlocks = [];
    this.passageRanges = [];
    this.blockOwners.clear();
  }
}
