import { describe, expect, it } from "vitest";
import { ReplayOutput } from "./replay_output.ts";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

function replay(text: string, split: number): string {
  const bytes = encoder.encode(text);
  const filter = new ReplayOutput();
  const first = filter.push(bytes.slice(0, split));
  const second = filter.push(bytes.slice(split));
  return decoder.decode(Uint8Array.from([...first, ...second, ...filter.finish()]));
}

describe("historical terminal output", () => {
  it.each([
    "\x1b]11;?\x07", "\x1b]11;?\x1b\\", "\x1b]10;?\x07",
    "\x1b]52;c;Y29waWVk\x07", "\x1b]52;c;Y29waWVk\x1b\\", "\x1b]52;c;?\x07",
    "\x1b]4;0;?\x07", "\x1b[6n", "\x1b[?6n", "\x1b[c",
    "\x1b[>0c", "\x1b[?2026$p", "\x1b[18t", "\x1bP$qm\x1b\\",
  ])("removes %j even across frame boundaries", (query) => {
    const text = `Grüße${query}❯ `;
    for (let split = 0; split <= encoder.encode(text).length; split++) {
      expect(replay(text, split)).toBe("Grüße❯ ");
    }
  });

  it("preserves colors, cursor positioning, titles, alternate screen and hyperlinks", () => {
    const text = "\x1b[31mred\x1b[0m\x1b[2;3H\x1b]11;rgb:0f/0f/0f\x07"
      + "\x1b]2;title?\x07\x1b[?1049h\x1b]8;;https://example.com/?q=x\x1b\\link";
    for (let split = 0; split <= text.length; split++) expect(replay(text, split)).toBe(text);
  });

  it("preserves the terminal modes a server restores ahead of the buffered output", () => {
    const text = "\x1b[>1u\x1b[?1049h\x1b[=5;1u\x1b[>3u\x1b[?1h\x1b=\x1b[?7l\x1b[?25l\x1b[?2004h"
      + "\x1b[?1004h\x1b[?1007h\x1b[?2031h\x1b[?2048h\x1b[2 q\x1b[?1003h\x1b[?1006h\x1b[>4;2m$ ";
    for (let split = 0; split <= text.length; split++) expect(replay(text, split)).toBe(text);
  });

  it.each([
    ["an unfinished CSI", "\x1b[38;5"],
    ["an unfinished OSC", "\x1b]0;tit"],
    ["an OSC cancelled with CAN", "\x1b]52;c;Y29w\x18"],
    ["a DCS cancelled with SUB", "\x1bP+q544e\x1a"],
  ])("judges a query after %s on its own", (_, before) => {
    for (const query of ["\x1b[c", "\x1b]11;?\x07", "\x1b[6n"]) {
      const text = `${before}${query}❯ `;
      for (let split = 0; split <= encoder.encode(text).length; split++) {
        expect(replay(text, split)).toBe(`${before}❯ `);
      }
    }
  });

  it("keeps the output after a cancelled clipboard write", () => {
    const text = "\x1b]52;c;Y29w\x18\x1b[1mbold\x1b[0m\x1b]0;title\x1b\\";
    for (let split = 0; split <= text.length; split++) expect(replay(text, split)).toBe(text);
  });

  it.each(["\x1b]7501;?\x07", "\x1b]7501;?\x1b\\"])(
    "removes the status probe %j of an older server, also across frames",
    (probe) => {
      const text = `a${probe}b`;
      for (let split = 0; split <= text.length; split++) expect(replay(text, split)).toBe("ab");
    },
  );

  it("keeps status reports", () => {
    for (const report of ["\x1b]7501;state=done\x07", "\x1b]7501;state=working:app=codex\x1b\\"]) {
      const text = `a${report}b`;
      for (let split = 0; split <= text.length; split++) expect(replay(text, split)).toBe(text);
    }
  });

  it("hands on a sequence the replay ends in, which live output ends", () => {
    expect(replay("prompt\x1b[38;2;153", 8)).toBe("prompt\x1b[38;2;153");
    expect(replay("prompt\x1b]11;", 8)).toBe("prompt\x1b]11;");
  });
});
