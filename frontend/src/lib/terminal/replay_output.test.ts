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

  it("cancels an unfinished historical escape before live data", () => {
    expect(replay("prompt\x1b]11;", 8)).toBe("prompt\x1b]11;\x18");
  });
});
