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
 */

/** ESC[?2026h and ESC[?2026l, around each batch. */
const BSU = new Uint8Array([0x1b, 0x5b, 0x3f, 0x32, 0x30, 0x32, 0x36, 0x68]);
const ESU = new Uint8Array([0x1b, 0x5b, 0x3f, 0x32, 0x30, 0x32, 0x36, 0x6c]);
/** A batch this large goes to the terminal without waiting for the frame. */
const MAX_WRITE_BUF = 4 * 1024 * 1024;

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

export class OrderedOutput {
  private chunks: Uint8Array[] = [];
  private bytes = 0;
  private scheduled = false;
  /** Writes handed to the terminal that it has not parsed yet. */
  private parsing = 0;
  /** Size changes waiting behind output, oldest first. */
  private queued: { cols: number; rows: number }[] = [];
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
    if (!this.scheduled) {
      this.scheduled = true;
      this.nextFrame(() => this.flush());
    }
    if (this.bytes > MAX_WRITE_BUF) this.flush();
  }

  /** Hand the batched output to the terminal. */
  flush(): void {
    if (this.chunks.length === 0 || this.disposed) return;
    const chunks = this.chunks;
    this.chunks = [];
    this.bytes = 0;
    this.scheduled = false;

    let total = BSU.length + ESU.length;
    for (const c of chunks) total += c.length;
    const merged = new Uint8Array(total);
    merged.set(BSU, 0);
    let offset = BSU.length;
    for (const c of chunks) {
      merged.set(c, offset);
      offset += c.length;
    }
    merged.set(ESU, offset);

    this.parsing++;
    this.terminal.write(merged, () => { this.parsing--; });
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
    this.bytes = 0;
    this.queued = [];
  }
}
