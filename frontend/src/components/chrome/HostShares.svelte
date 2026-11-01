<!--
  The local ports shared with one host, apart from its forwarded ports, and
  the form to share another. Only the user's click here starts a new share.
-->
<script lang="ts">
  import type { ShareRow } from "../../lib/remote/reverse_forwards.ts";
  import ShareForm from "./ShareForm.svelte";
  import Share from "./ShareRow.svelte";
  import { portsSummary } from "./remote_view.ts";

  let { target, rows }: { target: string; rows: readonly ShareRow[] } = $props();
  const id = $props.id();
  let adding = $state(false);
  const summary = $derived(portsSummary(rows));
</script>

<section class="host-shares" aria-label={`Shared with ${target}`}>
  <h4 class="heading">Shared with this host{#if summary}<span class="summary">{" · "}{summary}</span>{/if}</h4>
  {#each rows as row (row.remote)}<div class="share-entry"><Share {target} {row} /></div>{/each}
  <button class="add" onclick={() => adding = !adding} aria-expanded={adding} aria-controls={`${id}-form`} title={`Make a port of this machine reachable on ${target}`}>
    <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
    Share a local port
  </button>
  <div id={`${id}-form`}>
    {#if adding}<ShareForm {target} onclose={() => adding = false} />{/if}
  </div>
</section>

<style>
  .host-shares { min-width: 0; margin-top: 14px; font-size: 11.5px; color: var(--splitty-text-secondary); }
  .heading { margin: 0 0 4px; font-size: 10.5px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--splitty-text-muted); }
  .summary { font-weight: 400; letter-spacing: 0; text-transform: none; }
  .share-entry + .share-entry { border-top: 1px solid var(--splitty-border-subtle); }
  .add { display: inline-flex; align-items: center; gap: 6px; margin-top: 4px; min-height: 28px; padding: 0 8px; margin-left: -8px; border: 0; border-radius: 6px; background: none; color: var(--splitty-text-secondary); font: inherit; cursor: pointer; }
  .add:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }
  button:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: -2px; }
</style>
