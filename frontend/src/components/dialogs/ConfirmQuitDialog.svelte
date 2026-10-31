<!--
  Asks before the desktop app quits while processes run in local panes: the
  local server ends with the window, and with it those processes. Screens of
  hosts are only detached and keep running there.
-->
<script lang="ts">
  import { getTheme } from "../../lib/theme.ts";
  import type { RunningProcess } from "../../lib/close_guard.ts";
  import "./dialog.css";

  interface Props {
    open: boolean;
    processes: readonly RunningProcess[];
    /** Whether screens of hosts are open, which keep running there. */
    hostScreens: boolean;
    /** How many of the local sessions' scratchpads hold text, which end too. */
    scratchpads?: number;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let { open, processes, hostScreens, scratchpads = 0, onConfirm, onCancel }: Props = $props();
  const theme = $derived(getTheme());
  const SHOWN = 3;
  const shown = $derived(processes.slice(0, SHOWN));
  const more = $derived(processes.length - shown.length);

  function handleKeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    } else if (e.key === "Enter") {
      e.preventDefault();
      onConfirm();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <div
    class="dialog-backdrop"
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33;--dialog-danger:{theme.ui['danger'] ?? '#e06c75'}"
    role="dialog"
    aria-labelledby="quit-title"
  >
    <div class="dialog-panel">
      <h2 class="dialog-title" id="quit-title">Quit Splitty?</h2>
      {#if processes.length > 0}
        <p class="dialog-text">
          {processes.length === 1 ? "This process ends" : "These processes end"} with Splitty:
        </p>
        <ul class="quit-list">
          {#each shown as process}
            <li><strong>{process.name}</strong>{#if process.title}<span>{" in "}{process.title}</span>{/if}</li>
          {/each}
          {#if more > 0}<li>and {more} more</li>{/if}
        </ul>
      {/if}
      {#if scratchpads > 0}
        <p class="dialog-text">
          {scratchpads === 1 ? "A scratchpad holds text, which ends" : `${scratchpads} scratchpads hold text, which end`} with Splitty.
        </p>
      {/if}
      {#if hostScreens}
        <p class="dialog-text">Screens on hosts keep running there.</p>
      {/if}
      <div class="dialog-actions">
        <button onclick={onCancel} class="dialog-btn">Cancel</button>
        <button onclick={onConfirm} class="dialog-btn-danger">
          {processes.length + scratchpads === 1 ? "Quit and end it" : "Quit and end them"}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .quit-list { margin: 8px 0 14px; padding-left: 18px; font-size: 13px; line-height: 1.6; color: color-mix(in srgb, var(--dialog-fg) 67%, transparent); }
  .quit-list strong { color: var(--dialog-fg); }
</style>
