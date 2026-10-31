import { rememberDownload, savedDownload } from "./downloads";
import { openLocalEditor, openRemoteEditor } from "./editor";
import { app, BrowserWindow, Menu, shell, ipcMain, session, webContents, type Session, type WebContents } from "electron";
import { spawn, ChildProcess } from "child_process";
import path from "path";
import {
  exitFailure, isAcceptableTarget, parseConnectEvent, parseKnownHosts, remoteTitle, tunnelOrigin,
  type ConnectEvent, type ConnectionState, type KnownHost,
} from "./remote";

let splittyProcess: ChildProcess | null = null;
/**
 * Origin of the backend, e.g. `http://127.0.0.1:45299`.
 *
 * Deliberately *not* the full launch URL. That URL carries a bootstrap code, and
 * keeping it around to reuse for a second window meant handing out a credential
 * more than once (security review SR-03/SR-04). Additional
 * windows request a fresh code instead — see `requestBootstrap`.
 */
let backendOrigin: string | null = null;
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

/** Resolvers waiting for the next `SPLITTY_BOOTSTRAP=` line. */
const bootstrapWaiters: ((code: string) => void)[] = [];

/**
 * Ask the running backend for a fresh single-use bootstrap code.
 *
 * The request travels over the backend's stdin, which only this process holds
 * the write end of. Codes are spent on first use, so every window that needs to
 * establish a session has to ask for its own.
 */
function requestBootstrap(): Promise<string | null> {
  if (!splittyProcess?.stdin?.writable) return Promise.resolve(null);

  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      const idx = bootstrapWaiters.indexOf(waiter);
      if (idx >= 0) bootstrapWaiters.splice(idx, 1);
      resolve(null);
    }, 5000);

    const waiter = (code: string) => {
      clearTimeout(timer);
      resolve(code);
    };
    bootstrapWaiters.push(waiter);
    splittyProcess!.stdin!.write("bootstrap\n");
  });
}

function startSplitty(): Promise<{ port: string; url: string }> {
  return new Promise((resolve, reject) => {
    const bin = findBinary();
    splittyProcess = spawn(bin, ["serve", "--addr", "127.0.0.1:0", "--oneshot-token"], {
      env: cleanEnv(),
      stdio: ["pipe", "pipe", "pipe"],
    });

    const vars: Record<string, string> = {};
    let buffer = "";

    splittyProcess.stdout?.on("data", (chunk: Buffer) => {
      buffer += chunk.toString();
      const lines = buffer.split("\n");
      buffer = lines.pop() ?? "";

      for (const line of lines) {
        const match = line.match(/^SPLITTY_(PORT|BOOTSTRAP|URL)=(.+)$/);
        if (!match) continue;

        // A bootstrap line after startup is an answer to requestBootstrap.
        if (match[1] === "BOOTSTRAP" && vars.bootstrap) {
          bootstrapWaiters.shift()?.(match[2]);
          continue;
        }
        vars[match[1].toLowerCase()] = match[2];
      }

      if (vars.port && vars.bootstrap && vars.url) {
        resolve({ port: vars.port, url: vars.url });
      }
    });

    splittyProcess.stderr?.on("data", (chunk: Buffer) => {
      process.stderr.write(chunk);
    });

    splittyProcess.on("error", (err) => {
      reject(new Error(`Failed to start splitty: ${err.message}`));
    });

    splittyProcess.on("exit", (code) => {
      if (!vars.port) {
        reject(new Error(`splitty exited with code ${code} before becoming ready`));
      }
    });
  });
}

/**
 * Read the window settings needed before the first window is created.
 *
 * Spends a bootstrap code of its own rather than lifting a credential out of the
 * launch URL, which is what this used to do. The session it establishes lives in
 * the response cookie and is discarded — this call only needs one field.
 */
async function fetchSettings(origin: string): Promise<{ nativeTitleBar: boolean }> {
  try {
    const bootstrap = await requestBootstrap();
    if (!bootstrap) return { nativeTitleBar: false };

    const session = await fetch(new URL("/api/session", origin).toString(), {
      method: "POST",
      headers: { "Content-Type": "application/json", Origin: origin },
      body: JSON.stringify({ bootstrap }),
    });
    const cookie = session.headers.get("set-cookie");
    if (!session.ok || !cookie) return { nativeTitleBar: false };

    const res = await fetch(new URL("/api/home/settings", origin).toString(), {
      headers: { Cookie: cookie.split(";")[0], Origin: origin },
    });
    if (res.ok) {
      const settings = await res.json();
      return { nativeTitleBar: settings?.window?.nativeTitleBar ?? false };
    }
  } catch {}
  return { nativeTitleBar: false };
}

