import { describe, it, expect, vi, beforeEach } from "vitest";

const settings = { terminal: { allowOsc52Write: true } };
const { writeText, toast } = vi.hoisted(() => ({
  writeText: vi.fn(async () => true),
  toast: Object.assign(vi.fn(), { error: vi.fn() }),
}));
vi.mock("../settings.ts", () => ({ getSettings: () => settings }));
vi.mock("./terminal_shared.ts", () => ({ writeClipboardText: writeText }));
vi.mock("svelte-sonner", () => ({ toast }));
const { SplittyClipboardProvider } = await import("./clipboard_policy.ts");

beforeEach(() => {
  vi.clearAllMocks();
  writeText.mockResolvedValue(true);
  settings.terminal.allowOsc52Write = true;
});

describe("OSC 52 reads", () => {
  it("are always denied, regardless of the write setting", () => {
    const provider = new SplittyClipboardProvider();
    for (const enabled of [true, false]) {
      settings.terminal.allowOsc52Write = enabled;
      expect(provider.readText("c" as never)).toBe("");
      expect(provider.readText("p" as never)).toBe("");
    }
    expect(writeText).not.toHaveBeenCalled();
  });
});

describe("OSC 52 writes", () => {
  it("respect the opt-out", () => {
    settings.terminal.allowOsc52Write = false;
    new SplittyClipboardProvider().writeText("c" as never, "git status");
    expect(writeText).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
  });

  it("announce success only after copying, without displaying clipboard contents", async () => {
    new SplittyClipboardProvider().writeText("c" as never, "private oauth URL");
    expect(writeText).toHaveBeenCalledWith("private oauth URL");
    expect(toast).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(toast).toHaveBeenCalledExactlyOnceWith("Copied to clipboard", {
      id: "terminal-clipboard", duration: 2000,
    }));
  });

  it.each([false, "rejected"])("report failed copying (%s)", async (result) => {
    if (result === false) writeText.mockResolvedValue(false);
    else writeText.mockRejectedValue(new Error("denied"));
    new SplittyClipboardProvider().writeText("c" as never, "git status");
    await vi.waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast).not.toHaveBeenCalled();
  });

  it.each(["x".repeat(65537), "ä".repeat(32769)])("drop oversized payloads", (text) => {
    new SplittyClipboardProvider().writeText("c" as never, text);
    expect(writeText).not.toHaveBeenCalled();
  });

  it("accepts exactly 64 KiB of UTF-8", () => {
    new SplittyClipboardProvider().writeText("c" as never, "ä".repeat(32768));
    expect(writeText).toHaveBeenCalledOnce();
  });
});
