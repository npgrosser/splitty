<script lang="ts">
  import { getDesktopPlatform } from "../../lib/platform.ts";
  import { windowAPI, noDragRegion } from "../../lib/window.ts";
  import { getTheme, subscribeTheme } from "../../lib/theme.ts";
  import { onMount } from "svelte";

  const isMac = getDesktopPlatform() === "mac";

  let fg = $state(getTheme().ui["text-primary"]);

  onMount(() => subscribeTheme(() => { fg = getTheme().ui["text-primary"]; }));
</script>

{#if !isMac}
  <!-- macOS uses native traffic light buttons; only show custom controls on other platforms -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="window-controls">
    <button
      class="window-control-btn"
      title="Minimize"
      style="color: {fg}aa"
      use:noDragRegion
      onclick={() => windowAPI.minimize()}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor">
        <rect x="1" y="5.5" width="10" height="1" />
      </svg>
    </button>
    <button
      class="window-control-btn"
      title="Maximize"
      style="color: {fg}aa"
      use:noDragRegion
      onclick={() => windowAPI.toggleMaximize()}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1">
        <rect x="1.5" y="1.5" width="9" height="9" rx="1" />
      </svg>
    </button>
    <button
      class="window-control-btn window-control-close"
      title="Close"
      style="color: {fg}aa"
      use:noDragRegion
      onclick={() => windowAPI.close()}
    >
      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2">
        <line x1="2" y1="2" x2="10" y2="10" />
        <line x1="10" y1="2" x2="2" y2="10" />
      </svg>
    </button>
  </div>
{/if}

<style>
.window-controls {
  display: flex;
  align-items: center;
  height: 100%;
  padding-right: 0;
}

.window-control-btn {
  background: none;
  border: none;
  cursor: pointer;
  padding: 0;
  font-size: 16px;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  aspect-ratio: 1;
  transition: background 120ms ease;
}

.window-control-btn:hover {
  background: var(--splitty-hover-bg);
}

.window-control-close:hover {
  background: rgba(232, 17, 35, 0.9);
}
</style>
