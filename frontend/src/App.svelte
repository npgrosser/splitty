<script lang="ts">
  import { onMount } from "svelte";
  import { getAuthState, getAuthError, handleTokenSubmit, initAuth } from "./stores/auth.svelte.ts";
  import { settings } from "./stores/settings.svelte.ts";
  import { getTheme, subscribeTheme } from "./lib/theme.ts";
  import { onCloseRequest, runningOnQuit, scratchpadsOnQuit, hostScreensOpen, type CloseRequest, type RunningProcess } from "./lib/close_guard.ts";
  import { onTemplateApprovalRequest, type TemplateApprovalRequest } from "./lib/template_guard.ts";
  import { warmup } from "./lib/terminal/terminal_mount.ts";
  import { forEachConnection, localConnection } from "./lib/connection/registry.ts";
  import { attachNotifications, onNewNotification } from "./lib/notifications.ts";
  import { getDesktopPlatform, isDesktop, isPWA, shellKind } from "./lib/platform.ts";
  import { windowAPI } from "./lib/window.ts";
  import { remoteWindows } from "./lib/remoteWindows.ts";
  import { registerCommands } from "./lib/commands.ts";
  import { getHostPickerTarget, isHostPickerOpen, setHostPickerOpen } from "./stores/ui.svelte.ts";
  import { toast } from "svelte-sonner";
  import TokenDialog from "./components/dialogs/TokenDialog.svelte";
  import ConfirmCloseDialog from "./components/dialogs/ConfirmCloseDialog.svelte";
  import ConfirmQuitDialog from "./components/dialogs/ConfirmQuitDialog.svelte";
  import ConfirmTemplateDialog from "./components/dialogs/ConfirmTemplateDialog.svelte";
  import ContextMenu from "./components/ui/ContextMenu.svelte";
  import Toasts from "./components/ui/Toasts.svelte";
  import ShortcutsOverlay from "./components/chrome/ShortcutsOverlay.svelte";
  import CommandPalette from "./components/chrome/CommandPalette.svelte";
  import HostPickerDialog from "./components/dialogs/HostPickerDialog.svelte";
  import TitleBar from "./components/chrome/TitleBar.svelte";
  import ResizeEdges from "./components/chrome/ResizeEdges.svelte";
  import Workspace from "./components/workspace/Workspace.svelte";

  let theme = $state(getTheme());
  let closeRequest = $state<CloseRequest | null>(null);
  /** Quitting while processes run in local panes, which end with the app. */
  let quitRequest = $state<{ processes: RunningProcess[]; scratchpads: number; hostScreens: boolean; quit: () => void } | null>(null);
  let templateRequest = $state<TemplateApprovalRequest | null>(null);
  let windowTitle = $state("Splitty");
  let maximized = $state(false);

  // The Windows WSL shell is created frameless before the backend config is
  // available, so it consistently uses Splitty's custom title bar.
  const isFrameless = $derived(
    isDesktop && (!settings().window.nativeTitleBar || getDesktopPlatform() === "windows"),
  );

  warmup();

  onMount(() => {
    // The local connection listens before its socket opens, so no welcome is missed.
    localConnection();
    initAuth();
    const cleanupNotifications = forEachConnection(attachNotifications);

    const unsubToast = onNewNotification((n) => {
      const ns = settings().notifications;
      if (!ns.enabled || !ns.toasts) return;
      if (n.kind === "process.finished" && !ns.processFinished) return;
      if (n.kind === "session.exited" && !ns.processExited) return;
      if (n.kind === "session.crashed" && !ns.processCrashed) return;

      const opts = { description: n.body };
      switch (n.severity) {
        case "error": toast.error(n.title, opts); break;
        case "warning": toast.warning(n.title, opts); break;
        case "success": toast.success(n.title, opts); break;
        default: toast.info(n.title, opts); break;
      }
    });

    const unsubTheme = subscribeTheme(() => { theme = getTheme(); });
    // Only the desktop app's own UI opens hosts; a remote window has no bridge.
    const unregisterRemote = remoteWindows.available
      ? registerCommands([{ id: "remote.open", label: "Open host in new window…", run: () => { setHostPickerOpen(true); } }])
      : undefined;
    const unsubClose = onCloseRequest((req) => { closeRequest = req; });
    const unsubTemplate = onTemplateApprovalRequest((req) => { templateRequest = req; });

    let resizeCleanup: (() => void) | undefined;
    if (isDesktop) {
      const checkMax = () => {
        windowAPI.isMaximized().then((m) => { maximized = m; });
      };
      window.addEventListener("resize", checkMax);
      checkMax();
      resizeCleanup = () => window.removeEventListener("resize", checkMax);
    }

    // ── App-level quit guard ──
    let quitCleanup: (() => void) | undefined;

    if (shellKind === "tauri") {
      import("@tauri-apps/api/window").then((mod) => {
        const win = mod.getCurrentWindow();
        win.onCloseRequested(async (event) => {
          const processes = runningOnQuit();
          const scratchpads = scratchpadsOnQuit();
          if (processes.length > 0 || scratchpads > 0) {
            event.preventDefault();
            quitRequest = { processes, scratchpads, hostScreens: hostScreensOpen(), quit: () => { win.destroy(); } };
          }
        }).then((fn) => { quitCleanup = fn; });
      });
    } else if (shellKind === "electron") {
      const electron = (window as any).__SPLITTY_ELECTRON__;
      const forceClose = () => electron.invoke("window:force-close");
      quitCleanup = electron.onCloseRequested(() => {
        const processes = runningOnQuit();
        const scratchpads = scratchpadsOnQuit();
        if (processes.length > 0 || scratchpads > 0) {
          quitRequest = { processes, scratchpads, hostScreens: hostScreensOpen(), quit: forceClose };
        } else {
          forceClose();
        }
      });
    } else {
      // Web / PWA: use native beforeunload dialog
      const handleBeforeUnload = (e: BeforeUnloadEvent) => {
        if (runningOnQuit().length > 0 || scratchpadsOnQuit() > 0) {
          e.preventDefault();
        }
      };
      window.addEventListener("beforeunload", handleBeforeUnload);
      quitCleanup = () => window.removeEventListener("beforeunload", handleBeforeUnload);
    }

    return () => {
      unsubTheme();
      unregisterRemote?.();
      unsubClose();
      unsubTemplate();
      cleanupNotifications();
      unsubToast();
      resizeCleanup?.();
      quitCleanup?.();
    };
  });

  const authState = $derived(getAuthState());
  const authError = $derived(getAuthError());
