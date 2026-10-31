/**
 * The main windows' downloads (`splitty download`, `terminal-sessions`, Saving
 * An Offered File) by their `blob:` URL, with where each was saved.
 *
 * The page names a download only by that URL, which it made for exactly that
 * download. The shell shows only a file it saved itself and finished, and never
 * tells the page where (`access-control`, Showing A Saved Download).
 */

/** Downloads remembered at once; an older one can no longer be shown. */
const KEEP = 32;

const saved = new Map<string, string>();

/** The download of `url` finished, saved to `path`. */
export function rememberDownload(url: string, path: string): void {
  saved.delete(url);
  saved.set(url, path);
  if (saved.size > KEEP) saved.delete(saved.keys().next().value!);
}

/** Where the finished download of `url` is. */
export function savedDownload(url: unknown): string | undefined {
  return typeof url === "string" ? saved.get(url) : undefined;
}
