<script lang="ts">
  import { toast } from "svelte-sonner";
  import { onMount } from "svelte";
  import {
    getScreens, getActiveScreenId, getHostOrder, createScreen, renameScreen,
    setActiveScreen, moveScreenInGroup, subscribe as subscribeScreens, type Screen,
  } from "../../stores/screens.svelte.ts";
  import {
    getDrawerState, setDrawerState, isDrawerPinned, setDrawerPinned,
    setDrawerDragging, setDrawerLiveWidth, setRemoteScreenPickerOpen, setHostPickerOpen, showContextMenu,
    type DrawerState, type ContextMenuItem,
  } from "../../stores/ui.svelte.ts";
  import { shortcutKey } from "../../lib/shortcuts.ts";
  import { edgeReveal } from "../../actions/edge_reveal.ts";
  import { dragResize } from "../../actions/drag_resize.ts";
  import { swipeWheel } from "../../actions/swipe_wheel.ts";
  import { settings, updateSetting } from "../../stores/settings.svelte.ts";
  import { defaults } from "../../lib/settings.ts";
  import SettingsPanel from "./SettingsPanel.svelte";
  import type { DockviewApi } from "dockview-core";
  import { sessionForPane } from "../../lib/connection/screen_binding.ts";
  import { unreadCountForScreen, unreadForPane, unreadTitlesForScreen } from "../../stores/notifications.svelte.ts";
  import { markPaneRead } from "../../lib/notifications.ts";
  import { hostIndicator, hostTooltip, type HostIndicator } from "../../lib/remote/screen_status.ts";
  import { terminateConfirmedBefore } from "../../lib/remote/close_screen.ts";
  import { hostGroups, type HostGroup } from "../../lib/remote/on_hosts.ts";
  import type { HostCatalog } from "../../lib/remote/host_directory.ts";
  import type { HostPreference } from "../../lib/remote/host_preferences.ts";
  import type { KnownHost } from "../../lib/remote/known_hosts.ts";
  import { orderedScreens } from "../../lib/remote/screen_groups.ts";
  import type { HostScreen } from "../../lib/remote/remote_socket.ts";
  import { hostOf, type ConnKey } from "../../lib/connection/server_connection.ts";
  import { getLocalVersion, hostViews } from "../../stores/connections.svelte.ts";
  import { hostPortLists, portForwarding } from "../../stores/ports.svelte.ts";
  import HostsPanel from "./HostsPanel.svelte";
  import { remoteWindows } from "../../lib/remoteWindows.ts";
  import { unclaimedEscape } from "./inline_focus.ts";
  import SortableList from "../ui/SortableList.svelte";

  // ── Props ──
  interface Props {
    screenApis: ReadonlyMap<string, DockviewApi>;
    paneVersion?: number;
    /** Close a screen: a local one as the window does, a remote one detaches. */
    onCloseScreen: (id: string) => void;
    /** End a remote screen's panes and processes on its host. */
    onTerminateScreen: (id: string) => void;
    onActivatePane: (screenId: string, paneId: string) => void;
    /** Open a screen of a connected host that the window does not show. */
    onOpenHostScreen: (host: ConnKey, screen: HostScreen) => void;
    /** Connect a host whose screens the window shows but has not connected yet. */
    onConnectHost: (host: ConnKey) => void;
    onBrowseHost: (host: ConnKey, expanded: boolean) => void;
    /** `direct` creates the screen without first offering the host's other screens. */
    onNewHostScreen: (target: string, direct: boolean) => void;
    /** Update a host's Splitty to this app's version, after the user confirmed what ends. */
    onUpdateHost: (host: ConnKey) => void;
    catalog: readonly HostCatalog[];
    recentHosts: readonly KnownHost[];
    preferences?: readonly HostPreference[];
    onSetHostAlias?: (target: string, alias: string) => Promise<void>;
    onForgetHost?: (target: string) => Promise<void>;
    onRevealHost?: (target: string) => Promise<void>;
    /** The user dragged a host group from `from` to `to` among the shown hosts, `shown`. */
    onMoveHostGroup?: (shown: string[], from: number, to: number) => void;
  }

  let { screenApis, paneVersion, onCloseScreen, onTerminateScreen, onActivatePane, onOpenHostScreen, onConnectHost, onBrowseHost, onNewHostScreen, onUpdateHost, catalog, recentHosts, preferences = [], onSetHostAlias, onForgetHost, onRevealHost, onMoveHostGroup }: Props = $props();

  // ── State ──
  let screens = $state(orderedScreens(getScreens(), getHostOrder()));
  let activeScreenId = $state(getActiveScreenId());
  let editingId = $state<string | null>(null);
  let editValue = $state("");
  let collapsedScreens = $state(new Set<string>());
  let inputEl: HTMLInputElement | undefined = $state();

  const drawerState = $derived(getDrawerState());
  const pinned = $derived(isDrawerPinned());
  const isOpen = $derived(pinned || drawerState !== "closed");
  const activePanel = $derived(
    drawerState === "settings" || drawerState === "templates" || drawerState === "hosts"
      ? drawerState : null
  );
  const portLists = $derived(hostPortLists());
  // Host groups outlive their connections and include last-seen directories.
  // The screen list shows a host only while the window shows its screens;
  // every host is in the Hosts panel.
  const groups = $derived(hostGroups(screens, hostViews(), recentHosts, catalog, portLists, preferences, getHostOrder()));
  const openGroups = $derived(groups.filter((group) => group.open.length > 0));
  const localScreens = $derived(screens.filter((s) => s.conn === "local"));
  // "Ports": per host its listed ports and the local server's forwards
  // (`lib/remote/port_forwarding.ts`).
  const portTargets = $derived([...new Set([...groups.map((g) => g.target), ...recentHosts.map((h) => h.target)])]);
  $effect(() => {
    const controller = portForwarding();
    controller?.setViewOpen(isOpen);
    return () => controller?.setViewOpen(false);
  });

  // ── Hosts ──
  let hostFocus = $state<{ key: ConnKey; seq: number } | null>(null);

  async function showHost(key: ConnKey) {
    const group = groups.find((group) => group.key === key);
    if (group?.hidden) {
      try { await onRevealHost?.(group.target); }
      catch (cause) { toast.error("Could not add host", { description: cause instanceof Error ? cause.message : String(cause) }); return; }
    }
    hostFocus = { key, seq: (hostFocus?.seq ?? 0) + 1 };
    setDrawerState("hosts");
  }

  function closePanel() {
    setDrawerState(pinned ? "closed" : "open");
  }

  function hostState(conn: ConnKey) {
    return hostViews().find((h) => h.key === conn)?.state;
  }

  /** What a host's caption says besides its dot; nothing while all is well. */
  const CAPTION_NOTES: Partial<Record<HostIndicator, string>> = {
    connecting: "connecting…", reconnecting: "reconnecting…", failed: "failed", offline: "offline",
  };

  /** Screens of a host the window cannot reach are dimmed. */
  function unreachable(screen: Screen): boolean {
    if (screen.conn === "local") return false;
    const indicator = hostIndicator(hostState(screen.conn));
    return indicator === "failed" || indicator === "offline";
  }

  // ── Ports menu ──
  // A host's ports at a click: forwarded ones open, detected ones are offered;
  // the Hosts panel has every control.
  function portsOf(target: string) {
    return portLists.find((host) => host.target === target)?.rows ?? [];
  }

  function openPortsMenu(e: MouseEvent, group: HostGroup) {
    e.stopPropagation();
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const controller = portForwarding();
    const items: ContextMenuItem[] = portsOf(group.target).slice(0, 12).map((row) => {
      const label = [String(row.port), row.saved?.label || row.process].filter(Boolean).join(" · ");
      const busy = row.pending !== undefined;
      const forward = row.forward;
      if (forward) {
        return {
          label, detail: `localhost:${forward.local}`, badge: forward.state === "up" ? "Open" : "reconnecting…",
          disabled: busy, action: () => controller?.open(forward),
        };
      }
      return {
        label, detail: row.saved?.enabled === false ? "Paused · forward and open" : "Forward and open",
        disabled: busy, action: () => { void controller?.forward(group.target, row.port, { open: true }); },
      };
    });
    if (items.length === 0) items.push({ label: "No ports detected", disabled: true, action: () => {} });
    items.push({ label: "", action: () => {}, separator: true }, { label: "Manage ports…", action: () => showHost(group.key) });
    showContextMenu(items, rect.left, rect.bottom + 4);
  }

  // ── New screen ──
  function openNewMenu(e: MouseEvent) {
    e.stopPropagation();
    const row = (e.currentTarget as HTMLElement).parentElement ?? (e.currentTarget as HTMLElement);
    const rect = row.getBoundingClientRect();
    const separator: ContextMenuItem = { label: "", action: () => {}, separator: true };
    const hosts: ContextMenuItem[] = groups.filter((group) => !group.hidden).slice(0, 8).map((group) => ({
      label: group.alias || group.target,
      detail: group.alias ? group.target : group.state?.phase === "ready" ? "connected" : undefined,
      action: () => onNewHostScreen(group.target, false),
    }));
    showContextMenu([
      { label: "Local screen", shortcut: shortcutKey("newScreen"), action: () => { createScreen(); } },
      separator,
      ...hosts,
      { label: hosts.length ? "Other host…" : "On a host…", action: () => setRemoteScreenPickerOpen(true) },
      // A window of its own for one host, as a remote window of the desktop app.
      ...(remoteWindows.available ? [separator, { label: "New window on a host…", action: () => setHostPickerOpen(true) }] : []),
    ], rect.left, rect.bottom + 4);
  }

  // ── Sidebar resize ──
  const resizable = $derived(settings().drawer.resizable);
  const sidebarWidth = $derived(resizable ? settings().drawer.sidebarWidth : defaults.drawer.sidebarWidth);
  let liveWidth = $state(0); // 0 = use settings value
  const effectiveWidth = $derived(liveWidth || sidebarWidth);

  const dragResizeParams = $derived({
    min: 150,
    max: 400,
    getWidth: () => effectiveWidth,
    onDragStart: () => {
      liveWidth = sidebarWidth;
      setDrawerDragging(true);
      setDrawerLiveWidth(liveWidth);
    },
    onDrag: (w: number) => {
      liveWidth = w;
      setDrawerLiveWidth(w);
    },
    onDragEnd: (w: number) => {
      setDrawerDragging(false);
      setDrawerLiveWidth(0);
      liveWidth = 0;
      updateSetting("drawer", "sidebarWidth", w);
    },
    onReset: () => {
      liveWidth = 0;
      setDrawerLiveWidth(0);
      updateSetting("drawer", "sidebarWidth", defaults.drawer.sidebarWidth);
    },
  });

  // ── Leading icon extraction ──
  const LEADING_ICON_RE = /^([^\w\s\x00-\x7F]) /;
  function splitTitleIcon(title: string): { icon: string | null; label: string } {
    const m = title.match(LEADING_ICON_RE);
    if (m) return { icon: m[1], label: title.slice(m[0].length) };
    return { icon: null, label: title };
  }

  // ── Pane info types ──
  interface DrawerPaneInfo {
    paneId: string;
    groupId: string;
    title: string;
    isBaseShell: boolean;
  }

  // ── Collect pane info for a screen ──
  function getPanesForScreen(screenId: string, _version?: number): DrawerPaneInfo[] {
    const api = screenApis.get(screenId);
    if (!api) return [];
    return api.panels.map((panel) => {
      const session = sessionForPane(api, panel.id);
      return {
        paneId: panel.id,
        groupId: String((panel.group as any)?.id ?? ""),
        title: panel.title ?? "Terminal",
        isBaseShell: !session || session.lastIsBaseShell,
      };
    });
  }

  // ── Group consecutive panes by groupId ──
  function groupPanes(panes: DrawerPaneInfo[]): { groupId: string; panes: DrawerPaneInfo[] }[] {
    const groups: { groupId: string; panes: DrawerPaneInfo[] }[] = [];
    for (const pane of panes) {
      const last = groups[groups.length - 1];
      if (last && last.groupId === pane.groupId) {
        last.panes.push(pane);
      } else {
        groups.push({ groupId: pane.groupId, panes: [pane] });
      }
    }
    return groups;
  }

  // ── Screen subscription ──
  onMount(() => {
    const unsub = subscribeScreens(() => {
      screens = orderedScreens(getScreens(), getHostOrder());
      activeScreenId = getActiveScreenId();
    });
    return unsub;
  });

  // ── End a remote screen ──
  // × detaches and never asks; the second button ends the screen on its host,
  // after an inline question in its row the first time.
  let endingId = $state<string | null>(null);

  function canEnd(screen: Screen): boolean {
    return screen.conn !== "local" && hostViews().some((h) => h.key === screen.conn && h.state.phase === "ready");
  }

  function endScreen(screen: Screen) {
    if (terminateConfirmedBefore()) onTerminateScreen(screen.id);
    else endingId = screen.id;
  }

  // ── Rename ──
  function startRename(id: string) {
    const screen = screens.find((s) => s.id === id);
    editingId = id;
    editValue = screen?.name ?? "";
    setTimeout(() => inputEl?.select(), 0);
  }

  function commitRename() {
    if (editingId && editValue.trim()) {
      renameScreen(editingId, editValue.trim());
    }
    editingId = null;
  }

  // ── Edge reveal ──
  const edgeRevealParams = $derived({
    enabled: settings().drawer.edgeReveal && !pinned && drawerState === "closed",
    onReveal: () => setDrawerState("open"),
  });

  // ── Nav click ──
  function handleNavClick(view: DrawerState) {
    if (drawerState === view) {
      setDrawerState(pinned ? "closed" : "open");
    } else {
      setDrawerState(view);
    }
  }

  // ── Sidebar wheel ──
  // The wheel anywhere on the sidebar switches screens, so the list never
  // scrolls by wheel; the active screen is kept in view instead.
  let listEl: HTMLElement | undefined = $state();
  $effect(() => {
    const id = activeScreenId;
    listEl?.querySelector(`[data-screen="${CSS.escape(id)}"]`)?.scrollIntoView({ block: "nearest" });
  });

  function handleSwipe(dir: "up" | "down") {
    if (screens.length < 2) return;
    const idx = screens.findIndex((s) => s.id === activeScreenId);
    const next = dir === "down"
      ? Math.min(idx + 1, screens.length - 1)
      : Math.max(idx - 1, 0);
    if (next !== idx) setActiveScreen(screens[next].id);
  }

  // ── Escape handling ──
  onMount(() => {
    function handler(e: KeyboardEvent) {
      if (!isOpen || !unclaimedEscape(e)) return;

      if (editingId !== null || endingId !== null) {
        e.preventDefault();
        e.stopPropagation();
        editingId = null;
        endingId = null;
        return;
      }

      if (drawerState === "templates") return; // this panel handles its own escape

      if (activePanel) {
        e.preventDefault();
        e.stopPropagation();
        setDrawerState(pinned ? "closed" : "open");
      } else if (!pinned) {
        e.preventDefault();
        e.stopPropagation();
        setDrawerState("closed");
      }
    }
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  });

