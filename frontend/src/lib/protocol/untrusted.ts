/**
 * Strings from a server are data, never markup or instructions.
 *
 * The UI renders them as text (Svelte interpolation, `textContent`), so HTML in
 * a name stays visible text. What is left to bound is the rest: a value of the
 * wrong type, control and bidirectional-override characters that reorder or
 * hide what is shown, and lengths that push the UI apart. Every connection's
 * names, titles and notifications pass through here when they arrive.
 */

/** Screen and pane names. */
export const MAX_NAME = 200;
/** Titles: terminal titles, process names, notification titles. */
export const MAX_TITLE = 500;
/** Longer text: notification bodies, command lines. */
export const MAX_TEXT = 2000;
/** Values that go back to the server they came from: commands, working directories. */
export const MAX_VALUE = 64 * 1024;

// C0 and C1 controls, and the bidirectional embeddings, overrides and isolates.
// eslint-disable-next-line no-control-regex
const HIDDEN = /[\u0000-\u001f\u007f-\u009f‪-‮⁦-⁩]/g;

/** `value` as plain text of at most `max` characters; anything but a string becomes "". */
export function plainText(value: unknown, max: number): string {
  if (typeof value !== "string") return "";
  const text = value.replace(HIDDEN, "");
  return text.length > max ? text.slice(0, max) : text;
}

/**
 * Like {@link plainText}, but line breaks stay: for error details that carry a
 * program's error output (`ssh`).
 */
export function plainLines(value: unknown, max: number): string {
  if (typeof value !== "string") return "";
  const text = value.split(/\r?\n/).map((line) => line.replace(HIDDEN, "")).join("\n");
  return text.length > max ? text.slice(0, max) : text;
}

/** Like {@link plainText}, but an absent or non-string value stays undefined. */
export function optionalText(value: unknown, max: number): string | undefined {
  return typeof value === "string" ? plainText(value, max) : undefined;
}

/**
 * A value that goes back to the server it came from (a command, a working
 * directory) keeps its characters; only its type and length are checked.
 */
export function optionalBounded(value: unknown, max: number): string | undefined {
  if (typeof value !== "string") return undefined;
  return value.length > max ? value.slice(0, max) : value;
}
