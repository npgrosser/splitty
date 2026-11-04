import { NativeBroker } from "./native_broker";
import { NativeOwner } from "./native_owner";
import { startNativeHelper } from "./native_helper";
import { readWindowSettings, readKnownHosts } from "./native_settings";
import type { PipeEvents, NativeClosed } from "./native_pipe";
import { installAssets } from "./asset_protocol";
import { isPackagedDocument } from "./navigation";
import { isPackagedBlob, rememberDownload, savedDownload } from "./downloads";
import { openLocalEditor, openRemoteEditor } from "./editor";
import { app, BrowserWindow, Menu, shell, ipcMain, protocol, session, webContents, MessageChannelMain, type Session, type WebContents } from "electron";
import path from "path";
import {
  isAcceptableTarget, remoteTitle, migrationOffer, type MigrationOffer, type KnownHost, type ConnectionState,
} from "./remote";

// Secure packaged scheme; privileges never bypass CSP or enable service workers.
protocol.registerSchemesAsPrivileged([{ scheme: "splitty", privileges: { standard: true, secure: true, supportFetchAPI: true } }]);

const APP_ORIGIN = "splitty://app";
const APP_URL = `${APP_ORIGIN}/index.html`;
const broker = new NativeBroker(() => new MessageChannelMain(), isOwnDocument);
let owner: NativeOwner | undefined;
const remoteHelpers = new Set<ReturnType<typeof startNativeHelper>>();
ipcMain.on("native:document", event => broker.document(event));
ipcMain.on("native:open", (event, input) => broker.open(event, input));
ipcMain.on("native:cancel-open", (event, input) => broker.cancel(event, input));
let windowSettings: { nativeTitleBar: boolean } = { nativeTitleBar: false };

function findBinary(): string {
  if (app.isPackaged) {
    return path.join(process.resourcesPath, "splitty");
  }
  return path.join(__dirname, "..", "..", "target", "release", "splitty");
}

function cleanEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env };
  if (process.platform === "linux") {
    // Remove AppImage runtime variables so child shells behave normally.
    for (const key of ["APPIMAGE", "APPDIR", "OWD", "ARGV0", "APPIMAGE_EXTRACT_AND_RUN"]) {
      delete env[key];
    }
    if (env.PATH) {
      env.PATH = env.PATH.split(path.delimiter)
        .filter((p) => !p.startsWith("/tmp/.mount_"))
        .join(path.delimiter);
    }
  }
  return env;
}

// ---------------------------------------------------------------------------
// Per-WebContents policy: the one origin each window belongs to
// ---------------------------------------------------------------------------

/**
 * - `main`: a window on the app's own backend; the only role whose IPC calls
 *   are served.
 * - `popout`: a window the main UI opened on that origin (dockview pop-outs).
 * - `remote`: a packaged remote window, assigned one fixed SSH host.
 */
type WindowRole = "main" | "popout" | "remote";

interface WindowPolicy {
  /** The fixed packaged application origin. */
  origin: string;
  role: WindowRole;
}

/**
 * Registered for every window before it loads anything. A WebContents without a
 * policy navigates nowhere, opens nothing and is granted nothing, so one that
 * was created without registering stays inert.
 */
const policies = new WeakMap<WebContents, WindowPolicy>();

function policyOf(contents: WebContents): WindowPolicy | undefined {
  const own = policies.get(contents);
  if (own) return own;
  // Chromium creates a pop-out for a `window.open` its opener's handler
  // allowed. It inherits the opener's origin, never its role.
  const opener = contents.opener ? webContents.fromFrame(contents.opener) : undefined;
  const parent = opener ? policies.get(opener) : undefined;
  if (!parent || parent.role === "remote") return undefined;
  const inherited: WindowPolicy = { origin: parent.origin, role: "popout" };
  policies.set(contents, inherited);
  return inherited;
}

/**
 * The origin of a URL, or null for anything unparseable.
 *
 * Null is the direction that matters: this decides whether a navigation is
 * *allowed*, and it runs inside event handlers. An exception thrown here would
 * abort the handler before `preventDefault()` ran, so a malformed URL would
 * navigate through — the failure mode has to be "block", never "throw".
 */
