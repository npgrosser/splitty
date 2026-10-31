<script lang="ts">
  import type { PortRow } from "../../lib/remote/ports.ts";
  import ManualPortForm from "./ManualPortForm.svelte";
  import Port from "./PortRow.svelte";
  import { portsSummary } from "./remote_view.ts";

  let { target, rows, targets }: { target: string; rows: readonly PortRow[]; targets: readonly string[] } = $props();
  const id = $props.id();
  let adding = $state(false);
  const summary = $derived(portsSummary(rows));
  // Forwarded and saved ports first; the ones only detected are suggestions.
  const kept = $derived(rows.filter((row) => row.forward || row.saved));
  const detected = $derived(rows.filter((row) => !row.forward && !row.saved));
</script>

<section class="host-ports" aria-label={`Ports on ${target}`}>
  <h4 class="heading">Ports{#if summary}<span class="summary">{" · "}{summary}</span>{/if}</h4>
  {#each kept as row (row.port)}<div class="port-entry"><Port {target} {row} /></div>{/each}
  {#if detected.length > 0}
    {#if kept.length > 0}<p class="group">Detected</p>{/if}
    {#each detected as row (row.port)}<div class="port-entry"><Port {target} {row} /></div>{/each}
  {/if}
  {#if rows.length === 0 && !adding}<p class="empty">No ports detected.</p>{/if}
  <button class="add" onclick={() => adding = !adding} aria-expanded={adding} aria-controls={`${id}-form`}>
    <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
    Add port
  </button>
  <div id={`${id}-form`}>
    {#if adding}<ManualPortForm {target} {targets} onclose={() => adding = false} />{/if}
  </div>
</section>

<style>
  .host-ports { min-width: 0; margin-top: 14px; font-size: 11.5px; color: var(--splitty-text-secondary); }
  .heading { margin: 0 0 4px; font-size: 10.5px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--splitty-text-muted); }
  .summary { font-weight: 400; letter-spacing: 0; text-transform: none; }
  .port-entry + .port-entry { border-top: 1px solid var(--splitty-border-subtle); }
  .group { margin: 8px 0 0; font-size: 11px; color: var(--splitty-text-muted); }
  .empty { margin: 4px 0; color: var(--splitty-text-muted); font-size: 11.5px; }
  .add { display: inline-flex; align-items: center; gap: 6px; margin-top: 4px; min-height: 28px; padding: 0 8px; margin-left: -8px; border: 0; border-radius: 6px; background: none; color: var(--splitty-text-secondary); font: inherit; cursor: pointer; }
  .add:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }
  button:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: -2px; }
</style>
