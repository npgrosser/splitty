import type { IClipboardProvider, ClipboardSelectionType } from "@xterm/addon-clipboard";
import { toast } from "svelte-sonner";
import { writeClipboardText } from "./terminal_shared.ts";
import { getSettings } from "../settings.ts";

/**
 * Clipboard policy for OSC 52 escape sequences.
 *
 * OSC 52 lets a process running in the terminal reach the system clipboard by
 * printing an escape sequence. Splitty previously loaded xterm's stock
 * `ClipboardAddon`, which wires that straight through to
 * `navigator.clipboard` in both directions (security review
 * SR-20).
 *
 * Terminal output is not trusted input: it is produced by build tools, remote
 * hosts, package scripts and by `cat`-ing a file someone else wrote. Handing
 * that stream unmediated clipboard access is the problem this module fixes.
 *
 * # Reads are refused outright
 *
 * An OSC 52 query (`\e]52;c;?\a`) makes the stock provider call
 * `navigator.clipboard.readText()` and feed the result *back into the terminal's
 * input stream* — base64-encoded, as if the user had typed it. A process could
 * therefore harvest whatever the user last copied: a password out of a manager,
 * an API token, a customer record. There is no setting for this. The value of
 * being able to read the clipboard from a shell script does not come close to
 * the cost of getting it wrong, and a policy with an off switch invites someone
 * to flip it.
 *
 * # Writes are enabled by default
 *
 * Copy in terminal applications should work without setup. Users can disable
 * writes with terminal.allowOsc52Write. Writes are capped and acknowledged in
 * the UI; clipboard reads remain forbidden. Historical OSC 52 sequences are
 * removed by replay_output.ts before either renderer sees them.
 */

/**
 * Largest payload accepted for a clipboard write.
 *
 * Bounds the memory a single escape sequence can cause the renderer to hold. 64
 * KiB is far beyond any plausible copied command while still being a hard limit.
 */
const MAX_WRITE_BYTES = 64 * 1024;

export class SplittyClipboardProvider implements IClipboardProvider {
  /**
   * Always denies.
   *
   * Returning an empty string rather than throwing keeps the terminal parser on
   * its normal path: the addon replies with an empty payload, which reads to the
   * requesting process as "the clipboard is empty" — indistinguishable from the
   * legitimate case, and so it leaks nothing about the policy either.
   */
  readText(_selection: ClipboardSelectionType): string {
    return "";
  }

  writeText(_selection: ClipboardSelectionType, text: string): void {
    if (!getSettings().terminal.allowOsc52Write) return;
    if (!text || new TextEncoder().encode(text).byteLength > MAX_WRITE_BYTES) return;

    void writeClipboardText(text).then((copied) => {
      if (copied) toast("Copied to clipboard", { id: "terminal-clipboard", duration: 2000 });
      else toast.error("Could not copy to clipboard", { id: "terminal-clipboard" });
    }).catch(() => {
      toast.error("Could not copy to clipboard", { id: "terminal-clipboard" });
    });
  }
}
