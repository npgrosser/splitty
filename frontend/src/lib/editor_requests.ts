/** Requests from a server are untrusted data. Only the local confirm action launches. */
import { toast } from "svelte-sonner";
import { hostOf, type ServerConnection } from "./connection/server_connection.ts";
import { localEditorTarget, remoteEditorTarget } from "./editor_target.ts";
import { preferredEditor, isAppEditor } from "./editor_preferences.ts";
import { getSettings } from "./settings.ts";
import { isRemoteSession, shellKind } from "./platform.ts";
import { getLocalWslDistro, hasLocalEditorContext } from "../stores/connections.svelte.ts";
import type { EditorRequest, EditorOutcome } from "./protocol/types.ts";

// Across all connections in this window, even a malicious host cannot create
// unbounded prompts. Connection-specific limits below also bound one host.
let visible = 0;

export function watchEditorRequests(conn: ServerConnection): () => void {
  if (isRemoteSession) return () => {};
  let connected = false;
  let disposed = false;
  let lastPrompt = -Infinity;
  const seen = new Map<string, number>();
  let pending: { request: EditorRequest; dismiss: (outcome?: EditorOutcome) => void } | undefined;
  const send = (requestId: string, outcome: EditorOutcome) => {
    if (connected && !disposed) conn.socket.send({ type: "editor.result", requestId, outcome });
  };
  const stop = () => { connected = false; pending?.dismiss(); };
  const unsubDisconnect = conn.socket.onDisconnect(stop);
  const unsubMessage = conn.socket.onMessage((msg) => {
    if (msg.type === "mux.welcome") {
      pending?.dismiss();
      connected = Array.isArray(msg.capabilities) && msg.capabilities.includes("editor-open-v1");
      // Do not clear replay history or rate counters on reconnect.
      if (connected) conn.socket.send({ type: "editor.register" });
      return;
    }
    if (msg.type === "editor.cancelled" && pending?.request.requestId === msg.requestId) { pending.dismiss(); return; }
    if (msg.type === "closed" && pending?.request.sessionId === msg.sessionId) { pending.dismiss("disconnected"); return; }
    if (msg.type !== "editor.request" || !connected || disposed) return;
    if (typeof msg.requestId !== "string" || !/^[a-f0-9]{32}$/.test(msg.requestId) || seen.has(msg.requestId)) return;
    for (const [id, until] of seen) if (performance.now() >= until) seen.delete(id);
    if (seen.size >= 256) { send(msg.requestId, "denied"); return; }
    seen.set(msg.requestId, performance.now() + 300_000);
    if (pending || visible >= 3 || performance.now() - lastPrompt < 2000) { send(msg.requestId, "denied"); return; }
    if (typeof msg.path !== "string" || msg.path.length > 4096 || typeof msg.sessionId !== "string" || msg.sessionId.length > 256 || (msg.kind !== "file" && msg.kind !== "folder") || (msg.editor != null && !isAppEditor(msg.editor)) || !Number.isFinite(msg.expiresAt)) {
      send(msg.requestId, "failed"); return;
    }
    const requestId = msg.requestId;
    const session = conn.sessions.get(msg.sessionId);
    if (!session) { send(msg.requestId, "disconnected"); return; }
    const duration = Math.min(25_000, msg.expiresAt - Date.now());
    if (duration <= 0) { send(msg.requestId, "expired"); return; }
    const deadline = performance.now() + duration;
    const target = hostOf(conn.key);
    if (target === undefined && !hasLocalEditorContext()) {
      send(msg.requestId, "failed");
      lastPrompt = performance.now();
      toast.error("Local server details are not available yet", { description: "Retry after Splitty has loaded its server information." });
      return;
    }
    const editor = preferredEditor(getSettings().editors, conn.key, msg.path, msg.kind, msg.editor ?? undefined);
    const distro = target === undefined ? getLocalWslDistro() : undefined;
    const prepared = target === undefined
      ? localEditorTarget(msg.path, distro, editor, msg.kind)
      : remoteEditorTarget(target, msg.path, editor, msg.kind);
    if (!prepared.value) {
      send(msg.requestId, "failed");
      lastPrompt = performance.now();
      toast.error("Cannot open in editor", { description: prepared.reason });
      return;
    }
    lastPrompt = performance.now();
    visible++;
    let active = true;
    let notification: string | number;
    const finish = (outcome?: EditorOutcome) => {
      if (!active) return;
      active = false;
      clearTimeout(timer);
      visible--;
      pending = undefined;
      toast.dismiss(notification);
      if (outcome) send(msg.requestId, outcome);
    };
    const timer = setTimeout(() => finish("expired"), duration);
    pending = { request: msg, dismiss: finish };
    const name = editor === "zed" ? "Zed" : "VS Code";
    notification = toast(`Open ${msg.kind} in ${name}?`, {
      description: `${target ?? (distro ? `Local · WSL ${distro}` : "Local")} — ${msg.path}`,
      duration,
      closeButton: true,
      action: { label: "Open", onClick: () => {
        if (!active) return;
        if (!connected || disposed || conn.sessions.get(msg.sessionId) !== session || (target === undefined && getLocalWslDistro() !== distro)) { finish("disconnected"); return; }
        if (performance.now() >= deadline || Date.now() >= msg.expiresAt) { finish("expired"); return; }
        finish(); // Consume before dispatch; duplicate clicks cannot launch twice.
        const input = { editor, cwd: msg.path, kind: msg.kind, ...(target === undefined ? { wslDistro: distro } : { target }) };
        try {
          let dispatch: Promise<unknown>;
          if (shellKind === "web") {
            window.location.assign(prepared.value.uri);
            dispatch = Promise.resolve();
          } else if (shellKind === "tauri") {
            dispatch = (window as any).__TAURI_INTERNALS__.invoke(target === undefined ? "open_local_editor" : "open_remote_editor", input);
          } else {
            dispatch = (window as any).__SPLITTY_ELECTRON__.invoke(target === undefined ? "editor:open-local" : "editor:open-remote", input);
          }
          void dispatch.then(() => send(msg.requestId, "dispatched"), () => failed());
        } catch { failed(); }
        function failed() {
          send(requestId, "failed");
          toast.error(`Could not open ${name}`, { description: "Check the editor installation and OS protocol handler. VS Code remote paths need Remote - SSH or WSL support." });
        }
      } },
      cancel: { label: "Decline", onClick: () => finish("denied") },
      onDismiss: () => finish("denied"),
      onAutoClose: () => finish("expired"),
    });
  });
  return () => { pending?.dismiss("disconnected"); disposed = true; stop(); unsubMessage(); unsubDisconnect(); };
}