function originOf(url: string): string | null {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "splitty:" && parsed.host === "app" ? APP_ORIGIN : parsed.origin;
  } catch {
    return null;
  }
}

/** Does this URL name the entry assigned to this WebContents? */
function isOwnDocument(contents: WebContents, url: string): boolean {
  const policy = policyOf(contents);
  return policy !== undefined && isPackagedDocument(policy.role, url);
}

/**
 * Hand a URL to the system browser if, and only if, it is a web URL.
 *
 * Parsed rather than prefix-matched, so whatever reaches `shell.openExternal`
 * is the normalized form of a URL that really has an `http:` or `https:`
 * scheme. Everything else — `file:`, `javascript:`, OS protocol handlers — is
 * dropped.
 */
function openExternally(targetUrl: string): void {
  let parsed: URL;
  try {
    parsed = new URL(targetUrl);
  } catch {
    return;
  }
  if (parsed.protocol === "http:" || parsed.protocol === "https:") {
    void shell.openExternal(parsed.toString());
  }
}

/**
 * Browser permissions the UI uses: desktop notifications for bells and
 * finished commands, and writing to the clipboard on copy. Electron grants
 * every permission by default; everything not listed here is denied, and these
 * only to the window's packaged application origin
 * (D7).
 */
const ALLOWED_PERMISSIONS = new Set(["notifications", "clipboard-sanitized-write"]);

/**
 * Chromium passes no WebContents for some checks (notifications, cross-origin
 * subframes); those are decided by the session's own origin. Each session
 * belongs to one origin: the default session to the backend, a remote
 * window's private partition to packaged assets.
 */
function permissionAllowed(
  permission: string,
  requestingUrl: string,
  contents: WebContents | null,
  sessionOrigin: () => string | null,
): boolean {
  if (!ALLOWED_PERMISSIONS.has(permission)) return false;
  const origin = contents ? policyOf(contents)?.origin : sessionOrigin();
  return !!origin && originOf(requestingUrl) === origin;
}

function guardPermissions(ses: Session, sessionOrigin: () => string | null): void {
  ses.setPermissionRequestHandler((contents, permission, callback, details) => {
    callback(permissionAllowed(permission, details.requestingUrl, contents, sessionOrigin));
  });
  ses.setPermissionCheckHandler((contents, permission, requestingOrigin) => {
    return permissionAllowed(permission, requestingOrigin, contents, sessionOrigin);
  });
}

/**
 * Guards for every WebContents the app creates, windows and popouts alike
 * (security review SR-27/SR-28, see docs/security/findings.md).
 *
 * Applied from `web-contents-created` rather than per window, so a window added
 * later cannot be created without them. They read the window's policy when they
 * decide, which its creator registers before loading anything.
 */
function hardenWebContents(contents: WebContents): void {
  // Popouts of the app itself (dockview opens `/popout.html` on the backend
  // origin) stay in Splitty, with the same locked-down preferences. A remote
  // window opens none, its own pop-outs included (D6). Every other web target
  // goes to the system browser, and anything else is dropped. This is the
  // native backstop for code that bypasses `openExternalTarget`, such as
  // ghostty-web's bare `window.open` on OSC 8 hyperlinks.
  contents.setWindowOpenHandler(({ url: targetUrl }) => {
    const policy = policyOf(contents);
    if (!policy) return { action: "deny" };
    if (originOf(targetUrl) === policy.origin) {
      if (policy.role === "remote" || !isPackagedDocument("popout", targetUrl)) return { action: "deny" };
      return {
        action: "allow",
        overrideBrowserWindowOptions: {
          webPreferences: {
            preload: path.join(__dirname, "popout_preload.js"),
            sandbox: true,
            contextIsolation: true,
            nodeIntegration: false,
            devTools: !app.isPackaged,
          },
        },
      };
    }
    openExternally(targetUrl);
    return { action: "deny" };
  });

  // Top-level navigation stays on the window's assigned entry. Without this, anything
  // able to set `location` in the renderer could move the privileged window —
  // onto a foreign page. Web URLs go to the system browser, `file:` and the rest
  // nowhere.
  contents.on("will-navigate", (event) => {
    if (isOwnDocument(contents, event.url)) return;
    event.preventDefault();
    if (policyOf(contents)) openExternally(event.url);
  });

  // A server-side redirect is a navigation as well.
  contents.on("will-redirect", (event) => {
    if (event.isMainFrame && !isOwnDocument(contents, event.url)) event.preventDefault();
  });

  // Renderers must never gain Node privileges through an attached webview tag.
  contents.on("will-attach-webview", (event) => {
    event.preventDefault();
  });
}

