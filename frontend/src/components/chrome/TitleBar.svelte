<script lang="ts">
  import { orderedScreens } from "../../lib/remote/screen_groups.ts";
  import { settings } from "../../stores/settings.svelte.ts";
  import { emitUICommand } from "../../lib/client_commands.ts";
  import {
    getScreens, getActiveScreenId, getHostOrder, setActiveScreen,
  } from "../../stores/screens.svelte.ts";
  import { getDesktopPlatform } from "../../lib/platform.ts";
  import { windowAPI, dragRegion, noDragRegion } from "../../lib/window.ts";
  import { shellKind } from "../../lib/platform.ts";
  import { isDrawerPinned } from "../../stores/ui.svelte.ts";
  import ConnectionIndicator from "./ConnectionIndicator.svelte";
  import WindowControls from "./WindowControls.svelte";

  interface Props {
    title: string;
    maximized: boolean;
  }

  let { title, maximized }: Props = $props();

  const isMac = getDesktopPlatform() === "mac";
  const pinned = $derived(isDrawerPinned());

  function handleMouseDown(e: MouseEvent) {
    // When maximized on Linux, the WM steals mouse events near the top screen
    // edge for its own resize zones. Intercept mousedown anywhere on the
    // titlebar while maximized and manually unmaximize + reposition so the
    // user can drag the window out even from the very top of the screen.
    if (maximized && e.button === 0) {
      e.preventDefault();
      windowAPI.startDragFromMaximized(e.screenX, e.screenY);
    }
  }

  function handleDoubleClick() {
    // Tauri's data-tauri-drag-region already handles double-click-to-maximize natively.
    if (shellKind === "tauri") return;
    windowAPI.toggleMaximize();
  }

  function handleWheel(e: WheelEvent) {
    if (!settings().window.titleBarScroll) return;
    const screens = orderedScreens(getScreens(), getHostOrder());
    if (screens.length < 2) return;
    const activeId = getActiveScreenId();
    const idx = screens.findIndex((s) => s.id === activeId);
    const next = e.deltaY > 0
      ? Math.min(idx + 1, screens.length - 1)
      : Math.max(idx - 1, 0);
    if (next !== idx) setActiveScreen(screens[next].id);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="custom-titlebar"
  class:mac={isMac}
  use:dragRegion
  onmousedown={handleMouseDown}
  ondblclick={handleDoubleClick}
  onwheel={handleWheel}
>
  {#if !pinned}
    <button
      class="titlebar-drawer-toggle"
      use:noDragRegion
      onclick={(e) => { e.stopPropagation(); emitUICommand({ type: "drawer.toggle" }); }}
      title="Toggle drawer (Ctrl+B)"
    >
      <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
        <rect x="2" y="3" width="12" height="1.5" rx="0.5" />
        <rect x="2" y="7.25" width="12" height="1.5" rx="0.5" />
        <rect x="2" y="11.5" width="12" height="1.5" rx="0.5" />
      </svg>
    </button>
  {/if}
  <span class="custom-titlebar-title">{title}</span>
  <div class="titlebar-spacer"></div>
  <ConnectionIndicator />
  <WindowControls />
</div>

<style>
.custom-titlebar {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  height: var(--splitty-titlebar-height, 36px);
  flex-shrink: 0;
  background: var(--splitty-surface-raised);
  border-bottom: none;
  user-select: none;
}

/* Leave space for macOS native traffic light buttons */
.custom-titlebar.mac {
  padding-left: 70px;
}

.titlebar-drawer-toggle {
  background: none;
  border: none;
  color: var(--splitty-text-secondary);
  cursor: pointer;
  padding: 0 10px;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  transition: color 120ms ease, background 120ms ease;
}

.titlebar-drawer-toggle:hover {
  color: var(--splitty-text-primary);
  background: var(--splitty-hover-bg);
}

.titlebar-spacer {
  flex: 1;
}

.custom-titlebar-title {
  position: absolute;
  left: 0;
  right: 0;
  text-align: center;
  font-size: 12px;
  color: var(--splitty-text-secondary);
  pointer-events: none;
}
</style>
