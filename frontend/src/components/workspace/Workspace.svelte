<script lang="ts">
  import { onMount } from "svelte";
  import { DockviewManager } from "../../layout/dockview_manager.ts";
  import { settings } from "../../stores/settings.svelte.ts";
  import { subscribeTheme, getTheme } from "../../lib/theme.ts";
  import { getSplittyTheme } from "../../layout/dockview_theme.ts";
  import { updateAllTerminalThemes, applyAllTerminalSettings } from "../../lib/terminal/terminal_mount.ts";
  import { guardedClose, guardedCloseMultiple } from "../../lib/close_guard.ts";
  import {
    toggleDrawer, getDrawerState, setDrawerState, isDrawerPinned, isDrawerDragging, getDrawerLiveWidth,
    isShortcutsOpen, setShortcutsOpen, isCommandPaletteOpen, setCommandPaletteOpen,
    isRemoteScreenPickerOpen, setRemoteScreenPickerOpen,
  } from "../../stores/ui.svelte.ts";
  import { defaults, getSettings, updateSetting } from "../../lib/settings.ts";
  import { applyPageZoom, nextZoom, pageZoom, zoomLabel, type ZoomAction } from "../../lib/zoom.ts";
  import { watchEditorRequests } from "../../lib/editor_requests.ts";
  import { watchFileOffers } from "../../lib/file_offers.ts";
  import { Presence, watchWindowPresence } from "../../lib/terminal/presence.ts";
  import { registerCommands } from "../../lib/commands.ts";
  import { shortcutKey } from "../../lib/shortcuts.ts";
  import { subscribeUICommands } from "../../lib/client_commands.ts";
  import { addTerminalPane } from "../../lib/panes.ts";
  import {
    getScreens, getActiveScreenId, getHostOrder, setHostOrder, createScreen, removeScreen, addRemoteScreen, setScreenName,
    isDefaultScreenName, nextScreenName,
    setActiveScreen, setScreenMuxHooks, subscribe as subscribeScreens,
    type Screen as ScreenData,
  } from "../../stores/screens.svelte.ts";
  import { createScreenOp } from "../../lib/mux/mux_client.ts";
  import { ScreenStartup } from "../../lib/mux/screen_startup.ts";
  import { RemoteScreenSync } from "../../lib/mux/remote_screen_sync.ts";
  import {
    connectionFor, connections, ensureRemote, forEachConnection, remoteSocketFor, removeConnection, subscribeConnections,
  } from "../../lib/connection/registry.ts";
  import { sessionForPane } from "../../lib/connection/screen_binding.ts";
  import { resolveCommandTarget } from "../../lib/connection/routing.ts";
  import {
    LocalScreenSync, ScreenViews, connectScreenSync, screenMuxHooks, type ScreenSyncPolicy,
  } from "../../lib/connection/screen_sync.ts";
  import { hostKey, hostOf, type ConnKey, type ServerConnection } from "../../lib/connection/server_connection.ts";
  import { HostPreferences } from "../../lib/remote/host_preferences.ts";
  import { fetchRemoteHosts, openRemoteSocket, validateTarget } from "../../lib/remote/api.ts";
  import { RememberedScreens, hostToConnect } from "../../lib/remote/remembered.ts";
  import { HostDirectory } from "../../lib/remote/host_directory.ts";
  import { hostNeeded } from "../../lib/remote/on_hosts.ts";
  import { arrangeHostGroups, orderedScreens } from "../../lib/remote/screen_groups.ts";
  import { HostOrderStore } from "../../lib/remote/host_order.ts";
  import { updateImpact, updateWarning, type UpdateWarning } from "../../lib/remote/update.ts";
  import { apiFetch } from "../../lib/protocol/http.ts";
  import type { HostScreen } from "../../lib/remote/remote_socket.ts";
  import {
    blocksInput, remoteScreenStatus, type RemoteScreenFlags, type RemoteScreenStatus as RemoteStatus,
  } from "../../lib/remote/screen_status.ts";
  import {
    detachRemoteScreen, rememberTerminateConfirmed, removeRemoteScreen,
    terminateRemoteScreen, type CloseEffects,
  } from "../../lib/remote/close_screen.ts";
  import {
    availableScreens, forgetScreenFlags, getLocalVersion, hostState, remoteScreenFlags, setHostScreens,
    setLocalVersion, setLocalWslDistro, trackHost,
  } from "../../stores/connections.svelte.ts";
  import { startPortForwarding, stopPortForwarding } from "../../stores/ports.svelte.ts";
  import { NOTICE_DURATION, type PortNotice } from "../../lib/remote/port_forwarding.ts";
  import { openExternalTarget, reserveExternalOpen } from "../../lib/window.ts";
  import { newScreenId } from "../../lib/ids.ts";
  import { plainText } from "../../lib/protocol/untrusted.ts";
  import { toast } from "svelte-sonner";
  import { applyScreenSnapshot } from "../../layout/screen_snapshot.ts";
  import { applyTemplate } from "../../layout/template_apply.ts";
  import { guardedApplyTemplate } from "../../lib/template_guard.ts";
  import { extractTemplate } from "../../layout/template_extract.ts";
  import { setupKeyboardShortcuts, setupTabRevealKeys } from "../../lib/workspace/keyboard_shortcuts.ts";
  import { toggleScratchpad } from "../../lib/scratchpad.ts";
  import type { ClientCommand, Template } from "../../lib/protocol/types.ts";
  import TemplateDialog from "../dialogs/TemplateDialog.svelte";
  import HostPickerDialog from "../dialogs/HostPickerDialog.svelte";
  import UpdateHostDialog from "../dialogs/UpdateHostDialog.svelte";
  import RemoteScreenStatus from "./RemoteScreenStatus.svelte";
  import { markPaneRead, setWatchedPane } from "../../lib/notifications.ts";
  import Drawer from "../chrome/Drawer.svelte";
  import Screen from "./Screen.svelte";
  import type { DockviewApi } from "dockview-core";
  import { getDefaultShellProfileId } from "../../lib/shell_profiles.ts";

  // ── Props ──

  interface Props {
    onTitleChange?: (title: string) => void;
  }

  let { onTitleChange }: Props = $props();

  // ── Screen State ──

  // Layouts, bindings and queued snapshots of every screen, whatever its
  // connection; a snapshot only ever reaches a screen of the connection it came from.
  const views = new ScreenViews(applyScreenSnapshot);
  const screenManagers = new Map<string, DockviewManager>();

  let screens = $state(orderedScreens(getScreens(), getHostOrder()));
  let activeScreenId = $state(getActiveScreenId());

  function localScreenIds(): string[] {
    return getScreens().filter((s) => s.conn === "local").map((s) => s.id);
  }

  // Whether a local screen is built from the server's snapshot or created by
  // this client (mux-protocol, Client Resynchronization). The store's initial
  // screens are placeholders until the first welcome says what the server has.
  // Screens of other connections never pass through here.
  const startup = new ScreenStartup(localScreenIds(), {
    localScreenIds,
    createLocalScreen: (id, name) => { createScreen(name, id, { activate: false }); },
    removeLocalScreen: (id) => {
      removeScreen(id);
      forgetScreen(id);
    },
    resync: (id) => connectionFor("local")?.mux.resync(id),
    startFresh: startFreshScreen,
  });
  let templateDialogOpen = $state(false);
  let paneVersion = $state(0);

  // ── Remote Screens ──

  // One screen sync per host connection (lib/mux/remote_screen_sync.ts).
  const remoteSyncs = new Map<ConnKey, RemoteScreenSync>();
  const browsingHosts = new Set<ConnKey>();
  let directoryVersion = $state(0);
  const directory = new HostDirectory({ fetch: apiFetch, changed: () => { directoryVersion++; } });
  const hostCatalog = $derived.by(() => { void directoryVersion; return directory.entries(); });
  const recentHosts = $derived.by(() => { void directoryVersion; return directory.known(); });
  const hostPreferences = new HostPreferences({
    fetch: apiFetch,
    changed: () => { directoryVersion++; },
    visibility: (target, hidden) => directory.setHidden(target, hidden),
  });
  const hostMetadata = $derived.by(() => { void directoryVersion; return hostPreferences.entries(); });
  // The arranged order of the host groups, kept by the local hub (remote-hosts, Host Group Order).
  const hostOrder = new HostOrderStore({ fetch: apiFetch, changed: setHostOrder });
  const loadHostOrder = () => { void hostOrder.load(); };

  /** The user dragged a host group from `from` to `to` among the `shown` hosts. */
  function moveHostGroup(shown: string[], from: number, to: number) {
    const order = arrangeHostGroups(shown, from, to, getHostOrder());
    if (order) void hostOrder.save(order);
  }

  async function forgetHost(target: string) {
    await hostPreferences.forget(target);
    // The hub dropped it from the order as well.
    loadHostOrder();
    const key = hostKey(target);
    browsingHosts.delete(key);
    releaseUnusedHost(key);
  }

  /** The host whose update waits for the user's confirmation. */
  let updateHost = $state<{ conn: ConnKey; warning: UpdateWarning } | null>(null);
  /** Counts connections added and removed: a screen is mounted once its connection exists. */
  let connectionsVersion = $state(0);
  const NO_STATUS: RemoteStatus = { kind: "none" };

  // The remote screens remembered across restarts (lib/remote/remembered.ts):
  // back at their places once the local screens are settled, connected only
  // when shown, written after every change.
  const remembered = new RememberedScreens({
    fetch: apiFetch,
    screens: getScreens,
    skip: (key) => {
      const screen = getScreens().find((s) => s.id === key);
      return !!screen && screen.conn !== "local" && !!remoteSyncs.get(screen.conn)?.isPending(key);
    },
    restore: (steps) => {
      for (const { screen, index } of steps) {
        addRemoteScreen(hostKey(screen.target), screen.screenId, {
          name: screen.name ?? screen.screenId,
          index,
          activate: false,
        });
      }
    },
  });

  // ── Drawer sidebar width (for pinned margin) ──
  const drawerWidth = $derived(getDrawerLiveWidth() || (settings().drawer.resizable ? settings().drawer.sidebarWidth : defaults.drawer.sidebarWidth));
  const drawerDragging = $derived(isDrawerDragging());

  // (slide direction is computed inline in the template, like React)

  // ── Terminal Settings Effect ──

  $effect(() => {
    const _ts = settings().terminal;
    void _ts;
    // The terminals' font size follows the zoom with the terminals scope.
    void settings().appearance.zoom;
    void settings().appearance.zoomScope;
    applyAllTerminalSettings();
  });

  // ── Zoom (display-zoom) ──

  $effect(() => {
    const { zoom, zoomScope } = settings().appearance;
    applyPageZoom(pageZoom(zoom, zoomScope)).catch((error) => console.warn("could not set the page zoom", error));
  });

  function zoom(action: ZoomAction) {
    const next = nextZoom(getSettings().appearance.zoom, action);
    if (next !== getSettings().appearance.zoom) void updateSetting("appearance", "zoom", next);
    toast(`Zoom ${zoomLabel(next)}`, { id: "zoom", duration: 1200 });
  }

  // ── Window Title Tracking ──

  $effect(() => {
    const api = views.api(activeScreenId);
    const screen = screens.find((s) => s.id === activeScreenId);
    if (!api || !screen) return;

    let titleDisposable: { dispose(): void } | undefined;

    // A remote screen always names its host: `name @host · pane title`.
    const host = hostOf(screen.conn);
    const name = host ? `${screen.name} @${host}` : screen.name;
    const updateTitle = (paneTitle: string) => {
      const label = paneTitle || "Splitty";
      const full = (screens.length > 1 || host) && name
        ? `${name} · ${label}`
        : label;
      document.title = full;
      onTitleChange?.(full);
    };

    const onActiveChange = () => {
      titleDisposable?.dispose();
      const panel = api.activePanel;
      if (!panel) { updateTitle("Splitty"); return; }
      updateTitle(panel.title ?? "Splitty");
      titleDisposable = panel.api.onDidTitleChange((e) => updateTitle(e.title || "Splitty"));
      markPaneRead(screen.conn, panel.id);
    };

    const d = api.onDidActivePanelChange(onActiveChange);
    onActiveChange();

    return () => {
      d.dispose();
      titleDisposable?.dispose();
    };
  });

  // ── Helpers ──

  /**
   * The active screen's layout, for shortcuts, templates and focus. None while
   * the active screen is a remote one that takes no input (connecting,
   * reconnecting, an error, the reopen offer): a pane added then would bypass
   * the offer, or wait for a host that is not there.
   */
  function getActiveApi(): DockviewApi | undefined {
    const screen = getScreens().find((s) => s.id === activeScreenId);
    if (screen && blocksInput(remoteStatus(screen))) return undefined;
    return views.api(activeScreenId);
  }

  /**
   * The binding of a screen; a screen without one is not mounted: its
   * connection is gone, or not opened yet (a remembered screen not shown yet).
   */
  function bindingFor(screen: ScreenData, _connections: number) {
    return views.bind(screen, connectionFor(screen.conn), screen.id === activeScreenId);
  }

  /** Drop a screen's layout and binding; its binding sends nothing from now on. */
  function forgetScreen(key: string) {
    views.remove(key);
    screenManagers.delete(key);
  }

  function sendCloseScreen(key: string) {
    const screen = getScreens().find((s) => s.id === key);
    if (!screen) return;
    connectionFor(screen.conn)?.mux.sendOp(screen.wireId, { type: "close_screen", screenId: screen.wireId });
  }

  // ── Screen Callbacks (from Screen.svelte) ──

  function handleApiReady(key: string, api: DockviewApi, manager: DockviewManager) {
    screenManagers.set(key, manager);
    // Registers the layout with the screen's binding, through which every pane
    // added to it finds its connection, and applies a snapshot queued for it.
    if (views.ready(key, api)) return;
    const binding = views.binding(key);
    if (binding?.conn.kind === "local") startup.viewReady(key);
    else if (binding) remoteSyncs.get(binding.conn.key)?.viewReady(key);
  }

  /** First terminal pane of a screen the server does not know. create_screen
   *  carries the pane, so no split op for it. A host's screen gets no local
   *  shell profile: the host starts its own default shell. */
  function startFreshScreen(key: string) {
    const api = views.api(key);
    const binding = views.binding(key);
    if (!api || !binding) return;
    const shellProfileId = binding.conn.kind === "local" ? getDefaultShellProfileId() : undefined;
    const paneId = addTerminalPane(api, { shellProfileId, skipMuxNotify: true });
    if (binding.conn.kind !== "local") avoidHostNames(binding.conn.key, key);
    const screen = getScreens().find((s) => s.id === key);
    binding.sendOp(createScreenOp(binding.wireId, screen?.name, paneId, shellProfileId));
  }

  function handleScreenEmpty(key: string) {
    const screen = getScreens().find((s) => s.id === key);
    if (!screen || getScreens().length <= 1) return;
    // A screen whose last pane moved away was closed by its server with the move.
    if (!movedAway.delete(key)) sendCloseScreen(key);
    if (screen.conn !== "local") {
      // Its last pane was closed: the host's screen is gone too.
      remoteSyncs.get(screen.conn)?.terminated(screen.wireId);
      removeRemoteScreen(key, closeEffects);
      return;
    }
    forgetScreen(key);
    removeScreen(key);
  }

  // ── Moving a Pane to Another Screen ──

  /** Screens whose last pane moved away: their server closed them with the move. */
  const movedAway = new Set<string>();

  /**
   * Move the pane `paneId` of the screen `sourceKey`, with its running
   * session, to the screen `targetKey` of the same server, or to a new one
   * (null), and show it there (workspace-model, Moving a Pane to Another Screen).
   */
  function movePaneToScreen(sourceKey: string, paneId: string, targetKey: string | null) {
    const source = getScreens().find((s) => s.id === sourceKey);
    const binding = views.binding(sourceKey);
    const api = views.api(sourceKey);
    const panel = api?.getPanel(paneId);
    const conn = source && connectionFor(source.conn);
    if (!source || !binding || !api || !panel || !conn) return;
    const existing = targetKey === null ? undefined : getScreens().find((s) => s.id === targetKey);
    if (targetKey !== null && (!existing || existing.conn !== source.conn)) return;

    const wireId = existing?.wireId ?? newScreenId();
    const name = existing?.name ?? nextScreenName(source.conn);
    conn.mux.sendOp(source.wireId, existing
      ? { type: "move_pane_to_screen", paneId, targetScreenId: wireId }
      : { type: "move_pane_to_screen", paneId, targetScreenId: wireId, createScreen: true, screenName: name });

    // Let go of it here without ending it: no close_pane, a detach instead of a close.
    binding.handOver(paneId, binding.sessionForPane(paneId)?.id);
    if (api.panels.length === 1) movedAway.add(sourceKey);
    panel.api.close();

    // The target is built from its snapshot, where the pane attaches to its session.
    let targetScreen = existing;
    if (!targetScreen && source.conn === "local") {
      startup.expect(wireId);
      targetScreen = createScreen(name, wireId, { activate: false, after: sourceKey });
    } else if (!targetScreen && source.conn !== "local") {
      remoteSyncs.get(source.conn)?.adopt(wireId, name);
      targetScreen = addRemoteScreen(source.conn, wireId, { name, after: sourceKey, activate: false });
    }
    if (targetScreen) views.reveal(targetScreen.id, paneId);
    conn.mux.resync(wireId);
    if (targetScreen) setActiveScreen(targetScreen.id);
  }

  // ── Remote Screens ──

  /**
   * A new screen named before its host was reached may carry the default name
   * of a screen the host has: it takes the next free one. Names the user gave
   * stay. `creating`: the screen whose `create_screen` goes out now.
   */
  function avoidHostNames(key: ConnKey, creating?: string) {
    const sync = remoteSyncs.get(key);
    const onHost = sync?.catalog();
    if (!sync || !onHost) return;
    for (const screen of getScreens()) {
      if (screen.conn !== key || (screen.id !== creating && !sync.isPending(screen.id))) continue;
      if (!isDefaultScreenName(screen.name)) continue;
      const others = onHost.filter((s) => s.id !== screen.wireId).map((s) => s.name);
      if (others.includes(screen.name)) setScreenName(screen.id, nextScreenName(key, others));
    }
  }

  /** Hand what a host's screen sync knows to the store the UI reads. */
  function publishHostScreens(key: ConnKey, sync: RemoteScreenSync) {
    avoidHostNames(key);
    const flags: Record<string, RemoteScreenFlags> = {};
    for (const s of getScreens()) {
      if (s.conn !== key) continue;
      flags[s.id] = { pending: sync.isPending(s.id), needsChoice: sync.needsChoice(s.id), missing: sync.isMissing(s.id) };
    }
    setHostScreens(key, sync.available(), flags);
    const target = hostOf(key);
    const catalog = sync.catalog();
    if (target && catalog !== undefined && hostState(key)?.phase === "ready") directory.observe(target, catalog);
    // A new screen counts once its host has it (`create_screen` sent).
    remembered.changed();
  }

  /** Open the connection to a host, if the window has none (lazy connect, "On hosts"). */
  function connectHost(key: ConnKey) {
    const target = hostOf(key);
    if (target && !connectionFor(key)) ensureRemote(target, openRemoteSocket);
    else if (target) remoteSocketFor(key)?.retry();
  }

  /** Directory browsing is an explicit interest, independent of screen attachments. */
  function browseHost(key: ConnKey, expanded: boolean) {
    if (expanded) {
      browsingHosts.add(key);
      connectHost(key);
    } else {
      browsingHosts.delete(key);
      releaseUnusedHost(key);
    }
  }

  function releaseUnusedHost(key: ConnKey) {
    setTimeout(() => {
      if (!hostNeeded(key, getScreens(), browsingHosts)) removeConnection(key);
    }, 0);
  }

  /** A remote screen that becomes the active one connects its host. */
  function connectActiveHost() {
    const key = hostToConnect(getScreens(), getActiveScreenId(), (conn) => !!connectionFor(conn));
    if (key) connectHost(key);
  }

  /** Open a live or cached screen reference; its host alone decides whether it still exists. */
  function openHostScreen(key: ConnKey, hostScreen: HostScreen) {
    if (key === "local") return;
    const shown = getScreens().find((s) => s.conn === key && s.wireId === hostScreen.id);
    if (shown) setActiveScreen(shown.id);
    else addRemoteScreen(key, hostScreen.id, { name: hostScreen.name });
    connectHost(key);
  }

  /**
   * Updating a host's Splitty, from an incompatible host's "Update now" or a
   * host of another version in the Hosts panel: ask first, saying whether its
   * sessions keep running or naming what ends there.
   */
  function askUpdate(conn: ConnKey) {
    const target = hostOf(conn);
    const socket = remoteSocketFor(conn);
    if (!target || !socket) return;
    // The window's own pane counts, where it has a layout with panes for the screen.
    const shown = getScreens()
      .filter((s) => s.conn === conn)
      .map((s) => ({ wireId: s.wireId, panes: views.api(s.id)?.panels.length || undefined }));
    const { error, ready } = socket.state;
    const warning = updateWarning(target, {
      hostVersion: error?.version ?? ready?.daemon.version,
      hostProtocol: error?.protocol ?? ready?.daemon.protocol,
      keepsSessions: error?.keepsSessions ?? ready?.daemon.keepsSessions,
      localVersion: getLocalVersion(),
    }, updateImpact(error?.screens ?? ready?.screens, shown));
    updateHost = { conn, warning };
  }

  function updateConfirmed() {
    // Only this attempt carries `update=1`.
    if (updateHost) remoteSocketFor(updateHost.conn)?.retry({ update: true });
    updateHost = null;
  }

  /**
   * "New screen on host…": the screen appears at once and shows its host's
   * progress inside it. The host's connection opens with its first screen.
   */
  async function newRemoteScreen(input: string, direct = false) {
    const target = validateTarget(input);
    if (!target) {
      toast.error(`“${plainText(input, 80)}” is not a host`, { description: "Use [user@]host[:port]." });
      return;
    }
    try { await hostPreferences.reveal(target); }
    catch (error) { toast.error("Could not add host", { description: error instanceof Error ? error.message : String(error) }); return; }
    const conn = ensureRemote(target, openRemoteSocket);
    const key = conn.key;
    if (key === "local") return;
    // A host that failed or went offline gets another attempt with the new screen.
    const socket = remoteSocketFor(key);
    if (socket?.state.phase === "failed" || socket?.state.phase === "offline") socket.retry();
    // Not the name of a screen the host has, as far as the window knows.
    const onHost = [
      ...availableScreens(key),
      ...(directory.entries().find((h) => h.target === target)?.screens ?? []),
    ].map((s) => s.name);
    const screen = addRemoteScreen(key, newScreenId(), { name: nextScreenName(key, onHost) });
    remoteSyncs.get(key)?.addNew(screen.id);
    if (direct) remoteSyncs.get(key)?.decide(screen.id);
  }

  /** The reopen offer: open the host's `selected` screens in place of the new screen `pendingKey`. */
  function openHostScreens(pendingKey: string, selected: HostScreen[]) {
    const pending = getScreens().find((s) => s.id === pendingKey);
    if (!pending || pending.conn === "local") return;
    // The new screen is not created: its sync must not create it either once
    // the opened screens leave nothing to offer.
    remoteSyncs.get(pending.conn)?.forget(pendingKey);
    let after = pendingKey;
    let first: string | undefined;
    for (const hostScreen of selected) {
      if (getScreens().some((s) => s.conn === pending.conn && s.wireId === hostScreen.id)) continue;
      const opened = addRemoteScreen(pending.conn, hostScreen.id, { name: hostScreen.name, after, activate: false });
      after = opened.id;
      first ??= opened.id;
    }
    // The new screen was never created on the host; it only leaves.
    removeRemoteScreen(pendingKey, closeEffects);
    if (first) setActiveScreen(first);
  }

  function createInsteadOfReopen(key: string) {
    const screen = getScreens().find((s) => s.id === key);
    if (screen && screen.conn !== "local") remoteSyncs.get(screen.conn)?.decide(key);
  }

  function retryHost(key: ConnKey) {
    remoteSocketFor(key)?.retry();
  }

  function remoteStatus(screen: ScreenData): RemoteStatus {
    const target = hostOf(screen.conn);
    if (!target) return NO_STATUS;
    return remoteScreenStatus(target, hostState(screen.conn), remoteScreenFlags(screen.id), availableScreens(screen.conn));
  }

  const closeEffects: CloseEffects = {
    screens: getScreens,
    createLocalScreen: () => { createScreen(); },
    removeScreen: (key) => {
      forgetScreen(key);
      forgetScreenFlags(key);
      removeScreen(key);
    },
    connection: connectionFor,
    sync: (key) => remoteSyncs.get(key),
    panes: (key) => views.api(key)?.panels.length ?? 0,
    // Let detach messages leave before closing an unused connection.
    releaseHost: releaseUnusedHost,
  };

  /** End a remote screen on its host; the drawer asked first, if it had to. */
  function handleTerminateScreen(key: string) {
    if (terminateRemoteScreen(key, closeEffects)) rememberTerminateConfirmed();
  }

  // ── Ports ──

  /** A notice of the port forwarding (lib/remote/port_forwarding.ts) as a toast. */
  function showPortNotice(notice: PortNotice) {
    const options = {
      id: notice.id,
      description: notice.description,
      duration: notice.action ? NOTICE_DURATION : undefined,
      action: notice.action && { label: notice.action.label, onClick: notice.action.run },
    };
    if (notice.kind === "error") toast.error(notice.title, options);
    else if (notice.kind === "success") toast.success(notice.title, options);
    else toast.info(notice.title, options);
  }

  // ── Server Commands ──

  /**
   * A server command (mux-protocol, Server Commands) acts only on a pane of
   * the screens of the connection that sent it, and never switches screens.
   */
  function handleServerCommand(origin: ServerConnection, command: ClientCommand) {
    const target = resolveCommandTarget(origin, command, {
      screens: getScreens(),
      activeKey: getActiveScreenId(),
      hasPane: (key, paneId) => !!views.api(key)?.getPanel(paneId),
      activePane: (key) => views.api(key)?.activePanel?.id,
    }, { allowClipboardWrite: getSettings().terminal.allowOsc52Write });
    if (!target) return;
    const api = views.api(target.screenKey);
    if (!api) return;
    const { paneId } = target;

    switch (command.type) {
      case "pane.activate": {
        // Activates the pane within its screen; its terminal takes the
        // keyboard only if that screen is the active one.
        const panel = api.getPanel(paneId);
        if (panel) { panel.api.setActive(); panel.focus(); }
        break;
      }
      case "pane.copy": {
        const sel = sessionForPane(api, paneId)?.emulator.getSelection?.();
        if (sel) navigator.clipboard.writeText(sel);
        break;
      }
      case "pane.paste":
        navigator.clipboard.readText().then((text) => {
          if (text) sessionForPane(api, paneId)?.emulator.paste(text);
        });
        break;
      case "pane.clear":
        sessionForPane(api, paneId)?.emulator.clear();
        break;
      case "pane.close":
        guardedClose(api, paneId, () => api.getPanel(paneId)?.api.close());
        break;
      case "pane.split":
        addTerminalPane(api, { direction: command.direction, referencePanel: paneId });
        break;
    }
  }

  // ── Template Handlers ──

  // Routed through guardedApplyTemplate (security review SR-12):
  // a template that carries shell commands must show them and be confirmed
  // before anything is written into a PTY. Templates without commands apply
  // straight through.
  function handleApplyTemplate(template: Template, root?: string) {
    const api = getActiveApi();
    if (!api) return;
    guardedApplyTemplate(template, (approved) => applyTemplate(api, approved, root));
  }

  function handleApplyTemplateInNewScreen(template: Template, root?: string) {
    guardedApplyTemplate(template, (approved) => {
      const screen = createScreen();
      requestAnimationFrame(() => {
        const api = views.api(screen.id);
        if (api) {
          applyTemplate(api, approved, root);
        }
      });
    });
  }

  function handleExtractTemplate(name: string, root?: string): Template {
    const api = getActiveApi();
    if (!api) throw new Error("No active screen");
    return extractTemplate(api, name, root);
  }

  // ── Drawer Helpers ──

  function handleActivatePane(key: string, paneId: string) {
    setActiveScreen(key);
    const api = views.api(key);
    const panel = api?.getPanel(paneId);
    if (panel) {
      panel.api.setActive();
      panel.focus();
    }
  }

  function handleCloseScreen(key: string) {
    const screen = getScreens().find((s) => s.id === key);
    if (screen && screen.conn !== "local") {
      // Closing never asks and never destroys: a screen the host never had,
      // or no longer has, just goes; any other detaches and keeps running on
      // its host. Ending it there is the drawer's own action.
      const flags = remoteScreenFlags(key);
      if (flags.pending || flags.missing) {
        removeRemoteScreen(key, closeEffects);
        return;
      }
      const reopen: HostScreen = { id: screen.wireId, name: screen.name, panes: views.api(key)?.panels.length ?? 0 };
      detachRemoteScreen(key, closeEffects);
      // Closing did not ask, so it says what happened and offers the way back.
      toast(`“${screen.name}” keeps running on ${hostOf(screen.conn)}`, {
        id: `detached:${key}`,
        action: { label: "Reopen", onClick: () => openHostScreen(screen.conn, reopen) },
      });
      return;
    }

    const api = views.api(key);
    if (!api) {
      // The window always keeps one screen.
      if (getScreens().length <= 1) return;
      sendCloseScreen(key);
      forgetScreen(key);
      removeScreen(key);
      return;
    }

    // guardedCloseMultiple: idle panels close immediately,
    // running panels prompt for confirmation then close.
    // After all panels are gone, the auto-close-empty-screen logic
    // in Screen.svelte will clean up via onScreenEmpty.
    const panels = [...api.panels];
    guardedCloseMultiple(api, panels.map((p) => ({ id: p.id, close: () => p.api.close() })));
  }

  // ── Focus Terminal on Drawer Close ──

  let prevDrawerState = $state(getDrawerState());

  $effect(() => {
    const state = getDrawerState();
    if (prevDrawerState !== "closed" && state === "closed") {
      requestAnimationFrame(() => {
        const api = getActiveApi();
        const panel = api?.activePanel;
        if (panel) panel.focus();
      });
    }
    prevDrawerState = state;
  });

  // ── Webview Open (from terminal links) ──

  // The `splitty-open-webview` event and its handler are gone (security review
  // SR-01/SR-02): it existed only to turn a URL or local port into an
  // in-app iframe pane. Terminal links, the context menu and Mod+O now call
  // openExternalTarget (lib/window.ts) directly, which validates the scheme and
  // hands off to the system browser.

  // ── Mount / Cleanup ──

  onMount(() => {
    // Rename, switch and reorder go to each screen's own connection.
    setScreenMuxHooks(screenMuxHooks(connectionFor));

    // The hosts' ports and the local server's forwards; a host's connection
    // is watched below.
    const ports = startPortForwarding({
      fetch: apiFetch,
      open: (url) => openExternalTarget({ url }),
      reserveOpen: reserveExternalOpen,
      autoForward: () => getSettings().remote.autoForwardPorts,
      notify: showPortNotice,
    });

    const refreshPorts = () => { void ports.refresh(); };
    window.addEventListener("focus", refreshPorts);

    // The pane the user looks at: what happens there is seen, and coming back
    // to it, by a screen switch or to the window, reads what came meanwhile.
    const watchedPane = () => {
      const screen = getScreens().find((s) => s.id === getActiveScreenId());
      const paneId = screen ? views.api(screen.id)?.activePanel?.id : undefined;
      return screen && paneId ? { conn: screen.conn, paneId } : undefined;
    };
    const readWatchedPane = () => {
      const watched = watchedPane();
      if (watched && document.visibilityState === "visible") markPaneRead(watched.conn, watched.paneId);
    };
    setWatchedPane((conn, paneId) => {
      const watched = watchedPane();
      return document.hasFocus() && document.visibilityState === "visible" && watched?.conn === conn && watched.paneId === paneId;
    });
    window.addEventListener("focus", readWatchedPane);
    document.addEventListener("visibilitychange", readWatchedPane);

    // Each server learns when this window's user acts, for its PTY size policy.
    const presence = new Presence(
      () => {
        const active = getScreens().find((screen) => screen.id === getActiveScreenId());
        return active ? connectionFor(active.conn) : undefined;
      },
      { visible: () => document.visibilityState === "visible", focused: () => document.hasFocus() },
    );
    const stopWindowPresence = watchWindowPresence(presence, connections);

    // One screen sync policy and one command handler per connection.
    const cleanupConnections = forEachConnection((conn) => {
      let policy: ScreenSyncPolicy;
      const cleanups: (() => void)[] = [];
      if (conn.kind === "local") {
        // The remembered remote screens go back once the local ones are settled.
        policy = new LocalScreenSync(conn, startup, views, () => remembered.localScreensReady());
      } else {
        const sync: RemoteScreenSync = new RemoteScreenSync(conn, {
          screens: getScreens,
          snapshot: (key, state) => {
            views.snapshot(conn.key, key, state);
            if (state.name) setScreenName(key, state.name);
          },
          startFresh: startFreshScreen,
          changed: () => publishHostScreens(conn.key, sync),
        });
        policy = sync;
        remoteSyncs.set(conn.key, sync);
        cleanups.push(ports.watch(conn));
        const socket = remoteSocketFor(conn.key);
        if (socket) {
          cleanups.push(trackHost(conn.key, socket));
          cleanups.push(socket.onReady((info) => { sync.ready(info.screens); refreshPorts(); }));
        }
        cleanups.push(() => {
          sync.dispose();
          remoteSyncs.delete(conn.key);
        });
      }
      cleanups.push(watchEditorRequests(conn));
      cleanups.push(watchFileOffers(conn));
      // Before the screen sync: the server hears this window's state ahead of any attach.
      cleanups.push(presence.watch(conn));
      const resetSync = connectScreenSync(conn, policy);
      const unsubCommands = conn.onCommand((command) => handleServerCommand(conn, command));
      return () => {
        resetSync();
        unsubCommands();
        for (const cleanup of cleanups) cleanup();
      };
    });

    const unsubConnections = subscribeConnections(() => { connectionsVersion++; });

    const unsubScreens = subscribeScreens(() => {
      // Before the state below, so the screen that became active mounts with its connection.
      connectActiveHost();
      screens = orderedScreens(getScreens(), getHostOrder());
      if (getActiveScreenId() !== activeScreenId) queueMicrotask(readWatchedPane);
      activeScreenId = getActiveScreenId();
      remembered.changed();
      // What the reopen offers list depends on the screens the window shows.
      for (const [key, sync] of remoteSyncs) {
        sync.refresh();
        for (const screen of getScreens()) if (screen.conn === key) sync.updateOpen(screen.id);
        publishHostScreens(key, sync);
      }
    });

    const unsubTheme = subscribeTheme(() => {
      updateAllTerminalThemes();
      // Update DnD overlay behaviour from layout tokens.
      const theme = getSplittyTheme(getTheme().layout);
      for (const mgr of screenManagers.values()) {
        mgr.setThemeRaw(theme);
      }
    });

    const unsubUICommands = subscribeUICommands((cmd) => {
      switch (cmd.type) {
        case "drawer.toggle":
          toggleDrawer();
          break;
        case "pane.move-to-screen":
          movePaneToScreen(cmd.screenKey, cmd.paneId, cmd.target);
          break;
      }
    });

    const cleanupShortcuts = setupKeyboardShortcuts({
      getActiveApi,
      toggleDrawer,
      createScreen,
      setActiveScreen,
      getScreens: () => orderedScreens(getScreens(), getHostOrder()),
      toggleTemplateDialog: () => { templateDialogOpen = !templateDialogOpen; },
      toggleShortcutsOverlay: () => { setShortcutsOpen(!isShortcutsOpen()); },
      toggleCommandPalette: () => { setCommandPaletteOpen(!isCommandPaletteOpen()); },
      zoom,
    });

    const unregisterCommands = registerCommands([
      { id: "screen.new", label: "New Screen", shortcut: shortcutKey("newScreen"), run: () => { createScreen(); } },
      { id: "screen.newOnHost", label: "New screen on host…", run: () => { setRemoteScreenPickerOpen(true); } },
      { id: "templates", label: "Templates…", shortcut: shortcutKey("templates"), run: () => { templateDialogOpen = true; } },
      { id: "drawer.toggle", label: "Toggle Drawer", shortcut: shortcutKey("drawer"), run: toggleDrawer },
      {
        id: "scratchpad.toggle", label: "Toggle Scratchpad", shortcut: shortcutKey("scratchpad"),
        run: () => { const pane = getActiveApi()?.activePanel; if (pane) toggleScratchpad(pane.id); },
      },
      { id: "settings", label: "Settings", shortcut: shortcutKey("settings"), run: () => { setDrawerState("settings"); } },
      { id: "hosts", label: "Hosts and ports", run: () => { setDrawerState("hosts"); } },
      { id: "shortcuts", label: "Keyboard Shortcuts", shortcut: "?", run: () => { setShortcutsOpen(true); } },
    ]);

    const cleanupTabReveal = setupTabRevealKeys();

    void remembered.load();
    void directory.load();
    void hostPreferences.load();
    loadHostOrder();
    // Another window of this computer may have arranged the hosts meanwhile.
    window.addEventListener("focus", loadHostOrder);
    // A list changed right before the page goes is still written.
    const flushRemembered = () => { remembered.flush(true); directory.flush(true); };
    window.addEventListener("pagehide", flushRemembered);

    // The local server's version, which an updated host gets.
    apiFetch("/api/build")
      .then((r) => (r.ok ? r.json() : null))
      .then((build) => {
        if (typeof build?.version === "string") setLocalVersion(build.version);
        if (build) setLocalWslDistro(typeof build.wslDistro === "string" ? build.wslDistro : undefined);
      })
      .catch(() => {});

    return () => {
      window.removeEventListener("pagehide", flushRemembered);
      remembered.flush(true);
      remembered.dispose();
      directory.flush(true);
      directory.dispose();
      hostPreferences.dispose();
      browsingHosts.clear();
      unsubConnections();
      cleanupShortcuts();
      unregisterCommands();
      cleanupTabReveal();
      unsubScreens();
      unsubTheme();
      unsubUICommands();
      cleanupConnections();
      stopWindowPresence();
      window.removeEventListener("focus", refreshPorts);
      window.removeEventListener("focus", loadHostOrder);
      window.removeEventListener("focus", readWatchedPane);
      document.removeEventListener("visibilitychange", readWatchedPane);
      setWatchedPane(null);
      stopPortForwarding();
      setScreenMuxHooks({});
      views.clear();
      screenManagers.clear();
      // No remote layout can send close operations after its binding goes.
      // Ending the sockets detaches their views and makes an auth-driven
      // remount start with fresh welcomes instead of stale registered sockets.
      for (const conn of connections()) if (conn.kind === "remote") removeConnection(conn.key);
    };
  });