function createWindow(url: string, opts: { nativeTitleBar: boolean }): BrowserWindow {
  const isMac = process.platform === "darwin";
  const frameless = !opts.nativeTitleBar;

  const win = new BrowserWindow({
    width: 1280,
    height: 800,
    backgroundColor: "#181818",
    ...(frameless
      ? (isMac ? { titleBarStyle: "hiddenInset" as const } : { frame: false })
      : {}),
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      // The renderer runs sandboxed (security review SR-11). With
      // `sandbox: false` the preload script had full Node access, so any code
      // execution in the renderer reached the filesystem and process APIs
      // directly. The preload only uses `contextBridge` and `ipcRenderer` with a
      // channel allow list, both of which work under the sandbox.
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      // A packaged build ships no inspection surface for the window that holds
      // the session (the application menu that would carry the shortcut is
      // removed as well).
      devTools: !app.isPackaged,
    },
  });

  // Navigation, popup and webview guards are attached to every WebContents in
  // `hardenWebContents`; they pin this one to the backend.
  policies.set(win.webContents, { origin: APP_ORIGIN, role: "main" });
  const backend = owner;
  if (!backend) throw Error("Embedded backend is unavailable");
  broker.grant(win.webContents, { role: "main", open: (events, target, update) => backend.connect(events, target, update) });

  // On Linux with frameless windows, the WM reserves the top few pixels of a
  // maximized window for its own resize edge detection, stealing mouse events
  // from our custom titlebar. Disabling resize when maximized removes the WM's
  // resize zone so the entire titlebar remains draggable.
  if (frameless && process.platform === "linux") {
    win.on("maximize", () => { win.setResizable(false); });
    win.on("unmaximize", () => { win.setResizable(true); });
  }

  // Intercept window close to let the renderer show a confirmation dialog
  // when terminal processes are still running.
  win.on("close", (e) => {
    if (!(win as any)._forceClose) {
      e.preventDefault();
      win.webContents.send("window:close-requested");
    }
  });

  win.loadURL(url);
  return win;
}

// ---------------------------------------------------------------------------
// Remote windows: packaged UI with a fixed SSH native helper
// ---------------------------------------------------------------------------

/** What `remote:open` resolves with; `ipcRenderer.invoke` rejections lose their fields. */
type RemoteOpenResult = { ok: true } | { ok: false; error: { message: string; hint?: string; migration?: MigrationOffer } };

function openFailure(message: string, hint?: string): RemoteOpenResult {
  let migration: MigrationOffer | undefined;
  try {
    const diagnostic = JSON.parse(message);
    if (diagnostic?.type === "remote.error" && typeof diagnostic.message === "string") {
      message = diagnostic.message;
      if (typeof diagnostic.hint === "string") hint = diagnostic.hint;
      migration = migrationOffer(diagnostic.migration);
    }
  } catch { /* Ordinary native diagnostic. */ }
  return { ok: false, error: { message, ...(hint ? { hint } : {}), ...(migration ? { migration } : {}) } };
}

