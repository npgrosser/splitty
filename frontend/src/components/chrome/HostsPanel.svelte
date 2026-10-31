<!--
  The drawer's Hosts panel: every host the window knows, connected or not,
  with the screens it runs, its ports and the local ports shared with it.
  The screen list itself shows only the screens open in the window; whatever
  else a host has lives here.
-->
<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import type { Screen } from "../../stores/screens.svelte.ts";
  import type { HostGroup } from "../../lib/remote/on_hosts.ts";
  import type { HostScreen } from "../../lib/remote/remote_socket.ts";
  import type { PortRow } from "../../lib/remote/ports.ts";
  import type { ShareRow } from "../../lib/remote/reverse_forwards.ts";
  import type { ConnKey } from "../../lib/connection/server_connection.ts";
  import { describeFailure, hostIndicator, hostTooltip } from "../../lib/remote/screen_status.ts";
  import { updateNote } from "../../lib/remote/update.ts";
  import { getLocalVersion } from "../../stores/connections.svelte.ts";
  import { showContextMenu } from "../../stores/ui.svelte.ts";
  import { MAX_HOST_ALIAS } from "../../lib/remote/host_preferences.ts";
  import { focusInlineForm } from "./inline_focus.ts";
  import HostPorts from "./HostPorts.svelte";
  import HostShares from "./HostShares.svelte";
  import ManualPortForm from "./ManualPortForm.svelte";
  import { hostSummary } from "./remote_view.ts";

  interface Props {
    groups: readonly HostGroup<Screen>[];
    portLists: readonly { target: string; rows: readonly PortRow[] }[];
    /** Per host the local ports shared with it. */
    shareLists?: readonly { target: string; rows: readonly ShareRow[] }[];
    portTargets: readonly string[];
    /** The host to unfold and scroll to; a new `seq` asks again. */
    focus: { key: ConnKey; seq: number } | null;
    activeScreenId: string;
    onShowScreen: (id: string) => void;
    onOpenHostScreen: (host: ConnKey, screen: HostScreen) => void;
    onConnectHost: (host: ConnKey) => void;
    /** Unfolding a host browses it, which connects it; folding releases it. */
    onBrowseHost: (host: ConnKey, expanded: boolean) => void;
    onNewHostScreen: (target: string) => void;
    onConnectNew: () => void;
    /** Open the host in a window of its own; only in the desktop app. */
    onOpenWindow?: (target: string) => void;
    /** Update the host's Splitty to this app's version; asks first. */
    onUpdateHost: (host: ConnKey) => void;
    onSetHostAlias?: (target: string, alias: string) => Promise<void>;
    onForgetHost?: (target: string) => Promise<void>;
  }

  let {
    groups, portLists, shareLists = [], portTargets, focus, activeScreenId,
    onShowScreen, onOpenHostScreen, onConnectHost, onBrowseHost, onNewHostScreen, onConnectNew, onOpenWindow, onUpdateHost, onSetHostAlias, onForgetHost,
  }: Props = $props();
  const id = $props.id();

  // A host with open screens starts unfolded, any other folded: unfolding
  // one connects it, and the panel connects nothing on its own.
  let unfolded = $state(new Set<ConnKey>());
  let folded = $state(new Set<ConnKey>());
  let addingPort = $state(false);
  let editing = $state<string | null>(null);
  let alias = $state("");
  let saving = $state<string | null>(null);
  let error = $state<{ target: string; message: string } | null>(null);
  let aliasTrigger: HTMLButtonElement | undefined;

  function hostMenu(event: MouseEvent, group: HostGroup<Screen>) {
    if (saving) return;
    const trigger = event.currentTarget as HTMLButtonElement;
    const rect = trigger.getBoundingClientRect();
    showContextMenu([
      { label: "Change display name…", action: () => { aliasTrigger = trigger; alias = group.alias ?? ""; error = null; editing = group.target; } },
      { label: "Remove from list", detail: "Keeps sessions and port forwards running", action: () => { void remove(group); } },
    ], rect.left, rect.bottom + 4);
  }

  function closeAlias() {
    const trigger = aliasTrigger;
    const restoreFocus = document.activeElement === document.body
      || !!listEl?.querySelector(".alias-form")?.contains(document.activeElement);
    editing = null;
    aliasTrigger = undefined;
    // The menu item that opened the editor is gone; return to its host button
    // once a completed save has re-enabled it. Keep any focus moved elsewhere.
    void tick().then(() => {
      if (restoreFocus && trigger?.isConnected && !trigger.disabled) trigger.focus();
    });
  }

  async function saveAlias(target: string) {
    if (saving) return;
    saving = target;
    error = null;
    try { await onSetHostAlias?.(target, alias); closeAlias(); }
    catch (cause) { error = { target, message: cause instanceof Error ? cause.message : String(cause) }; }
    finally { saving = null; }
  }

  async function remove(group: HostGroup<Screen>) {
    if (saving) return;
    saving = group.target;
    error = null;
    try {
      await onForgetHost?.(group.target);
      if (editing === group.target) { editing = null; aliasTrigger = undefined; }
      unfolded = new Set([...unfolded].filter((key) => key !== group.key));
    }
    catch (cause) { error = { target: group.target, message: cause instanceof Error ? cause.message : String(cause) }; }
    finally { saving = null; }
  }

  let listEl: HTMLElement | undefined = $state();

  function isOpen(group: HostGroup<Screen>): boolean {
    return unfolded.has(group.key) || (group.open.length > 0 && !folded.has(group.key));
  }

  function setOpen(group: HostGroup<Screen>, open: boolean) {
    const next = new Set(unfolded);
    const nextFolded = new Set(folded);
    if (open) { next.add(group.key); nextFolded.delete(group.key); }
    else { next.delete(group.key); nextFolded.add(group.key); }
    unfolded = next;
    folded = nextFolded;
    onBrowseHost(group.key, open);
  }

  // Only a new request moves the panel, not the host list changing under it.
  $effect(() => {
    const request = focus;
    if (!request) return;
    untrack(() => {
      const group = groups.find((g) => g.key === request.key);
      if (!group) return;
      if (!isOpen(group)) setOpen(group, true);
      void tick().then(() => {
        listEl?.querySelector(`[data-host="${CSS.escape(group.key)}"]`)?.scrollIntoView({ block: "nearest" });
      });
    });
  });

  // Closing the panel ends what it browsed; hosts with open screens stay.
  onDestroy(() => {
    for (const key of unfolded) onBrowseHost(key, false);
  });

  /** The version a connected host runs when it is not this app's; then it can be updated. */
  function olderVersion(group: HostGroup<Screen>): string | undefined {
    const version = group.state?.phase === "ready" ? group.state.ready?.daemon.version : undefined;
    return updateNote(version, getLocalVersion()) ? version : undefined;
  }

  function lastSeen(group: HostGroup<Screen>): string | undefined {
    return group.lastSeen ? new Date(group.lastSeen * 1000).toLocaleString() : undefined;
  }
