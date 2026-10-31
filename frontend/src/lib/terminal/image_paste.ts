/**
 * Files pasted or dropped into a pane (`terminal-sessions`, Image Paste In A
 * Pane and Files Into A Pane): each goes to the pane's own server, which
 * stores it as a file, and the file's path is pasted into the pane. Images go
 * with `save_image`, which older hosts know too; every other file goes in
 * chunks with `upload`. Claude Code and other agents take a pasted path as the
 * file, also when they run on a host whose clipboard is empty.
 *
 * The files come from the browser's `paste` and `drop` events; the page may
 * not read the clipboard itself (`clipboard-read=()`). Ctrl+V therefore goes
 * through the browser's paste instead of straight to the terminal: a file is
 * uploaded, anything else reaches the terminal as `^V`, as before.
 */

import { toast } from "svelte-sonner";
import { isMac } from "../shortcuts.ts";
import { toBase64 } from "../protocol/base64.ts";

export { toBase64 };

/** Largest image `save_image` takes; a larger one is uploaded as a file. */
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
/** Largest file an upload sends; the server refuses larger ones. */
export const MAX_FILE_BYTES = 512 * 1024 * 1024;
/** The image types the server stores with `save_image`. */
const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
/** How long Ctrl+V waits for the browser's paste before it sends `^V` anyway. */
export const CTRL_V_FALLBACK_MS = 300;
/** A paste of fewer bytes shows no progress. */
export const PROGRESS_MIN_BYTES = 4 * 1024 * 1024;

/** What a paste or drop hands over (a `DataTransfer`). */
export interface TransferData {
  readonly files: ArrayLike<File>;
  readonly types: readonly string[];
  getData(format: string): string;
}

/** The pane, or the text field, files go into. */
export interface ImagePasteTarget {
  /** Store the image (base64) on the pane's server; resolves with its path. */
  saveImage(data: string): Promise<string>;
  /** Store a file on the pane's server in chunks; resolves with its path. */
  uploadFile(file: File, onProgress?: (received: number) => void): Promise<string>;
  /** Paste text into the pane as a bracketed paste. */
  paste(text: string): void;
  /** Send Ctrl+V's `^V` to the terminal. */
  sendCtrlV(): void;
  /** Tell the user why a file was not pasted. */
  failed(reason: string, needsUpdate: boolean): void;
  /** Bytes stored so far of a paste's files; `done` once it ended, stored or not. */
  progress?(stored: number, total: number, done: boolean): void;
}

/** Whether `file` goes with `save_image` rather than as an upload. */
export function isImage(file: File): boolean {
  return IMAGE_TYPES.has(file.type) && file.size <= MAX_IMAGE_BYTES;
}

/** The files of a paste: only when it carries no text, which a paste keeps pasting as text. */
export function pastedFiles(data: TransferData | null): File[] {
  if (!data || data.getData("text/plain")) return [];
  return Array.from(data.files);
}

/** The files of a drop. */
export function droppedFiles(data: TransferData | null): File[] {
  return data ? Array.from(data.files) : [];
}

function hasFiles(data: DataTransfer | null): boolean {
  return !!data && Array.from(data.types).includes("Files");
}

/** Ctrl+V without other modifiers; on macOS the paste is Cmd+V, so Ctrl+V stays the terminal's. */
export function isPlainCtrlV(e: Pick<KeyboardEvent, "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "code">, mac = isMac): boolean {
  return !mac && e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey && e.code === "KeyV";
}

/** Whether the browser pastes into `element`: only into what can be edited. */
function pastesInto(element: Element | null | undefined): boolean {
  if (!element) return false;
  return element.tagName === "TEXTAREA" || element.tagName === "INPUT" || (element as HTMLElement).isContentEditable === true;
}

/**
 * Store `files` one after the other and paste their paths once, in their
 * order, separated by spaces. Files over the limit are refused before
 * anything is sent; any failure pastes nothing and says why.
 */
export async function pasteFiles(files: readonly File[], target: ImagePasteTarget): Promise<void> {
  const tooLarge = files.find((file) => file.size > MAX_FILE_BYTES);
  if (tooLarge) {
    target.failed(`${tooLarge.name || "A file"} is larger than 512 MB.`, false);
    return;
  }
  const total = files.reduce((sum, file) => sum + file.size, 0);
  let stored = 0;
  const paths: string[] = [];
  try {
    for (const file of files) {
      if (isImage(file)) {
        paths.push(await target.saveImage(toBase64(new Uint8Array(await file.arrayBuffer()))));
      } else {
        const before = stored;
        paths.push(await target.uploadFile(file, (received) => target.progress?.(before + received, total, false)));
      }
      stored += file.size;
      target.progress?.(stored, total, false);
    }
  } catch (err) {
    const needsUpdate = typeof err === "object" && err !== null && "needsUpdate" in err && err.needsUpdate === true;
    target.failed(readError(err) ?? (err instanceof Error ? err.message : String(err)), needsUpdate);
    return;
  } finally {
    target.progress?.(stored, total, true);
  }
  if (paths.length > 0) target.paste(paths.join(" "));
}