interface RemoteWindow {
  target: string; win: BrowserWindow; state: ConnectionState;
  connected: boolean; rendererGone: boolean;
  helpers: Set<ReturnType<typeof startNativeHelper>>; rememberNextOpen: boolean; opensAllowed: boolean;
  lastFailure?: NativeClosed;
  ready(): void; failed(error: Error): void;
}
interface PendingOpen {
  update: boolean;
  promise: Promise<RemoteOpenResult>;
  helper?: ReturnType<typeof startNativeHelper>;
  requesters: Map<WebContents, { document: string; remove(): void }>;
  cancel(requester: WebContents): void;
}
const remoteWindows = new Map<string, RemoteWindow>();
const pendingOpens = new Map<string, PendingOpen>();
let remoteWindowCount = 0;
ipcMain.on("native:state", (event, input) => {
  const state = broker.remoteState(event, input);
  if (!state) return;
  for (const entry of remoteWindows.values()) if (entry.win.webContents === event.sender && !entry.win.isDestroyed()) {
    entry.state = state; entry.rendererGone = false;
    entry.win.setTitle(remoteTitle(entry.target, state));
    if (state === "connected") { entry.connected = true; entry.ready(); }
    else if (state === "disconnected") entry.failed(Error(entry.lastFailure?.message ?? "The remote workspace could not finish connecting"));
    break;
  }
});

function reportRemoteProgress(target: string, value: { phase: string; message: string }): void {
  const pending = pendingOpens.get(target);
  if (!pending) return;
  for (const [contents, grant] of pending.requesters) if (requesterCurrent(contents, grant.document)) {
    contents.send("remote:progress", { target, step: value.phase, message: value.message });
  }
}

function remoteHelper(target: string, events: PipeEvents, rememberTarget = false, update = false) {
  const helper = startNativeHelper(findBinary(), cleanEnv(), events, target, undefined, false, rememberTarget, update);
  remoteHelpers.add(helper);
  void helper.exited.then(() => remoteHelpers.delete(helper));
  return helper;
}