</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div use:edgeReveal={edgeRevealParams} style="display:contents">
  <!-- Backdrop -->
  {#if isOpen && !pinned}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="drawer-backdrop visible"
      onclick={() => setDrawerState("closed")}
    ></div>
  {:else if pinned && activePanel}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="drawer-backdrop panel-visible"
      onclick={() => setDrawerState("closed")}
    ></div>
  {/if}

  <!-- Drawer -->
  <div class="drawer" class:open={isOpen} class:pinned style:--drawer-sidebar-width="{effectiveWidth}px">
    <!-- Sidebar -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="drawer-sidebar"
      onclick={() => { if (activePanel) closePanel(); }}
      use:swipeWheel={{ onSwipe: handleSwipe }}
    >
      <div class="drawer-head">
        <span class="drawer-head-label">Screens</span>
        <button
          class="drawer-icon-btn drawer-pin-btn"
          class:active={pinned}
          onclick={(e) => { e.stopPropagation(); setDrawerPinned(!pinned); }}
          title={pinned ? "Unpin drawer" : "Pin drawer"}
          aria-label={pinned ? "Unpin drawer" : "Pin drawer"}
          aria-pressed={pinned}
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
            <path d="M10.97 2.29a1 1 0 0 0-1.41 0L6.05 5.8a2.2 2.2 0 0 1-.94.55L3.37 6.9a.5.5 0 0 0-.24.83l5.14 5.14a.5.5 0 0 0 .83-.24l.55-1.74c.1-.34.29-.64.55-.94l3.51-3.51a1 1 0 0 0 0-1.41L10.97 2.29zM7 12.59l-1.3 1.3a.5.5 0 0 1-.7 0L3.11 12a.5.5 0 0 1 0-.7L4.41 10 7 12.59z" />
          </svg>
        </button>
      </div>

      {#snippet serverIcon(size: number)}
        <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" aria-hidden="true">
          <rect x="2.5" y="2.5" width="11" height="4.5" rx="1.2" /><rect x="2.5" y="9" width="11" height="4.5" rx="1.2" />
          <circle cx="5" cy="4.75" r="0.5" fill="currentColor" /><circle cx="5" cy="11.25" r="0.5" fill="currentColor" />
        </svg>
      {/snippet}

      {#snippet plusIcon()}
        <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
      {/snippet}

      {#snippet screenList(items: Screen[], conn: ConnKey)}
      <SortableList
        items={items}
        key={(s) => s.id}
        onReorder={(from, to) => moveScreenInGroup(conn, from, to)}
        class="drawer-screen-list"
      >
        {#snippet item(screen, { dragging })}
          {@const panes = getPanesForScreen(screen.id, paneVersion)}
          {@const paneGroups = groupPanes(panes)}
          {@const multiGroup = paneGroups.length > 1}
          {@const isActive = screen.id === activeScreenId}
          {@const isCollapsed = collapsedScreens.has(screen.id)}
          {@const unread = unreadCountForScreen(screen.conn, screen.id)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="drawer-screen"
            class:active={isActive}
            class:unreachable={unreachable(screen)}
            data-screen={screen.id}
            style:opacity={dragging ? 0.5 : undefined}
            onclick={() => setActiveScreen(screen.id)}
          >
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="drawer-screen-row"
              ondblclick={(e) => { e.preventDefault(); startRename(screen.id); }}
            >
              <span class="drawer-screen-index">{screens.findIndex((s) => s.id === screen.id) + 1}</span>
              {#if editingId === screen.id}
                <input
                  bind:this={inputEl}
                  class="drawer-screen-input"
                  aria-label="Screen name"
                  bind:value={editValue}
                  onblur={commitRename}
                  onkeydown={(e) => {
                    if (e.key === "Enter") commitRename();
                    if (e.key === "Escape") { e.stopPropagation(); editingId = null; }
                  }}
                  onclick={(e) => e.stopPropagation()}
                />
              {:else}
                <button class="drawer-screen-name" title={screen.name} aria-current={isActive ? "true" : undefined}
                  onclick={(e) => { e.stopPropagation(); setActiveScreen(screen.id); if (activePanel) closePanel(); }}
                  onkeydown={(e) => { if (e.key === "F2") { e.preventDefault(); startRename(screen.id); } }}
                >{screen.name}</button>
              {/if}
              {#if unread > 0}
                <span class="drawer-screen-badge" title={`${unread} unread: ${unreadTitlesForScreen(screen.conn, screen.id).slice(0, 5).join(" · ")}`}>{unread}</span>
              {/if}
              {#if endingId !== screen.id && editingId !== screen.id}
                <span class="drawer-screen-actions">
                  {#if panes.length > 1}
                    <button
                      class="drawer-icon-btn drawer-screen-toggle"
                      class:collapsed={isCollapsed}
                      onclick={(e) => {
                        e.stopPropagation();
                        const next = new Set(collapsedScreens);
                        if (next.has(screen.id)) next.delete(screen.id);
                        else next.add(screen.id);
                        collapsedScreens = next;
                      }}
                      title={isCollapsed ? "Show panes" : "Hide panes"}
                      aria-label={isCollapsed ? `Show panes in ${screen.name}` : `Hide panes in ${screen.name}`}
                      aria-expanded={!isCollapsed}
                    >
                      <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" stroke-linejoin="round" /></svg>
                    </button>
                  {/if}
                  {#if canEnd(screen)}
                    <button
                      class="drawer-icon-btn drawer-screen-end"
                      title="End on {hostOf(screen.conn)}: stops its terminals and their processes"
                      aria-label={`End ${screen.name} on its host`}
                      onclick={(e) => { e.stopPropagation(); endScreen(screen); }}
                    >
                      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" aria-hidden="true"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5M7 7v4M9 7v4" /></svg>
                    </button>
                  {/if}
                  <button
                    class="drawer-icon-btn drawer-screen-close"
                    title={screen.conn === "local" ? "Close screen" : "Detach: leave the window, keeps running on the host"}
                    aria-label={`Close ${screen.name}`}
                    onclick={(e) => { e.stopPropagation(); onCloseScreen(screen.id); }}
                  >
                    <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 4.5l7 7M11.5 4.5l-7 7" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" /></svg>
                  </button>
                </span>
              {/if}
            </div>
            {#if endingId === screen.id}
              <div class="drawer-screen-ask" role="group" aria-label="End {screen.name}">
                <span class="drawer-screen-ask-text">End it on {hostOf(screen.conn)}, with its processes?</span>
                <span class="drawer-screen-ask-actions">
                  <button class="drawer-screen-ask-btn danger" onclick={(e) => { e.stopPropagation(); endingId = null; onTerminateScreen(screen.id); }}>End</button>
                  <button class="drawer-screen-ask-btn" onclick={(e) => { e.stopPropagation(); endingId = null; }}>Keep</button>
                </span>
              </div>
            {/if}
            {#snippet paneItem(pane: DrawerPaneInfo, screen: Screen)}
              {@const { icon, label } = splitTitleIcon(pane.title || "shell")}
              {@const paneNotif = unreadForPane(screen.conn, pane.paneId)}
              <button
                class="drawer-pane-item"
                class:running={!pane.isBaseShell}
                class:idle={pane.isBaseShell}
                title={pane.title || "shell"}
                onclick={(e) => { e.stopPropagation(); markPaneRead(screen.conn, pane.paneId); onActivatePane(screen.id, pane.paneId); }}
              >
                {#if icon}
                  <span class="drawer-pane-icon">{icon}</span>
                {:else}
                  <span class="drawer-pane-dot"></span>
                {/if}
                <span class="drawer-pane-label">{label}</span>
                {#if paneNotif}
                  <span class="drawer-pane-badge {paneNotif.severity}" class:pulse={pane.isBaseShell}></span>
                {/if}
              </button>
            {/snippet}
            {#if panes.length > 0}
              <div class="drawer-screen-panes" class:collapsed={isCollapsed} inert={isCollapsed}>
                {#each paneGroups as group}
                  {@const isStack = multiGroup && group.panes.length > 1}
                  {#if isStack}
                    <div class="drawer-pane-stack">
                      {#each group.panes as pane}
                        {@render paneItem(pane, screen)}
                      {/each}
                    </div>
                  {:else}
                    {#each group.panes as pane}
                      {@render paneItem(pane, screen)}
                    {/each}
                  {/if}
                {/each}
              </div>
            {/if}
          </div>
        {/snippet}
      </SortableList>
      {/snippet}

      <div class="drawer-screens" bind:this={listEl}>
        <section aria-label="Local screens">
          {@render screenList(localScreens, "local")}
        </section>
        <!-- A host group moves by its caption; its screens move inside it. -->
        <SortableList
          items={openGroups}
          key={(group) => group.key}
          handle=".drawer-host-caption"
          onReorder={(from, to) => onMoveHostGroup?.(openGroups.map((group) => group.target), from, to)}
        >
          {#snippet item(group, { dragging })}
          {@const indicator = hostIndicator(group.state)}
          {@const ports = portsOf(group.target)}
          <section class="drawer-host" aria-label={`Screens on ${group.target}`} style:opacity={dragging ? 0.5 : undefined}>
            <div class="drawer-host-caption">
              <button
                class="drawer-host-name"
                title={`${hostTooltip(group.target, group.state, getLocalVersion())}\nShow screens and ports`}
                onclick={(e) => { e.stopPropagation(); showHost(group.key); }}
              >
                {@render serverIcon(13)}
                <span class="drawer-host-target">{group.alias || group.target}</span>
                <span class="drawer-host-dot {indicator}" aria-label={indicator === "idle" ? "not connected" : indicator}></span>
                {#if CAPTION_NOTES[indicator]}<span class="drawer-host-note">{CAPTION_NOTES[indicator]}</span>{/if}
              </button>
              {#if ports.length > 0}
                {@const forwarded = ports.filter((row) => row.forward?.state === "up").length}
                <button
                  class="drawer-host-ports"
                  class:forwarded={forwarded > 0}
                  title={forwarded > 0 ? `${forwarded} of ${ports.length} ports forwarded` : `${ports.length} ${ports.length === 1 ? "port" : "ports"} detected`}
                  aria-label={`Ports on ${group.target}`}
                  aria-haspopup="menu"
                  onclick={(e) => openPortsMenu(e, group)}
                >
                  <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.5 5.5h10l-2.5-2.5M13.5 10.5h-10l2.5 2.5" /></svg>
                  <span>{forwarded > 0 ? forwarded : ports.length}</span>
                </button>
              {/if}
              <button
                class="drawer-icon-btn drawer-host-add"
                title={`New screen on ${group.target}`}
                aria-label={`New screen on ${group.target}`}
                onclick={(e) => { e.stopPropagation(); onNewHostScreen(group.target, true); }}
              >{@render plusIcon()}</button>
            </div>
            {@render screenList(group.open, group.key)}
          </section>
          {/snippet}
        </SortableList>
      </div>

      <div class="drawer-new">
        <button class="drawer-new-main" title={`New screen (${shortcutKey("newScreen")})`} onclick={(e) => { e.stopPropagation(); createScreen(); }}>
          <span class="drawer-new-icon">{@render plusIcon()}</span>
          <span>New screen</span>
        </button>
        <button class="drawer-new-more" title="New screen on a host…" aria-label="New screen on a host…" aria-haspopup="menu" onclick={openNewMenu}>
          <svg width="12" height="12" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4" stroke="currentColor" stroke-width="1.6" fill="none" stroke-linecap="round" stroke-linejoin="round" /></svg>
        </button>
      </div>

      <div class="drawer-spacer"></div>

      <nav class="drawer-nav">
        <button
          class="drawer-nav-item"
          class:active={drawerState === "hosts"}
          onclick={(e) => { e.stopPropagation(); handleNavClick("hosts"); }}
        >
          <span class="drawer-nav-icon">{@render serverIcon(15)}</span>
          <span>Hosts</span>
        </button>
        <button
          class="drawer-nav-item"
          class:active={drawerState === "templates"}
          onclick={(e) => { e.stopPropagation(); handleNavClick("templates"); }}
        >
          <span class="drawer-nav-icon">
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" aria-hidden="true"><rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1" /><rect x="9" y="2.5" width="4.5" height="4.5" rx="1" /><rect x="2.5" y="9" width="4.5" height="4.5" rx="1" /><rect x="9" y="9" width="4.5" height="4.5" rx="1" /></svg>
          </span>
          <span>Templates</span>
          <span class="drawer-nav-shortcut">{shortcutKey("templates")}</span>
        </button>
        <button
          class="drawer-nav-item"
          class:active={drawerState === "settings"}
          onclick={(e) => { e.stopPropagation(); handleNavClick("settings"); }}
        >
          <span class="drawer-nav-icon">
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" aria-hidden="true"><path d="M2.5 5h6.5M12.5 5h1M2.5 11h1.5M7.5 11h6" /><circle cx="10.75" cy="5" r="1.75" /><circle cx="5.75" cy="11" r="1.75" /></svg>
          </span>
          <span>Settings</span>
          <span class="drawer-nav-shortcut">{shortcutKey("settings")}</span>
        </button>
      </nav>
    </div>

    <!-- Resize handle -->
    {#if resizable}
      <div class="drawer-resize-handle" use:dragResize={dragResizeParams}></div>
    {/if}

    <!-- Panel -->
    <div class="drawer-panel" class:expanded={activePanel !== null}>
      <div class="drawer-panel-inner">
        {#if activePanel === "settings"}
          <div class="drawer-panel-title">Settings</div>
          <SettingsPanel />
        {:else if activePanel === "templates"}
          <div class="drawer-panel-title">Templates</div>
          <div class="drawer-placeholder">
            <span class="drawer-placeholder-text">Use {shortcutKey("templates")} for templates</span>
          </div>
        {:else if activePanel === "hosts"}
          <HostsPanel
            groups={groups.filter((group) => !group.hidden)}
            {onSetHostAlias}
            {onForgetHost}
            {portLists}
            {portTargets}
            focus={hostFocus}
            {activeScreenId}
            onShowScreen={(id) => { setActiveScreen(id); closePanel(); }}
            onOpenHostScreen={(host, screen) => { onOpenHostScreen(host, screen); closePanel(); }}
            {onConnectHost}
            {onBrowseHost}
            onNewHostScreen={(target) => { onNewHostScreen(target, true); closePanel(); }}
            onConnectNew={() => { closePanel(); setRemoteScreenPickerOpen(true); }}
            onUpdateHost={(host) => { closePanel(); onUpdateHost(host); }}
            onOpenWindow={remoteWindows.available ? (target) => { closePanel(); setHostPickerOpen(true, target); } : undefined}
          />
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
.drawer-backdrop {
  position: absolute;
  inset: 0;
  background: rgba(0, 0, 0, 0.25);
  opacity: 0;
  pointer-events: none;
  transition: opacity 180ms ease;
  z-index: 10;
}
.drawer-backdrop.visible { opacity: 1; pointer-events: auto; }
.drawer-backdrop.panel-visible { opacity: 0; pointer-events: auto; }

.drawer {
  position: absolute;
  top: 0; left: 0; bottom: 0;
  display: flex;
  z-index: 20;
  transform: translateX(-100%);
  transition: transform 220ms cubic-bezier(0.25, 0.1, 0.25, 1);
}
.drawer.open { transform: translateX(0); }
.drawer.pinned { transform: translateX(0); }

.drawer-sidebar {
  width: var(--drawer-sidebar-width, 240px);
  background: var(--splitty-surface-raised);
  border-right: 1px solid var(--splitty-border-subtle);
  display: flex;
  flex-direction: column;
  min-width: 0;
  user-select: none;
  font-size: 12.5px;
  color: var(--splitty-text-secondary);
}

.drawer-resize-handle {
  width: 6px;
  cursor: col-resize;
  background: transparent;
  flex-shrink: 0;
  position: relative;
  z-index: 1;
  margin-left: -3px;
  margin-right: -3px;
  transition: background 120ms ease;
}
.drawer-resize-handle:hover,
.drawer-resize-handle:active {
  background: var(--splitty-accent);
  opacity: 0.3;
}

/* ── Shared ── */
.drawer-icon-btn { flex-shrink: 0; width: 24px; height: 24px; display: flex; align-items: center; justify-content: center; padding: 0; border: 0; border-radius: 5px; background: none; color: var(--splitty-text-muted); cursor: pointer; transition: color 80ms ease, background 80ms ease, opacity 80ms ease; }
.drawer-icon-btn:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }

/* ── Head ── */
.drawer-head { display: flex; align-items: center; justify-content: space-between; padding: 10px 8px 6px 16px; }
.drawer-head-label { font-size: 10.5px; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase; color: var(--splitty-text-muted); }
.drawer-pin-btn.active { color: var(--splitty-text-secondary); }

/* ── Screen list ── */
.drawer-screens { flex: 0 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; padding: 2px 0 4px; scrollbar-width: thin; }
.drawer-screens::-webkit-scrollbar { width: 4px; }
.drawer-screens::-webkit-scrollbar-track { background: transparent; }
.drawer-screens::-webkit-scrollbar-thumb { background: var(--splitty-scrollbar); }

.drawer-screen { margin: 1px 8px; padding: 1px 0 3px; border-radius: 6px; cursor: pointer; transition: background 80ms ease, opacity 120ms ease; }
.drawer-screen:hover { background: var(--splitty-hover-bg); }
.drawer-screen.active { background: var(--splitty-active-bg); }
.drawer-screen.unreachable:not(.active) { opacity: 0.55; }

.drawer-screen-row { display: flex; align-items: center; gap: 8px; height: 30px; padding: 0 4px 0 8px; }
.drawer-screen-index { flex-shrink: 0; width: 14px; text-align: right; font-family: "JetBrains Mono", ui-monospace, monospace; font-size: 10.5px; color: var(--splitty-text-muted); font-variant-numeric: tabular-nums; }
.drawer-screen.active .drawer-screen-index { color: var(--splitty-accent); }
.drawer-screen-name { flex: 1; min-width: 0; padding: 0; border: 0; background: none; font: inherit; font-weight: 500; color: var(--splitty-text-secondary); text-align: left; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
.drawer-screen:hover .drawer-screen-name, .drawer-screen.active .drawer-screen-name { color: var(--splitty-text-primary); }
.drawer-screen-input { flex: 1; min-width: 0; padding: 2px 6px; border: 1px solid var(--splitty-accent); border-radius: 4px; background: var(--splitty-surface-base); color: var(--splitty-text-primary); font: inherit; outline: none; }

/* Row actions show on hover and keyboard focus, and take no room otherwise. */
.drawer-screen-actions { display: none; align-items: center; gap: 1px; }
.drawer-screen:hover .drawer-screen-actions, .drawer-screen:focus-within .drawer-screen-actions { display: flex; }
.drawer-screen-toggle svg { transition: transform 120ms ease; }
.drawer-screen-toggle.collapsed svg { transform: rotate(-90deg); }
.drawer-screen-end:hover { color: var(--splitty-danger); }

.drawer-screen-badge { flex-shrink: 0; min-width: 16px; height: 16px; padding: 0 5px; border-radius: 8px; background: var(--splitty-accent); color: var(--splitty-surface-base); font-size: 10px; font-weight: 600; line-height: 16px; text-align: center; }

.drawer-screen-ask { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; padding: 2px 8px 6px 30px; font-size: 11.5px; color: var(--splitty-text-secondary); cursor: default; }
.drawer-screen-ask-text { flex: 1 1 100%; line-height: 1.45; }
.drawer-screen-ask-actions { display: flex; gap: 6px; }
.drawer-screen-ask-btn { min-height: 24px; padding: 0 10px; border: 1px solid var(--splitty-border-default); border-radius: 5px; background: none; color: var(--splitty-text-primary); font: inherit; cursor: pointer; }
.drawer-screen-ask-btn:hover { background: var(--splitty-hover-bg); }
.drawer-screen-ask-btn.danger { border-color: var(--splitty-danger); color: var(--splitty-danger); }
.drawer-screen-ask-btn.danger:hover { background: var(--splitty-danger-hover); }

/* ── Panes ── */
.drawer-screen-panes { overflow: hidden; max-height: 400px; transition: max-height 150ms cubic-bezier(0.25, 0.1, 0.25, 1); }
.drawer-screen-panes.collapsed { max-height: 0; }

.drawer-pane-stack { position: relative; margin-left: 30px; padding-left: 9px; }
.drawer-pane-stack::before { content: ""; position: absolute; left: 0; top: 4px; bottom: 4px; width: 1.5px; border-radius: 1px; background: var(--splitty-border-default); }
.drawer-pane-stack .drawer-pane-item { padding-left: 0; }

.drawer-pane-item { width: 100%; display: flex; align-items: center; gap: 7px; height: 21px; padding: 0 8px 0 30px; border: 0; background: none; text-align: left; font-family: "JetBrains Mono", ui-monospace, monospace; font-size: 11px; color: var(--splitty-text-muted); cursor: pointer; transition: color 80ms ease; }
.drawer-pane-item.running { color: var(--splitty-text-secondary); }
.drawer-pane-item:hover { color: var(--splitty-text-primary); }

.drawer-pane-dot { width: 12px; display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
.drawer-pane-dot::after { content: ""; width: 5px; height: 5px; border-radius: 50%; background: var(--splitty-text-muted); }
.drawer-pane-item.running .drawer-pane-dot::after { background: var(--splitty-text-primary); }

.drawer-pane-icon { flex-shrink: 0; width: 12px; text-align: center; font-size: 10px; line-height: 1; }
.drawer-pane-item.running .drawer-pane-icon { color: var(--splitty-text-primary); font-size: 12px; filter: drop-shadow(0 0 2px currentColor); }
.drawer-pane-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.drawer-pane-badge { width: 6px; height: 6px; border-radius: 50%; margin-left: auto; flex-shrink: 0; }
.drawer-pane-badge.info { background: var(--splitty-accent); }
.drawer-pane-badge.success { background: var(--splitty-success); }
.drawer-pane-badge.warning { background: var(--splitty-warning); }
.drawer-pane-badge.error { background: var(--splitty-danger); }
.drawer-pane-badge.pulse { animation: badge-pulse 2s ease-in-out infinite; }
@keyframes badge-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }

/* ── Host captions: one quiet line above a host's screens ── */
.drawer-host { margin-top: 8px; }
.drawer-host-caption { display: flex; align-items: center; gap: 2px; margin: 0 8px 1px; padding-top: 8px; border-top: 1px solid var(--splitty-border-subtle); }
.drawer-host-name { flex: 1; min-width: 0; display: flex; align-items: center; gap: 7px; height: 26px; padding: 0 8px; border: 0; border-radius: 5px; background: none; color: var(--splitty-text-secondary); font: inherit; font-size: 11.5px; text-align: left; cursor: pointer; transition: color 80ms ease, background 80ms ease; }
.drawer-host-name:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }
.drawer-host-name > svg { flex-shrink: 0; }
.drawer-host-target { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.drawer-host-note { flex-shrink: 0; font-size: 10.5px; }
.drawer-host-dot { flex-shrink: 0; width: 7px; height: 7px; border-radius: 50%; background: var(--splitty-text-muted); }
.drawer-host-dot.connected { background: var(--splitty-success); }
.drawer-host-dot.connecting, .drawer-host-dot.reconnecting { background: var(--splitty-warning); animation: badge-pulse 1.2s ease-in-out infinite; }
.drawer-host-dot.failed { background: var(--splitty-danger); }
.drawer-host-dot.offline { opacity: 0.6; }
.drawer-host-dot.idle { box-sizing: border-box; background: transparent; border: 1.5px solid var(--splitty-text-muted); }
.drawer-host-add { width: 26px; height: 26px; }
.drawer-host-ports { flex-shrink: 0; display: flex; align-items: center; gap: 4px; height: 22px; padding: 0 6px; border: 0; border-radius: 11px; background: none; color: var(--splitty-text-muted); font: inherit; font-size: 10.5px; font-variant-numeric: tabular-nums; cursor: pointer; }
.drawer-host-ports:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }
.drawer-host-ports.forwarded { color: var(--splitty-accent); }

/* ── New screen ── */
.drawer-new { flex-shrink: 0; display: flex; align-items: stretch; margin: 4px 8px 0; border-radius: 6px; }
.drawer-new-main { flex: 1; min-width: 0; display: flex; align-items: center; gap: 8px; height: 32px; padding: 0 8px; border: 0; border-radius: 6px 0 0 6px; background: none; color: var(--splitty-text-secondary); font: inherit; text-align: left; cursor: pointer; }
.drawer-new-icon { width: 14px; display: flex; justify-content: flex-end; }
.drawer-new-more { flex-shrink: 0; width: 30px; display: flex; align-items: center; justify-content: center; padding: 0; border: 0; border-radius: 0 6px 6px 0; background: none; color: var(--splitty-text-muted); cursor: pointer; }
.drawer-new:hover { background: var(--splitty-hover-bg); }
.drawer-new-main:hover, .drawer-new-more:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }

.drawer-spacer { flex: 1; min-height: 8px; }

/* ── Nav ── */
.drawer-nav { flex-shrink: 0; display: flex; flex-direction: column; gap: 1px; padding: 6px 8px 10px; border-top: 1px solid var(--splitty-border-subtle); }
.drawer-nav-item { display: flex; align-items: center; gap: 10px; height: 30px; padding: 0 8px; border: 0; border-radius: 6px; background: none; color: var(--splitty-text-secondary); font: inherit; text-align: left; cursor: pointer; transition: color 80ms ease, background 80ms ease; }
.drawer-nav-item:hover { color: var(--splitty-text-primary); background: var(--splitty-hover-bg); }
.drawer-nav-item.active { color: var(--splitty-text-primary); background: var(--splitty-active-bg); }
.drawer-nav-icon { width: 15px; display: flex; align-items: center; justify-content: center; opacity: 0.8; }
.drawer-nav-item.active .drawer-nav-icon { opacity: 1; }
.drawer-nav-shortcut { margin-left: auto; font-size: 10px; color: var(--splitty-text-muted); font-family: system-ui, sans-serif; letter-spacing: 0.01em; }

/* ── Panel ── */
.drawer-panel { width: 0; background: var(--splitty-surface-raised); border-right: 1px solid var(--splitty-border-subtle); overflow: hidden; transition: width 180ms cubic-bezier(0.25, 0.1, 0.25, 1); }
.drawer-panel.expanded { width: 340px; }
.drawer-panel-inner { width: 340px; height: 100%; box-sizing: border-box; overflow-y: auto; padding: 20px; opacity: 0; transition: opacity 120ms ease 60ms; }
.drawer-panel.expanded .drawer-panel-inner { opacity: 1; }
.drawer-panel-inner::-webkit-scrollbar { width: 6px; }
.drawer-panel-inner::-webkit-scrollbar-track { background: transparent; }
.drawer-panel-inner::-webkit-scrollbar-thumb { background: var(--splitty-scrollbar); border-radius: 3px; }
.drawer-panel-inner::-webkit-scrollbar-thumb:hover { background: var(--splitty-text-muted); }
.drawer-panel-title { font-size: 14px; font-weight: 600; color: var(--splitty-text-primary); margin-bottom: 16px; letter-spacing: 0.01em; }

.drawer-placeholder { display: flex; flex-direction: column; align-items: center; justify-content: center; height: 200px; color: var(--splitty-border-default); gap: 6px; }
.drawer-placeholder-text { font-size: 13px; color: var(--splitty-text-muted); }

button:focus-visible, input:focus-visible { outline: 2px solid var(--splitty-accent); outline-offset: -2px; }
@media (prefers-reduced-motion: reduce) {
  .drawer-host-dot, .drawer-pane-badge.pulse { animation: none; }
  .drawer-screen-toggle svg { transition: none; }
}
</style>
