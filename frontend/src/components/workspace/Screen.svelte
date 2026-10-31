<script lang="ts">
  import { onMount, type Snippet } from "svelte";
  import { DockviewManager } from "../../layout/dockview_manager.ts";
  import { setupAutoHideTabs } from "../../layout/auto_hide_tabs.ts";
  import { bridgeDockviewToMux, type ScreenBinding } from "../../lib/connection/screen_binding.ts";
  import type { DockviewApi } from "dockview-core";

  // ── Props ──

  interface Props {
    screenId: string;
    /** The screen's connection; its layout API is registered with it in `onApiReady`. */
    binding: ScreenBinding;
    isActive: boolean;
    slideDirection?: "up" | "down";
    tabSettings: { autoHide: boolean; autoHideMulti: boolean; syncRow: boolean };
    onApiReady: (screenId: string, api: DockviewApi, manager: DockviewManager) => void;
    onPaneVersionChange: () => void;
    onScreenEmpty: (screenId: string) => void;
    /** The panes take no input (a remote screen whose host is not connected). */
    blocked?: boolean;
    /** Shown over the panes (a remote screen's status). */
    overlay?: Snippet;
  }

  let {
    screenId,
    binding,
    isActive,
    slideDirection,
    tabSettings,
    onApiReady,
    onPaneVersionChange,
    onScreenEmpty,
    blocked = false,
    overlay,
  }: Props = $props();

  // ── Internal State ──

  let containerEl: HTMLElement;
  let manager: DockviewManager | undefined;
  let autoHideCleanup: (() => void) | undefined;
  let muxBridgeCleanup: (() => void) | undefined;
  const panelDisposables: { dispose(): void }[] = [];

  // ── Auto-Hide Tabs Effect ──

  $effect(() => {
    const _tabs = tabSettings;
    if (manager) {
      autoHideCleanup?.();
      autoHideCleanup = setupAutoHideTabs(manager.api, _tabs);
    }
  });

  // ── Active Screen ──
  // Only the active screen's terminals take focus; becoming active hands the
  // keyboard to its active pane.

  $effect(() => {
    binding.setActive(isActive);
  });

  // ── Lifecycle ──

  onMount(() => {
    manager = new DockviewManager(containerEl);
    const api = manager.api;

    // Notify parent — handles initial pane creation or snapshot apply
    onApiReady(screenId, api, manager);

    // Bridge dockview events → mux backend
    muxBridgeCleanup = bridgeDockviewToMux(binding, api, containerEl);

    // Setup auto-hide tabs
    autoHideCleanup = setupAutoHideTabs(api, tabSettings);

    // Track panel add/remove/title changes for Drawer reactivity
    const trackPanelTitles = (panel: { id: string; api: { onDidTitleChange(fn: () => void): { dispose(): void } } }) => {
      const d = panel.api.onDidTitleChange(() => onPaneVersionChange());
      panelDisposables.push(d);
      return d;
    };

    const disposeAdd = api.onDidAddPanel((panel) => {
      onPaneVersionChange();
      trackPanelTitles(panel);
    });

    // Auto-close empty screens (when more than one exists)
    const disposeRemove = api.onDidRemovePanel(() => {
      onPaneVersionChange();
      setTimeout(() => {
        if (api.panels.length === 0) {
          onScreenEmpty(screenId);
        }
      }, 0);
    });

    // Track titles of existing panels (from snapshot restore)
    for (const panel of api.panels) {
      trackPanelTitles(panel);
    }

    return () => {
      disposeAdd.dispose();
      disposeRemove.dispose();
      for (const d of panelDisposables) d.dispose();
      autoHideCleanup?.();
      muxBridgeCleanup?.();
      manager?.dispose();
      manager = undefined;
    };
  });
</script>

<!-- An inactive screen is inert: it can neither take focus nor be clicked, so
     nothing a hidden screen does (a snapshot rebuild, a server's nudge) can pull
     the keyboard into it. A blocked screen's panes are inert as well; only the
     overlay above them takes input. -->
<div
  class="screen-layout"
  data-active={isActive ? "true" : "false"}
  data-slide={slideDirection}
  inert={!isActive}
>
  <div bind:this={containerEl} class="screen-dockview" inert={blocked}></div>
  {@render overlay?.()}
</div>