function openRemote(target: string, event: Electron.IpcMainInvokeEvent, update = false): Promise<RemoteOpenResult> {
  const requester = event.sender, document = broker.callerDocument(event);
  if (!document) return Promise.resolve(openFailure("The requesting document is no longer assigned"));
  // An existing but not yet ready window still belongs to its coalesced open.
  const active = pendingOpens.get(target);
  if (active) {
    if (active.update !== update) return Promise.resolve(openFailure("Another connection is active for this host. Wait for it to finish before updating."));
    addRequester(active, requester, document); return active.promise;
  }
  const existing = remoteWindows.get(target);
  if (existing && !existing.win.isDestroyed()) {
    if (update) {
      if (existing.state !== "disconnected") return Promise.resolve(openFailure("The host is still connected or reconnecting. Wait for it to finish before updating."));
      // Never arm an existing document: automatic reconnects must not consume
      // a launcher's confirmation. Retire its attachment and open a fresh one.
      existing.opensAllowed = false;
      existing.win.destroy();
    } else {
      if (existing.win.isMinimized()) existing.win.restore();
      existing.win.focus();
      if (existing.rendererGone || existing.state !== "connected") return reopenRemote(existing, requester, document);
      return Promise.resolve({ ok: true });
    }
  }
  if (pendingOpens.size >= 4 || remoteWindows.size + pendingOpens.size >= 32) return Promise.resolve(openFailure("Remote window capacity exhausted"));
  const requesters: PendingOpen["requesters"] = new Map();
  let openedWindow: BrowserWindow | undefined, cancelled = false;
  let assignedEvents: PipeEvents | undefined;
  let initialFailure: NativeClosed | undefined;
  const helper = remoteHelper(target, {
    closed: reason => { initialFailure = reason; assignedEvents?.closed(reason); },
    progress: value => {
      assignedEvents?.progress?.(value);
      reportRemoteProgress(target, value);
    },
  }, true, update);
  const windowHelpers = new Set([helper]);
  void helper.exited.then(() => windowHelpers.delete(helper));
  const promise = helper.pipe.ready.then(async () => {
    if (initialFailure) throw Error(initialFailure.message);
    if (cancelled || ![...requesters].some(([contents, grant]) => requesterCurrent(contents, grant.document))) {
      throw Error("The requesting document is no longer assigned");
    }
    const partition = `remote-${++remoteWindowCount}`;
    const isolatedSession = session.fromPartition(partition);
    installAssets(isolatedSession, path.join(__dirname, "ui"));
    guardPermissions(isolatedSession, () => APP_ORIGIN);
    isolatedSession.on("will-download", event => event.preventDefault());
    const win = new BrowserWindow({ width: 1280, height: 800, backgroundColor: "#181818",
      title: remoteTitle(target, "reconnecting"), webPreferences: {
        partition, preload: path.join(__dirname, "preload.js"), sandbox: true, contextIsolation: true,
        nodeIntegration: false, devTools: !app.isPackaged,
      } });
    openedWindow = win;
    policies.set(win.webContents, { origin: APP_ORIGIN, role: "remote" });
    let initial = true;
    broker.grant(win.webContents, { role: "remote", target, open: events => {
      // Cancellation revokes pending retry/backoff intents before a delayed
      // renderer event can allocate another child under the same document.
      if (!entry.opensAllowed) throw Error("The remote connection attempt was cancelled");
      if (initial) {
        initial = false; assignedEvents = events;
        // Ready may have been followed by EOF while the packaged page loaded.
        // Replay the terminal state before handing off the already-settled
        // promises, so the opening client receives failure instead of hanging.
        if (initialFailure) events.closed(initialFailure);
        return { pipe: helper.pipe, released: helper.exited };
      }
      const remember = entry.rememberNextOpen; entry.rememberNextOpen = false;
      const next = remoteHelper(target, {
        progress: value => { events.progress?.(value); reportRemoteProgress(target, value); },
        closed: reason => { entry.lastFailure = reason; events.closed(reason); },
      }, remember);
      windowHelpers.add(next); void next.exited.then(() => windowHelpers.delete(next));
      return { pipe: next.pipe, released: next.exited };
    } });
    let ready!: () => void, failed!: (error: Error) => void;
    const usable = new Promise<void>((resolve, reject) => { ready = resolve; failed = reject; });
    // Loading and the UI handshake run concurrently; install rejection handling
    // immediately so a renderer failure during load is not unhandled.
    void usable.catch(() => {});
    const entry: RemoteWindow = { target, win, state: "reconnecting", connected: false, rendererGone: false,
      helpers: windowHelpers, rememberNextOpen: false, opensAllowed: true, ready, failed };
    remoteWindows.set(target, entry);
    win.on("page-title-updated", event => event.preventDefault());
    win.webContents.on("will-prevent-unload", event => event.preventDefault());
    win.webContents.on("render-process-gone", () => {
      entry.rendererGone = true; entry.state = "disconnected";
      if (!win.isDestroyed()) win.setTitle(remoteTitle(target, entry.state));
      entry.failed(Error("The remote workspace renderer stopped"));
    });
    win.on("closed", () => {
      entry.failed(Error("The remote window closed before connecting"));
      for (const owned of windowHelpers) owned.close();
      if (remoteWindows.get(target) === entry) remoteWindows.delete(target);
    });
    const startupDeadline = setTimeout(() => failed(Error("The remote workspace startup timed out")), 125_000);
    try { await win.loadURL(APP_URL); await usable; win.focus(); return { ok: true } as const; }
    catch (error) { win.destroy(); throw error; }
    finally { clearTimeout(startupDeadline); }
  }).catch(error => { helper.close(); return openFailure(error instanceof Error ? error.message : String(error)); })
    .finally(async () => {
      for (const grant of requesters.values()) grant.remove();
      requesters.clear();
      // Successful transfer is counted by remoteWindows. Cancellation/failure
      // retains opening capacity until the actual helper has been reaped.
      if (!openedWindow || openedWindow.isDestroyed()) await Promise.all([...windowHelpers].map(owned => owned.exited));
      if (pendingOpens.get(target)?.helper === helper) pendingOpens.delete(target);
    });
  const pending: PendingOpen = { promise, helper, requesters, update, cancel: contents => {
    requesters.get(contents)?.remove(); requesters.delete(contents);
    if (!requesters.size) { cancelled = true; helper.close(); openedWindow?.destroy(); }
  } };
  pendingOpens.set(target, pending);
  addRequester(pending, requester, document);
  return promise;
}

