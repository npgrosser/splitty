// Keyboard shortcut handlers for the workspace.
// Extracted from Workspace.svelte for maintainability.

import type { DockviewApi } from "dockview-core";
import { modKey } from "../shortcuts.ts";
import { guardedClose } from "../close_guard.ts";
import { addTerminalPane, findNeighborGroup } from "../panes.ts";
import { openExternalTarget } from "../window.ts";
import { getSettings } from "../settings.ts";
import { handlesZoomKeys, zoomAction, type ZoomAction } from "../zoom.ts";
import { toggleScratchpad } from "../scratchpad.ts";

export interface ShortcutContext {
  getActiveApi: () => DockviewApi | undefined;
  toggleDrawer: () => void;
  createScreen: () => void;
  setActiveScreen: (id: string) => void;
  getScreens: () => { id: string }[];
  toggleTemplateDialog: () => void;
  toggleShortcutsOverlay: () => void;
  toggleCommandPalette: () => void;
  /** Zoom in, out or back to 100 % (display-zoom). */
  zoom: (action: ZoomAction) => void;
}

export function setupKeyboardShortcuts(ctx: ShortcutContext): () => void {
  function handleKeydown(e: KeyboardEvent) {
    // ? → toggle shortcuts overlay (only when no modifier keys held)
    if (e.key === "?" && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag !== "INPUT" && tag !== "TEXTAREA") {
        e.preventDefault();
        ctx.toggleShortcutsOverlay();
        return;
      }
    }

    // Cmd/Ctrl and +, − or 0 → zoom, unless a browser keeps them (needs no active screen)
    const zoom = zoomAction(e);
    if (zoom && handlesZoomKeys(getSettings().appearance.zoomScope)) {
      e.preventDefault();
      ctx.zoom(zoom);
      return;
    }

    // Mod+Shift+P → toggle command palette (needs no active screen)
    if (modKey(e) && e.shiftKey && !e.ctrlKey && e.key.toLowerCase() === "p") {
      e.preventDefault();
      ctx.toggleCommandPalette();
      return;
    }

    const api = ctx.getActiveApi();
    if (!api) return;

    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;

    // Ctrl+B → toggle drawer
    if (e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey && key === "b") {
      e.preventDefault();
      ctx.toggleDrawer();
      return;
    }

    // Ctrl+Shift+Space → new screen
    if (e.ctrlKey && e.shiftKey && key === " ") {
      e.preventDefault();
      ctx.createScreen();
      return;
    }

    // Mod+Shift+1-9 → switch to screen N
    if (modKey(e) && e.shiftKey && /^Digit[1-9]$/.test(e.code)) {
      e.preventDefault();
      const idx = parseInt(e.code[5]) - 1;
      const screens = ctx.getScreens();
      if (idx < screens.length) {
        ctx.setActiveScreen(screens[idx].id);
      }
      return;
    }

    // Mod+T → new tab
    if (modKey(e) && !e.shiftKey && !e.ctrlKey && key === "t") {
      e.preventDefault();
      addTerminalPane(api);
      return;
    }

    // Mod+D → split right
    if (modKey(e) && !e.shiftKey && !e.ctrlKey && key === "d") {
      e.preventDefault();
      const ref = api.activePanel?.id;
      if (ref) {
        addTerminalPane(api, { direction: "right", referencePanel: ref });
      }
      return;
    }

    // Mod+Shift+D → split down
    if (modKey(e) && e.shiftKey && !e.ctrlKey && key === "d") {
      e.preventDefault();
      const ref = api.activePanel?.id;
      if (ref) {
        addTerminalPane(api, { direction: "below", referencePanel: ref });
      }
      return;
    }

    // Mod+W → close pane (with close guard)
    if (modKey(e) && !e.shiftKey && !e.ctrlKey && key === "w") {
      e.preventDefault();
      const active = api.activePanel;
      if (active) {
        guardedClose(api, active.id, () => active.api.close());
      }
      return;
    }

    // Mod+P → toggle template dialog
    if (modKey(e) && !e.ctrlKey && !e.shiftKey && key === "p") {
      e.preventDefault();
      ctx.toggleTemplateDialog();
      return;
    }

    // Mod+O → open a URL or local port in the system browser.
    // Used to create an in-app iframe pane; see openExternalTarget for why.
    if (modKey(e) && !e.ctrlKey && !e.shiftKey && key === "o") {
      e.preventDefault();
      const input = prompt("URL or port number:");
      if (!input) return;
      const trimmed = input.trim();
      if (!trimmed) return;
      const portMatch = trimmed.match(/^(\d+)(\?.*)?$/);
      const port = portMatch ? parseInt(portMatch[1], 10) : 0;
      if (port > 0 && port <= 65535) {
        openExternalTarget({ port, query: portMatch![2] ?? "" });
      } else {
        openExternalTarget({ url: trimmed });
      }
      return;
    }

    // Mod+Shift+Arrow → focus neighbor pane (directional)
    if (modKey(e) && e.shiftKey && !e.ctrlKey) {
      const dirMap: Record<string, "left" | "right" | "up" | "down"> = {
        ArrowLeft: "left",
        ArrowRight: "right",
        ArrowUp: "up",
        ArrowDown: "down",
      };
      const dir = dirMap[key];
      if (dir) {
        e.preventDefault();
        const neighbor = findNeighborGroup(api, dir);
        if (neighbor) {
          const panel = neighbor.activePanel;
          if (panel) {
            panel.api.setActive();
            panel.focus();
          }
        }
        return;
      }
    }

    // Mod+J → the active pane's scratchpad (session-scratchpad)
    if (modKey(e) && !e.ctrlKey && !e.shiftKey && key === "j") {
      const pane = api.activePanel;
      if (pane && toggleScratchpad(pane.id)) e.preventDefault();
      return;
    }

    // Mod+M → maximize / restore pane
    if (modKey(e) && !e.shiftKey && !e.ctrlKey && key === "m") {
      e.preventDefault();
      if (api.hasMaximizedGroup()) {
        api.exitMaximizedGroup();
      } else {
        api.activePanel?.api.maximize();
      }
      return;
    }

    // Ctrl+Shift+Arrow → resize pane
    if (e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey) {
      const active = api.activePanel;
      if (!active) return;
      const delta = 50;

      if (key === "ArrowRight") {
        e.preventDefault();
        active.api.setSize({ width: (active.api.width ?? 100) + delta });
      } else if (key === "ArrowLeft") {
        e.preventDefault();
        active.api.setSize({ width: Math.max(50, (active.api.width ?? 100) - delta) });
      } else if (key === "ArrowDown") {
        e.preventDefault();
        active.api.setSize({ height: (active.api.height ?? 100) + delta });
      } else if (key === "ArrowUp") {
        e.preventDefault();
        active.api.setSize({ height: Math.max(50, (active.api.height ?? 100) - delta) });
      }
    }
  }

  window.addEventListener("keydown", handleKeydown);
  return () => window.removeEventListener("keydown", handleKeydown);
}

