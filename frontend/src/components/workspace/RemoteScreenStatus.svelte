<script lang="ts">
  import { describeFailure, offersUpdate, type RemoteScreenStatus } from "../../lib/remote/screen_status.ts";
  import type { HostScreen } from "../../lib/remote/remote_socket.ts";

  // What a remote screen shows inside itself while it is not plainly running
  // (`remote-screens`): the connection's progress, an error with its next
  // step (for an incompatible host, "Update now", which asks first), the
  // reopen offer, a dropped or unreachable host, a screen gone from its host.
  // Every string from the host or the hub is rendered as text.

  interface Props {
    status: RemoteScreenStatus;
    onCancel: () => void;
    onRetry: () => void;
    onOpen: (screens: HostScreen[]) => void;
    onCreateNew: () => void;
    onRemove: () => void;
    /** Ask whether to update the host's Splitty (an incompatible host). */
    onUpdate: () => void;
  }

  let { status, onCancel, onRetry, onOpen, onCreateNew, onRemove, onUpdate }: Props = $props();

  let selected = $state(new Set<string>());
  let showDetails = $state(false);
  let copied = $state(false);

  // The offer preselects the first screen, as the sketch does, and keeps the
  // user's choice until the host's list changes.
  const offerIds = $derived(JSON.stringify(status.kind === "offer" ? status.screens.map((s) => s.id) : []));
  $effect(() => {
    selected = new Set((JSON.parse(offerIds) as string[]).slice(0, 1));
  });

  const failure = $derived(status.kind === "failed" ? describeFailure(status.target, status.error) : undefined);

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  function openSelected() {
    if (status.kind !== "offer") return;
    const screens = status.screens.filter((s) => selected.has(s.id));
    if (screens.length > 0) onOpen(screens);
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      setTimeout(() => { copied = false; }, 1500);
    } catch {
      // No clipboard: the text stays selectable.
    }
  }

  function panes(n: number): string {
    return n === 1 ? "1 pane" : n > 1 ? `${n} panes` : "";
  }
</script>

