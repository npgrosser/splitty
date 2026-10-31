import { localEditorTarget, remoteEditorTarget } from "./editor_target";

/** The narrow sender proof from main's registered window policy and senderFrame. */
export interface EditorSender {
  role?: string;
  origin?: string;
  frameOrigin?: string;
  topFrame: boolean;
}

/** No caller-supplied URI, executable or shell command reaches the OS. */
export async function openRemoteEditor(
  sender: EditorSender,
  input: unknown,
  open: (uri: string) => Promise<void>,
  local = false,
): Promise<void> {
  if (sender.role !== "main" || !sender.topFrame || !sender.origin || sender.frameOrigin !== sender.origin) {
    throw new Error("Editor handoff is available only from Splitty's own window.");
  }
  if (!input || typeof input !== "object") throw new Error("Remote editor target and directory are required.");
  const { target, cwd, editor = "vscode", wslDistro, kind = "folder" } = input as { target?: unknown; cwd?: unknown; editor?: unknown; wslDistro?: unknown; kind?: unknown };
  if (editor !== "vscode" && editor !== "zed" && editor !== "sftp") throw new Error("Unsupported editor.");
  if (wslDistro !== undefined && typeof wslDistro !== "string") throw new Error("Unsupported WSL distribution.");
  if (kind !== "file" && kind !== "folder") throw new Error("Unsupported path kind.");
  const result = local ? localEditorTarget(cwd, wslDistro, editor, kind) : remoteEditorTarget(target, cwd, editor, kind);
  if (!result.value) throw new Error(result.reason);
  await open(result.value.uri);
}

/** Same exact-origin sender checks for local folders. */
export async function openLocalEditor(sender: EditorSender, input: unknown, open: (uri: string) => Promise<void>): Promise<void> {
  return openRemoteEditor(sender, input, open, true);
}