/** What to say when the browser cannot read a dropped file, typically a folder. */
function readError(err: unknown): string | undefined {
  if (typeof DOMException !== "undefined" && err instanceof DOMException && (err.name === "NotFoundError" || err.name === "NotReadableError")) {
    return "A dropped file could not be read. Folders cannot be dropped; archive them first.";
  }
  return undefined;
}

/** Tell the user why a file was not pasted (`ImagePasteTarget.failed`). */
export function reportImageFailure(reason: string, needsUpdate: boolean): void {
  if (needsUpdate) toast.error("File not pasted", { description: "The Splitty of this pane's host is too old to take this file. Update it under Hosts in the drawer." });
  else toast.error("File not pasted", { description: reason });
}

/**
 * Show a paste's progress in a notice once it holds more than
 * {@link PROGRESS_MIN_BYTES}; the notice goes when the paste ended.
 */
export function progressNotice(): NonNullable<ImagePasteTarget["progress"]> {
  let id: string | number | undefined;
  return (stored, total, done) => {
    if (total < PROGRESS_MIN_BYTES) return;
    if (done) {
      if (id !== undefined) toast.dismiss(id);
      id = undefined;
      return;
    }
    const percent = Math.floor((stored / total) * 100);
    id = toast.loading("Uploading files", { id, description: `${percent} % of ${formatBytes(total)}` });
  };
}

/** `bytes` for people: B, KB, MB or GB with one decimal above KB. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

/**
 * Handle pasted and dropped files in a text field (`session-scratchpad`,
 * Images In The Scratchpad): the files go to the server like a pane's, and
 * their paths into the text at the cursor. Text pastes as the field does.
 * Returns the cleanup.
 */
export function attachImageInsert(
  field: HTMLTextAreaElement,
  store: Pick<ImagePasteTarget, "saveImage" | "uploadFile">,
  insert: (text: string) => void,
): () => void {
  const target: ImagePasteTarget = {
    saveImage: (data) => store.saveImage(data),
    uploadFile: (file, onProgress) => store.uploadFile(file, onProgress),
    paste: insert,
    sendCtrlV: () => {},
    failed: reportImageFailure,
    progress: progressNotice(),
  };

  const paste = (e: ClipboardEvent) => {
    const found = pastedFiles(e.clipboardData);
    if (found.length === 0) return;
    e.preventDefault();
    void pasteFiles(found, target);
  };
  const dragover = (e: DragEvent) => {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
  };
  const drop = (e: DragEvent) => {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    const found = droppedFiles(e.dataTransfer);
    if (found.length > 0) void pasteFiles(found, target);
  };

  field.addEventListener("paste", paste);
  field.addEventListener("dragover", dragover);
  field.addEventListener("drop", drop);
  return () => {
    field.removeEventListener("paste", paste);
    field.removeEventListener("dragover", dragover);
    field.removeEventListener("drop", drop);
  };
}

/**
 * Handle pasted and dropped files in the pane mounted in `container`.
 * Returns the cleanup.
 */
export function attachImagePaste(container: HTMLElement, target: ImagePasteTarget): () => void {
  let ctrlV: ReturnType<typeof setTimeout> | undefined;

  const settleCtrlV = (): boolean => {
    if (ctrlV === undefined) return false;
    clearTimeout(ctrlV);
    ctrlV = undefined;
    return true;
  };

  const keydown = (e: KeyboardEvent) => {
    if (!isPlainCtrlV(e) || e.repeat || !pastesInto(container.ownerDocument.activeElement)) return;
    // Not to the terminal yet: the browser's paste, which follows, decides.
    e.stopImmediatePropagation();
    settleCtrlV();
    ctrlV = setTimeout(() => {
      ctrlV = undefined;
      target.sendCtrlV();
    }, CTRL_V_FALLBACK_MS);
  };

  const paste = (e: ClipboardEvent) => {
    const viaCtrlV = settleCtrlV();
    const found = pastedFiles(e.clipboardData);
    if (found.length > 0) {
      e.preventDefault();
      e.stopImmediatePropagation();
      void pasteFiles(found, target);
    } else if (viaCtrlV) {
      e.preventDefault();
      e.stopImmediatePropagation();
      target.sendCtrlV();
    }
  };

  const dragover = (e: DragEvent) => {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
  };

  const drop = (e: DragEvent) => {
    if (!hasFiles(e.dataTransfer)) return;
    e.preventDefault();
    e.stopPropagation();
    const found = droppedFiles(e.dataTransfer);
    if (found.length > 0) void pasteFiles(found, target);
  };

  container.addEventListener("keydown", keydown, true);
  container.addEventListener("paste", paste, true);
  container.addEventListener("dragover", dragover, true);
  container.addEventListener("drop", drop, true);
  return () => {
    settleCtrlV();
    container.removeEventListener("keydown", keydown, true);
    container.removeEventListener("paste", paste, true);
    container.removeEventListener("dragover", dragover, true);
    container.removeEventListener("drop", drop, true);
  };
}
