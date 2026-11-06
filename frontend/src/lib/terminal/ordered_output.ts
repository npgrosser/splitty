/**
 * A terminal's output and grid size changes, applied in the order they arrive.
 *
 * Output is batched per animation frame and wrapped in a synchronized update
 * (DEC 2026). A size change first hands the batch to the terminal and takes
 * effect only once the terminal has parsed all output before it, so output a
 * process wrote for the old size is never parsed at the new one. Without that,
 * a resize overtook the batch and xterm.js's asynchronous parser; in a hidden
 * window, where animation frames stop, all output since the window was hidden
 * was parsed after any resize in between.
 *
 * A batch is wrapped only where inserting bytes changes nothing (see
 * {@link StreamPosition}), and so are the terminal's own bytes inserted; it
 * is not wrapped while a synchronized update of the program's own is open
 * (Claude Code and Codex send their own), which the batch's ESC[?2026l ended
 * halfway: a half-drawn frame showed. Output
 * frames split the stream anywhere, and the ESC of an inserted ESC[?2026l
 * aborts a sequence it lands in: the rest of a colour such as
 * ESC[38;2;153;153;153m showed as the text `;153;153;153m`.
 *
 * Each write the terminal gets ends between characters and holds at most
 * {@link MAX_PARSE} bytes. xterm.js 6.0 decodes every write (and every 128 KiB
 * of a larger one) on its own and drops a character split after its bytes
 * E2 80, such as `…`, `—` or `’`; the rest of the line moved one column left.
 */

/** ESC[?2026h and ESC[?2026l, around each batch. */
const BSU = new Uint8Array([0x1b, 0x5b, 0x3f, 0x32, 0x30, 0x32, 0x36, 0x68]);
const ESU = new Uint8Array([0x1b, 0x5b, 0x3f, 0x32, 0x30, 0x32, 0x36, 0x6c]);
/** A batch this large goes to the terminal without waiting for the frame. */
const MAX_WRITE_BUF = 4 * 1024 * 1024;
/** The most xterm.js decodes in one piece (its MAX_PARSEBUFFER_LENGTH). */
const MAX_PARSE = 128 * 1024;

/** The terminal an {@link OrderedOutput} feeds. */
export interface OutputTerminal {
  /** Parse `data`, calling `parsed` once it is (also right away, for a synchronous parser). */
  write(data: Uint8Array, parsed: () => void): void;
  /** Resize the grid now. */
  resize(cols: number, rows: number): void;
  readonly cols: number;
  readonly rows: number;
}

const EMPTY = new Uint8Array(0);

const ESC = 0x1b;
/** CAN and SUB end any sequence. */
const CAN = 0x18;
const SUB = 0x1a;
const BEL = 0x07;
const DEL = 0x7f;

const enum At {
  Ground,
  /** Inside a UTF-8 character. */
  Utf8,
  Escape,
  EscapeIntermediate,
  Csi,
  /** OSC, which BEL also ends. */
  Osc,
  /** DCS, SOS, PM and APC, which only ST ends. */
  String,
}

/**
 * Where the byte stream stands, following the transitions of xterm.js's
 * parser closely enough to tell where bytes can be inserted without changing
 * what the stream does: between sequences and characters, and not right after
 * a printed character, which a REP (ESC[Nb) repeats until a control or a
 * sequence comes in between. Where it cannot tell, it says no.
 */
class StreamPosition {
  private at = At.Ground;
  /** The last thing in ground was a printed character. */
  private printed = false;
  private lead = 0;
  private continuation = 0;
  /** The parameters and intermediates of the CSI sequence so far, cut short. */
  private csi = "";
  /** The program's own synchronized update (DEC 2026) is open. */
  synchronized = false;

  /** Bytes can be inserted here. */
  get insertable(): boolean {
    return this.at === At.Ground && !this.printed;
  }

  scan(data: Uint8Array): void {
    for (const byte of data) this.step(byte);
  }

  private step(byte: number): void {
    if (this.at === At.Utf8) {
      if (byte >= 0x80 && byte <= 0xbf) {
        if (--this.continuation === 0) this.character(byte);
        return;
      }
      // An incomplete character; the byte starts afresh.
      this.at = At.Ground;
    }
    if (byte === ESC) {
      this.at = At.Escape;
      return;
    }
    if (byte === CAN || byte === SUB) {
      this.ended();
      return;
    }
    switch (this.at) {
      case At.Ground:
        if (byte >= 0xc2 && byte <= 0xf4) this.startCharacter(byte);
        else if (byte < 0x20) this.printed = false; // a control executes
        else if (byte < DEL) this.printed = true;
        return;
      case At.Escape:
        if (byte === 0x5b) this.startCsi(); // [
        else if (byte === 0x5d) this.at = At.Osc; // ]
        else if (byte === 0x50 || byte === 0x58 || byte === 0x5e || byte === 0x5f) this.at = At.String; // P X ^ _
        else if (byte >= 0x20 && byte <= 0x2f) this.at = At.EscapeIntermediate;
        else if (byte >= 0x30 && byte < DEL) this.ended();
        // Controls execute inside the sequence.
        return;
      case At.EscapeIntermediate:
        if (byte >= 0x30 && byte < DEL) this.ended();
        return;
      case At.Csi:
        if (byte >= 0x40 && byte <= 0x7e) {
          this.csiEnded(byte);
          this.ended();
        } else if (this.csi.length < 32) {
          this.csi += String.fromCharCode(byte);
        }
        return;
      case At.Osc:
        if (byte === BEL) this.ended();
        return;
      case At.String:
        return;
    }
  }

  private startCharacter(lead: number): void {
    this.lead = lead;
    this.continuation = lead >= 0xf0 ? 3 : lead >= 0xe0 ? 2 : 1;
    this.at = At.Utf8;
  }

