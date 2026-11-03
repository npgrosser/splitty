import { afterEach, expect, it, vi } from "vitest";

const fixture = vi.hoisted(() => ({ parser: {}, load: vi.fn(), terminal: vi.fn(), font: vi.fn() }));
vi.mock("ghostty-web", () => ({
  Ghostty: { load: fixture.load },
  Terminal: class { constructor(options: unknown) { fixture.terminal(options); } loadAddon() {} },
  FitAddon: class {}, OSC8LinkProvider: class {}, UrlRegexProvider: class {},
}));
vi.mock("../theme.ts", () => ({ getXtermTheme: () => ({}) }));
vi.mock("../settings.ts", () => ({ getSettings: () => ({ appearance: { zoom: 1, zoomScope: "terminals" }, terminal: { xterm: {} } }) }));
vi.mock("../terminal_config.ts", () => ({ getConfig: () => ({ fontSize: 12, fontFamily: "monospace", cursorStyle: "block" }) }));
vi.mock("../shortcuts.ts", () => ({ isMac: false }));
vi.mock("../window.ts", () => ({ windowAPI: {} }));
vi.mock("../zoom.ts", () => ({ zoomedFontSize: (size: number) => size }));
vi.mock("./terminal_shared.ts", () => ({ ensureFont: fixture.font }));
vi.mock("./hyperlinks.ts", () => ({ openInBrowser: vi.fn() }));
afterEach(() => { vi.resetModules(); vi.clearAllMocks(); });

it("loads one explicit packaged WASM asset and shares the parser across terminal views", async () => {
  fixture.load.mockResolvedValue(fixture.parser); fixture.font.mockResolvedValue(undefined);
  const { warmup, GhosttyTerminalEmulator } = await import("./ghostty_terminal_emulator.ts");
  warmup(); warmup();
  await Promise.all([GhosttyTerminalEmulator.create(), GhosttyTerminalEmulator.create()]);
  expect(fixture.load).toHaveBeenCalledOnce();
  const path = fixture.load.mock.calls[0][0] as string;
  expect(path).toMatch(/ghostty-vt\.wasm$/);
  expect(path.startsWith("data:")).toBe(false);
  expect(path).not.toMatch(/^https?:/);
  expect(fixture.terminal).toHaveBeenCalledTimes(2);
  for (const [options] of fixture.terminal.mock.calls) expect(options.ghostty).toBe(fixture.parser);
});

it("retries the same packaged resource after an initialization error without a default lookup", async () => {
  fixture.load.mockRejectedValueOnce(Error("asset unavailable")).mockResolvedValue(fixture.parser);
  const { GhosttyTerminalEmulator } = await import("./ghostty_terminal_emulator.ts");
  await expect(GhosttyTerminalEmulator.create()).rejects.toThrow("asset unavailable");
  await GhosttyTerminalEmulator.create();
  expect(fixture.load).toHaveBeenCalledTimes(2);
  expect(fixture.load.mock.calls[0][0]).toBe(fixture.load.mock.calls[1][0]);
  expect(fixture.terminal).toHaveBeenCalledOnce();
});
