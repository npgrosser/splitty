<script lang="ts">
  import type { ShareRow } from "../../lib/remote/reverse_forwards.ts";
  import { reverseForwarding } from "../../stores/ports.svelte.ts";
  import { showContextMenu } from "../../stores/ui.svelte.ts";
  import { shareMenu, sharePrimary, shareStatus, type ShareMenuAction } from "./remote_view.ts";

  let { target, row }: { target: string; row: ShareRow } = $props();
  const busy = $derived(row.pending !== undefined);
  const primary = $derived(sharePrimary(row));
  const mapping = $derived(`localhost:${row.local} → ${target}:${row.remote}`);

  // Start, Resume and Retry share a listed rule again without the warning:
  // the user saw it when sharing the port.
  function runPrimary() {
    const controller = reverseForwarding();
    if (primary === "Stop") void controller?.stop(target, row.remote);
    else void controller?.share(target, row.remote, row.local);
  }

  function run(action: ShareMenuAction) {
    const controller = reverseForwarding();
    if (action === "stop") void controller?.stop(target, row.remote);
    else if (action === "remember") void controller?.remember(target, row.remote, row.local);
    else void controller?.forget(target, row.remote);
  }

  function openMenu(e: MouseEvent) {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    showContextMenu(shareMenu(row).map((item) => ({ label: item.label, detail: item.detail, action: () => run(item.action) })), rect.left, rect.bottom + 4);
  }
</script>

<div class="share" role="group" aria-label={`localhost:${row.local} shared with ${target} as port ${row.remote}`} aria-busy={busy}>
  <div class="line">
    <span class="mapping" title={mapping}>{mapping}</span>
    <button class="primary" disabled={busy}
      title={primary === "Stop" ? (row.saved ? "Stop sharing and pause the saved share" : "Stop sharing") : undefined}
      onclick={runPrimary}
    >{primary}</button>
    <button class="more" disabled={busy} aria-label={`More for shared port ${row.local}`} aria-haspopup="menu" onclick={openMenu}>
      <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><circle cx="3.5" cy="8" r="1.2" /><circle cx="8" cy="8" r="1.2" /><circle cx="12.5" cy="8" r="1.2" /></svg>
    </button>
  </div>
  <div class="detail"><span role="status">{shareStatus(row)}</span>{#if row.saved && row.forward}{" · "}<span>Remembered</span>{/if}</div>
  {#if row.error}<p class="error" role="status">{[row.error.message, row.error.hint].filter(Boolean).join(" ")}</p>{/if}
</div>

<style>
  .share { min-width: 0; padding: 7px 0; }
  .line { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .mapping { flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font-family: "JetBrains Mono", ui-monospace, monospace; font-size: 11px; color: var(--splitty-text-primary); }
  .detail { margin: 1px 0 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font-size: 11px; color: var(--splitty-text-muted); }
  .error { margin: 5px 0 0; font-size: 11px; line-height: 1.5; overflow-wrap: anywhere; color: var(--splitty-danger); }
  button { flex-shrink: 0; max-width: 100%; min-height: 24px; padding: 0 8px; border: 1px solid var(--splitty-border-default); border-radius: 5px; background: none; color: var(--splitty-text-secondary); font: inherit; font-size: 11px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--splitty-hover-bg); color: var(--splitty-text-primary); }
  button:disabled { opacity: 0.5; cursor: default; }
  .primary { color: var(--splitty-text-primary); }
  .more { width: 24px; padding: 0; display: flex; align-items: center; justify-content: center; border-color: transparent; color: var(--splitty-text-muted); }
  button:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: 1px; }
</style>