  /** A UTF-8 character ended with `last`. xterm.js takes U+0080 to U+009F as C1 controls. */
  private character(last: number): void {
    this.at = At.Ground;
    if (this.lead !== 0xc2 || last > 0x9f) {
      this.printed = true;
      return;
    }
    if (last === 0x9b) this.startCsi();
    else if (last === 0x9d) this.at = At.Osc;
    else if (last === 0x90 || last === 0x98 || last === 0x9e || last === 0x9f) this.at = At.String;
    else this.printed = false;
  }

  private startCsi(): void {
    this.at = At.Csi;
    this.csi = "";
  }

  /** DECSET and DECRST of mode 2026 open and close a synchronized update. */
  private csiEnded(final: number): void {
    if ((final !== 0x68 && final !== 0x6c) || !this.csi.startsWith("?")) return;
    if (this.csi.slice(1).split(";").includes("2026")) this.synchronized = final === 0x68;
  }

  /** A control or a sequence ended: back in ground, after no printed character. */
  private ended(): void {
    this.at = At.Ground;
    this.printed = false;
  }
}

/**
 * How many bytes at the end of `data` start a UTF-8 character that has not
 * ended yet.
 */
function unfinishedCharacter(data: Uint8Array): number {
  for (let back = 1; back <= Math.min(3, data.length); back++) {
    const byte = data[data.length - back];
    if (byte < 0x80) return 0;
    if (byte >= 0xc0) {
      const length = byte >= 0xf0 ? 4 : byte >= 0xe0 ? 3 : 2;
      return back < length ? back : 0;
    }
  }
  return 0;
}

const isContinuation = (byte: number) => byte >= 0x80 && byte <= 0xbf;

export class OrderedOutput {
  private chunks: Uint8Array[] = [];
  /** The start of a character the last write held back, for the next. */
  private carry: Uint8Array = EMPTY;
  /** The terminal's own bytes, waiting for a point between sequences. */
  private own: Uint8Array[] = [];
  private bytes = 0;
  private scheduled = false;
  /** Writes handed to the terminal that it has not parsed yet. */
  private parsing = 0;
  /** Size changes waiting behind output, oldest first. */
  private queued: { cols: number; rows: number }[] = [];
  private readonly position = new StreamPosition();
  private disposed = false;

  constructor(
    private readonly terminal: OutputTerminal,
    private readonly nextFrame: (cb: () => void) => void = (cb) => { requestAnimationFrame(cb); },
  ) {}

  /** The grid's columns, counting a size change that still waits behind output. */
  get cols(): number {
    return this.queued.at(-1)?.cols ?? this.terminal.cols;
  }

  /** The grid's rows, counting a size change that still waits behind output. */
  get rows(): number {
    return this.queued.at(-1)?.rows ?? this.terminal.rows;
  }

  write(data: Uint8Array): void {
    if (this.disposed) return;
    this.chunks.push(data);
    this.bytes += data.length;
    this.schedule();
    if (this.bytes > MAX_WRITE_BUF) this.flush();
  }

  /**
   * Complete sequences of the terminal's own, such as a reset, given to the
   * terminal at the next point between the output's sequences and characters,
   * never inside one.
   */
  insert(data: Uint8Array): void {
    if (this.disposed) return;
    this.own.push(data);
    this.schedule();
  }

  private schedule(): void {
    if (this.scheduled) return;
    this.scheduled = true;
    this.nextFrame(() => this.flush());
  }

  /** Hand the batched output to the terminal. */
  flush(): void {
    this.scheduled = false;
    if (this.disposed) return;
    const startsInsertable = this.position.insertable;
    const own = startsInsertable ? this.own : [];
    if (this.chunks.length === 0 && own.length === 0) return;
    const chunks = this.chunks;
    this.chunks = [];
    this.bytes = 0;
    if (startsInsertable) this.own = [];

    for (const c of chunks) this.position.scan(c);
    const wrap = chunks.length > 0 && startsInsertable && this.position.insertable
      && !this.position.synchronized;
    // A held-back character start only comes before output that is not insertable.
    const parts = [this.carry, ...(wrap ? [BSU] : []), ...own, ...chunks, ...(wrap ? [ESU] : [])];

    let total = 0;
    for (const part of parts) total += part.length;
    const merged = new Uint8Array(total);
    let offset = 0;
    for (const part of parts) {
      merged.set(part, offset);
      offset += part.length;
    }

    const end = total - unfinishedCharacter(merged);
    this.carry = merged.slice(end);
    for (let start = 0; start < end;) {
      let stop = Math.min(end, start + MAX_PARSE);
      while (stop < end && stop > start && isContinuation(merged[stop])) stop--;
      if (stop === start) stop = Math.min(end, start + MAX_PARSE);
      this.parsing++;
      this.terminal.write(merged.subarray(start, stop), () => { this.parsing--; });
      start = stop;
    }
  }

  /** Resize the grid after the output that arrived before. */
  resize(cols: number, rows: number): void {
    if (this.disposed || (cols === this.cols && rows === this.rows)) return;
    this.flush();
    if (this.parsing === 0 && this.queued.length === 0) {
      this.terminal.resize(cols, rows);
      return;
    }
    // The terminal calls back in the order it was written to, so each size
    // change takes effect between the output before and after it.
    this.queued.push({ cols, rows });
    this.parsing++;
    this.terminal.write(EMPTY, () => {
      this.parsing--;
      const size = this.queued.shift();
      if (size && !this.disposed) this.terminal.resize(size.cols, size.rows);
    });
  }

  dispose(): void {
    this.disposed = true;
    this.chunks = [];
    this.carry = EMPTY;
    this.own = [];
    this.bytes = 0;
    this.queued = [];
  }
}