</script>

<div
  class="app splitty-theme"
  class:frameless={isFrameless}
  style="
    background: {theme.ui['surface-base']};
    width: 100%;
    height: 100%;
    overflow: hidden;
    --splitty-window-border-radius: {maximized ? '0px' : `${settings().appearance.borderRadius}px`};
    {settings().window.titleBarHeight > 0 ? `--splitty-titlebar-height: ${settings().window.titleBarHeight}px;` : ''}
    {settings().appearance.fontSize > 0 ? `--splitty-font-size: ${settings().appearance.fontSize}px;` : ''}
    {settings().appearance.fontFamily ? `--splitty-font-family: ${settings().appearance.fontFamily};` : ''}
  "
>
  <ConfirmCloseDialog
    open={closeRequest !== null}
    processName={closeRequest?.processName ?? ""}
    applicationTitle={closeRequest?.applicationTitle}
    scratchpads={closeRequest?.scratchpads ?? 0}
    onConfirm={() => {
      closeRequest?.close();
      closeRequest = null;
    }}
    onCancel={() => { closeRequest = null; }}
  />

  <ConfirmQuitDialog
    open={quitRequest !== null}
    processes={quitRequest?.processes ?? []}
    hostScreens={quitRequest?.hostScreens ?? false}
    scratchpads={quitRequest?.scratchpads ?? 0}
    onConfirm={() => {
      quitRequest?.quit();
      quitRequest = null;
    }}
    onCancel={() => { quitRequest = null; }}
  />

  <ConfirmTemplateDialog
    open={templateRequest !== null}
    templateName={templateRequest?.templateName ?? ""}
    commands={templateRequest?.commands ?? []}
    onRunAll={() => { templateRequest?.approveFull(); templateRequest = null; }}
    onLayoutOnly={() => { templateRequest?.approveLayoutOnly(); templateRequest = null; }}
    onCancel={() => { templateRequest?.cancel(); templateRequest = null; }}
  />

  <TokenDialog
    open={authState === "needsToken"}
    error={authError}
    onSubmit={handleTokenSubmit}
  />

  {#if isFrameless}
    <TitleBar title={windowTitle} {maximized} />
    {#if shellKind === "tauri" && !maximized}
      <ResizeEdges />
    {/if}
  {/if}

  {#if isPWA}
    <div class="pwa-titlebar">
      <span class="pwa-titlebar-title">{windowTitle}</span>
    </div>
  {/if}

  <!-- Mounted through a reconnect: unmounting would dispose every pane, and a
       disposed pane ends its session (`close`, queued until the socket is back). -->
  {#if authState === "connected" || authState === "reconnecting"}
    <Workspace onTitleChange={(t) => { windowTitle = t; }} />
  {/if}

  <ContextMenu />
  <ShortcutsOverlay />
  <CommandPalette />
  {#if remoteWindows.available}
    <HostPickerDialog
      open={isHostPickerOpen()}
      title="Open host in new window"
      colors={{ bg: theme.ui['surface-base'], fg: theme.ui['text-primary'], danger: theme.ui['danger'] ?? '#e06c75' }}
      hosts={() => remoteWindows.hosts()}
      opener={remoteWindows}
      initialTarget={getHostPickerTarget()}
      onClose={() => { setHostPickerOpen(false); }}
    />
  {/if}
  <Toasts theme={theme.variant} />
</div>

<style>
.app.frameless {
  display: flex;
  flex-direction: column;
  border-radius: var(--splitty-window-border-radius, 1px);
  overflow: hidden;
  transition: border-radius 0.15s ease;
}

.app.frameless :global(.dv-dockview) {
  flex: 1;
  min-height: 0;
}

.pwa-titlebar {
  app-region: drag;
  display: flex;
  align-items: center;
  justify-content: center;
  height: var(--splitty-titlebar-height, 40px);
  flex-shrink: 0;
  background: var(--splitty-surface-raised);
  user-select: none;
  padding-left: env(titlebar-area-x, 0);
  padding-right: calc(100% - env(titlebar-area-x, 0) - env(titlebar-area-width, 100%));
}

.pwa-titlebar-title {
  font-size: 12px;
  color: var(--splitty-text-secondary);
  pointer-events: none;
}
</style>
