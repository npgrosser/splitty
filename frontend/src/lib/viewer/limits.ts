/**
 * The limits of viewer panes (`viewer-panes`), as the server applies them.
 *
 * The window checks the same bounds before it sends `open_viewer`, so an op it
 * builds is never refused for its size, and it checks what a server sends
 * against them, since a host is not trusted to keep them.
 */

/** The capability of servers with viewer panes, in `mux.welcome`. */
export const VIEWER_CAPABILITY = "viewer-v1";

/** The largest file a viewer pane reads. */
export const MAX_FILE_BYTES = 8 * 1024 * 1024;
/** The largest text document, in bytes of UTF-8. */
export const MAX_TEXT_BYTES = 262_144;
/** The most a `viewer.chunk` carries, decoded. */
export const CHUNK_BYTES = 1024 * 1024;
/** The longest file path, in bytes of UTF-8. */
export const MAX_PATH_BYTES = 4096;
/** The longest text title, in characters. */
export const MAX_TITLE_CHARS = 200;
/** Markdown and HTML documents up to this size are rendered; larger ones open as source. */
export const RENDER_LIMIT_BYTES = 2 * 1024 * 1024;

const encoder = new TextEncoder();

export function utf8Length(text: string): number {
  return encoder.encode(text).length;
}

// C0 and C1 controls; the server refuses them in paths and titles.
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;
// The invisible formatting characters the server refuses in paths: zero-width
// and directional marks, bidirectional embeddings and overrides, word joiners
// and invisible operators, and the byte order mark.
const INVISIBLE = /[\u200b-\u200f\u202a-\u202e\u2060-\u206f\ufeff]/;
// A lone surrogate has no UTF-8 form; the server could not even parse the op.
const LONE_SURROGATE = /\p{Cs}/u;

/**
 * A path the server takes for a `file` source, by the rule of editor requests:
 * absolute, at most 4,096 bytes of UTF-8, without a leading `//`, a
 * backslash, a `.` or `..` segment, control characters or invisible
 * formatting characters.
 */
export function isViewerPath(path: string): boolean {
  return path.startsWith("/")
    && !path.startsWith("//")
    && !path.includes("\\")
    && !CONTROL.test(path)
    && !INVISIBLE.test(path)
    && !LONE_SURROGATE.test(path)
    && utf8Length(path) <= MAX_PATH_BYTES
    && !path.split("/").some((segment) => segment === "." || segment === "..");
}

/** A working directory the server takes for a viewer pane: an absolute path. */
export function isViewerCwd(cwd: string): boolean {
  return cwd.startsWith("/") && !LONE_SURROGATE.test(cwd);
}

/** A title the server takes for a `text` source: 1 to 200 characters, no controls. */
export function isViewerTitle(title: string): boolean {
  const chars = [...title].length;
  return chars >= 1 && chars <= MAX_TITLE_CHARS && !CONTROL.test(title) && !LONE_SURROGATE.test(title);
}

/** `text` with every lone surrogate replaced, so that it has a UTF-8 form. */
export function wellFormed(text: string): string {
  return text.replace(/\p{Cs}/gu, "\ufffd");
}

/** `title` without controls, cut to the title limit at a character boundary. */
export function viewerTitle(title: string): string {
  const clean = wellFormed(title).replace(new RegExp(CONTROL.source, "g"), " ").trim();
  const chars = [...clean];
  return chars.length > MAX_TITLE_CHARS ? chars.slice(0, MAX_TITLE_CHARS).join("").trimEnd() : clean;
}
