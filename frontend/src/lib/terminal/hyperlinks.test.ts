import { beforeEach, describe, expect, it, vi } from "vitest";

const toast = vi.hoisted(() => vi.fn());
const openExternalTarget = vi.hoisted(() => vi.fn(() => true));
vi.mock("svelte-sonner", () => ({ toast }));
vi.mock("../window.ts", () => ({ openExternalTarget }));

import { openHyperlink } from "./hyperlinks.ts";

beforeEach(() => vi.clearAllMocks());

describe("OSC 8 hyperlinks", () => {
  it("open directly in the system browser without a notice", () => {
    openHyperlink("https://example.com/docs");
    expect(openExternalTarget).toHaveBeenCalledExactlyOnceWith({ url: "https://example.com/docs" });
    expect(toast).not.toHaveBeenCalled();
  });

  it("hand a remote pane's link to its own opener", () => {
    const open = vi.fn();
    openHyperlink("http://localhost:5173/", open);
    expect(open).toHaveBeenCalledExactlyOnceWith("http://localhost:5173/");
    expect(openExternalTarget).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
  });
});