{#if status.kind !== "none"}
  <div
    class="remote-status"
    class:remote-status-dim={status.kind === "reconnecting"}
    role={status.kind === "failed" ? "alert" : "status"}
    aria-live="polite"
  >
    <div class="remote-status-panel">
      {#if status.kind === "connecting"}
        <div class="remote-status-host">{status.target}</div>
        {#if status.progress.length > 0}
          <ol class="remote-status-steps">
            {#each status.progress as step, i}
              <li class:current={i === status.progress.length - 1}>{step.message}</li>
            {/each}
          </ol>
        {:else}
          <p class="remote-status-text">Connecting…</p>
        {/if}
        <div class="remote-status-actions">
          {#if status.fresh}
            <button class="remote-status-btn" onclick={onCancel}>Cancel</button>
          {:else}
            <button class="remote-status-btn" title="Remove from this window; it keeps running on {status.target}" onclick={onRemove}>Close</button>
          {/if}
        </div>

      {:else if status.kind === "failed" && failure}
        <div class="remote-status-host">{status.target}</div>
        <p class="remote-status-headline">{failure.headline}</p>
        {#if failure.message}
          <p class="remote-status-text remote-status-pre">{failure.message}</p>
        {/if}
        {#if failure.hint}
          <p class="remote-status-text remote-status-pre">{failure.hint}</p>
        {/if}
        {#if failure.details}
          <button class="remote-status-link" onclick={() => { showDetails = !showDetails; }}>
            {showDetails ? "Hide details" : "Details"}
          </button>
          {#if showDetails}
            <pre class="remote-status-details">{failure.details}</pre>
          {/if}
        {/if}
        <div class="remote-status-actions">
          <button class="remote-status-btn" onclick={onRemove}>Close</button>
          <button class="remote-status-btn" onclick={() => copy(failure.copy)}>{copied ? "Copied" : "Copy"}</button>
          {#if offersUpdate(status)}
            <button class="remote-status-btn" onclick={onRetry}>Retry</button>
            <button class="remote-status-btn remote-status-primary" onclick={onUpdate}>Update now…</button>
          {:else}
            <button class="remote-status-btn remote-status-primary" onclick={onRetry}>Retry</button>
          {/if}
        </div>

      {:else if status.kind === "offer"}
        <p class="remote-status-headline">
          {status.screens.length === 1 ? "1 screen is" : `${status.screens.length} screens are`} already running on {status.target}
        </p>
        <ul class="remote-status-offer">
          {#each status.screens as screen (screen.id)}
            <li>
              <label>
                <input type="checkbox" checked={selected.has(screen.id)} onchange={() => toggle(screen.id)} />
                <span class="remote-status-offer-name">{screen.name}</span>
                <span class="remote-status-offer-panes">{panes(screen.panes)}</span>
              </label>
            </li>
          {/each}
        </ul>
        <div class="remote-status-actions">
          <button class="remote-status-btn" onclick={onCreateNew}>Create new screen</button>
          <button class="remote-status-btn remote-status-primary" disabled={selected.size === 0} onclick={openSelected}>Open selected</button>
        </div>

      {:else if status.kind === "reconnecting"}
        <p class="remote-status-headline">Reconnecting to {status.target}…</p>
        {#if status.progress.length > 0}
          <ol class="remote-status-steps">
            {#each status.progress as step, i}
              <li class:current={i === status.progress.length - 1}>{step.message}</li>
            {/each}
          </ol>
        {/if}
        {#if status.error}
          <p class="remote-status-text">{describeFailure(status.target, status.error).headline}</p>
        {/if}
        <div class="remote-status-actions">
          <button class="remote-status-btn" onclick={onRetry}>Retry now</button>
        </div>

      {:else if status.kind === "offline"}
        <p class="remote-status-headline">{status.target} is offline</p>
        <p class="remote-status-text">
          {status.error ? describeFailure(status.target, status.error).headline : "The connection to the host is gone."}
          Its sessions keep running there.
        </p>
        <div class="remote-status-actions">
          <button class="remote-status-btn" onclick={onRemove}>Close</button>
          <button class="remote-status-btn remote-status-primary" onclick={onRetry}>Retry</button>
        </div>

      {:else if status.kind === "missing"}
        <p class="remote-status-headline">This screen no longer exists on {status.target}</p>
        <p class="remote-status-text">It was closed there, or the host's Splitty lost it.</p>
        <div class="remote-status-actions">
          <button class="remote-status-btn remote-status-primary" onclick={onRemove}>Remove</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
.remote-status {
  position: absolute;
  inset: 0;
  z-index: 5;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: var(--splitty-surface-base);
  color: var(--splitty-text-secondary);
  font-size: 13px;
}
.remote-status-dim {
  background: color-mix(in srgb, var(--splitty-surface-base) 70%, transparent);
}
.remote-status-panel {
  width: 460px;
  max-width: 100%;
  max-height: 100%;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 20px 24px;
  border: 1px solid var(--splitty-border-subtle);
  border-radius: 8px;
  background: var(--splitty-surface-raised);
}
.remote-status-host {
  font-family: "JetBrains Mono", ui-monospace, monospace;
  font-size: 12px;
  color: var(--splitty-text-muted);
  overflow-wrap: anywhere;
}
.remote-status-headline {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--splitty-text-primary);
  overflow-wrap: anywhere;
}
.remote-status-text {
  margin: 0;
  line-height: 1.5;
  overflow-wrap: anywhere;
}
.remote-status-pre {
  white-space: pre-wrap;
  user-select: text;
}
.remote-status-steps {
  margin: 0;
  padding: 0 0 0 18px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  color: var(--splitty-text-muted);
}
.remote-status-steps li.current {
  color: var(--splitty-text-primary);
}
.remote-status-details {
  margin: 0;
  max-height: 160px;
  overflow: auto;
  padding: 8px;
  border-radius: 4px;
  background: var(--splitty-hover-bg);
  font-size: 11px;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  user-select: text;
}
.remote-status-offer {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.remote-status-offer label {
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
}
.remote-status-offer-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--splitty-text-primary);
}
.remote-status-offer-panes {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--splitty-text-muted);
}
.remote-status-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 4px;
}
.remote-status-btn {
  background: transparent;
  border: 1px solid var(--splitty-border-default);
  border-radius: 4px;
  padding: 6px 14px;
  font: inherit;
  color: var(--splitty-text-primary);
  cursor: pointer;
}
.remote-status-btn:hover {
  background: var(--splitty-hover-bg);
}
.remote-status-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.remote-status-primary {
  background: var(--splitty-active-bg);
}
.remote-status-link {
  align-self: flex-start;
  background: none;
  border: none;
  padding: 0;
  font: inherit;
  font-size: 12px;
  color: var(--splitty-text-muted);
  text-decoration: underline;
  cursor: pointer;
}
</style>
