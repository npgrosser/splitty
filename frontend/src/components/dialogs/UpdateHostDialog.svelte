<script lang="ts">
  import { getTheme } from "../../lib/theme.ts";
  import type { UpdateWarning } from "../../lib/remote/update.ts";
  import "./dialog.css";

  // Updating a host's Splitty (`remote-screens`, Host Update) stops its daemon
  // and ends every session there, so the user confirms a warning that says
  // what ends (`lib/remote/update.ts`). Enter does not confirm: only the
  // button does. Escape cancels.

  interface Props {
    open: boolean;
    warning: UpdateWarning | null;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let { open, warning, onConfirm, onCancel }: Props = $props();

  const theme = $derived(getTheme());
  let panelEl: HTMLDivElement | undefined = $state();

  $effect(() => {
    if (!open) return;
    // Away from the terminal, so keys reach only the dialog.
    requestAnimationFrame(() => panelEl?.focus());
  });

  function handleKeydown(e: KeyboardEvent) {
    if (!open || e.key !== "Escape") return;
    e.preventDefault();
    onCancel();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open && warning}
  <div
    class="dialog-backdrop"
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33;--dialog-danger:{theme.ui['danger'] ?? '#e06c75'}"
    role="dialog"
    aria-modal="true"
    aria-labelledby="update-host-title"
  >
    <div bind:this={panelEl} class="dialog-panel dialog-panel-compact update-host" tabindex="-1">
      <h2 id="update-host-title" class="dialog-title dialog-title-sm">{warning.title}</h2>
      <p class="dialog-text">{warning.versions}</p>
      <p class="dialog-text"><strong>{warning.ends}</strong></p>
      <div class="dialog-actions">
        <button class="dialog-btn" onclick={onCancel}>Cancel</button>
        <button class="dialog-btn-danger" onclick={onConfirm}>Update now</button>
      </div>
    </div>
  </div>
{/if}

<style>
.update-host {
  outline: none;
  overflow-wrap: anywhere;
}
</style>