/** Keep deliberate retries coalesced until the existing document is connected.
 * Cancellation stops only its owned attachment, leaving the window available. */
function reopenRemote(entry: RemoteWindow, requester: WebContents, document: string): Promise<RemoteOpenResult> {
  if (pendingOpens.size >= 4) return Promise.resolve(openFailure("Remote window capacity exhausted"));
  const { target, win } = entry;
  const requesters: PendingOpen["requesters"] = new Map();
  let succeed!: () => void, fail!: (error: Error) => void;
  const usable = new Promise<void>((resolve, reject) => { succeed = resolve; fail = reject; });
  const previousReady = entry.ready, previousFailed = entry.failed;
  entry.ready = succeed; entry.failed = fail; entry.lastFailure = undefined; entry.opensAllowed = true;
  const stop = (message: string) => {
    entry.opensAllowed = false; entry.rememberNextOpen = false;
    for (const helper of entry.helpers) helper.pipe.close({ code: "cancelled", retryable: false, message });
    fail(Error(message));
  };
  const deadline = setTimeout(() => stop("The remote workspace startup timed out"), 125_000);
  let successful = false;
  const promise = usable.then(() => { successful = true; return { ok: true } as const; })
    .catch(error => openFailure(error instanceof Error ? error.message : String(error)))
    .finally(async () => {
      clearTimeout(deadline);
      entry.ready = previousReady; entry.failed = previousFailed;
      for (const grant of requesters.values()) grant.remove();
      requesters.clear();
      // Failure does not free opening capacity before child exit is observed.
      if (!successful) await Promise.all([...entry.helpers].map(helper => helper.exited));
      if (pendingOpens.get(target) === pending) pendingOpens.delete(target);
    });
  const pending: PendingOpen = { promise, requesters, update: false, cancel: contents => {
    requesters.get(contents)?.remove(); requesters.delete(contents);
    if (!requesters.size) stop("The requesting document is no longer assigned");
  } };
  pendingOpens.set(target, pending); addRequester(pending, requester, document);
  if (entry.rendererGone) { entry.rememberNextOpen = true; win.webContents.reload(); }
  else if (entry.state === "disconnected") {
    entry.rememberNextOpen = true;
    if (!broker.retryRemote(win.webContents)) stop("The remote document is no longer assigned. Reload this window.");
  }
  return promise;
}

function requesterCurrent(contents: WebContents, document: string): boolean {
  try { return broker.authorized({ sender: contents, senderFrame: contents.mainFrame }, document); }
  catch { return false; }
}
function addRequester(pending: PendingOpen, contents: WebContents, document: string): void {
  const current = pending.requesters.get(contents);
  if (current?.document === document) return;
  current?.remove();
  const revoked = () => pending.cancel(contents);
  const navigating = (details: Electron.Event<Electron.WebContentsDidStartNavigationEventParams>) => {
    if (details.isMainFrame && !details.isSameDocument && isOwnDocument(contents, details.url)) revoked();
  };
  const remove = () => {
    contents.removeListener("did-start-navigation", navigating);
    contents.removeListener("render-process-gone", revoked);
    contents.removeListener("destroyed", revoked);
  };
  pending.requesters.set(contents, { document, remove });
  contents.on("did-start-navigation", navigating);
  contents.on("render-process-gone", revoked);
  contents.once("destroyed", revoked);
  if (!requesterCurrent(contents, document)) revoked();
}
function cancelRemote(target: string, requester: WebContents): void { pendingOpens.get(target)?.cancel(requester); }

/** Coalesce UI metadata reads onto one bounded native client. */
let hostsRead: Promise<KnownHost[]> | undefined;
function knownHosts(): Promise<KnownHost[]> {
  if (!owner) return Promise.resolve([]);
  return hostsRead ??= readKnownHosts(owner).catch(() => []).finally(() => { hostsRead = undefined; });
}

