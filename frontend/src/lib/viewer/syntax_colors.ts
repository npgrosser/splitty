/**
 * The colors of syntax highlighting in viewer panes (`syntax.ts`): custom
 * properties of the pane taken from the terminal palette, so a document reads
 * like the terminal next to it.
 *
 * Kept apart from `syntax.ts`, which loads the editor's parser: the pane sets
 * these before its view has loaded (`panes/viewer_pane.ts`).
 */

/** The custom properties of the `sv-*` classes, from a terminal palette. */
export function syntaxColors(palette: Readonly<Record<string, string>>): Record<string, string> {
  const pick = (...keys: string[]) => keys.map((key) => palette[key]).find(Boolean) ?? "inherit";
  return {
    "--sv-keyword": pick("magenta", "bright-magenta"),
    "--sv-string": pick("green", "bright-green"),
    "--sv-number": pick("yellow", "bright-yellow"),
    "--sv-comment": pick("bright-black", "white"),
    "--sv-function": pick("blue", "bright-blue"),
    "--sv-type": pick("yellow", "bright-yellow"),
    "--sv-property": pick("cyan", "bright-cyan"),
    "--sv-operator": pick("cyan", "bright-cyan"),
    "--sv-deleted": pick("red", "bright-red"),
  };
}
