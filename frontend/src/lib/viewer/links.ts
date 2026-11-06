/**
 * What a link of a rendered document does (`access-control`, Rendered
 * Documents Are Inert): `http:` and `https:` go to the external opener, a
 * relative link opens the file it names in a viewer pane, `#…` scrolls within
 * the document, and anything else does nothing.
 *
 * A relative link is resolved against the document's directory and
 * normalized, so the path sent is one the server takes (`isViewerPath`). A link
 * that climbs above `/`, or whose path still breaks that rule, is refused: it
 * opens nothing, and the pane says so.
 */

import { isViewerPath } from "./limits.ts";

export type DocumentLink =
  | { readonly kind: "fragment"; readonly id: string }
  | { readonly kind: "external"; readonly url: string }
  | { readonly kind: "file"; readonly path: string }
  /** A relative link to no path a viewer pane can show. */
  | { readonly kind: "refused" }
  | { readonly kind: "none" };

const NONE: DocumentLink = { kind: "none" };
const REFUSED: DocumentLink = { kind: "refused" };

function decoded(text: string): string | undefined {
  try {
    return decodeURIComponent(text);
  } catch {
    return undefined;
  }
}

/**
 * The absolute path that the relative or root-relative `href` of the document
 * at `documentPath` names. The document's path is taken as it is, the link's
 * path is percent-decoded segment by segment; empty and `.` segments are
 * dropped and `..` takes the previous segment away. Undefined when the link
 * has no path, climbs above `/`, holds an escape that does not decode or
 * decodes to a slash, or comes out as a path the server refuses.
 */
export function resolveDocumentLink(documentPath: string, href: string): string | undefined {
  const end = href.search(/[?#]/);
  const linkPath = end < 0 ? href : href.slice(0, end);
  if (!linkPath) return undefined;
  const segments: string[] = [];
  if (!linkPath.startsWith("/")) {
    // The document's directory: its path without the file's name.
    segments.push(...documentPath.split("/").slice(0, -1).filter((segment) => segment !== ""));
  }
  for (const raw of linkPath.split("/")) {
    const segment = decoded(raw);
    if (segment === undefined || segment.includes("/")) return undefined;
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (segments.length === 0) return undefined;
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  const path = `/${segments.join("/")}`;
  return isViewerPath(path) ? path : undefined;
}

/** What following `href` of the document at `documentPath` does; a text document has no path. */
export function classifyLink(href: string, documentPath: string | undefined): DocumentLink {
  const value = href.trim();
  if (value.startsWith("#")) return { kind: "fragment", id: decoded(value.slice(1)) ?? value.slice(1) };
  const scheme = /^([a-z][a-z\d+.-]*):/i.exec(value)?.[1].toLowerCase();
  if (scheme === "http" || scheme === "https") {
    try {
      return { kind: "external", url: new URL(value).href };
    } catch {
      return NONE;
    }
  }
  // Another scheme, or a link to another host.
  if (scheme || value.startsWith("//") || value.startsWith("\\\\")) return NONE;
  // No path: the document itself.
  if (/^(\?|$)/.test(value)) return NONE;
  if (!documentPath) return REFUSED;
  const path = resolveDocumentLink(documentPath, value);
  return path ? { kind: "file", path } : REFUSED;
}
