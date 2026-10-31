<script lang="ts">
  import type { PortRow } from "../../lib/remote/ports.ts";
  import { MAX_FORWARD_LABEL } from "../../lib/remote/ports.ts";
  import { portNumber } from "../../lib/remote/manual_port.ts";
  import { portForwarding } from "../../stores/ports.svelte.ts";
  import { showContextMenu } from "../../stores/ui.svelte.ts";
  import { portMenu, portPrimary, portStatus, type PortMenuAction } from "./remote_view.ts";
  import { focusInlineForm } from "./inline_focus.ts";

  let { target, row }: { target: string; row: PortRow } = $props();
  let editor = $state<"label" | "local" | null>(null);
  let label = $state("");
  let local = $state("");
  let inputError = $state("");
  const busy = $derived(row.pending !== undefined);
  const primary = $derived(portPrimary(row));
  const name = $derived(row.saved?.label || row.process || "");
  const address = $derived(row.forward?.local ?? row.saved?.local);

  function edit(kind: "label" | "local") {
    label = row.saved?.label ?? "";
    local = "";
    inputError = "";
    editor = kind;
  }

  function runPrimary() {
    const controller = portForwarding();
    if (row.forward) controller?.open(row.forward);
    else void controller?.forward(target, row.port);
  }

  function run(action: PortMenuAction) {
    const controller = portForwarding();
    if (action === "stop") void controller?.stop(target, row.port);
    else if (action === "forget") void controller?.forget(target, row.port);
    else edit(action);
  }

  function openMenu(e: MouseEvent) {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    showContextMenu(portMenu(row).map((item) => ({ label: item.label, detail: item.detail, action: () => run(item.action) })), rect.left, rect.bottom + 4);
  }

  async function saveLabel() {
    if ([...label].length > MAX_FORWARD_LABEL) { inputError = "Label must be at most 200 characters."; return; }
    if (await portForwarding()?.save(target, row.port, { label,
      ...(row.saved || !row.forward ? {} : { local: row.forward.local }),
    })) editor = null;
  }

  async function useAlternate() {
    const number = portNumber(local);
    if (number === null) { inputError = "Enter a local port from 1 to 65535."; return; }
    inputError = "";
    if (await portForwarding()?.forward(target, row.port, { local: number })) editor = null;
  }
</script>

<div class="port" role="group" aria-label={`Port ${row.port} on ${target}`} aria-busy={busy}>
  <div class="line">
    <strong class="number">{row.port}</strong>
    <span class="name" title={name}>{name}</span>
    <button class="primary" class:open={primary === "Open"} disabled={busy}
      title={primary === "Open" ? "Verify this forward and open in browser" : undefined}
      onclick={runPrimary}
    >{primary}{#if primary === "Open"} <span aria-hidden="true">↗</span>{/if}</button>
    <button class="more" disabled={busy} aria-label={`More for port ${row.port}`} aria-haspopup="menu" onclick={openMenu}>
      <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><circle cx="3.5" cy="8" r="1.2" /><circle cx="8" cy="8" r="1.2" /><circle cx="12.5" cy="8" r="1.2" /></svg>
    </button>
  </div>
  <!-- A port only detected says nothing more than its Forward button. -->
  {#if row.forward || row.saved || row.error || row.pending}
    <div class="detail">{#if address !== undefined}<span class="address">localhost:{address}</span>{" · "}{/if}<span role="status">{portStatus(row)}</span>{#if row.saved?.label && row.process}{" · "}<span title={row.process}>{row.process}</span>{/if}</div>
  {/if}
  {#if row.error}<p class="error" role="status">{[row.error.message, row.error.hint].filter(Boolean).join(" ")}</p>{/if}
  {#if !row.forward && row.error?.code === "local_port_conflict"}
    <button class="recovery" disabled={busy} onclick={() => edit("local")}>Use another local port…</button>
  {/if}
  {#if editor === "label"}
    <form aria-label={row.saved ? "Edit port label" : "Remember port"} use:focusInlineForm={() => { if (!busy) editor = null; }} onsubmit={(event) => { event.preventDefault(); void saveLabel(); }}>
      <label>Label <span class="muted">(optional)</span><input bind:value={label} placeholder="Frontend or API" disabled={busy} /></label>
      {#if !row.saved}<p class="muted">Remember this mapping for future host connections.</p>{/if}
      {#if inputError}<p role="alert">{inputError}</p>{/if}
      <div class="actions"><button class="primary" disabled={busy}>{row.saved ? "Save label" : "Remember"}</button><button type="button" disabled={busy} onclick={() => editor = null}>Cancel</button></div>
    </form>
  {:else if editor === "local" && !row.forward}
    <form aria-label="Change local port" use:focusInlineForm={() => { if (!busy) editor = null; }} onsubmit={(event) => { event.preventDefault(); void useAlternate(); }}>
      <label>Alternate local port<input bind:value={local} inputmode="numeric" placeholder="15173" disabled={busy} required /></label>
      <p class="muted">Uses this exact port. A saved mapping changes only after success.</p>
      {#if inputError}<p role="alert">{inputError}</p>{/if}
      <div class="actions"><button class="primary" disabled={busy}>Forward here</button><button type="button" disabled={busy} onclick={() => editor = null}>Cancel</button></div>
    </form>
  {/if}
</div>

<style>
  .port { min-width: 0; padding: 7px 0; }
  .line { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .number { flex-shrink: 0; min-width: 40px; font-family: "JetBrains Mono", ui-monospace, monospace; font-size: 11.5px; font-weight: 600; color: var(--splitty-text-primary); font-variant-numeric: tabular-nums; }
  .name { flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--splitty-text-primary); }
  .detail { margin: 1px 0 0 48px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font-size: 11px; color: var(--splitty-text-muted); }
  .address { font-family: "JetBrains Mono", ui-monospace, monospace; font-size: 10.5px; color: var(--splitty-text-secondary); }
  .muted { color: var(--splitty-text-muted); }
  p { margin: 5px 0 0; font-size: 11px; line-height: 1.5; overflow-wrap: anywhere; }
  .error { margin-left: 48px; color: var(--splitty-danger); }
  button { flex-shrink: 0; max-width: 100%; min-height: 24px; padding: 0 8px; border: 1px solid var(--splitty-border-default); border-radius: 5px; background: none; color: var(--splitty-text-secondary); font: inherit; font-size: 11px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--splitty-hover-bg); color: var(--splitty-text-primary); }
  button:disabled { opacity: 0.5; cursor: default; }
  .primary { color: var(--splitty-text-primary); }
  .primary.open { border-color: transparent; color: var(--splitty-accent); }
  .more { width: 24px; padding: 0; display: flex; align-items: center; justify-content: center; border-color: transparent; color: var(--splitty-text-muted); }
  .recovery { margin: 6px 0 0 48px; }
  form { margin: 8px 0 0 48px; padding-top: 8px; border-top: 1px solid var(--splitty-border-subtle); }
  label { display: flex; flex-wrap: wrap; gap: 3px; font-size: 11px; }
  input { box-sizing: border-box; width: 100%; min-width: 0; padding: 6px; border: 1px solid var(--splitty-border-default); border-radius: 5px; background: var(--splitty-surface-base); color: var(--splitty-text-primary); font: inherit; user-select: text; }
  .actions { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: 1px; }
</style>
