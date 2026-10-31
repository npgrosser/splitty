<script lang="ts">
  import { onMount } from "svelte";
  import { getTheme } from "../../lib/theme.ts";
  import { shortcuts, screenShortcutKey } from "../../lib/shortcuts.ts";
  import { isShortcutsOpen, setShortcutsOpen } from "../../stores/ui.svelte.ts";
  import "../dialogs/dialog.css";

  let theme = $derived(getTheme());

  function close() {
    setShortcutsOpen(false);
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) close();
  }

  onMount(() => {
    function handleKeydown(e: KeyboardEvent) {
      if (!isShortcutsOpen()) return;
      if (e.key === "Escape") {
        e.preventDefault();
        close();
      }
    }
    window.addEventListener("keydown", handleKeydown, true);
    return () => window.removeEventListener("keydown", handleKeydown, true);
  });

  const shortcutEntries = Object.values(shortcuts);
</script>

{#if isShortcutsOpen()}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="dialog-backdrop"
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33;--dialog-danger:{theme.ui['danger'] ?? '#e06c75'}"
    onclick={handleBackdropClick}
  >
    <div class="shortcuts-panel">
      <h2 class="shortcuts-title">
        keyboard shortcuts
      </h2>

      <div class="shortcuts-list">
        {#each shortcutEntries as { label, key } (key)}
          <div class="shortcuts-item">
            <span class="shortcuts-label">{label}</span>
            <kbd class="shortcuts-kbd">{key}</kbd>
          </div>
        {/each}

        <div class="shortcuts-item shortcuts-item-last">
          <span class="shortcuts-label">Switch Screen</span>
          <kbd class="shortcuts-kbd">{screenShortcutKey(1)}–{screenShortcutKey(9)}</kbd>
        </div>
      </div>
    </div>
  </div>
{/if}
