/**
 * What of a document reaches the page (`access-control`, Rendered Documents
 * Are Inert).
 *
 * Markdown output and HTML documents pass DOMPurify with its HTML and SVG
 * profiles and a stricter policy on top:
 *
 * - No element that runs, embeds, submits, navigates or loads: `script`,
 *   frames, `object`, `embed`, forms and their controls, `base`, `meta`,
 *   `link`, media, `template`, `style` (an HTML document's style sheets are
 *   taken apart separately, see {@link sanitizeStyleSheet}).
 * - No event handlers (DOMPurify), no `srcset`, `ping`, `target`, `download`,
 *   `autofocus`, `tabindex`, `accesskey`, `contenteditable`, `popover` and
 *   their kin.
 * - Links lose their `href`: it moves to `data-splitty-href`, and the pane
 *   handles a click itself (`render.ts`), so nothing in a document navigates.
 * - Images keep only `data:image/…` sources; any other image is dropped
 *   before it reaches a live document, so it is never requested, and shows its
 *   alt text instead. SVG references stay within the document (`#…`).
 * - Style attributes and style sheets lose every declaration that would load
 *   something (`url()` other than `data:image/…` and in-document references,
 *   `image-set()` and its kin, also spelled with escapes or inside `var()`),
 *   every declaration whose value the CSSOM does not show (a shorthand with
 *   `var()` is checked by its own text), cursor images, and every rule that
 *   could reach outside the shadow root the document is shown in (`:host`,
 *   `::slotted`, `::part`) or load fonts and sheets (`@import`, `@font-face`).
 *
 * DOMPurify's protection against DOM clobbering is off: the result goes into a
 * shadow root, where IDs and names reach neither `document` nor `window`, and
 * headings keep their IDs for `#…` links.
 */

import DOMPurify, { type Config } from "dompurify";

type Purifier = ReturnType<typeof DOMPurify>;

const SVG_NS = "http://www.w3.org/2000/svg";

const FORBID_TAGS = [
  "script", "noscript", "iframe", "frame", "frameset", "object", "embed", "applet", "portal",
  "form", "input", "button", "select", "option", "optgroup", "textarea", "datalist",
  "base", "meta", "link", "style", "title",
  "area", "map", "audio", "video", "source", "track", "picture", "canvas",
  "dialog", "template", "slot", "content", "shadow", "element", "decorator", "menuitem",
  "marquee", "blink",
];

const FORBID_ATTR = [
  "srcset", "sizes", "background", "ping", "poster", "action", "formaction", "target", "download",
  "popover", "popovertarget", "popovertargetaction", "command", "commandfor", "interestfor",
  "autofocus", "contenteditable", "tabindex", "accesskey",
  "part", "exportparts", "slot", "is", "nonce", "integrity", "crossorigin", "referrerpolicy",
  "usemap", "ismap", "shadowroot", "shadowrootmode",
];

const CONFIG: Config & { RETURN_DOM_FRAGMENT: true } = {
  USE_PROFILES: { html: true, svg: true, svgFilters: true },
  FORBID_TAGS,
  FORBID_ATTR,
  SANITIZE_DOM: false,
  ALLOW_UNKNOWN_PROTOCOLS: false,
  RETURN_DOM_FRAGMENT: true,
};

/** Attributes whose value is text for people, never a CSS value or a reference. */
const TEXT_ATTRIBUTES = new Set(["alt", "title", "aria-label", "aria-description", "aria-roledescription", "aria-valuetext", "aria-placeholder"]);

/** Properties whose `url(#…)` names an element of the document instead of loading a resource. */
const REFERENCE_PROPERTIES = new Set([
  "fill", "stroke", "filter", "clip-path", "mask", "marker", "marker-start", "marker-mid", "marker-end",
]);

