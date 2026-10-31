<script lang="ts" module>
  /** The dialog's colours; the setup page has no theme of its own. */
  export interface DialogColors {
    bg: string;
    /** A `#rrggbb` colour; the border is derived from it. */
    fg: string;
    danger: string;
  }
</script>

<script lang="ts">
  import { untrack } from "svelte";
  import { toOpenError, type KnownHost, type RemoteWindowsAPI } from "../../lib/remoteWindows.ts";
  import {
    formatLastConnected, hostRows, initialPickerState, moveSelection, pickerReducer, rowTarget,
    type HostRow, type PickerEvent, type PickerState,
  } from "../../lib/host_picker.ts";
  import "./dialog.css";

  interface Props {
    open: boolean;
    title: string;
    colors: DialogColors;
    /** The hosts to offer: the desktop shell's, or the local server's (`/api/remote/hosts`). */
    hosts: () => Promise<KnownHost[]>;
    /**
     * Opens the picked host while the dialog shows its progress and a failure
     * (a remote window).
     */
    opener?: Pick<RemoteWindowsAPI, "open" | "cancel" | "onProgress">;
    /**
     * Instead of an opener: takes the picked target and the dialog closes at
     * once (a remote screen shows its progress inside the screen).
     */
    onPick?: (target: string) => void;
    /** A host to open as soon as the dialog opens, showing its progress. */
    initialTarget?: string;
    onClose: () => void;
  }

  let { open, title, colors, hosts: listHosts, opener, onPick, initialTarget, onClose }: Props = $props();

  let picker = $state<PickerState>(initialPickerState);
  let hosts = $state<KnownHost[]>([]);
  let loading = $state(false);
  let query = $state("");
  let selected = $state(0);
  let inputEl: HTMLInputElement | undefined = $state();
  let retryEl: HTMLButtonElement | undefined = $state();

  const rows = $derived(hostRows(hosts, query));
  const active = $derived(Math.min(selected, rows.length - 1));

  function dispatch(event: PickerEvent): PickerState {
    picker = pickerReducer(picker, event);
    return picker;
  }

  $effect(() => {
    if (!open) return;
    // The attempt counter survives a reopen, so a late result of an open
    // cancelled by closing the dialog cannot match a new one.
    picker = { ...initialPickerState, attempt: untrack(() => picker.attempt) };
    query = "";
    selected = 0;
    hosts = [];
    loading = true;

    let alive = true;
    untrack(listHosts)
      .then((found) => { if (alive) hosts = found; })
      .catch((error: unknown) => { console.warn("could not list known hosts", error); })
      .finally(() => { if (alive) loading = false; });
    const from = untrack(() => opener);
    const unsubscribe = from?.onProgress((progress) => dispatch({ type: "progress", progress }));
    const start = untrack(() => initialTarget);
    if (start) void untrack(() => openTarget(start));

    return () => {
      alive = false;
      unsubscribe?.();
      // Closing the dialog ends an open it started.
      if (picker.phase === "opening") void from?.cancel(picker.target);
    };
  });

  $effect(() => {
    if (!open) return;
    // At once, so keys typed right after opening reach the dialog and not the
    // terminal, and once more after a frame, should what opened it (the
    // command palette) give the focus back as it closes.
    const target = picker.phase === "choosing" ? inputEl : picker.phase === "failed" ? retryEl : undefined;
    target?.focus();
    requestAnimationFrame(() => target?.focus());
  });

  async function openTarget(target: string) {
    if (onPick) {
      onClose();
      onPick(target);
      return;
    }
    if (!opener) return;
    const previous = picker.attempt;
    const { attempt } = dispatch({ type: "open", target });
    // Not started: another open is in flight.
    if (attempt === previous) return;
    try {
      await opener.open(target);
      if (dispatch({ type: "succeeded", attempt }).phase === "done") onClose();
    } catch (cause) {
      dispatch({ type: "failed", attempt, error: toOpenError(cause) });
    }
  }

  function cancel() {
    if (picker.phase !== "opening") return;
    void opener?.cancel(picker.target);
    dispatch({ type: "cancel" });
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      if (picker.phase === "opening") cancel();
      else if (picker.phase === "failed") dispatch({ type: "back" });
      else onClose();
      return;
    }
    if (picker.phase !== "choosing") return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      selected = moveSelection(active, e.key === "ArrowDown" ? 1 : -1, rows.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const row = rows[active];
      if (row) void openTarget(rowTarget(row));
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) onClose();
  }

  function detail(row: HostRow): string {
    if (row.kind === "typed") return "connect";
    if (row.host.source === "ssh-config") return "from ~/.ssh/config";
    return row.host.lastConnected !== undefined ? formatLastConnected(row.host.lastConnected) : "recent";
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="dialog-backdrop"
    role="presentation"
    style="--dialog-bg:{colors.bg};--dialog-fg:{colors.fg};--dialog-border:{colors.fg}33;--dialog-danger:{colors.danger}"
    onclick={handleBackdropClick}
  >
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="dialog-panel dialog-panel-compact host-picker"
      role="dialog"
      aria-modal="true"
      aria-labelledby="host-picker-title"
      tabindex="-1"
      onkeydown={handleKeydown}
    >
      <h2 id="host-picker-title" class="dialog-title dialog-title-sm">{title}</h2>

      {#if picker.phase === "choosing"}
        <input
          bind:this={inputEl}
          bind:value={query}
          oninput={() => { selected = 0; }}
          type="text"
          placeholder="user@host[:port]"
          aria-label="Host"
          spellcheck="false"
          autocomplete="off"
          class="dialog-input dialog-input-sm"
        />

        <div class="template-list" role="listbox" aria-label="Known hosts">
          {#if loading && rows.length === 0}
            <p class="dialog-text dialog-text-center dialog-text-muted">Loading hosts…</p>
          {:else if rows.length === 0}
            <p class="dialog-text dialog-text-center dialog-text-muted">
              {query.trim() ? "Not a target: use [user@]host[:port]." : "No known hosts yet. Type user@host to connect."}
            </p>
          {:else}
            {#each rows as row, i}
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <div
                class="template-item"
                class:template-item-selected={i === active}
                role="option"
                aria-selected={i === active}
                tabindex="-1"
                onmouseenter={() => { selected = i; }}
                onclick={() => openTarget(rowTarget(row))}
              >
                <span class="host-picker-identity"><span class="template-item-name">{row.kind === "known" ? row.host.alias || row.host.target : row.target}</span>{#if row.kind === "known" && row.host.alias}<span class="host-picker-address">{row.host.target}</span>{/if}</span>
                <span class="host-picker-detail">{detail(row)}</span>
              </div>
            {/each}
          {/if}
        </div>
      {:else}
        <p class="dialog-text">
          {picker.phase === "failed" ? "Could not open" : "Connecting to"} <strong>{picker.target}</strong>{picker.phase === "failed" ? "" : "…"}
        </p>

        {#if picker.progress.length > 0}
          <ol class="host-picker-progress" aria-live="polite">
            {#each picker.progress as line, i}
              <li class:host-picker-progress-current={picker.phase === "opening" && i === picker.progress.length - 1}>{line}</li>
            {/each}
          </ol>
        {/if}

        {#if picker.phase === "failed" && picker.error}
          <p class="dialog-error host-picker-message" role="alert">{picker.error.message}</p>
          {#if picker.error.hint}
            <p class="dialog-text host-picker-message">{picker.error.hint}</p>
          {/if}
        {/if}

        <div class="dialog-actions">
          {#if picker.phase === "opening"}
            <button class="dialog-btn" onclick={cancel}>Cancel</button>
          {:else}
            <button class="dialog-btn" onclick={() => dispatch({ type: "back" })}>Back</button>
            <button bind:this={retryEl} class="dialog-btn-primary" onclick={() => openTarget(picker.target)}>Retry</button>
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}
