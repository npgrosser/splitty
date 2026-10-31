import type { IContentRenderer, GroupPanelPartInitParameters } from "dockview-core";
import type { RestoreMode } from "../lib/protocol/types.ts";
import { AttachTransferredError, type TerminalSession } from "../lib/terminal/terminal_session.ts";
import { attachTerminalSession, createTerminalSession } from "../lib/terminal/terminal_mount.ts";
import { PaneSpawnRejectedError, PaneSpawnedTimeoutError } from "../lib/mux/mux_client.ts";
import { bindingForApi, type ScreenBinding } from "../lib/connection/screen_binding.ts";
import { getTheme, subscribeTheme } from "../lib/theme.ts";
import { getSettings } from "../lib/settings.ts";
import { normalizeKnownShellName } from "../lib/shell.ts";
import { showContextMenu } from "../stores/ui.svelte.ts";
import { openPaneLink } from "../stores/ports.svelte.ts";
import { paneMenuItems } from "../lib/context_menus.ts";
import { setupScrollToBottom } from "./scroll_to_bottom.ts";
import { setupTerminalSearch } from "./terminal_search.ts";
import { setupPtySizeMarker } from "./pty_size_marker.ts";
import { mountManagedJob } from "../lib/terminal/managed_job_mount.ts";
import { ScratchpadArea } from "./scratchpad_area.ts";


export interface TerminalPaneParams {
  jobId?: string;
  jobState?: string;
  jobError?: string | null;
  command?: string;
  dir?: string;
  root?: string;
  restoreMode?: RestoreMode;
  shellProfileId?: string;
  existingSessionId?: string;
  /** The server spawns this pane's PTY (a mux op was sent for it) — wait for
   *  `mux.pane_spawned` and attach instead of creating a session directly. */
  muxManaged?: boolean;
}

/** Shorten the server's home directory to ~/... for display. */
function shortenHome(path: string, home: string): string {
  if (!home) return path;
  if (path === home) return "~";
  if (path.startsWith(home + "/")) return "~" + path.slice(home.length);
  return path;
}

function formatTitle(proc: string, cwd: string, home: string): string {
  if (!proc) return "Terminal";
  const shellName = normalizeKnownShellName(proc);
  if (shellName) {
    return cwd ? `${shellName}: ${shortenHome(cwd, home)}` : shellName;
  }
  return proc;
}

/**
 * Imperative terminal pane renderer for dockview-core.
 *
 * Implements IContentRenderer: creates a TerminalSession, handles
 * resize debouncing, focus/active state sync, and cleanup.
 *
 * The pane's server is the connection of its screen's binding, found from the
 * layout API; a pane whose layout has no binding is not mounted. Its terminal
 * takes focus only while its screen is the active one.
 */
export class TerminalPaneRenderer implements IContentRenderer {
  readonly element: HTMLElement;

  private binding: ScreenBinding | null = null;
  private paneId: string | null = null;
  private session: TerminalSession | null = null;
  /** The terminal and its overlays; the scratchpad sits below it. */
  private terminalBox: HTMLElement;
  private container: HTMLElement;
  private scratchpad: ScratchpadArea | null = null;
  private cleanups: (() => void)[] = [];
  private debounceTimer: number | undefined;
  private lastFitWidth = 0;
  private lastFitHeight = 0;
  private disposed = false;

  constructor() {
    // Outer wrapper — captures mouse events for activation
    this.element = document.createElement("div");
    this.element.style.width = "100%";
    this.element.style.height = "100%";
    this.element.style.overflow = "hidden";
    this.element.style.position = "relative";
    this.element.style.display = "flex";
    this.element.style.flexDirection = "column";

    this.terminalBox = document.createElement("div");
    this.terminalBox.style.position = "relative";
    this.terminalBox.style.flex = "1 1 auto";
    this.terminalBox.style.minHeight = "0";
    this.terminalBox.style.overflow = "hidden";
    this.element.appendChild(this.terminalBox);

    // Inner container — terminal mounts here
    this.container = document.createElement("div");
    this.container.style.width = "100%";
    this.container.style.height = "100%";
    this.container.style.overflow = "hidden";
    this.container.style.background = getTheme().ui["surface-base"];
    this.terminalBox.appendChild(this.container);
  }