// ---------------------------------------------------------------------------
// IPC handlers — resolve the target window from the event sender
// ---------------------------------------------------------------------------

/** Every exposed shell action is checked against the actual registered main
 * frame and the current private-preload challenge, including queued old IPC. */
function handleNativeCommand(channel: string, handler: (event: Electron.IpcMainInvokeEvent, ...args: any[]) => unknown): void {
  ipcMain.handle(channel, (event, envelope: unknown) => {
    if (!envelope || typeof envelope !== "object") throw Error("Native command has no document assignment");
    const value = envelope as { document?: unknown; args?: unknown };
    if (!broker.authorized(event, value.document) || !Array.isArray(value.args)) throw Error("Native command is not authorized for this document");
    return handler(event, ...value.args);
  });
}

function winFromEvent(event: Electron.IpcMainInvokeEvent): BrowserWindow | null {
  return BrowserWindow.fromWebContents(event.sender);
}

handleNativeCommand("window:minimize", (event) => {
  winFromEvent(event)?.minimize();
});

handleNativeCommand("window:maximize", (event) => {
  winFromEvent(event)?.maximize();
});

handleNativeCommand("window:unmaximize", (event) => {
  winFromEvent(event)?.unmaximize();
});

handleNativeCommand("window:toggle-maximize", (event) => {
  const win = winFromEvent(event);
  if (win) {
    win.isMaximized() ? win.unmaximize() : win.maximize();
  }
});

handleNativeCommand("window:close", (event) => {
  winFromEvent(event)?.close();
});

handleNativeCommand("window:force-close", (event) => {
  const win = winFromEvent(event);
  if (win) {
    (win as any)._forceClose = true;
    win.close();
  }
});

handleNativeCommand("window:start-drag", (event, screenX: number, screenY: number) => {
  const win = winFromEvent(event);
  if (!win) return;

  // If maximized, unmaximize and reposition so the cursor stays proportionally
  // in the same relative spot on the titlebar, then start the drag.
  if (win.isMaximized()) {
    const bounds = win.getBounds();
    const ratio = screenX / bounds.width;
    win.unmaximize();
    const newBounds = win.getBounds();
    const newX = Math.round(screenX - newBounds.width * ratio);
    const newY = screenY;
    win.setPosition(newX, newY);
  }

  // Start native window drag — this is a workaround for Linux WM edge zones
  // stealing mouse events at the top of the screen when maximized.
  // Electron doesn't have a direct startDrag for windows, so we rely on
  // the CSS app-region drag taking over after unmaximize + reposition.
});

// The zoom shortcuts zoom the whole page (display-zoom); the factor stays within their steps.
handleNativeCommand("window:set-zoom", (event, factor: unknown) => {
  if (typeof factor !== "number" || !Number.isFinite(factor)) return;
  event.sender.setZoomFactor(Math.min(3, Math.max(0.5, factor)));
});

handleNativeCommand("window:is-maximized", (event) => {
  return winFromEvent(event)?.isMaximized() ?? false;
});

handleNativeCommand("shell:open-external", (_event, url: unknown) => {
  if (typeof url === "string") openExternally(url);
});

/**
 * Is this call from the shell's own UI: the top frame of a main window on the
 * packaged origin? handleNativeCommand also verifies the actual current main
 * frame and document challenge. Remote preloads carry their assigned transport
 * but do not receive local shell-command authority.
 */
function fromOwnUi(event: Electron.IpcMainInvokeEvent): boolean {
  const policy = policies.get(event.sender);
  const frame = event.senderFrame;
  return policy?.role === "main" && !!frame && frame.parent === null && frame.origin === policy.origin;
}

handleNativeCommand("editor:open-remote", (event, input: unknown) => {
  const policy = policies.get(event.sender);
  const frame = event.senderFrame;
  return openRemoteEditor({
    role: policy?.role,
    origin: policy?.origin,
    frameOrigin: frame?.origin,
    topFrame: !!frame && frame.parent === null,
  }, input, (uri) => shell.openExternal(uri));
});