const RESOURCE_FUNCTION = /(?<![\w-])(?:-webkit-|-moz-)?(url|src|image-set|image|cross-fade|element|paint)\s*\(\s*(['"]?)\s*/gi;

/**
 * `text` with its CSS escapes resolved. The CSSOM resolves escapes in the
 * values it parses, but a value holding `var()` or another function that is
 * substituted where the value is used keeps its text as written, so
 * `u\72 l(…)` there is a `url()` once it is used.
 */
export function unescapeCss(text: string): string {
  return text.replace(/\\(?:([0-9a-fA-F]{1,6})[ \t\n\r\f]?|([\s\S]))/g, (_match, hex: string | undefined, other: string | undefined) => {
    if (hex !== undefined) {
      const code = Number.parseInt(hex, 16);
      return code === 0 || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff) ? "�" : String.fromCodePoint(code);
    }
    return other === "\n" ? "" : (other ?? "");
  });
}

/**
 * Whether a CSS value may stay: it loads nothing. A `url()` may hold a
 * `data:image/…` URL, and `#…` in the properties that name an element of the
 * document; every other `url()` and every `image-set()`, `src()`, `image()`,
 * `cross-fade()`, `element()` and `paint()` goes, also when escapes spell it.
 * `attr()` stays only in `content`, `cursor` keeps only keywords (the system
 * draws a cursor image around the pointer, beyond the pane), and a custom
 * property with an escape goes, since it could spell any of these where it is
 * used.
 */
export function cssValueAllowed(property: string, value: string): boolean {
  const name = property.toLowerCase();
  if (name.startsWith("--") && value.includes("\\")) return false;
  const text = value.includes("\\") ? unescapeCss(value) : value;
  if (name === "cursor" && text.includes("(")) return false;
  if (/(?<![\w-])attr\s*\(/i.test(text) && name !== "content") return false;
  for (const match of text.matchAll(RESOURCE_FUNCTION)) {
    if (match[1].toLowerCase() !== "url") return false;
    const target = text.slice(match.index + match[0].length);
    if (/^data:image\//i.test(target)) continue;
    if (target.startsWith("#") && REFERENCE_PROPERTIES.has(name)) continue;
    return false;
  }
  return true;
}

/** The property names of a serialized declaration block, in order. */
export function declaredNames(cssText: string): string[] {
  const names: string[] = [];
  let depth = 0;
  let quote = "";
  let start = 0;
  for (let i = 0; i <= cssText.length; i++) {
    const char = cssText[i];
    if (quote) {
      if (char === "\\") i++;
      else if (char === quote) quote = "";
      continue;
    }
    if (char === "\\") {
      i++;
    } else if (char === "\"" || char === "'") {
      quote = char;
    } else if (char === "(" || char === "[" || char === "{") {
      depth++;
    } else if (char === ")" || char === "]" || char === "}") {
      depth = Math.max(0, depth - 1);
    } else if (char === undefined || (char === ";" && depth === 0)) {
      const declaration = cssText.slice(start, i);
      const colon = declaration.indexOf(":");
      if (colon > 0) names.push(declaration.slice(0, colon).trim().toLowerCase());
      start = i + 1;
    }
  }
  return names;
}

const longhandCache = new Map<string, readonly string[]>();
let probe: CSSStyleDeclaration | null = null;

/** The longhands the engine expands `property` into; a longhand is its own. */
function engineLonghands(property: string): readonly string[] {
  const cached = longhandCache.get(property);
  if (cached) return cached;
  probe ??= document.createElement("div").style;
  probe.cssText = "";
  probe.setProperty(property, "initial");
  const longhands: string[] = [];
  for (let i = 0; i < probe.length; i++) longhands.push(probe.item(i));
  probe.cssText = "";
  longhandCache.set(property, longhands);
  return longhands;
}

/**
 * Remove the declarations of `style` that load something; returns how many
 * remain.
 *
 * A longhand set by a shorthand whose value holds `var()` or another
 * substitution reads as `""`: the engine keeps the shorthand's text and
 * resolves it where it is used. Such a shorthand serializes as itself, so its
 * text is checked in its place. A longhand that reads as `""` and that no
 * checked shorthand covers goes, since its value cannot be seen.
 */
export function sanitizeDeclarations(
  style: CSSStyleDeclaration,
  longhandsOf: (property: string) => readonly string[] = engineLonghands,
): number {
  const unseen = new Set<string>();
  for (let i = style.length - 1; i >= 0; i--) {
    const property = style.item(i);
    const value = style.getPropertyValue(property);
    if (value === "") unseen.add(property);
    else if (!cssValueAllowed(property, value)) style.removeProperty(property);
  }
  if (unseen.size === 0) return style.length;
  for (const name of declaredNames(style.cssText)) {
    if (name.startsWith("--")) continue;
    const longhands = longhandsOf(name);
    if (!longhands.some((longhand) => unseen.has(longhand))) continue;
    const value = style.getPropertyValue(name);
    if (value !== "" && cssValueAllowed(name, value)) {
      for (const longhand of longhands) unseen.delete(longhand);
    } else {
      style.removeProperty(name);
    }
  }
  for (const property of unseen) style.removeProperty(property);
  return style.length;
}

/** An attribute value that is a CSS value or a list of them (SVG presentation attributes). */
function attributeAllowed(name: string, value: string): boolean {
  if (TEXT_ATTRIBUTES.has(name) || name.startsWith("data-") || name.startsWith("aria-")) return true;
  if (value.includes("\\") && value.includes("(")) return false;
  return cssValueAllowed(name, value);
}

function isImageData(value: string): boolean {
  return /^\s*data:image\//i.test(value);
}

/** The `href` of an element, plain or `xlink:href`; both are removed. */
function takeHref(element: Element): string | null {
  const href = element.getAttribute("href") ?? element.getAttribute("xlink:href");
  element.removeAttribute("href");
  element.removeAttribute("xlink:href");
  return href;
}

function afterAttributes(node: Element): void {
  const element = node;
  const name = element.localName.toLowerCase();
  if (element.hasAttribute("style")) {
    const style = (element as HTMLElement).style;
    if (!style || sanitizeDeclarations(style) === 0) element.removeAttribute("style");
  }
  for (const attribute of [...element.attributes]) {
    if (attribute.name === "style" || attribute.name === "href" || attribute.name === "xlink:href" || attribute.name === "src") continue;
    if (!attributeAllowed(attribute.name, attribute.value)) element.removeAttribute(attribute.name);
  }
  if (name === "a") {
    const href = takeHref(element)?.trim();
    if (href) {
      element.setAttribute("data-splitty-href", href);
      element.setAttribute("tabindex", "0");
      if (!element.hasAttribute("role")) element.setAttribute("role", "link");
    }
    return;
  }
  if (name === "img") {
    const src = element.getAttribute("src");
    if (!src || !isImageData(src)) {
      element.removeAttribute("src");
      element.setAttribute("data-splitty-missing", "");
    }
    return;
  }
  if (element.namespaceURI === SVG_NS) {
    if (!element.hasAttribute("href") && !element.hasAttribute("xlink:href")) return;
    const href = takeHref(element)?.trim() ?? "";
    const embedsImage = name === "image" || name === "feimage";
    if (embedsImage ? isImageData(href) : href.startsWith("#")) element.setAttribute("href", href);
  }
}

let purifier: Purifier | null = null;

function instance(): Purifier {
  if (purifier) return purifier;
  const created = DOMPurify(window);
  // `data-splitty-*` is the pane's own; a document cannot set it.
  created.addHook("uponSanitizeAttribute", (_node, event) => {
    if (event.attrName.toLowerCase().startsWith("data-splitty-")) event.keepAttr = false;
  });
  created.addHook("afterSanitizeAttributes", afterAttributes);
  purifier = created;
  return created;
}

/**
 * `html` as an inert fragment. Images it may not show become their alt text.
 * Nothing of it has been in a live document: DOMPurify parses into an inert
 * one, so an image dropped here was never requested.
 */
export function sanitizeHtml(html: string): DocumentFragment {
  const fragment = instance().sanitize(html, CONFIG);
  for (const image of [...fragment.querySelectorAll("img[data-splitty-missing]")]) {
    const alt = image.getAttribute("alt")?.trim();
    const replacement = image.ownerDocument.createElement("span");
    replacement.className = "missing-image";
    replacement.textContent = alt || "Image";
    replacement.title = "Only images embedded in the document are shown";
    image.replaceWith(replacement);
  }
  return fragment;
}

/** An HTML document's own style sheets and the classes of its `body`, read without running or loading anything. */
export function documentStyles(html: string): { css: string[]; bodyClass?: string } {
  const parsed = new DOMParser().parseFromString(html, "text/html");
  const css = [...parsed.querySelectorAll("style")].map((style) => style.textContent ?? "");
  const bodyClass = parsed.body?.getAttribute("class")?.replace(/[^\w\s-]/g, "").trim().slice(0, 256);
  return { css, ...(bodyClass ? { bodyClass } : {}) };
}

/** The rules that may stay, by their CSSOM class. */
const GROUP_RULES = new Set(["CSSMediaRule", "CSSSupportsRule", "CSSContainerRule", "CSSLayerBlockRule"]);

/** Selectors that reach the shadow host or other trees. */
const ESCAPING_SELECTOR = /:host|::?slotted|::?part\b|::?view-transition|::?picker\b/i;

/** A document's `html`, `:root` and `body` are the wrappers the pane puts around it. */
export function documentSelector(selector: string): string {
  return selector
    .replace(/:root(?![\w-])/gi, "[data-splitty-html]")
    .replace(/(^|[\s>+~,(])(html|body)(?![\w-])/gi, (_match, before: string, name: string) =>
      `${before}${name.toLowerCase() === "html" ? "[data-splitty-html]" : "[data-splitty-body]"}`);
}

interface RuleList {
  readonly cssRules: CSSRuleList;
  deleteRule(index: number): void;
}

function sanitizeRules(list: RuleList): void {
  for (let i = list.cssRules.length - 1; i >= 0; i--) {
    if (!sanitizeRule(list.cssRules[i])) list.deleteRule(i);
  }
}

/** Sanitize `rule` in place; false when it has to go. */
function sanitizeRule(rule: CSSRule): boolean {
  const kind = rule.constructor.name;
  if (kind === "CSSStyleRule") {
    const style = rule as CSSStyleRule;
    if (ESCAPING_SELECTOR.test(style.selectorText)) return false;
    const rewritten = documentSelector(style.selectorText);
    if (rewritten !== style.selectorText) {
      // A selector the engine does not take back stays as it was: the rule goes.
      style.selectorText = rewritten;
      if (documentSelector(style.selectorText) !== style.selectorText) return false;
    }
    sanitizeDeclarations(style.style);
    // Nested rules (CSS nesting) where the engine has them.
    if ("cssRules" in style && "deleteRule" in style) sanitizeRules(style as unknown as RuleList);
    return true;
  }
  if (kind === "CSSNestedDeclarations") {
    sanitizeDeclarations((rule as CSSRule & { style: CSSStyleDeclaration }).style);
    return true;
  }
  if (GROUP_RULES.has(kind)) {
    sanitizeRules(rule as CSSGroupingRule);
    return true;
  }
  if (kind === "CSSLayerStatementRule") return true;
  if (kind === "CSSKeyframesRule") {
    for (const frame of [...(rule as CSSKeyframesRule).cssRules]) sanitizeDeclarations((frame as CSSKeyframeRule).style);
    return true;
  }
  // @import, @font-face, @namespace, @page, @property, @counter-style,
  // @scope, @starting-style, @view-transition and anything newer.
  return false;
}

/**
 * The sanitized rules of an HTML document's style sheets, as a sheet of
 * `doc`: the CSSOM parses the text, and rules and declarations that load
 * something or reach outside the document are removed from the parsed sheet,
 * so nothing is reassembled from strings. Null where the engine cannot
 * construct a sheet; the document then shows without its styles.
 */
export function sanitizeStyleSheet(css: readonly string[], doc: Document): CSSStyleSheet | null {
  const Sheet = doc.defaultView?.CSSStyleSheet;
  if (!Sheet || css.length === 0) return null;
  try {
    const sheet = new Sheet();
    sheet.replaceSync(css.join("\n"));
    sanitizeRules(sheet);
    return sheet;
  } catch (err) {
    console.warn("viewer: the document's styles were not applied", err);
    return null;
  }
}

/** Whether `element` came out of the sanitizer as a link. */
export function linkTarget(element: Element | null): string | undefined {
  return element?.closest("[data-splitty-href]")?.getAttribute("data-splitty-href") ?? undefined;
}