  init(params: GroupPanelPartInitParameters): void {
    const api = params.api;
    const paneParams = params.params as TerminalPaneParams | undefined;
    const binding = bindingForApi(params.containerApi);
    if (!binding) {
      console.warn(`pane ${api.id}: its layout belongs to no screen, not mounted`);
      return;
    }
    this.binding = binding;
    this.paneId = api.id;
    this.element.dataset.paneId = api.id;
    const conn = binding.conn;
    const focusIfActive = () => {
      // The scratchpad keeps the keyboard it has.
      if (this.scratchpad?.contains(document.activeElement)) return;
      if (!binding.suppressFocus && api.isActive && binding.isActive()) this.session?.emulator.focus();
    };
    // A pane takes part in its session's PTY size only while it is shown.
    const isShown = () => api.isVisible && binding.isActive();
    const syncShown = () => this.session?.setShown(isShown());

    // Sync background with theme changes
    const unsubTheme = subscribeTheme(() => {
      this.container.style.background = getTheme().ui["surface-base"];
    });
    this.cleanups.push(unsubTheme);

    // Click-to-activate
    this.element.addEventListener("mousedown", (e: MouseEvent) => {
      if (!api.isActive) api.setActive();
      if (!this.scratchpad?.contains(e.target as Node)) this.session?.emulator.focus();
    }, true);

    // Pane context menu
    this.element.addEventListener("contextmenu", (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      showContextMenu(paneMenuItems(api.id, params.containerApi), e.clientX, e.clientY);
    });

    // Resize fit — debounced per experimental.resizeDebounce setting
    const doFit = () => {
      this.session?.fit();
      this.lastFitWidth = this.container.offsetWidth;
      this.lastFitHeight = this.container.offsetHeight;
    };

    const debouncedFit = () => {
      const { resizeDebounce } = getSettings().experimental;
      clearTimeout(this.debounceTimer);
      if (resizeDebounce <= 0) {
        doFit();
      } else {
        this.debounceTimer = window.setTimeout(doFit, resizeDebounce);
      }
    };

    const fitIfResized = () => {
      if (this.container.offsetWidth !== this.lastFitWidth || this.container.offsetHeight !== this.lastFitHeight) {
        debouncedFit();
      }
    };

    // A host's pane opens a link to the host's loopback at its forward
    // (`remote-screens`, Detected Ports In The Window); a local pane's links
    // open as they are.
    const openLink = conn.kind === "remote" ? (url: string) => openPaneLink(conn.key, url) : undefined;

    const spawnDirectly = () => createTerminalSession(this.container, conn.sessions, {
      command: paneParams?.command,
      dir: paneParams?.dir,
      root: paneParams?.root,
      restoreMode: paneParams?.restoreMode,
      shellProfileId: paneParams?.shellProfileId,
      paneId: api.id,
      screenId: binding.wireId,
      shown: isShown,
    }, openLink);

    const mountTerminal = (existingSessionId?: string) => {
      const managed = mountManagedJob({ ...paneParams, existingSessionId }, {
        attach: (id) => attachTerminalSession(this.container, conn.sessions, id, {
          shellProfileId: paneParams?.shellProfileId,
          shown: isShown,
        }, openLink),
        showOutcome: (text) => { this.container.textContent = text; },
      });
      if (managed && !managed.promise) return () => {};
      let createPromise: Promise<TerminalSession>;

      if (managed?.promise) {
        createPromise = managed.promise;
      } else if (existingSessionId) {
        // Snapshot rebuild or location change — attach to known session
        createPromise = attachTerminalSession(this.container, conn.sessions, existingSessionId, {
          shellProfileId: paneParams?.shellProfileId,
          shown: isShown,
        }, openLink);
      } else if (paneParams?.muxManaged) {
        // Mux-managed: server spawns PTY, we wait for pane_spawned then attach.
        // Falls back to a direct spawn when the server rejected the op or is too
        // slow — a pane that never gets its session stays blank forever, which is
        // far worse than a duplicate spawn: the server resolves our paneId to the
        // same session id, so get-or-create makes the duplicate harmless.
        createPromise = conn.mux.waitForPaneSpawned(api.id)
          .then((sessionId) => attachTerminalSession(this.container, conn.sessions, sessionId, {
            shellProfileId: paneParams?.shellProfileId,
            shown: isShown,
          }, openLink))
          .catch((err) => {
            const recoverable = err instanceof PaneSpawnedTimeoutError
              || err instanceof PaneSpawnRejectedError;
            if (!recoverable || paneParams?.jobId) throw err;
            console.warn(`pane ${api.id}: ${err.message} — spawning directly`);
            return spawnDirectly();
          });
      } else {
        // No mux (standalone/fallback) — create directly
        createPromise = spawnDirectly();
      }

      let disposed = false;

      createPromise.then((s) => {
        if (disposed) {
          // The pane went away while its session was still being set up. The
          // server owns the PTY of a screen's pane (close_pane kills it) and the
          // same pane id may be re-created by a snapshot rebuild, so only let go
          // locally — killing it here would take down a live session. A newer
          // view that took this pane's attach over gets the session instead
          // (attachTerminalSession rejects then), and detach() leaves the
          // stream alone while another view holds or waits for the session.
          s.detach();
          return;
        }
        this.session = s;
        binding.linkPane(api.id, s.id);
        // The session's scratchpad, where its server keeps one (`session-scratchpad`).
        if (conn.mux.scratchpads && !this.scratchpad) {
          conn.scratchpads.link(api.id, s.id);
          this.scratchpad = new ScratchpadArea({
            paneId: api.id,
            sessionId: s.id,
            store: conn.scratchpads,
            screenId: () => binding.wireId,
            saveImage: (data) => s.client.saveImage(data),
            uploadFile: (file, onProgress) => s.client.uploadFile(file, onProgress),
            focusTerminal: () => this.session?.emulator.focus(),
          });
          this.element.appendChild(this.scratchpad.element);
        }

        focusIfActive();

        const unsubTitle = s.onTitleChange((title, cwd) => {
          if (this.disposed) return;
          if (paneParams?.jobId) return;
          api.setTitle(formatTitle(title, cwd, conn.sessions.homeDir));
        });
        this.cleanups.push(unsubTitle);

        const unsubEnd = s.onEnd((exitCode) => {
          if (this.disposed) return;
          if (paneParams?.jobId) return; // Keep the output of completed jobs visible.
          const mode = getSettings().terminal.closeOnExit;
          if (mode === "always" || (mode === "graceful" && exitCode === 0)) {
            api.close();
          }
        });
        this.cleanups.push(unsubEnd);

        // Scroll-to-bottom pill
        const unsubScrollPill = setupScrollToBottom(this.terminalBox, s.emulator);
        this.cleanups.push(unsubScrollPill);

        // Terminal search (Cmd+F / Ctrl+F)
        const search = setupTerminalSearch(this.terminalBox, s.emulator);
        this.cleanups.push(() => search.dispose());

        // A PTY size set by another client
        this.cleanups.push(setupPtySizeMarker(this.terminalBox, this.container, s));
      }).catch((err) => {
        if (err instanceof AttachTransferredError) {
          // A newer view of the same session (e.g. a snapshot rebuild of this
          // pane) took the attach over; it shows the session now.
          console.debug(`pane ${api.id}: ${err.message}`);
          return;
        }
        console.error(`pane ${api.id}: failed to mount terminal`, err);
      });

      return () => { disposed = true; };
    };

    // Always mount immediately so background tabs receive events (title
    // updates, exit notifications, etc.).  The emulator's openDeferred
    // handles the case where the container has no layout yet, and the
    // visibility-change refit below corrects dimensions once shown.
    let disposeMount: (() => void) | undefined = mountTerminal(paneParams?.existingSessionId);
    // Cancel an in-flight mount when the pane is disposed, so a session that
    // arrives late is released instead of wiring itself into a dead renderer.
    this.cleanups.push(() => disposeMount?.());

    // ResizeObserver for container
    const ro = new ResizeObserver(() => debouncedFit());
    ro.observe(this.container);
    this.cleanups.push(() => ro.disconnect());

    // Dockview dimension/visibility changes
    const dimDisposable = api.onDidDimensionsChange(() => debouncedFit());
    this.cleanups.push(() => dimDisposable.dispose());

    const visDisposable2 = api.onDidVisibilityChange(() => {
      if (api.isVisible) debouncedFit();
      syncShown();
    });
    this.cleanups.push(() => visDisposable2.dispose());

    // Browser tab visibility — xterm.js loses render metrics while hidden
    const onDocVisible = () => {
      if (document.visibilityState === "visible" && api.isVisible) {
        debouncedFit();
      }
    };
    document.addEventListener("visibilitychange", onDocVisible);
    this.cleanups.push(() => document.removeEventListener("visibilitychange", onDocVisible));

    // Active state → focus/blur + cursor blink toggle
    const activeDisposable = api.onDidActiveChange(() => {
      if (!this.session) return;
      if (api.isActive) {
        // Blink only when the user asked for it — forcing true here made a
        // freshly opened pane (created with the configured value) start static
        // and then blink as soon as it was focused once.
        this.session.emulator.setCursorBlink?.(getSettings().terminal.xterm.cursorBlink);
        // Only on the active screen: a snapshot rebuild of a hidden screen
        // activates its panes too, and must not pull the keyboard there.
        requestAnimationFrame(focusIfActive);
      } else {
        this.session.emulator.setCursorBlink?.(false);
      }
    });
    this.cleanups.push(() => activeDisposable.dispose());

    // The screen becomes the active one: its active pane takes the keyboard,
    // which the previous screen lost when it became inert.
    this.cleanups.push(binding.onActivate(() => requestAnimationFrame(focusIfActive)));
    this.cleanups.push(binding.onActivate(syncShown), binding.onDeactivate(syncShown));

    // Location change (grid ↔ popout): re-init terminal in new document context
    let lastLocationType = api.location.type;
    const locationDisposable = api.onDidLocationChange(() => {
      const newType = api.location.type;
      if (newType === lastLocationType) return;
      lastLocationType = newType;

      if (!this.session) return;
      const sessionId = this.session.id;
      this.session.detach();
      this.session = null;

      disposeMount?.();
      this.container.innerHTML = "";
      disposeMount = mountTerminal(sessionId);
    });
    this.cleanups.push(() => locationDisposable.dispose());
  }

  dispose(): void {
    this.disposed = true;
    clearTimeout(this.debounceTimer);
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];

    if (this.paneId) this.binding?.unlinkPane(this.paneId);
    this.scratchpad?.dispose();
    this.scratchpad = null;

    // A local screen's pane ends its session; a binding with the `detach`
    // teardown only lets go of it.
    if (this.session) {
      this.binding?.release(this.session);
      this.session = null;
    }
  }
}
