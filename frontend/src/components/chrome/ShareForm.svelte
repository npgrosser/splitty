<!--
  Share a local port with one host: the user's own action, never offered from
  detected ports, links or anything a host sends. The statement of who can use
  the port stays visible before Share; administrative ports get a stronger one.
-->
<script lang="ts">
  import { portNumber } from "../../lib/remote/manual_port.ts";
  import { shareInput, shareWarning } from "../../lib/remote/reverse_forwards.ts";
  import { reverseForwarding } from "../../stores/ports.svelte.ts";
  import { focusInlineForm } from "./inline_focus.ts";

  let { target, onclose }: { target: string; onclose: () => void } = $props();
  const id = $props.id();
  let local = $state("");
  let remote = $state("");
  let remember = $state(false);
  let busy = $state(false);
  let error = $state("");
  const localPort = $derived(portNumber(local));
  const warning = $derived(shareWarning(target, localPort));

  async function submit() {
    if (busy) return;
    const parsed = shareInput({ local, remote, remember });
    if (parsed.error) { error = parsed.error; return; }
    const value = parsed.value!;
    const controller = reverseForwarding();
    if (!controller) { error = "Sharing is not ready yet."; return; }
    error = "";
    busy = true;
    const result = await controller.share(target, value.port, value.local, { remember: value.remember });
    busy = false;
    // A failed Remember keeps the share running; the form says it was not remembered.
    const failure = controller.errorOf(target, value.port);
    if (!result || (value.remember && failure)) {
      error = failure ? [failure.message, failure.hint].filter(Boolean).join(" ") : "The port could not be shared.";
    } else onclose();
  }
</script>

<form class="share-form" aria-label={`Share a local port with ${target}`} aria-busy={busy} aria-describedby={error ? `${id}-error` : undefined} use:focusInlineForm={() => { if (!busy) onclose(); }} onsubmit={(event) => { event.preventDefault(); void submit(); }}>
  <div class="ports">
    <label>Local port
      <input bind:value={local} inputmode="numeric" placeholder="11434" required disabled={busy} aria-describedby={`${id}-statement${warning.strong ? ` ${id}-strong` : ""}`} />
    </label>
    <label>Port on host <span>(optional)</span>
      <input bind:value={remote} inputmode="numeric" placeholder={localPort === null ? "Same" : String(localPort)} disabled={busy} aria-describedby={`${id}-remote-hint`} />
    </label>
  </div>
  <p id={`${id}-remote-hint`}>Programs on {target} reach it at localhost and this port; by default the same number.</p>
  <label class="remember"><input type="checkbox" bind:checked={remember} disabled={busy} aria-describedby={remember ? `${id}-remember-hint` : undefined} /> Remember</label>
  {#if remember}<p id={`${id}-remember-hint`}>Shares it again when this host connects, until you stop or forget it.</p>{/if}
  <p id={`${id}-statement`} class="statement">{warning.statement}</p>
  {#if warning.strong}<p id={`${id}-strong`} class="strong">{warning.strong}</p>{/if}
  {#if error}<p id={`${id}-error`} class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button class="primary" type="submit" disabled={busy} aria-describedby={`${id}-statement${warning.strong ? ` ${id}-strong` : ""}`}>{busy ? "Working…" : "Share"}</button>
    <button type="button" disabled={busy} onclick={onclose}>Cancel</button>
  </div>
</form>

<style>
  .share-form { display: flex; flex-direction: column; gap: 8px; padding: 8px 0; min-width: 0; font-size: 11px; color: var(--splitty-text-secondary); container-type: inline-size; }
  .ports { display: grid; grid-template-columns: minmax(0, 1fr); gap: 8px; }
  label { display: flex; flex-wrap: wrap; align-content: start; gap: 3px; min-width: 0; }
  label span, p { color: var(--splitty-text-muted); }
  input { box-sizing: border-box; width: 100%; min-width: 0; border: 1px solid var(--splitty-border-subtle); border-radius: 3px; background: var(--splitty-surface-raised); color: var(--splitty-text-primary); padding: 6px; font: inherit; user-select: text; }
  .remember { flex-direction: row; align-items: center; gap: 6px; }
  .remember input { width: auto; }
  p { margin: 0; font-size: 10px; line-height: 1.5; overflow-wrap: anywhere; }
  .statement { color: var(--splitty-text-secondary); }
  .strong { padding: 6px 8px; border: 1px solid var(--splitty-danger); border-radius: 3px; color: var(--splitty-danger); font-size: 11px; }
  .error { color: var(--splitty-text-primary); }
  .actions { display: flex; flex-wrap: wrap; gap: 5px; }
  button { min-width: 0; max-width: 100%; min-height: 26px; white-space: normal; border: 1px solid var(--splitty-border-subtle); border-radius: 3px; padding: 4px 6px; background: none; color: var(--splitty-text-secondary); font: inherit; cursor: pointer; }
  button.primary { background: var(--splitty-hover-bg); color: var(--splitty-text-primary); }
  button:hover:not(:disabled) { background: var(--splitty-hover-bg); color: var(--splitty-text-primary); }
  button:disabled { opacity: 0.6; cursor: default; }
  input:focus-visible, button:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: 2px; }
  @container (min-width: 240px) { .ports { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
