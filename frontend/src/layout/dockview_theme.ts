import type { DockviewTheme } from "dockview-core";

/** Build a DockviewTheme from optional splitty layout tokens. */
export function getSplittyTheme(layout?: Record<string, string>): DockviewTheme {
  const scope = layout?.["dnd-overlay-scope"] ?? "local";
  const target = layout?.["dnd-overlay-target"] ?? "content";
  return {
    name: "splitty",
    className: "splitty-theme",
    dndOverlayMounting: scope === "global" ? "absolute" : "relative",
    dndPanelOverlay: target === "group" ? "group" : "content",
  };
}
