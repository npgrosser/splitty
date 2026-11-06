import { afterEach, expect, it, vi } from "vitest";

const fixture = vi.hoisted(() => ({
  parser: {}, load: vi.fn(), terminal: vi.fn(), font: vi.fn(), input: vi.fn(),
  focusEvents: true, screen: "normal", wheel: undefined as ((e: unknown) => boolean) | undefined,
}));
vi.mock("ghostty-web", () => ({
  Ghostty: { load: fixture.load },
  Terminal: class {
    rows = 24;
    buffer = { active: { get type() { return fixture.screen; } } };
    constructor(options: unknown) { fixture.terminal(options); }
    loadAddon() {}
    open() {}
    registerLinkProvider() {}
    attachCustomKeyEventHandler() {}
    attachCustomWheelEventHandler(handler: (e: unknown) => boolean) { fixture.wheel = handler; }
    input(data: string, wasUserInput?: boolean) { fixture.input(data, wasUserInput); }
    hasFocusEvents() { return fixture.focusEvents; }
    getMode() { return false; }
  },
  FitAddon: class {}, OSC8LinkProvider: class {}, UrlRegexProvider: class {},
}));
vi.mock("../theme.ts", () => ({ getXtermTheme: () => ({}) }));
vi.mock("../settings.ts", () => ({ getSettings: () => ({ appearance: { zoom: 1, zoomScope: "terminals" }, terminal: { xterm: {} } }) }));
vi.mock("../terminal_config.ts", () => ({ getConfig: () => ({ fontSize: 12, fontFamily: "monospace", cursorStyle: "block" }) }));
vi.mock("../shortcuts.ts", () => ({ isMac: false }));
vi.mock("../window.ts", () => ({ windowAPI: {} }));
vi.mock("../zoom.ts", () => ({ zoomedFontSize: (size: number) => size }));
vi.mock("./terminal_shared.ts", () => ({ ensureFont: fixture.font, isCopyShortcut: () => false, wheelDeltaToLines: () => 3 }));
vi.mock("./hyperlinks.ts", () => ({ openInBrowser: vi.fn() }));
afterEach(() => { vi.resetModules(); vi.clearAllMocks(); fixture.focusEvents = true; fixture.screen = "normal"; });

/** A pane container with ghostty-web's hidden textarea inside it. */
class FakeElement extends EventTarget {
  readonly children: FakeElement[] = [];
  contains(node: unknown): boolean { return node === this || this.children.some(child => child.contains(node)); }
}

async function openedPane() {
  fixture.load.mockResolvedValue(fixture.parser); fixture.font.mockResolvedValue(undefined);
  const { GhosttyTerminalEmulator } = await import("./ghostty_terminal_emulator.ts");
  const emulator = await GhosttyTerminalEmulator.create();
  const container = new FakeElement();
  const textarea = new FakeElement();
  container.children.push(textarea);
  emulator.open(container as unknown as HTMLElement);
  const focus = (type: "focusin" | "focusout", relatedTarget: unknown) =>
    container.dispatchEvent(Object.assign(new Event(type), { relatedTarget }));
  return { container, textarea, focus };
}

it("reports the pane gaining and losing focus once the application enables mode 1004", async () => {
  const { container, textarea, focus } = await openedPane();
  focus("focusin", null);
  focus("focusout", textarea);
  focus("focusin", container);
  focus("focusout", null);
  expect(fixture.input.mock.calls).toEqual([["\x1b[I", true], ["\x1b[O", true]]);
});

it("sends no focus reports while mode 1004 is off", async () => {
  fixture.focusEvents = false;
  const { focus } = await openedPane();
  focus("focusin", null);
  focus("focusout", null);
  expect(fixture.input).not.toHaveBeenCalled();
});

it("sends Shift+Enter and alternate-screen wheel keys to the application, not to the screen", async () => {
  const { container } = await openedPane();
  const key = { key: "Enter", shiftKey: true, ctrlKey: false, altKey: false, metaKey: false };
  container.dispatchEvent(Object.assign(new Event("keydown", { cancelable: true }), key));
  fixture.screen = "alternate";
  expect(fixture.wheel?.({ deltaY: 120, deltaMode: 0 })).toBe(true);
  expect(fixture.input.mock.calls).toEqual([
    ["\x1b[13;2u", true], ["\x1b[B", true], ["\x1b[B", true], ["\x1b[B", true],
  ]);
});

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