handleNativeCommand("editor:open-local", (event, input: unknown) => {
  const policy = policies.get(event.sender);
  const frame = event.senderFrame;
  return openLocalEditor({
    role: policy?.role,
    origin: policy?.origin,
    frameOrigin: frame?.origin,
    topFrame: !!frame && frame.parent === null,
  }, input, (uri) => shell.openExternal(uri));
});

// Show a finished download of this window in the file manager (access-control,
// Showing A Saved Download): named by its URL, never by a path from the page.
handleNativeCommand("download:reveal", (event, url: unknown) => {
  const document = broker.callerDocument(event);
  const saved = document ? savedDownload(event.sender, document, url) : undefined;
  if (!saved) throw new Error("No finished download of this URL.");
  shell.showItemInFolder(saved);
});

handleNativeCommand("remote:hosts", (event) => {
  return fromOwnUi(event) ? knownHosts() : [];
});

handleNativeCommand("remote:open", (event, target: unknown, options?: unknown): RemoteOpenResult | Promise<RemoteOpenResult> => {
  if (!fromOwnUi(event)) return openFailure("Remote windows open only from Splitty's own window.");
  if (!isAcceptableTarget(target)) return openFailure("Not a valid target: use [user@]host[:port].");
  if (options !== undefined && (!options || typeof options !== "object" || Array.isArray(options) ||
      Object.keys(options).some(key => key !== "update") ||
      ("update" in options && typeof options.update !== "boolean"))) return openFailure("Invalid remote connection options");
  return openRemote(target, event, (options as { update?: boolean } | undefined)?.update === true);
});

handleNativeCommand("remote:cancel", (event, target: unknown) => {
  if (fromOwnUi(event) && typeof target === "string") cancelRemote(target, event.sender);
});

// ---------------------------------------------------------------------------
// App lifecycle
// ---------------------------------------------------------------------------

const gotLock = app.requestSingleInstanceLock();

if (!gotLock) {
  // Another instance is already running — it will handle the new window.
  app.quit();
} else {
  app.on("second-instance", () => { if (owner) createWindow(APP_URL, windowSettings); });

  app.on("web-contents-created", (_event, contents) => hardenWebContents(contents));

  app.whenReady().then(async () => {
    Menu.setApplicationMenu(null);
    installAssets(session.defaultSession, path.join(__dirname, "ui"));

    // Electron answers every permission request with "granted" unless told
    // otherwise. Remote windows' partitions get theirs when they are created.
    guardPermissions(session.defaultSession, () => APP_ORIGIN);
    // Files a main window saved (`splitty download`) are remembered to be shown.
    session.defaultSession.on("will-download", (event, item, contents) => {
      const url = item.getURL();
      let document: string | undefined;
      try { document = broker.callerDocument({ sender: contents, senderFrame: contents.mainFrame }); } catch { /* Destroyed source. */ }
      if (!document || !isPackagedBlob(url)) { event.preventDefault(); return; }
      const assignedDocument = document;
      item.once("done", (_done, state) => {
        if (state === "completed" && item.getSavePath() && requesterCurrent(contents, assignedDocument)) {
          rememberDownload(contents, assignedDocument, url, item.getSavePath());
        }
      });
    });

    owner = new NativeOwner(findBinary(), cleanEnv(), { closed: reason => console.error(reason.message) });
    await owner.ready;
    windowSettings = await readWindowSettings(owner);
    createWindow(APP_URL, windowSettings);
  }).catch((err) => {
    console.error(err);
    app.quit();
  });

  app.on("window-all-closed", () => {
    app.quit();
  });

  let quitting = false, cleaned = false;
  app.on("before-quit", event => {
    if (cleaned) return;
    event.preventDefault();
    if (quitting) return;
    quitting = true;
    for (const win of BrowserWindow.getAllWindows()) win.destroy();
    owner?.close();
    const cleanup = [...remoteHelpers].map(helper => { helper.close(); return helper.exited; });
    if (owner) cleanup.push(owner.helper.exited);
    void Promise.all(cleanup).then(() => { cleaned = true; app.quit(); });
  });
}