// ---------------------------------------------------------------------------
// Per-WebContents policy: the one origin each window belongs to
// ---------------------------------------------------------------------------

/**
 * - `main`: a window on the app's own backend; the only role whose IPC calls
 *   are served.
 * - `popout`: a window the main UI opened on that origin (dockview pop-outs).
 * - `remote`: a remote window, pinned to its tunnel origin (`desktop-remote`).
 */
type WindowRole = "main" | "popout" | "remote";

interface WindowPolicy {
  /** `http://127.0.0.1:<port>`; a remote window's changes with a new session. */
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
    return new URL(url).origin;
  } catch {
    return null;
  }
}

/** Does this URL belong to the origin the WebContents is pinned to? */
function isOwnOrigin(contents: WebContents, url: string): boolean {
  const policy = policyOf(contents);
  return policy !== undefined && originOf(url) === policy.origin;
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
 * only to the window's own origin — a remote window's tunnel origin included
 * (D7).
 */
const ALLOWED_PERMISSIONS = new Set(["notifications", "clipboard-sanitized-write"]);

/**
 * Chromium passes no WebContents for some checks (notifications, cross-origin
 * subframes); those are decided by the session's own origin. Each session
 * belongs to one origin: the default session to the backend, a remote
 * window's partition to its tunnel.
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
      if (policy.role === "remote") return { action: "deny" };
      return {
        action: "allow",
        overrideBrowserWindowOptions: {
          webPreferences: {
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

  // Top-level navigation stays on the window's origin. Without this, anything
  // able to set `location` in the renderer could move the privileged window —
  // which holds the session cookie — onto a foreign page, or a remote window
  // off its tunnel. Web URLs go to the system browser, `file:` and the rest
  // nowhere.
  contents.on("will-navigate", (event) => {
    if (isOwnOrigin(contents, event.url)) return;
    event.preventDefault();
    if (policyOf(contents)) openExternally(event.url);
  });

  // A server-side redirect is a navigation as well.
  contents.on("will-redirect", (event) => {
    if (event.isMainFrame && !isOwnOrigin(contents, event.url)) event.preventDefault();
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
  if (backendOrigin) policies.set(win.webContents, { origin: backendOrigin, role: "main" });

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
// Remote windows (desktop-remote): a host's UI through a tunnel the app holds
// ---------------------------------------------------------------------------

/** What `remote:open` resolves with; `ipcRenderer.invoke` rejections lose their fields. */
type RemoteOpenResult = { ok: true } | { ok: false; error: { message: string; hint?: string } };

function openFailure(message: string, hint?: string): RemoteOpenResult {
  return { ok: false, error: hint ? { message, hint } : { message } };
}

/** A remote window and the `splitty connect --json` process holding its tunnel. */
interface RemoteWindow {
  target: string;
  win: BrowserWindow;
  policy: WindowPolicy;
  /** Set while the connection is alive; null once it gave up or ended. */
  child: ChildProcess | null;
}

/** An open in flight. Concurrent opens of the same target share it. */
interface PendingOpen {
  promise: Promise<RemoteOpenResult>;
  child: ChildProcess;
  /** The trusted UIs that asked; they get its progress. */
  requesters: Set<WebContents>;
  finish: (result: RemoteOpenResult) => void;
}

/** One window per target (D3). */
const remoteWindows = new Map<string, RemoteWindow>();
const pendingOpens = new Map<string, PendingOpen>();
let remoteWindowCount = 0;

function setRemoteState(entry: RemoteWindow, state: ConnectionState): void {
  if (!entry.win.isDestroyed()) entry.win.setTitle(remoteTitle(entry.target, state));
}

/** The connection gave up or its process ended; the next open runs it again (D11). */
function markDisconnected(entry: RemoteWindow): void {
  entry.child = null;
  setRemoteState(entry, "disconnected");
}

/**
 * A window for the host's UI that can call nothing of the shell: no preload,
 * sandboxed, and a cookie store of its own that lives in memory only (no
 * `persist:` prefix) and that no other window uses.
 */
function createRemoteWindow(target: string, origin: string, child: ChildProcess): RemoteWindow {
  const partition = `remote-${++remoteWindowCount}`;
  const policy: WindowPolicy = { origin, role: "remote" };
  guardPermissions(session.fromPartition(partition), () => policy.origin);

  const win = new BrowserWindow({
    width: 1280,
    height: 800,
    backgroundColor: "#181818",
    title: remoteTitle(target, "connected"),
    webPreferences: {
      partition,
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      devTools: !app.isPackaged,
    },
  });
  policies.set(win.webContents, policy);
  const entry: RemoteWindow = { target, win, policy, child };

  // The title names the host and its connection state, not the page's title.
  win.on("page-title-updated", (event) => event.preventDefault());

  // The web UI's `beforeunload` guard for running processes would silently
  // cancel closing the window and loading a new session. Neither ends anything
  // on the host, whose daemon keeps the sessions.
  win.webContents.on("will-prevent-unload", (event) => event.preventDefault());

  // Its close is not intercepted: the confirmation dialog needs the preload.
  // Closing ends the tunnel, never the host's daemon or its sessions.
  win.on("closed", () => {
    entry.child?.kill("SIGTERM");
    if (remoteWindows.get(target) === entry) remoteWindows.delete(target);
  });
  return entry;
}

/** The window for a ready connection: the target's own after a give-up, or a new one. */
function attachConnection(target: string, origin: string, child: ChildProcess): RemoteWindow {
  const existing = remoteWindows.get(target);
  if (existing) {
    existing.policy.origin = origin;
    existing.child = child;
    setRemoteState(existing, "connected");
    return existing;
  }
  const entry = createRemoteWindow(target, origin, child);
  remoteWindows.set(target, entry);
  return entry;
}

/**
 * Run `splitty connect` for a target and map its events (design.md, The shell
 * API). The open resolves once the window shows the host, and fails with the
 * `failed` event, or with the end of stderr if the process ended without one.
 */
function connectRemote(target: string, requester: WebContents): Promise<RemoteOpenResult> {
  // `--` keeps the target a positional argument whatever it starts with.
  const child = spawn(findBinary(), ["connect", "--no-open", "--json", "--", target], {
    env: cleanEnv(),
    stdio: ["ignore", "pipe", "pipe"],
  });

  let resolve!: (result: RemoteOpenResult) => void;
  let settled = false;
  const pending: PendingOpen = {
    promise: new Promise((r) => { resolve = r; }),
    child,
    requesters: new Set([requester]),
    finish: (result) => {
      if (pendingOpens.get(target) === pending) pendingOpens.delete(target);
      if (settled) return;
      settled = true;
      resolve(result);
    },
  };
  pendingOpens.set(target, pending);

  let entry: RemoteWindow | null = null;
  let stderrTail = "";

  const handle = (event: ConnectEvent): void => {
    switch (event.event) {
      case "progress":
      case "notice": {
        const step = event.event === "progress" ? event.step : "notice";
        for (const contents of pending.requesters) {
          if (!contents.isDestroyed()) contents.send("remote:progress", { target, step, message: event.message });
        }
        break;
      }
      case "ready": {
        if (settled) break;
        const origin = tunnelOrigin(event.url);
        if (!origin) {
          child.kill("SIGTERM");
          pending.finish(openFailure(`splitty connect reported a session outside the tunnel: ${event.url}`));
          break;
        }
        // From here on the open can no longer be cancelled; closing the window ends it.
        if (pendingOpens.get(target) === pending) pendingOpens.delete(target);
        const shown = attachConnection(target, origin, child);
        entry = shown;
        // Settled once the page's document is there, not when every
        // resource has loaded: a slow resource through the tunnel would
        // otherwise keep the picker waiting on a window that is already up.
        const shownUp = (): void => {
          if (!shown.win.isDestroyed()) shown.win.focus();
          pending.finish({ ok: true });
        };
        shown.win.webContents.once("dom-ready", shownUp);
        void shown.win.loadURL(event.url).catch(() => {}).then(shownUp);
        break;
      }
      case "url": {
        // A restarted or moved daemon: a new session for the same window.
        const origin = tunnelOrigin(event.url);
        if (!entry || entry.win.isDestroyed() || !origin) break;
        entry.policy.origin = origin;
        setRemoteState(entry, "connected");
        void entry.win.loadURL(event.url).catch(() => {});
        break;
      }
      case "lost":
      case "retry":
        if (entry) setRemoteState(entry, "reconnecting");
        break;
      case "restored":
        if (entry) setRemoteState(entry, "connected");
        break;
      case "failed":
        if (entry) markDisconnected(entry);
        else pending.finish(openFailure(event.message, event.hint));
        break;
      case "closed":
        break;
    }
  };

  let buffer = "";
  child.stdout?.on("data", (chunk: Buffer) => {
    buffer += chunk.toString();
    const lines = buffer.split("\n");
    buffer = lines.pop() ?? "";
    for (const line of lines) {
      const event = parseConnectEvent(line);
      if (event) handle(event);
    }
  });

  child.stderr?.on("data", (chunk: Buffer) => {
    process.stderr.write(chunk);
    stderrTail = (stderrTail + chunk.toString()).slice(-4096);
  });

  child.on("error", (err) => {
    pending.finish(openFailure(`Could not run splitty: ${err.message}`));
  });

  child.on("exit", (code) => {
    if (entry && entry.child === child) markDisconnected(entry);
    pending.finish(openFailure(exitFailure(code, stderrTail)));
  });

  return pending.promise;
}

/**
 * Open a host: focus its window while its connection is alive, join an open of
 * it in flight, or connect (again, into the same window after a give-up).
 */
function openRemote(target: string, requester: WebContents): Promise<RemoteOpenResult> {
  const existing = remoteWindows.get(target);
  if (existing?.child) {
    if (existing.win.isMinimized()) existing.win.restore();
    existing.win.focus();
    return Promise.resolve({ ok: true });
  }
  const pending = pendingOpens.get(target);
  if (pending) {
    pending.requesters.add(requester);
    return pending.promise;
  }
  return connectRemote(target, requester);
}

function cancelRemote(target: string): void {
  const pending = pendingOpens.get(target);
  if (!pending) return;
  pending.child.kill("SIGTERM");
  pending.finish(openFailure("cancelled"));
}

/** `splitty remote-hosts --json`; nothing when it fails. */
function knownHosts(): Promise<KnownHost[]> {
  return new Promise((resolve) => {
    const child = spawn(findBinary(), ["remote-hosts", "--json"], {
      env: cleanEnv(),
      stdio: ["ignore", "pipe", "ignore"],
    });
    let output = "";
    const timer = setTimeout(() => {
      child.kill();
      resolve([]);
    }, 10_000);
    child.stdout?.on("data", (chunk: Buffer) => { output += chunk.toString(); });
    child.on("error", () => {
      clearTimeout(timer);
      resolve([]);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve(code === 0 ? parseKnownHosts(output) : []);
    });
  });
}

// ---------------------------------------------------------------------------
// IPC handlers — resolve the target window from the event sender
// ---------------------------------------------------------------------------

function winFromEvent(event: Electron.IpcMainInvokeEvent): BrowserWindow | null {
  return BrowserWindow.fromWebContents(event.sender);
}

ipcMain.handle("window:minimize", (event) => {
  winFromEvent(event)?.minimize();
});

ipcMain.handle("window:maximize", (event) => {
  winFromEvent(event)?.maximize();
});

ipcMain.handle("window:unmaximize", (event) => {
  winFromEvent(event)?.unmaximize();
});

ipcMain.handle("window:toggle-maximize", (event) => {
  const win = winFromEvent(event);
  if (win) {
    win.isMaximized() ? win.unmaximize() : win.maximize();
  }
});

ipcMain.handle("window:close", (event) => {
  winFromEvent(event)?.close();
});

ipcMain.handle("window:force-close", (event) => {
  const win = winFromEvent(event);
  if (win) {
    (win as any)._forceClose = true;
    win.close();
  }
});

ipcMain.handle("window:start-drag", (event, screenX: number, screenY: number) => {
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
ipcMain.handle("window:set-zoom", (event, factor: unknown) => {
  if (typeof factor !== "number" || !Number.isFinite(factor)) return;
  event.sender.setZoomFactor(Math.min(3, Math.max(0.5, factor)));
});

ipcMain.handle("window:is-maximized", (event) => {
  return winFromEvent(event)?.isMaximized() ?? false;
});

ipcMain.handle("shell:open-external", (_event, url: unknown) => {
  if (typeof url === "string") openExternally(url);
});

/**
 * Is this call from the shell's own UI: the top frame of a main window on the
 * backend origin (access-control, Desktop Shell Command Scope)? Remote windows
 * have no preload and cannot call at all; this keeps the remote channels, which
 * run processes, closed to every other frame as well.
 */
function fromOwnUi(event: Electron.IpcMainInvokeEvent): boolean {
  const policy = policies.get(event.sender);
  const frame = event.senderFrame;
  return policy?.role === "main" && !!frame && frame.parent === null && frame.origin === policy.origin;
}

ipcMain.handle("editor:open-remote", (event, input: unknown) => {
  const policy = policies.get(event.sender);
  const frame = event.senderFrame;
  return openRemoteEditor({
    role: policy?.role,
    origin: policy?.origin,
    frameOrigin: frame?.origin,
    topFrame: !!frame && frame.parent === null,
  }, input, (uri) => shell.openExternal(uri));
});

ipcMain.handle("editor:open-local", (event, input: unknown) => {
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
ipcMain.handle("download:reveal", (event, url: unknown) => {
  const saved = fromOwnUi(event) ? savedDownload(url) : undefined;
  if (!saved) throw new Error("No finished download of this URL.");
  shell.showItemInFolder(saved);
});

ipcMain.handle("remote:hosts", (event) => {
  return fromOwnUi(event) ? knownHosts() : [];
});

ipcMain.handle("remote:open", (event, target: unknown): RemoteOpenResult | Promise<RemoteOpenResult> => {
  if (!fromOwnUi(event)) return openFailure("Remote windows open only from Splitty's own window.");
  if (!isAcceptableTarget(target)) return openFailure("Not a valid target: use [user@]host[:port].");
  return openRemote(target, event.sender);
});

ipcMain.handle("remote:cancel", (event, target: unknown) => {
  if (fromOwnUi(event) && typeof target === "string") cancelRemote(target);
});

// ---------------------------------------------------------------------------
// App lifecycle
// ---------------------------------------------------------------------------

const gotLock = app.requestSingleInstanceLock();

if (!gotLock) {
  // Another instance is already running — it will handle the new window.
  app.quit();
} else {
  // Each additional window gets its own bootstrap code. Reusing the original
  // launch URL would hand out an already-spent credential — and, before codes
  // became single-use, a reusable one (SR-03/SR-04).
  app.on("second-instance", () => {
    if (!backendOrigin) return;
    void requestBootstrap().then((bootstrap) => {
      if (!bootstrap) return;
      createWindow(`${backendOrigin}/?bootstrap=${bootstrap}&desktop=1`, windowSettings);
    });
  });

  app.on("web-contents-created", (_event, contents) => hardenWebContents(contents));

  app.whenReady().then(async () => {
    Menu.setApplicationMenu(null);

    // Electron answers every permission request with "granted" unless told
    // otherwise. Remote windows' partitions get theirs when they are created.
    guardPermissions(session.defaultSession, () => backendOrigin);
    // Files a main window saved (`splitty download`) are remembered to be shown.
    session.defaultSession.on("will-download", (_event, item, contents) => {
      if (policyOf(contents)?.role !== "main") return;
      const url = item.getURL();
      item.once("done", (_done, state) => {
        if (state === "completed" && item.getSavePath()) rememberDownload(url, item.getSavePath());
      });
    });

    const { port, url } = await startSplitty();
    backendOrigin = `http://127.0.0.1:${port}`;
    windowSettings = await fetchSettings(backendOrigin);
    createWindow(url, windowSettings);
  }).catch((err) => {
    console.error(err);
    app.quit();
  });

  app.on("window-all-closed", () => {
    app.quit();
  });

  app.on("will-quit", () => {
    if (splittyProcess && !splittyProcess.killed) {
      splittyProcess.kill();
    }
    // Quitting ends every tunnel; the hosts' daemons keep running.
    for (const entry of remoteWindows.values()) entry.child?.kill("SIGTERM");
    for (const pending of pendingOpens.values()) pending.child.kill("SIGTERM");
  });
}
