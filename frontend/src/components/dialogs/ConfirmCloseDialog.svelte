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
    /** How many annotations the closing viewer panes hold. */
    annotations?: number;
    /** How many closing terminals are still starting. */
    starting?: number;
    /** The server did not say what closing ends. */
    unknown?: boolean;
    /** Closing panes, or replacing the screen by a template. */
    action?: "close" | "replace";
    onConfirm: () => void;
    onCancel: () => void;
  }

  let { open, processName, applicationTitle, scratchpads = 0, annotations = 0, starting = 0, unknown = false, action = "close", onConfirm, onCancel }: Props = $props();

  const title = $derived(
    action === "replace" ? "Replace this screen?"
    : unknown ? "Close?"
    : processName ? "Close running process?"
    : starting > 0 ? "Close a starting terminal?"
    : scratchpads > 0 && annotations > 0 ? "Close and lose scratchpad text and annotations?"
    : annotations > 0 ? "Close and lose annotations?"
    : "Close and lose scratchpad text?",
  );

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
      <h2 class="dialog-title">{title}</h2>

      {#if action === "replace"}
        <p class="dialog-text">Applying the template ends the screen's terminals and viewers.</p>
      {/if}
      {#if unknown}
        <p class="dialog-text">
          This server does not say what {action === "replace" ? "replacing" : "closing"} ends: running programs, scratchpad text and annotations may be lost.
        </p>
      {/if}
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
      {#if starting > 0}
        <p class="dialog-text">
          {starting === 1 ? "A terminal is still starting." : `${starting} terminals are still starting.`}
        </p>
      {/if}
      {#if scratchpads > 0}
        <p class="dialog-text">
          {scratchpads === 1 ? "A scratchpad holds text, which" : `${scratchpads} scratchpads hold text, which`}
          closing ends with {scratchpads === 1 ? "its session" : "their sessions"}.
        </p>
      {/if}
      {#if annotations > 0}
        <p class="dialog-text">
          {annotations === 1 ? "Closing deletes an annotation and its replies." : `Closing deletes ${annotations} annotations and their replies.`}
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
          {action === "replace" ? "Replace" : "Close"}
        </button>
      </div>
    </div>
  </div>
{/if}
