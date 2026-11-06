import { afterEach, expect, it, vi } from "vitest";

vi.mock("../theme.ts", () => ({ getXtermTheme: () => ({}) }));
vi.mock("../settings.ts", () => ({
  getSettings: () => ({ appearance: { zoom: 1, zoomScope: "terminals" }, terminal: { xterm: {} } }),
}));
vi.mock("../terminal_config.ts", () => ({ getConfig: () => ({ fontSize: 12, fontFamily: "monospace", cursorStyle: "block" }) }));
vi.mock("../window.ts", () => ({ windowAPI: {} }));
vi.mock("./terminal_shared.ts", () => ({ ensureFont: async () => {} }));

afterEach(() => {
  vi.unstubAllGlobals();
});

/** Where a program finds the cursor after writing `text` at the start of a line, as the terminal reports it. */
async function cursorAfter(text: string): Promise<string> {
  vi.stubGlobal("requestAnimationFrame", (cb: () => void) => setTimeout(cb, 0));
  const { XtermTerminalEmulator } = await import("./xterm_terminal_emulator.ts");
  const emulator = await XtermTerminalEmulator.create();
  try {
    const reply = new Promise<string>((done) => emulator.onData(done));
    emulator.write(new TextEncoder().encode(`${text}\x1b[6n`));
    return await reply;
  } finally {
    emulator.dispose();
  }
}

it("counts characters as wide as the programs writing them do", async () => {
  // Emoji with the variation selector, a modifier, a ZWJ sequence: two columns each.
  expect(await cursorAfter("⚠️")).toBe("\x1b[1;3R");
  expect(await cursorAfter("👍🏽")).toBe("\x1b[1;3R");
  expect(await cursorAfter("🏳️‍🌈")).toBe("\x1b[1;3R");
  // A combining mark adds no column; the TUI symbols stay narrow.
  expect(await cursorAfter("é⏺✻⎿")).toBe("\x1b[1;5R");
});
