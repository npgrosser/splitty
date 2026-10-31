<script lang="ts">
  import { manualPort } from "../../lib/remote/manual_port.ts";
  import { portForwarding } from "../../stores/ports.svelte.ts";
  import { focusInlineForm } from "./inline_focus.ts";

  let { target = "", targets = [], onclose }: {
    target?: string; targets?: readonly string[]; onclose: () => void;
  } = $props();
  const id = $props.id();
  let host = $state("");
  let remote = $state("");
  let local = $state("");
  let label = $state("");
  let remember = $state(false);
  let busy = $state(false);
  let error = $state("");

  async function submit(start: boolean) {
    if (busy) return;
    const parsed = manualPort({ target: target || host, remote, local, label, remember });
    if (parsed.error) { error = parsed.error; return; }
    const value = parsed.value!;
    const controller = portForwarding();
    if (!controller) { error = "Port forwarding is not ready yet."; return; }
    error = "";
    busy = true;
    const result = start
      ? await controller.forward(value.target, value.port, value)
      : await controller.save(value.target, value.port, { local: value.local, label: value.label });
    busy = false;
    if (!result || (start && remember && controller.errorOf(value.target, value.port))) {
      const failure = controller.errorOf(value.target, value.port);
      error = failure ? [failure.message, failure.hint].filter(Boolean).join(" ") : "The mapping could not be saved or started.";
    } else onclose();
  }
</script>

<form class="port-form" aria-label={target ? `Add port on ${target}` : "Add port"} aria-busy={busy} aria-describedby={error ? `${id}-error` : undefined} use:focusInlineForm={() => { if (!busy) onclose(); }} onsubmit={(event) => { event.preventDefault(); void submit(true); }}>
  {#if target}
    <p class="host">Host <strong>{target}</strong></p>
  {:else}
    <label>SSH target
      <input list={`${id}-targets`} bind:value={host} placeholder="user@host" autocomplete="off" spellcheck="false" required disabled={busy} />
    </label>
    <datalist id={`${id}-targets`}>
      {#each targets as suggestion}<option value={suggestion}></option>{/each}
    </datalist>
  {/if}
  <div class="ports">
    <label>Remote port
      <input bind:value={remote} inputmode="numeric" placeholder="5173" required disabled={busy} />
    </label>
    <label>Local port <span>(optional)</span>
      <input bind:value={local} inputmode="numeric" placeholder="Auto" aria-describedby={`${id}-local-hint`} disabled={busy} />
    </label>
  </div>
  <p id={`${id}-local-hint`}>Set a local port to use that exact number.</p>
  <label class="remember"><input type="checkbox" bind:checked={remember} disabled={busy} aria-describedby={remember ? `${id}-remember-hint` : undefined} /> Remember</label>
  {#if remember}
    <label>Label <span>(optional)</span>
      <input bind:value={label} placeholder="Frontend or API" disabled={busy} />
    </label>
    <p id={`${id}-remember-hint`}>Restores when this host connects. Saving without starting uses the remote port unless a local port is set or already forwarded.</p>
  {/if}
  {#if error}<p id={`${id}-error`} class="error" role="alert">{error}</p>{/if}
  <div class="actions">
    <button class="primary" type="submit" disabled={busy}>{busy ? "Working…" : "Forward"}</button>
    <button type="button" disabled={busy} onclick={onclose}>Cancel</button>
    {#if remember}<button class="save-only" type="button" disabled={busy} onclick={() => void submit(false)}>Save without starting</button>{/if}
  </div>
</form>

<style>
  .port-form { display: flex; flex-direction: column; gap: 8px; padding: 8px 0; min-width: 0; font-size: 11px; color: var(--splitty-text-secondary); container-type: inline-size; }
  .host { display: flex; flex-direction: column; gap: 2px; }
  .host strong { color: var(--splitty-text-secondary); font-weight: 500; overflow-wrap: anywhere; }
  .ports { display: grid; grid-template-columns: minmax(0, 1fr); gap: 8px; }
  label { display: flex; flex-wrap: wrap; align-content: start; gap: 3px; min-width: 0; }
  label span, p { color: var(--splitty-text-muted); }
  input { box-sizing: border-box; width: 100%; min-width: 0; border: 1px solid var(--splitty-border-subtle); border-radius: 3px; background: var(--splitty-surface-raised); color: var(--splitty-text-primary); padding: 6px; font: inherit; user-select: text; }
  .remember { flex-direction: row; align-items: center; gap: 6px; }
  .remember input { width: auto; }
  p { margin: 0; font-size: 10px; line-height: 1.5; overflow-wrap: anywhere; }
  .error { color: var(--splitty-text-primary); }
  .actions { display: flex; flex-wrap: wrap; gap: 5px; }
  button { min-width: 0; max-width: 100%; min-height: 26px; white-space: normal; border: 1px solid var(--splitty-border-subtle); border-radius: 3px; padding: 4px 6px; background: none; color: var(--splitty-text-secondary); font: inherit; cursor: pointer; }
  button.primary { background: var(--splitty-hover-bg); color: var(--splitty-text-primary); }
  .save-only { flex-basis: 100%; text-align: left; }
  button:hover:not(:disabled) { background: var(--splitty-hover-bg); color: var(--splitty-text-primary); }
  button:disabled { opacity: 0.6; cursor: default; }
  input:focus-visible, button:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: 2px; }
  @container (min-width: 240px) { .ports { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
