/**
 * Fixed folder URIs for editors and, for remote folders, SFTP clients. Keep
 * native builders and their shared vectors in sync.
 */
export type PathKind = "file" | "folder";
export type Editor = "vscode" | "zed" | "sftp";
export type EditorTarget = { target: string; cwd: string; uri: string };
export type EditorTargetResult = { value: EditorTarget; reason?: never } | { value?: never; reason: string };

const ALIAS_HINT = "Use a local SSH alias for this target, then reconnect through that alias in Splitty. Configure the same alias in your editor's SSH config (Windows and WSL configs are separate).";

export function remoteEditorTarget(target: unknown, cwd: unknown, editor: Editor = "vscode", kind: PathKind = "folder"): EditorTargetResult {
  if (editor !== "vscode" && editor !== "zed" && editor !== "sftp") return { reason: "Unsupported editor." };
  if (typeof target !== "string" || target.length > 255 || target.startsWith("-")) {
    return { reason: `Unsupported SSH target. ${ALIAS_HINT}` };
  }
  const parts = target.split("@");
  const host = parts.at(-1)!;
  const userValid = parts.length === 1 || (parts.length === 2 && /^[a-zA-Z0-9_.-]+$/.test(parts[0]));
  if (editor === "sftp") {
    // An sftp:// authority carries a port and a bracketed IPv6 literal as they are.
    if (!userValid || !hostWithPortValid(host)) return { reason: "Unsupported SSH target." };
  } else {
    if (/[:\[\]]/.test(target)) {
      return { reason: `Explicit SSH ports and IPv6 addresses need an SSH alias. ${ALIAS_HINT}` };
    }
    // Remote-SSH also uses hex-encoded authorities internally. Do not mistake
    // an all-hex alias for an encoded host description and open another host.
    if (editor === "vscode" && /^[a-fA-F0-9]+$/.test(target)) {
      return { reason: "This target is ambiguous with VS Code's encoded SSH authorities. Use an SSH alias containing a non-hex character, then reconnect through it in Splitty." };
    }
    if (!userValid || !hostnameValid(host)) {
      return { reason: `Unsupported SSH target. ${ALIAS_HINT}` };
    }
  }
  if (editor === "sftp" && kind !== "folder") return { reason: "SFTP handoff supports folders only." };
  const folder = folderPath(cwd, "remote", kind);
  if (folder.reason !== undefined) return { reason: folder.reason };
  const uri = editor === "sftp"
    ? `sftp://${target}${folder.path}`
    : editor === "zed"
      ? `zed://ssh/${target}${folder.path}`
      : `vscode://vscode-remote/ssh-remote+${encodePart(target)}${folder.path}`;
  return { value: { target, cwd: cwd as string, uri } };
}

function hostnameValid(host: string): boolean {
  return host.length > 0 && host.length <= 253 && host.split(".").every((label) =>
    label.length <= 63 && /^[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?$/.test(label));
}

/** A hostname, alias or IPv4 address, or a bracketed IPv6 literal, each with an optional port. */
function hostWithPortValid(host: string): boolean {
  const portValid = (port: string | undefined) => port === undefined || (/^\d{1,5}$/.test(port) && Number(port) >= 1 && Number(port) <= 65535);
  const ipv6 = /^\[([0-9a-fA-F:.]+)\](?::([^:]*))?$/.exec(host);
  if (ipv6) {
    try {
      new URL(`http://[${ipv6[1]}]/`);
    } catch {
      return false;
    }
    return portValid(ipv6[2]);
  }
  const [name, port, extra] = host.split(":");
  return extra === undefined && hostnameValid(name) && portValid(port);
}

/** A local folder, using the server's WSL distribution where supported. */
export function localEditorTarget(cwd: unknown, wslDistro?: string, editor: Editor = "vscode", kind: PathKind = "folder"): { value: { cwd: string; uri: string }; reason?: never } | { value?: never; reason: string } {
  if (editor === "sftp") return { reason: "An SFTP client opens remote folders only." };
  if (editor !== "vscode" && editor !== "zed") return { reason: "Unsupported editor." };
  const folder = folderPath(cwd, "local", kind);
  if (folder.reason !== undefined) return { reason: folder.reason };
  if (wslDistro === undefined) return { value: { cwd: cwd as string, uri: `${editor}://file${folder.path}` } };
  if (editor === "zed") return { reason: "Zed cannot open WSL folders through a URL yet. Open the distribution from Zed’s Remote Projects instead." };
  if (typeof wslDistro !== "string" || !/^[A-Za-z0-9._-]{1,64}$/.test(wslDistro)) return { reason: "The WSL distribution's name is not one VS Code can open." };
  return { value: { cwd: cwd as string, uri: `vscode://vscode-remote/wsl+${encodePart(wslDistro)}${folder.path}` } };
}

/**
 * A remote terminal's folder as other tools take it: `target:/path` (scp,
 * rsync, sshfs), or `ssh://target/path` when the target carries a port,
 * which that form cannot.
 */
export function sshPath(target: unknown, cwd: unknown): string | undefined {
  if (typeof target !== "string" || !target || typeof cwd !== "string" || !cwd.startsWith("/")) return undefined;
  if (/[\u0000-\u001f\u007f-\u009f]/.test(target + cwd)) return undefined;
  return /:\d+$/.test(target) || target.includes("[") ? `ssh://${target}${cwd}` : `${target}:${cwd}`;
}

/** The URI path of a terminal's folder; a trailing slash forces folder handling for ':123' / '.code-workspace'. */
function folderPath(cwd: unknown, side: "local" | "remote", kind: PathKind): { path: string; reason?: never } | { path?: never; reason: string } {
  if (kind !== "file" && kind !== "folder") return { reason: "Unsupported path kind." };
  if (kind === "file" && typeof cwd === "string" && (cwd.endsWith("/") || cwd.includes(":"))) {
    return { reason: "File paths containing colons or ending with a slash cannot be handed off unambiguously." };
  }
  if (typeof cwd !== "string" || !cwd) {
    return { reason: `The ${side} terminal has not reported its working directory yet.` };
  }
  if (!cwd.startsWith("/") || cwd.startsWith("//") || /[\\\u0000-\u001f\u007f-\u009f\u200b-\u200f\u202a-\u202e\u2060-\u206f\ufeff]/.test(cwd)
    || cwd.split("/").some((part) => part === "." || part === "..") || new TextEncoder().encode(cwd).length > 4096) {
    return { reason: `The ${side} working directory must be a supported absolute Linux/macOS path without control characters or dot segments.` };
  }
  try {
    // Encode data, not a URL; retain slashes only as directory separators.
    return { path: cwd.split("/").map(encodePart).join("/") + (kind === "file" || cwd.endsWith("/") ? "" : "/") };
  } catch {
    return { reason: `The ${side} working directory contains unsupported Unicode.` };
  }
}

function encodePart(value: string): string {
  return encodeURIComponent(value).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`);
}
