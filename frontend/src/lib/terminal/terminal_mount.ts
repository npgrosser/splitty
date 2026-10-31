/**
 * Mounting terminal sessions into the DOM: the emulator, `create` and
 * `attach` through a connection's session client, and the settings and theme
 * that apply to the terminals of every connection.
 */

import { getXtermTheme } from "../theme.ts";
import { getConfig } from "../terminal_config.ts";
import { getSettings, type SplittySettings } from "../settings.ts";
import { connections } from "../connection/registry.ts";
import { XtermTerminalEmulator, warmup as warmupXterm } from "./xterm_terminal_emulator.ts";
import type { TerminalEmulator } from "./terminal_emulator.ts";
import type { SessionClient } from "./session_client.ts";
import { AttachTransferredError, type AttachedSession } from "./attach_registry.ts";
import { TerminalSession, type CreateSessionOptions } from "./terminal_session.ts";
import { attachImagePaste, progressNotice, reportImageFailure } from "./image_paste.ts";

/**
 * Pasted and dropped files go to the session's own server, and their paths
 * into the pane (`image_paste.ts`).
 */
function wireImagePaste(container: HTMLElement, session: TerminalSession): void {
  let alive = true;
  const detachPaste = attachImagePaste(container, {
    saveImage: (data) => session.client.saveImage(data),
    uploadFile: (file, onProgress) => session.client.uploadFile(file, onProgress),
    paste: (text) => { if (alive) session.emulator.paste(text); },
    sendCtrlV: () => { if (alive) session.client.socket.send({ type: "input", id: session.id, data: "\x16" }); },
    failed: reportImageFailure,
    progress: progressNotice(),
  });
  session.own(() => {
    alive = false;
    detachPaste();
  });
}

// ── Emulator Factory ──

/**
 * The emulator of the active renderer. `openLink` takes the links clicked in
 * it (a host's pane); without it they open as they are.
 */
async function createEmulator(openLink?: (url: string) => void): Promise<TerminalEmulator> {
  const renderer = getSettings().terminal.renderer;
  if (renderer === "ghostty") {
    const { GhosttyTerminalEmulator } = await import("./ghostty_terminal_emulator.ts");
    return GhosttyTerminalEmulator.create(openLink);
  }
  return XtermTerminalEmulator.create(openLink);
}

/**
 * Create a new terminal session on `client`'s server.
 * Creates the emulator, mounts into container, requests a PTY from the backend,
 * and wires up input/output/resize.
 */
export async function createTerminalSession(
  container: HTMLElement,
  client: SessionClient,
  options?: CreateSessionOptions,
  openLink?: (url: string) => void,
): Promise<TerminalSession> {
  const emulator = await createEmulator(openLink);

  // If container already has layout, open synchronously for instant render.
  // Otherwise, wait for layout first so we know the real dimensions and —
  // critically — so that wire() is called before the backend starts sending
  // output.  Starting the PTY in parallel with layout caused a race: the
  // shell could produce its prompt before wire() registered the output
  // handler, losing the initial output (visible as a blank first tab when
  // the window takes a moment to lay out, e.g. desktop-entry launches).
  const hasLayout = container.offsetWidth > 0 && container.offsetHeight > 0;

  let cols: number;
  let rows: number;
  if (!hasLayout) {
    ({ cols, rows } = await emulator.openDeferred(container));
  } else {
    // Fast path: container already has dimensions
    emulator.open(container);
    const fitResult = emulator.fit();
    cols = fitResult?.cols ?? emulator.cols;
    rows = fitResult?.rows ?? emulator.rows;
  }

  const created = await client.requestSession({
    cols,
    rows,
    command: options?.command,
    dir: options?.dir,
    root: options?.root,
    restoreMode: options?.restoreMode,
    paneId: options?.paneId,
    screenId: options?.screenId,
    shellProfileId: options?.shellProfileId,
  });

  const session = new TerminalSession(client, created.sessionId, emulator, {
    ...options,
    shellProfileId: created.shellProfileId ?? options?.shellProfileId,
  });
  session.wire();
  wireImagePaste(container, session);
  return session;
}

/**
 * Attach a new terminal to an existing backend PTY session of `client`'s server.
 * Used when a session is transferred to a new browser window or rebuilt from snapshot.
 *
 * Rejects with `AttachTransferredError` when a newer view of the same session
 * takes the attach over before it completes; the newer view then gets the
 * session's buffered and later output.
 */
export async function attachTerminalSession(
  container: HTMLElement,
  client: SessionClient,
  sessionId: string,
  options?: Pick<CreateSessionOptions, "shellProfileId" | "shown">,
  openLink?: (url: string) => void,
): Promise<TerminalSession> {
  const emulator = await createEmulator(openLink);
  const dims = await emulator.openDeferred(container);

  // Tell backend to start streaming this session's output on our connection.
  // Its replay arrives before `attached` and is buffered until wire().
  let attached: AttachedSession;
  let session: TerminalSession;
  try {
    attached = await client.attach(sessionId, dims.cols, dims.rows);
    session = new TerminalSession(client, sessionId, emulator, options);
    session.wire(attached);
    wireImagePaste(container, session);
  } catch (err) {
    // A view that lost its attach to a newer one is never shown.
    if (err instanceof AttachTransferredError) emulator.dispose();
    throw err;
  }
  return session;
}

// ── All Terminals ──

function allSessions(): TerminalSession[] {
  return connections().flatMap((conn) => [...conn.sessions.all()]);
}

/** Update the color theme on all open terminal emulators. */
export function updateAllTerminalThemes(): void {
  const theme = getXtermTheme();
  for (const session of allSessions()) {
    session.emulator.applyTheme(theme);
  }
}

/** Apply current xterm settings to all open terminal emulators and re-fit. */
export function applyAllTerminalSettings(): void {
  const settings: SplittySettings = getSettings();
  const config = getConfig();
  for (const session of allSessions()) {
    session.emulator.applySettings(settings, config);
  }
}

/**
 * Pre-load the terminal backend and font so the first tab opens instantly.
 * Call early (e.g. on app mount).
 */
export function warmup(): void {
  const renderer = getSettings().terminal.renderer;
  if (renderer === "ghostty") {
    import("./ghostty_terminal_emulator.ts").then(m => m.warmup()).catch(() => {});
  } else {
    warmupXterm();
  }
}

// ── Vite HMR Cleanup ──

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    for (const session of allSessions()) {
      session.detach();
    }
  });
}