</script>

<div class="hosts" bind:this={listEl}>
  <div class="hosts-head">
    <h2 class="title">Hosts</h2>
    <button class="connect" onclick={onConnectNew}>
      <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
      Connect…
    </button>
  </div>

  {#if groups.length === 0}
    <div class="empty">
      <p>No hosts yet.</p>
      <p class="muted">Connect to a machine over SSH to open screens there and forward its ports.</p>
    </div>
  {/if}

  {#each groups as group (group.key)}
    {@const open = isOpen(group)}
    {@const indicator = hostIndicator(group.state)}
    {@const ports = portLists.find((host) => host.target === group.target)?.rows ?? []}
    <section class="host" class:open data-host={group.key} aria-label={`Host ${group.target}`}>
      <div class="host-heading">
      <button
        class="host-head"
        aria-expanded={open}
        aria-controls={`${id}-${group.key}`}
        title={hostTooltip(group.target, group.state, getLocalVersion())}
        onclick={() => setOpen(group, !open)}
      >
        <span class="dot {indicator}" aria-hidden="true"></span>
        <span class="identity">
          <span class="target">{group.alias || group.target}</span>
          {#if group.alias}<span class="address">{group.target}</span>{/if}
          <span class="summary">{hostSummary(group)}</span>
        </span>
        <svg class="chevron" width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M6 4l4 4-4 4" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" stroke-linejoin="round" /></svg>
      </button>
      {#if onSetHostAlias && onForgetHost}
        <button class="more" aria-label={`More for host ${group.alias || group.target}`} aria-haspopup="menu" disabled={saving !== null} onclick={(event) => hostMenu(event, group)}>
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><circle cx="3.5" cy="8" r="1.2" /><circle cx="8" cy="8" r="1.2" /><circle cx="12.5" cy="8" r="1.2" /></svg>
        </button>
      {/if}
      </div>
      {#if editing === group.target}
        <form class="alias-form" aria-label={`Display name for ${group.target}`} use:focusInlineForm={() => { if (!saving) closeAlias(); }} onsubmit={(event) => { event.preventDefault(); void saveAlias(group.target); }}>
          <label>Display name<input bind:value={alias} placeholder={group.target} disabled={saving !== null} aria-describedby={`${id}-${group.key}-alias-hint`} /></label>
          <p class="note" id={`${id}-${group.key}-alias-hint`}>Optional, up to {MAX_HOST_ALIAS} characters. Leave empty to show the SSH address.</p>
          <div class="actions"><button class="button" disabled={saving !== null}>{saving === group.target ? "Saving…" : "Save"}</button><button class="button" type="button" disabled={saving !== null} onclick={closeAlias}>Cancel</button></div>
        </form>
      {/if}
      {#if error?.target === group.target}<p class="host-error" role="alert">{error.message}</p>{/if}
      <div id={`${id}-${group.key}`}>
        {#if open}
        <div class="host-body">
          {#if group.state?.phase === "connecting" || group.state?.phase === "reconnecting"}
            <p class="note" role="status">{group.state.progress.at(-1)?.message || "Connecting…"}</p>
          {:else if group.state?.error}
            <p class="note error" role="status" title={group.state.error.message}>{describeFailure(group.target, group.state.error).headline}</p>
            {#if group.state.error.hint}<p class="note">{group.state.error.hint}</p>{/if}
            <button class="button" onclick={() => onConnectHost(group.key)}>Retry</button>
          {/if}
          {#if olderVersion(group)}
            <div class="update">
              <p class="note">Runs Splitty {olderVersion(group)}; this app is {getLocalVersion()}. Updating ends its screens there.</p>
              <button class="button" onclick={() => onUpdateHost(group.key)}>Update…</button>
            </div>
          {/if}

          <h4 class="heading" title={group.stale ? lastSeen(group) : undefined}>
            Screens{#if group.stale && group.screens.length > 0}<span class="summary">{" · "}last seen</span>{/if}
          </h4>
          <ul class="screens">
            {#each group.open as screen (screen.id)}
              <li>
                <button class="screen" class:current={screen.id === activeScreenId} aria-current={screen.id === activeScreenId ? "true" : undefined} onclick={() => onShowScreen(screen.id)}>
                  <span class="name">{screen.name}</span>
                  <span class="meta">In window</span>
                </button>
              </li>
            {/each}
            {#each group.screens as hostScreen (hostScreen.id)}
              <li>
                <button class="screen" title={`Open ${hostScreen.name || hostScreen.id}`} onclick={() => onOpenHostScreen(group.key, hostScreen)}>
                  <span class="name">{hostScreen.name || hostScreen.id}</span>
                  <span class="meta">{hostScreen.panes} {hostScreen.panes === 1 ? "pane" : "panes"}</span>
                  <span class="open-hint">Open</span>
                </button>
              </li>
            {/each}
          </ul>
          {#if group.open.length === 0 && group.screens.length === 0}
            <p class="note">{group.state?.phase === "ready" ? "No screens on this host." : "No screens seen yet."}</p>
          {/if}
          <div class="actions">
            <button class="add" onclick={() => onNewHostScreen(group.target)}>
              <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
              New screen
            </button>
            {#if onOpenWindow}
              <button class="add" title={`Open ${group.target} in a window of its own`} onclick={() => onOpenWindow(group.target)}>
                <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" aria-hidden="true"><rect x="2.5" y="4.5" width="9" height="9" rx="1.5" /><path d="M6 2.5h6a1.5 1.5 0 0 1 1.5 1.5v6" stroke-linecap="round" /></svg>
                New window
              </button>
            {/if}
          </div>

          <HostPorts target={group.target} rows={ports} targets={portTargets} />
          <HostShares target={group.target} rows={shareLists.find((host) => host.target === group.target)?.rows ?? []} />
        </div>
        {/if}
      </div>
    </section>
  {/each}

  <div class="other">
    <button class="add" onclick={() => addingPort = !addingPort} aria-expanded={addingPort} aria-controls={`${id}-manual-port`} title="Forward a port of any SSH target, even without an open screen">
      Forward a port on another host…
    </button>
    <div id={`${id}-manual-port`}>
      {#if addingPort}<ManualPortForm targets={portTargets} onclose={() => addingPort = false} />{/if}
    </div>
  </div>
</div>

<style>
  .hosts { display: flex; flex-direction: column; gap: 6px; font-size: 12px; color: var(--splitty-text-secondary); }
  .hosts-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
  .title { margin: 0; font-size: 14px; font-weight: 600; color: var(--splitty-text-primary); letter-spacing: 0.01em; }
  .connect { display: inline-flex; align-items: center; gap: 6px; min-height: 28px; padding: 0 10px; border: 1px solid var(--splitty-border-default); border-radius: 6px; background: none; color: var(--splitty-text-primary); font: inherit; cursor: pointer; }
  .connect:hover { background: var(--splitty-hover-bg); }

  .empty p { margin: 0 0 6px; line-height: 1.5; }
  .muted { color: var(--splitty-text-muted); }

  .host { border: 1px solid var(--splitty-border-subtle); border-radius: 8px; }
  .host.open { border-color: var(--splitty-border-default); }
  .host-heading { display: flex; align-items: center; min-width: 0; }
  .host-head { flex: 1; min-width: 0; display: flex; align-items: center; gap: 10px; padding: 10px 12px; border: 0; border-radius: 8px; background: none; color: inherit; font: inherit; text-align: left; cursor: pointer; }
  .host-head:hover { background: var(--splitty-hover-bg); }
  .identity { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .target { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; color: var(--splitty-text-primary); }
  .address { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 11px; color: var(--splitty-text-muted); }
  .summary { font-size: 11px; color: var(--splitty-text-muted); }
  .chevron { flex-shrink: 0; color: var(--splitty-text-muted); transition: transform 120ms ease; }
  .host.open .chevron { transform: rotate(90deg); }
  .host-body { padding: 2px 12px 12px; }

  .dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; background: var(--splitty-text-muted); }
  .dot.connected { background: var(--splitty-success); }
  .dot.connecting, .dot.reconnecting { background: var(--splitty-warning); animation: pulse 1.2s ease-in-out infinite; }
  .dot.failed { background: var(--splitty-danger); }
  .dot.offline { opacity: 0.6; }
  .dot.idle { box-sizing: border-box; background: transparent; border: 1.5px solid var(--splitty-text-muted); }
  @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.35; } }

  .heading { margin: 10px 0 4px; font-size: 10.5px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--splitty-text-muted); }
  .heading .summary { font-size: inherit; font-weight: 400; letter-spacing: 0; text-transform: none; }
  .screens { list-style: none; margin: 0 -8px; padding: 0; }
  .screen { width: 100%; display: flex; align-items: center; gap: 8px; min-height: 28px; padding: 0 8px; border: 0; border-radius: 6px; background: none; color: var(--splitty-text-primary); font: inherit; text-align: left; cursor: pointer; }
  .screen:hover { background: var(--splitty-hover-bg); }
  .screen.current { background: var(--splitty-active-bg); }
  .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .meta { flex-shrink: 0; font-size: 11px; color: var(--splitty-text-muted); }
  .open-hint { flex-shrink: 0; font-size: 11px; color: var(--splitty-accent); opacity: 0; }
  .screen:hover .open-hint, .screen:focus-visible .open-hint { opacity: 1; }

  .note { margin: 6px 0; font-size: 11.5px; line-height: 1.5; color: var(--splitty-text-muted); overflow-wrap: anywhere; }
  .note.error { color: var(--splitty-danger); }
  .update { display: flex; align-items: center; gap: 10px; margin: 6px 0 2px; }
  .update .note { flex: 1; margin: 0; }
  .button { flex-shrink: 0; min-height: 26px; padding: 0 10px; border: 1px solid var(--splitty-border-default); border-radius: 6px; background: none; color: var(--splitty-text-primary); font: inherit; cursor: pointer; }
  .button:hover { background: var(--splitty-hover-bg); }

  .add { display: inline-flex; align-items: center; gap: 6px; min-height: 28px; margin: 2px 0 0 -8px; padding: 0 8px; border: 0; border-radius: 6px; background: none; color: var(--splitty-text-secondary); font: inherit; cursor: pointer; }
  .add:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }
  .actions { display: flex; flex-wrap: wrap; gap: 0 12px; }
  .other { margin-top: 6px; padding-left: 8px; font-size: 11.5px; }

  button:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: -2px; }
  @media (prefers-reduced-motion: reduce) {
    .dot { animation: none; }
    .chevron { transition: none; }
  }

  .more { display: flex; align-items: center; justify-content: center; flex-shrink: 0; width: 26px; height: 26px; margin-right: 8px; padding: 0; border: 0; border-radius: 5px; background: none; color: var(--splitty-text-muted); cursor: pointer; }
  .more:hover:not(:disabled) { background: var(--splitty-hover-bg); color: var(--splitty-text-primary); }
  .more:disabled { opacity: 0.5; cursor: default; }
  .alias-form { margin: 0 12px 12px; padding-top: 8px; border-top: 1px solid var(--splitty-border-subtle); }
  .alias-form label { display: flex; flex-direction: column; gap: 5px; font-size: 11px; }
  .alias-form input { box-sizing: border-box; width: 100%; min-width: 0; padding: 6px; border: 1px solid var(--splitty-border-default); border-radius: 5px; background: var(--splitty-surface-base); color: var(--splitty-text-primary); font: inherit; user-select: text; }
  .host-error { margin: 0 12px 12px; color: var(--splitty-danger); font-size: 11px; overflow-wrap: anywhere; }
  .more:focus-visible, .alias-form input:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: 1px; }
</style>
