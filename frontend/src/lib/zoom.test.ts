import { describe, expect, it, vi } from "vitest";

const settings = { appearance: { zoom: 1, zoomScope: "window" as "window" | "terminals" } };
vi.mock("./settings.ts", () => ({ getSettings: () => settings }));
vi.mock("./platform.ts", () => ({ shellKind: "web" }));

import { handlesZoomKeys, nextZoom, pageZoom, zoomAction, zoomedFontSize, zoomLabel, ZOOM_STEPS } from "./zoom.ts";
import { isSplittyShortcut } from "./terminal/terminal_shared.ts";

const key = (fields: Partial<KeyboardEvent>) => ({ key: "", code: "", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...fields }) as KeyboardEvent;

describe("zoom levels", () => {
  it("steps in and out and back to 100 % (Bounded steps)", () => {
    expect(nextZoom(1, "in")).toBe(1.1);
    expect(nextZoom(1, "out")).toBe(0.9);
    expect(nextZoom(1.75, "reset")).toBe(1);
    expect(nextZoom(3, "in")).toBe(3);
    expect(nextZoom(0.5, "out")).toBe(0.5);
    // A level between the steps moves to the next one.
    expect(nextZoom(1.3, "in")).toBe(1.5);
    expect(nextZoom(1.3, "out")).toBe(1.25);
    expect(ZOOM_STEPS[0]).toBe(0.5);
    expect(zoomLabel(1.25)).toBe("125 %");
  });
});

describe("zoom keys", () => {
  it("takes Cmd on macOS and Ctrl elsewhere, with +, =, - and 0", () => {
    expect(zoomAction(key({ metaKey: true, key: "+" }), true)).toBe("in");
    expect(zoomAction(key({ metaKey: true, key: "=" }), true)).toBe("in");
    expect(zoomAction(key({ metaKey: true, key: "-" }), true)).toBe("out");
    expect(zoomAction(key({ metaKey: true, key: "0" }), true)).toBe("reset");
    expect(zoomAction(key({ ctrlKey: true, key: "+" }), true)).toBeNull();
    expect(zoomAction(key({ ctrlKey: true, key: "=" }), false)).toBe("in");
    expect(zoomAction(key({ ctrlKey: true, code: "NumpadSubtract", key: "-" }), false)).toBe("out");
    expect(zoomAction(key({ ctrlKey: true, code: "Numpad0", key: "Insert" }), false)).toBe("reset");
    expect(zoomAction(key({ metaKey: true, key: "+" }), false)).toBeNull();
    expect(zoomAction(key({ ctrlKey: true, altKey: true, key: "+" }), false)).toBeNull();
    expect(zoomAction(key({ ctrlKey: true, key: "a" }), false)).toBeNull();
  });

  it("leaves the keys to a browser with the window scope", () => {
    expect(handlesZoomKeys("window", "web")).toBe(false);
    expect(handlesZoomKeys("terminals", "web")).toBe(true);
    expect(handlesZoomKeys("window", "tauri")).toBe(true);
    expect(handlesZoomKeys("window", "electron")).toBe(true);
  });

  it("keeps the zoom keys from a focused terminal when Splitty takes them (Zoom in from a terminal)", () => {
    const zoomIn = key({ ctrlKey: true, metaKey: true, key: "+" });
    const plain = key({ [navigator.platform.toUpperCase().includes("MAC") ? "metaKey" : "ctrlKey"]: true, key: "+" });
    settings.appearance.zoomScope = "terminals";
    expect(isSplittyShortcut(plain)).toBe(true);
    expect(isSplittyShortcut(zoomIn)).toBe(false);
    // A browser with the window scope: the browser's zoom gets them.
    settings.appearance.zoomScope = "window";
    expect(isSplittyShortcut(plain)).toBe(false);
  });
});

describe("what zooms", () => {
  it("scales only the terminals' font size with the terminals scope (Only the terminals)", () => {
    expect(zoomedFontSize(14, 1.5, "terminals")).toBe(21);
    expect(pageZoom(1.5, "terminals")).toBe(1);
    expect(zoomedFontSize(14, 1.5, "window")).toBe(14);
    expect(pageZoom(1.5, "window")).toBe(1.5);
  });
});
