/** Historical terminal queries must not produce fresh input at a shell prompt.
 * Keep rendering/state sequences, but remove requests for terminal reports.
 * Operates on bytes so UTF-8 and queries split across replay frames survive.
 */
export class ReplayOutput {
  private pending: number[] = [];

  push(data: Uint8Array): Uint8Array {
    const output: number[] = [];
    for (const byte of data) {
      if (!this.pending.length) {
        if (byte === 0x1b) this.pending.push(byte);
        else if (byte !== 0x05) output.push(byte); // ENQ
        continue;
      }
      this.pending.push(byte);
      const kind = this.pending[1];
      const length = this.pending.length;
      if (length === 2 && [0x5b, 0x5d, 0x50, 0x5f, 0x5e].includes(kind)) continue;
      const string = [0x5d, 0x50, 0x5f, 0x5e].includes(kind);
      const complete = string
        ? (kind === 0x5d && byte === 7) || (length > 2 && this.pending[length - 2] === 0x1b && byte === 0x5c)
        : kind === 0x5b ? length > 2 && byte >= 0x40 && byte <= 0x7e : true;
      if (!complete) continue;
      if (!this.hasSideEffect()) {
        for (const value of this.pending) output.push(value);
      }
      this.pending = [];
    }
    return Uint8Array.from(output);
  }

  finish(): Uint8Array {
    // Preserve a partial state sequence, but cancel it before live output.
    // Otherwise a trailing ESC/OSC from scrollback could solicit a response.
    const tail = this.pending.length ? Uint8Array.from([...this.pending, 0x18]) : new Uint8Array();
    this.pending = [];
    return tail;
  }

  private hasSideEffect(): boolean {
    const sequence = String.fromCharCode(...this.pending.slice(0, 256));
    if (this.pending[1] === 0x5d) {
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
        || /^\x1b\[(?:1[134589]|2[01])t$/.test(sequence);
    }
    return /^\x1bP[+$]q/.test(sequence) || sequence === '\x1bZ';
  }
}
