import { describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";
import { OrderedOutput, type OutputTerminal } from "./ordered_output.ts";

const decoder = new TextDecoder();
const encoder = new TextEncoder();

/**
 * A terminal that parses what it was given only when told to, as xterm.js does
 * later, and an output whose animation frames run only when told to, as in a
 * hidden window.
 */
function setup() {
  const size = { cols: 80, rows: 24 };
  const events: string[] = [];
  /** Every write, as the terminal received it. */
  const raw: number[][] = [];
  const pending: { data: Uint8Array; parsed: () => void }[] = [];
  const terminal: OutputTerminal = {
    get cols() { return size.cols; },
    get rows() { return size.rows; },
    write: (data, parsed) => {
      if (data.length) raw.push([...data]);
      pending.push({ data, parsed });
    },
    resize: (cols, rows) => {
      size.cols = cols;
      size.rows = rows;
      events.push(`resize ${cols}x${rows}`);
    },
  };
  const frames: (() => void)[] = [];
  const output = new OrderedOutput(terminal, (cb) => frames.push(cb));
  return {
    output,
    events,
    raw,
    /** The bytes the terminal received, as text with ESC shown as `^`. */
    received: () => raw.map((bytes) => decoder.decode(Uint8Array.from(bytes)).replace(/\x1b/g, "^")),
    write: (text: string) => output.write(encoder.encode(text)),
    writeBytes: (bytes: number[]) => output.write(Uint8Array.from(bytes)),
    frame: () => { for (const cb of frames.splice(0)) cb(); },
    /** Parse everything given to the terminal so far, in order. */
    parse: () => {
      for (const { data, parsed } of pending.splice(0)) {
        const text = decoder.decode(data).replace(/\x1b\[\?2026[hl]/g, "");
        if (text) events.push(`${text} at ${size.cols}x${size.rows}`);
        parsed();
      }
    },
  };
}

describe("OrderedOutput", () => {
  it("parses output that arrived before a resize at the old size", () => {
    const { output, events, write, frame, parse } = setup();
    write("old");
    output.resize(100, 30);
    expect([output.cols, output.rows]).toEqual([100, 30]);
    expect(events).toEqual([]);

    write("new");
    frame();
    parse();
    expect(events).toEqual(["old at 80x24", "resize 100x30", "new at 100x30"]);
  });

  it("resizes at once when no output waits", () => {
    const { output, events, write, frame, parse } = setup();
    output.resize(100, 30);
    expect(events).toEqual(["resize 100x30"]);

    write("text");
    frame();
    parse();
    output.resize(90, 20);
    expect(events).toEqual(["resize 100x30", "text at 100x30", "resize 90x20"]);
  });

  it("keeps each resize between the output before and after it", () => {
    const { output, events, write, frame, parse } = setup();
    write("a");
    output.resize(100, 30);
    write("b");
    output.resize(90, 20);
    write("c");
    expect([output.cols, output.rows]).toEqual([90, 20]);
    frame();
    parse();
    expect(events).toEqual(["a at 80x24", "resize 100x30", "b at 100x30", "resize 90x20", "c at 90x20"]);
  });

  it("drops the resizes still waiting once disposed", () => {
    const { output, events, write, parse } = setup();
    write("a");
    output.resize(100, 30);
    output.dispose();
    parse();
    expect(events).toEqual(["a at 80x24"]);
  });

  it("wraps a batch in a synchronized update", () => {
    const { received, write, frame } = setup();
    write("text\r\n");
    frame();
    expect(received()).toEqual(["^[?2026htext\r\n^[?2026l"]);
  });

  it("never puts the synchronized update inside a split sequence", () => {
    const { received, write, frame } = setup();
    write("a\x1b[38;2;153");
    frame();
    write(";153;153mb\r\n");
    frame();
    write("c\r\n");
    frame();
    expect(received()).toEqual(["a^[38;2;153", ";153;153mb\r\n", "^[?2026hc\r\n^[?2026l"]);
  });

  it("writes a character split between frames whole, without a synchronized update around it", () => {
    const { raw, writeBytes, frame } = setup();
    writeBytes([0x0a, 0xe2, 0x80]);
    frame();
    writeBytes([0xa6, 0x0a]);
    frame();
    expect(raw).toEqual([[0x0a], [0xe2, 0x80, 0xa6, 0x0a]]);
  });

  it("splits a large batch into writes xterm.js decodes whole, between characters", () => {
    const { raw, write, frame } = setup();
    const text = "x".repeat(131063) + "…".repeat(1000) + "\r\n";
    write(text);
    frame();
    expect(raw.length).toBeGreaterThan(1);
    for (const bytes of raw) {
      expect(bytes.length).toBeLessThanOrEqual(128 * 1024);
      expect(bytes[0] >= 0x80 && bytes[0] <= 0xbf).toBe(false);
    }
    expect(decoder.decode(Uint8Array.from(raw.flat()))).toBe(`\x1b[?2026h${text}\x1b[?2026l`);
  });

  it("inserts the terminal's own bytes only between sequences", () => {
    const { received, write, frame, output } = setup();
    write("a\x1b[38;2;153");
    frame();
    output.insert(encoder.encode("\x1b]112\x1b\\"));
    frame();
    expect(received()).toEqual(["a^[38;2;153"]);
    write(";153;153mb\r\n");
    frame();
    write("c\r\n");
    frame();
    expect(received()).toEqual(["a^[38;2;153", ";153;153mb\r\n", "^[?2026h^]112^\\c\r\n^[?2026l"]);
  });

  it("inserts them on their own when no output follows", () => {
    const { received, write, frame, output } = setup();
    write("a\r\n");
    frame();
    output.insert(encoder.encode("\x1b]112\x1b\\"));
    frame();
    expect(received()).toEqual(["^[?2026ha\r\n^[?2026l", "^]112^\\"]);
  });

  it("never puts it right after a printed character, which a repeat would repeat", () => {
    const { received, write, frame } = setup();
    write("-");
    frame();
    write("\x1b[79b\r\n");
    frame();
    expect(received()).toEqual(["-", "^[79b\r\n"]);
  });

  it("takes UTF-8 encoded C1 controls as the sequences they start", () => {
    const { received, write, frame } = setup();
    write("\u009b38;2;153");
    frame();
    write(";153;153m\r\n");
    frame();
    expect(received()).toEqual(["\u009b38;2;153", ";153;153m\r\n"]);
  });

  it("leaves a synchronized update of the program's own open across frames", () => {
    const { received, write, frame } = setup();
    write("\x1b[?2026h\x1b[2J\r\n");
    frame();
    write("frame\r\n\x1b[?2026l");
    frame();
    write("after\r\n");
    frame();
    expect(received()).toEqual([
      "^[?2026h^[2J\r\n",
      "^[?2026hframe\r\n^[?2026l^[?2026l",
      "^[?2026hafter\r\n^[?2026l",
    ]);
  });

  it("knows where strings end: BEL or ST for OSC, ST for DCS", () => {
    const { received, write, frame } = setup();
    write("\x1b]0;title");
    frame();
    write("\x07\x1b]8;;x");
    frame();
    write("\x1b\\\x1bPq#0");
    frame();
    write("\x07");
    frame();
    write("\x1b\\");
    frame();
    expect(received()).toEqual(["^]0;title", "\x07^]8;;x", "^\\^Pq#0", "\x07", "^\\"]);
    write("\r\n");
    frame();
    expect(received().at(-1)).toEqual("^[?2026h\r\n^[?2026l");
  });
});

describe("OrderedOutput feeding xterm.js", () => {
  /** An xterm.js terminal fed through an output whose frames run when told to. */
  function xterm() {
    const terminal = new Terminal({ cols: 80, rows: 5, allowProposedApi: true });
    const frames: (() => void)[] = [];
    const output = new OrderedOutput({
      write: (data, parsed) => terminal.write(data, parsed),
      resize: (cols, rows) => terminal.resize(cols, rows),
      get cols() { return terminal.cols; },
      get rows() { return terminal.rows; },
    }, (cb) => frames.push(cb));
    return {
      terminal,
      /** Write each part in a frame of its own. */
      frames: async (...parts: (string | number[])[]) => {
        for (const part of parts) {
          output.write(typeof part === "string" ? encoder.encode(part) : Uint8Array.from(part));
          for (const cb of frames.splice(0)) cb();
        }
        await new Promise<void>((done) => terminal.write("", done));
      },
      line: (y: number) => terminal.buffer.active.getLine(y)?.translateToString(true) ?? "",
    };
  }

  it("shows a colour split between frames as a colour", async () => {
    const { terminal, frames, line } = xterm();
    try {
      await frames("\x1b[38;2;153", ";153;153mgrey\x1b[0m\r\n");
      expect(line(0)).toBe("grey");
    } finally {
      terminal.dispose();
    }
  });

  it("shows a character split between frames", async () => {
    const { terminal, frames, line } = xterm();
    try {
      // xterm.js 6.0 drops a character whose write ends after E2 80.
      await frames([0x61, 0xe2, 0x80], [0xa6, 0x62, 0x0d, 0x0a]);
      expect(line(0)).toBe("a…b");
    } finally {
      terminal.dispose();
    }
  });

  it("shows every character of a batch larger than xterm.js decodes at once", async () => {
    const { terminal, frames } = xterm();
    try {
      // 131072 bytes end after E2 80 of the 43691st `…`.
      await frames("…".repeat(50000));
      let shown = "";
      for (let y = 0; y < terminal.buffer.active.length; y++) {
        shown += terminal.buffer.active.getLine(y)?.translateToString(true) ?? "";
      }
      expect(shown.length).toBe(50000);
    } finally {
      terminal.dispose();
    }
  });

  it("repeats a character whose repeat comes in the next frame", async () => {
    const { terminal, frames, line } = xterm();
    try {
      await frames("-", "\x1b[9b\r\n");
      expect(line(0)).toBe("----------");
    } finally {
      terminal.dispose();
    }
  });
});
