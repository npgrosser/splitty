import { describe, expect, it } from "vitest";
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
  const pending: { data: Uint8Array; parsed: () => void }[] = [];
  const terminal: OutputTerminal = {
    get cols() { return size.cols; },
    get rows() { return size.rows; },
    write: (data, parsed) => { pending.push({ data, parsed }); },
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
    write: (text: string) => output.write(encoder.encode(text)),
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
});
