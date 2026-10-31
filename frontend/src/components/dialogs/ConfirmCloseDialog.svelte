<script lang="ts">
  import { getTheme } from "../../lib/theme.ts";
  import "./dialog.css";

  interface Props {
    open: boolean;
    /** The running process; empty when only scratchpad text would be lost. */
    processName: string;
    applicationTitle?: string;
    /** How many closing panes have a scratchpad that holds text. */
    scratchpads?: number;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let { open, processName, applicationTitle, scratchpads = 0, onConfirm, onCancel }: Props = $props();

  const theme = $derived(getTheme());

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
  >
    <div class="dialog-panel">
      <h2 class="dialog-title">
        {processName ? "Close running process?" : "Close and lose scratchpad text?"}
      </h2>

      {#if processName}
        <p class="dialog-text">
          {#if applicationTitle}
            <strong>{applicationTitle}</strong>
            ({processName}) is still running. Close it?
          {:else}
            <strong>{processName}</strong>
            is still running. Close it?
          {/if}
        </p>
      {/if}
      {#if scratchpads > 0}
        <p class="dialog-text">
          {scratchpads === 1 ? "A scratchpad holds text, which" : `${scratchpads} scratchpads hold text, which`}
          closing ends with {scratchpads === 1 ? "its session" : "their sessions"}.
        </p>
      {/if}

      <div class="dialog-actions">
        <button
          onclick={onCancel}
          class="dialog-btn"
        >
          Cancel
        </button>
        <button
          onclick={onConfirm}
          class="dialog-btn-danger"
        >
          Close
        </button>
      </div>
    </div>
  </div>
{/if}
