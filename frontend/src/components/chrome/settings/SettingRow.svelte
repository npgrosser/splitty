<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    label: string;
    hint?: string;
    showReset?: boolean;
    onReset?: () => void;
    disabled?: boolean;
    children: Snippet;
  }

  let { label, hint, showReset = false, onReset, disabled = false, children }: Props = $props();
</script>

<div class="setting-row" class:setting-row-disabled={disabled}>
  <div class="setting-info">
    <span class="setting-label">{label}</span>
    {#if hint}
      <span class="setting-hint">{hint}</span>
    {/if}
  </div>
  <div class="setting-control">
    {@render children()}
    {#if showReset && onReset}
      <button class="reset-btn" onclick={onReset} title="Reset to default">&times;</button>
    {/if}
  </div>
</div>
