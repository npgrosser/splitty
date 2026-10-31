import type { DockviewApi } from "dockview-core";
import { toast } from "svelte-sonner";
import type { ContextMenuItem } from "../stores/ui.svelte.ts";
import { bindingForApi } from "./connection/screen_binding.ts";
import { hostOf } from "./connection/server_connection.ts";
import { localEditorTarget, remoteEditorTarget, sshPath, type Editor } from "./editor_target.ts";
import { getLocalWslDistro } from "../stores/connections.svelte.ts";
import { plainText } from "./protocol/untrusted.ts";
import { shellKind, isRemoteSession } from "./platform.ts";

const NAMES: Record<Editor, string> = { vscode: "VS Code", zed: "Zed", sftp: "SFTP Client" };

function setupHelp(editor: Editor, remote: boolean): string {
  if (editor === "sftp") {
    return "Splitty cannot confirm whether an SFTP client opened. Install one that opens sftp:// links, such as WinSCP or Cyberduck, and allow the browser/OS prompt; Linux file managers open them themselves."
      + " The client connects with its own SSH settings, so it has to know an SSH alias as well (Windows and WSL configs are separate). Otherwise connect Splitty to user@host.";
  }
  const name = NAMES[editor];
  return `Splitty cannot confirm whether ${name} opened. Install ${name} on this computer and allow the browser/OS prompt.`
    + (remote ? (editor === "vscode" ? " Install Remote - SSH and configure SSH access. Windows VS Code uses its own SSH config, separate from WSL." : " Configure SSH access in Zed. Windows and WSL use separate SSH configurations.")
      : editor === "vscode" ? " For WSL folders, install VS Code’s WSL extension." : " Open WSL distributions from Zed’s Remote Projects; URL handoff is not supported yet.");
}

/** Folder data belongs to the selected terminal, never to a page's ?host. */
export function editorMenuItem(api: DockviewApi, paneId: string): ContextMenuItem | undefined {
  const binding = bindingForApi(api);
  if (isRemoteSession || !binding) return undefined;
  const session = binding.sessionForPane(paneId);
  const remote = binding.conn.kind === "remote";
  const prepare = (editor: Editor) => remote
    ? remoteEditorTarget(hostOf(binding.conn.key), session?.lastCwd, editor)
    : localEditorTarget(session?.lastCwd, getLocalWslDistro(), editor);
  // An SFTP client is for moving files to and from a host; a local folder needs none.
  const editors: Editor[] = remote ? ["vscode", "zed", "sftp"] : ["vscode", "zed"];
  const children = editors.map((editor): ContextMenuItem => {
    const prepared = prepare(editor);
    return {
      label: NAMES[editor],
      disabled: !prepared.value,
      detail: prepared.reason,
      action: () => {
        const current = bindingForApi(api);
        if (current !== binding || !session || current.sessionForPane(paneId) !== session) {
          toast.error("Terminal is no longer available", { description: "Open the menu again from the terminal you want to edit." });
          return;
        }
        const fresh = prepare(editor);
        if (!fresh.value) {
          toast.error("Cannot open folder", { description: fresh.reason });
          return;
        }
        // Dispatch immediately while the menu click still has user activation.
        void handoff(editor, remote, fresh.value.uri, session.lastCwd!, remote ? hostOf(current.conn.key) : undefined);
      },
    };
  });
  return {
    label: "Open Folder in",
    detail: plainText(session?.lastCwd, 120),
    children,
    action: () => {},
  };
}

/** A remote terminal's folder as `user@host:/path`, for scp, rsync and sshfs. */
export function sshPathMenuItem(api: DockviewApi, paneId: string): ContextMenuItem | undefined {
  const binding = bindingForApi(api);
  if (isRemoteSession || binding?.conn.kind !== "remote") return undefined;
  const session = binding.sessionForPane(paneId);
  const path = sshPath(hostOf(binding.conn.key), session?.lastCwd);
  if (!path) return undefined;
  return {
    label: "Copy SSH Path",
    detail: plainText(path, 120),
    action: () => { void navigator.clipboard.writeText(path); },
  };
}

async function handoff(editor: Editor, remote: boolean, uri: string, cwd: string, target?: string): Promise<void> {
  const name = NAMES[editor];
  try {
    if (shellKind === "web") {
      window.location.assign(uri);
    } else {
      const input = remote ? { editor, target, cwd } : { editor, cwd, wslDistro: getLocalWslDistro() };
      if (shellKind === "tauri") {
        await (window as any).__TAURI_INTERNALS__.invoke(remote ? "open_remote_editor" : "open_local_editor", input);
      } else {
        await (window as any).__SPLITTY_ELECTRON__.invoke(remote ? "editor:open-remote" : "editor:open-local", input);
      }
    }
  } catch {
    toast.error(`Could not open ${name}`, { description: setupHelp(editor, remote), duration: Infinity, closeButton: true });
  }
}
