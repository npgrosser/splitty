<script lang="ts">
  import { getTheme } from "../../lib/theme.ts";
  import { filterCommands, listCommands, type PaletteCommand } from "../../lib/commands.ts";
  import { isCommandPaletteOpen, setCommandPaletteOpen } from "../../stores/ui.svelte.ts";
  import "../dialogs/dialog.css";

  const theme = $derived(getTheme());
  const open = $derived(isCommandPaletteOpen());

  let commands = $state<PaletteCommand[]>([]);
  let query = $state("");
  let selected = $state(0);
  let inputEl: HTMLInputElement | undefined = $state();

  const matches = $derived(filterCommands(commands, query));
  const active = $derived(Math.min(selected, matches.length - 1));

  $effect(() => {
    if (!open) return;
    commands = listCommands();
    query = "";
    selected = 0;
    requestAnimationFrame(() => inputEl?.focus());
  });

  function close() {
    setCommandPaletteOpen(false);
  }

  function run(command: PaletteCommand | undefined) {
    if (!command) return;
    close();
    command.run();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && matches.length > 0) {
      e.preventDefault();
      const delta = e.key === "ArrowDown" ? 1 : -1;
      selected = (active + delta + matches.length) % matches.length;
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(matches[active]);
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) close();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="dialog-backdrop"
    role="presentation"
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33;--dialog-danger:{theme.ui['danger'] ?? '#e06c75'}"
    onclick={handleBackdropClick}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog-panel dialog-panel-compact" onkeydown={handleKeydown}>
      <input
        bind:this={inputEl}
        bind:value={query}
        oninput={() => { selected = 0; }}
        type="text"
        placeholder="Type a command"
        aria-label="Command"
        spellcheck="false"
        autocomplete="off"
        class="dialog-input dialog-input-sm"
      />

      <div class="template-list" role="listbox" aria-label="Commands">
        {#if matches.length === 0}
          <p class="dialog-text dialog-text-center dialog-text-muted">No matching command.</p>
        {:else}
          {#each matches as command, i}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="template-item"
              class:template-item-selected={i === active}
              role="option"
              aria-selected={i === active}
              tabindex="-1"
              onmouseenter={() => { selected = i; }}
              onclick={() => run(command)}
            >
              <span class="template-item-name">{command.label}</span>
              {#if command.shortcut}
                <kbd class="shortcuts-kbd">{command.shortcut}</kbd>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}