export function setupTabRevealKeys(): () => void {
  const appEl = document.querySelector(".app");
  let holdingReveal = false;

  const onTabKeyDown = (e: KeyboardEvent) => {
    if ((e.key === "x" || e.key === "X") && modKey(e)) {
      if (e.shiftKey && !e.ctrlKey) {
        e.preventDefault();
        appEl?.toggleAttribute("data-tabs-inline");
      } else if (e.ctrlKey && !e.shiftKey) {
        e.preventDefault();
        holdingReveal = true;
        appEl?.setAttribute("data-modifier-reveal", "");
      }
    }
  };

  const onTabKeyUp = (e: KeyboardEvent) => {
    if (holdingReveal && (e.key === "x" || e.key === "X" || !modKey(e) || !e.ctrlKey)) {
      holdingReveal = false;
      appEl?.removeAttribute("data-modifier-reveal");
    }
  };

  const clearReveal = () => {
    holdingReveal = false;
    appEl?.removeAttribute("data-modifier-reveal");
  };

  window.addEventListener("keydown", onTabKeyDown, true);
  window.addEventListener("keyup", onTabKeyUp, true);
  window.addEventListener("blur", clearReveal);

  return () => {
    window.removeEventListener("keydown", onTabKeyDown, true);
    window.removeEventListener("keyup", onTabKeyUp, true);
    window.removeEventListener("blur", clearReveal);
    clearReveal();
  };
}
