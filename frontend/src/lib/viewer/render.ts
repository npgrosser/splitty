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
 */

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

  /** Show `text` as `format`, replacing what was shown. */
  render(format: "markdown" | "html", text: string): void {
    const generation = ++this.generation;
    const doc = this.host.ownerDocument;
    const base = doc.createElement("style");
    base.textContent = BASE_CSS;
    const nodes: Node[] = [base];
    let sheet: CSSStyleSheet | null = null;
    if (format === "markdown") {
      const body = doc.createElement("div");
      body.className = "markdown-body";
      body.append(sanitizeHtml(markdownToHtml(text)));
      nodes.push(body);
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
    }
  }
}
