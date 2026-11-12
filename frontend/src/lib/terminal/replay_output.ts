/** Historical terminal queries must not produce fresh input at a shell prompt.
 * Keep rendering/state sequences, but remove requests for terminal reports.
 * Operates on bytes so UTF-8 and queries split across replay frames survive.
 * A sequence ends where xterm.js ends it, also at CAN, SUB and an ESC that
 * does not end a string: one that never ended asks for nothing, and what
 * follows it is judged on its own.
 */
const ESC = 0x1b;
const CAN = 0x18;
const SUB = 0x1a;
const STRINGS = [0x5d, 0x50, 0x5f, 0x5e]; // OSC, DCS, APC, PM

export class ReplayOutput {
  private pending: number[] = [];

  push(data: Uint8Array): Uint8Array {
    const output: number[] = [];
    for (const byte of data) this.step(byte, output);
    return Uint8Array.from(output);
  }

  private step(byte: number, output: number[]): void {
    if (!this.pending.length) {
      if (byte === ESC) this.pending.push(byte);
      else if (byte !== 0x05) output.push(byte); // ENQ
      return;
    }
    const string = STRINGS.includes(this.pending[1]);
    if (byte === CAN || byte === SUB) {
      this.handOn(output);
      output.push(byte);
      return;
    }
    if (byte === ESC && !string) {
      this.handOn(output);
      this.pending.push(byte);
      return;
    }
    if (string && this.pending.length > 2 && this.pending.at(-1) === ESC && byte !== 0x5c) {
      // An ESC that is not ST ended the string and starts a sequence of its own.
      this.pending.pop();
      this.handOn(output);
      this.pending.push(ESC);
      this.step(byte, output);
      return;
    }
    this.pending.push(byte);
    const kind = this.pending[1];
    const length = this.pending.length;
    const inString = STRINGS.includes(kind);
    if (length === 2 && (kind === 0x5b || inString)) return;
    const complete = inString
      ? (kind === 0x5d && byte === 7) || (length > 2 && this.pending[length - 2] === ESC && byte === 0x5c)
      : kind === 0x5b ? length > 2 && byte >= 0x40 && byte <= 0x7e : true;
    if (!complete) return;
    if (!this.hasSideEffect()) this.handOn(output);
    this.pending = [];
  }

  /** Hand the pending bytes on unchanged. */
  private handOn(output: number[]): void {
    for (const value of this.pending) output.push(value);
    this.pending = [];
  }

  /**
   * The end of the replay: the bytes of a sequence that has not ended yet,
   * unchanged. Live output follows the replay without a gap and ends that
   * sequence, so the sequence is live: cancelling it showed the rest of a
   * colour such as ESC[38;2;153;153;153m as the text `;153;153m`.
   */
  finish(): Uint8Array {
    const tail = Uint8Array.from(this.pending);
    this.pending = [];
    return tail;
  }

  private hasSideEffect(): boolean {
    const sequence = String.fromCharCode(...this.pending.slice(0, 256));
    if (this.pending[1] === 0x5d) {
      // The program status probe: a current server answers and removes it,
      // an older one forwarded it.
      if (sequence === "\x1b]7501;?\x07" || sequence === "\x1b]7501;?\x1b\\") return true;
      // Clipboard writes must not overwrite newer copies on resume.
      if (/^\x1b\]52;/.test(sequence)) return true;
      // OSC dynamic/palette color queries. Color setters,
      // titles, hyperlinks and shell integration must still be replayed.
      return /^\x1b\](?:4|10|11|12|13|14|15|16|17|18|19|52);/.test(sequence)
        && this.pending.includes(0x3f);
    }
    if (this.pending[1] === 0x5b) {
      // Device attributes, status/cursor reports, mode and window reports.
      return /^\x1b\[[?>=]?[\d;]*c$/.test(sequence)
        || /^\x1b\[\??[\d;]*n$/.test(sequence)
        || /^\x1b\[\??[\d;]*\$p$/.test(sequence)
        || /^\x1b\[(?:1[1345689]|2[01])t$/.test(sequence)
        // The kitty keyboard query and the version report (XTVERSION).
        || sequence === "\x1b[?u"
        || /^\x1b\[>[\d;]*q$/.test(sequence);
    }
    return /^\x1bP[+$]q/.test(sequence) || sequence === '\x1bZ';
  }
}
