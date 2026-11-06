/**
 * How a viewer pane shows its document (`viewer-panes`, Showing A Document),
 * by the file's name and its first bytes. Pure.
 *
 * - `.md`, `.markdown`, `.mdown` and `.html`, `.htm` up to 2 MiB: rendered,
 *   with their source on request; larger ones as source.
 * - PNG, JPEG, GIF, WebP and SVG, named so and recognized by their content:
 *   an image.
 * - Anything else without a NUL byte in its first 8 KiB: source, highlighted
 *   by the file's name and decoded as UTF-8 with replacement characters.
 * - Anything else: its size, none of its bytes.
 * - A `text` document: plain text.
 */

import type { ViewerSource } from "../protocol/types.ts";
import { toBase64 } from "../protocol/base64.ts";
import { RENDER_LIMIT_BYTES } from "./limits.ts";

/** The bytes in which a NUL marks a binary file. */
export const BINARY_PROBE_BYTES = 8 * 1024;

export type ImageType = "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml";

export type Presentation =
  /** Rendered Markdown or HTML, which can switch to its source. */
  | { readonly kind: "rendered"; readonly format: "markdown" | "html" }
  | { readonly kind: "image"; readonly type: ImageType }
  /** Text with line numbers; highlighted by `fileName`, plain without one. */
  | { readonly kind: "source"; readonly fileName?: string }
  | { readonly kind: "binary"; readonly size: number };

const MARKDOWN = new Set(["md", "markdown", "mdown"]);
const HTML = new Set(["html", "htm"]);
const IMAGES: Record<string, ImageType> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
};

/** The last segment of a path. */
export function fileName(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

/** The directory of an absolute path, `/` for a file at the root. */
export function directoryOf(path: string): string {
  const at = path.replace(/\/+$/, "").lastIndexOf("/");
  return at <= 0 ? "/" : path.slice(0, at);
}

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function presentationOf(source: ViewerSource, bytes: Uint8Array): Presentation {
  if (source.kind === "text") return { kind: "source" };
  const name = fileName(source.path);
  const extension = extensionOf(name);
  const fits = bytes.length <= RENDER_LIMIT_BYTES;
  if (MARKDOWN.has(extension)) return fits ? { kind: "rendered", format: "markdown" } : { kind: "source", fileName: name };
  if (HTML.has(extension)) return fits ? { kind: "rendered", format: "html" } : { kind: "source", fileName: name };
  const named = IMAGES[extension];
  if (named) {
    const type = imageType(bytes);
    if (type && (type === "image/svg+xml") === (named === "image/svg+xml")) return { kind: "image", type };
  }
  if (isBinary(bytes)) return { kind: "binary", size: bytes.length };
  return { kind: "source", fileName: name };
}

/** Whether the first 8 KiB hold a NUL byte. */
export function isBinary(bytes: Uint8Array): boolean {
  return bytes.subarray(0, BINARY_PROBE_BYTES).includes(0);
}

/** The image type the content shows: magic bytes, or SVG markup. */
export function imageType(bytes: Uint8Array): ImageType | undefined {
  const starts = (...values: number[]) => values.every((value, i) => bytes[i] === value);
  if (starts(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)) return "image/png";
  if (starts(0xff, 0xd8, 0xff)) return "image/jpeg";
  if (starts(0x47, 0x49, 0x46, 0x38) && (bytes[4] === 0x37 || bytes[4] === 0x39) && bytes[5] === 0x61) return "image/gif";
  if (starts(0x52, 0x49, 0x46, 0x46) && bytes[8] === 0x57 && bytes[9] === 0x45 && bytes[10] === 0x42 && bytes[11] === 0x50) return "image/webp";
  if (!isBinary(bytes) && /<svg[\s>]/i.test(decodeText(bytes.subarray(0, BINARY_PROBE_BYTES)))) return "image/svg+xml";
  return undefined;
}

/** UTF-8 with replacement characters; a byte order mark is dropped. */
export function decodeText(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

/** A `data:` URL of an image, the only kind of source an `<img>` of a viewer gets. */
export function imageDataUrl(type: ImageType, bytes: Uint8Array): string {
  return `data:${type};base64,${toBase64(bytes)}`;
}