</script>

<div class="workspace-root" style="position:relative;width:100%;height:100%;overflow:hidden">
  <Drawer
    screenApis={views.layouts}
    {paneVersion}
    onCloseScreen={handleCloseScreen}
    onTerminateScreen={handleTerminateScreen}
    onActivatePane={handleActivatePane}
    onOpenHostScreen={openHostScreen}
    onConnectHost={connectHost}
    onBrowseHost={browseHost}
    onNewHostScreen={(target, direct) => newRemoteScreen(target, direct)}
    onUpdateHost={askUpdate}
    catalog={hostCatalog}
    {recentHosts}
    preferences={hostMetadata}
    onSetHostAlias={(target, alias) => hostPreferences.setAlias(target, alias)}
    onForgetHost={forgetHost}
    onRevealHost={(target) => hostPreferences.reveal(target)}
    onMoveHostGroup={moveHostGroup}
  />

  <div
    class="screen-layouts{isDrawerPinned() ? ' drawer-pinned' : ''}"
    style:--drawer-sidebar-width="{drawerWidth}px"
    style:transition={drawerDragging ? 'none' : undefined}
  >
    {#each screens as screen, idx (screen.id)}
      {@const binding = bindingFor(screen, connectionsVersion)}
      {@const activeIdx = screens.findIndex((s) => s.id === activeScreenId)}
      {@const isActive = screen.id === activeScreenId}
      {@const slide = isActive ? undefined : idx < activeIdx ? "up" : "down"}
      {#if binding}
        {@const status = remoteStatus(screen)}
        <Screen
          screenId={screen.id}
          {binding}
          {isActive}
          slideDirection={slide}
          tabSettings={settings().tabs}
          onApiReady={handleApiReady}
          onPaneVersionChange={() => {
            paneVersion++;
            remoteSyncs.get(screen.conn)?.updateOpen(screen.id, views.api(screen.id)?.panels.length);
          }}
          onScreenEmpty={handleScreenEmpty}
          blocked={blocksInput(status)}
        >
          {#snippet overlay()}
            <RemoteScreenStatus
              {status}
              onCancel={() => removeRemoteScreen(screen.id, closeEffects)}
              onRetry={() => retryHost(screen.conn)}
              onOpen={(selected) => openHostScreens(screen.id, selected)}
              onCreateNew={() => createInsteadOfReopen(screen.id)}
              onRemove={() => removeRemoteScreen(screen.id, closeEffects)}
              onUpdate={() => askUpdate(screen.conn)}
            />
          {/snippet}
        </Screen>
      {/if}
    {/each}
  </div>
</div>

<HostPickerDialog
  open={isRemoteScreenPickerOpen()}
  title="New screen on host"
  colors={{ bg: getTheme().ui['surface-base'], fg: getTheme().ui['text-primary'], danger: getTheme().ui['danger'] ?? '#e06c75' }}
  hosts={fetchRemoteHosts}
  onPick={newRemoteScreen}
  onClose={() => { setRemoteScreenPickerOpen(false); }}
/>


<UpdateHostDialog
  open={updateHost !== null}
  warning={updateHost?.warning ?? null}
  onConfirm={updateConfirmed}
  onCancel={() => { updateHost = null; }}
/>

<TemplateDialog
  open={templateDialogOpen}
  onClose={() => { templateDialogOpen = false; }}
  onApply={handleApplyTemplate}
  onApplyNewScreen={handleApplyTemplateInNewScreen}
  onExtract={handleExtractTemplate}
/>
