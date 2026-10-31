<script lang="ts">
  import type { Snippet } from "svelte";

  interface Props {
    title: string;
    collapsed: boolean;
    onToggle: () => void;
    children: Snippet;
  }

  let { title, collapsed, onToggle, children }: Props = $props();
</script>

<button class="section-header" onclick={onToggle}>
  <span class="section-chevron" class:collapsed></span>
  <span>{title}</span>
</button>
{#if !collapsed}
  <div class="section-body">
    {@render children()}
  </div>
{/if}
