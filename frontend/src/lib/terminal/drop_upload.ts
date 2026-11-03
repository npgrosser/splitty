/** Batch transfers shared by the pane and scratchpad drop dialog. */
import { MAX_FILE_BYTES } from "./image_paste.ts";
import type { UploadOptions } from "./session_client.ts";

export interface DropUploadTarget {
  uploadFile(file: File, progress: ((received: number) => void) | undefined, options: UploadOptions): Promise<string>;
  paste(text: string): void;
}

export interface DropProgress {
  stored: number;
  total: number;
  index: number;
  completed: number;
}

/** Preserve plain paths; quote shell metacharacters, including spaces and quotes. */
export function quoteUploadPath(path: string): string {
  return /^[\p{L}\p{N}_./-]+$/u.test(path) ? path : `'${path.replaceAll("'", `'"'"'`)}'`;
}

export function uploadProblem(files: readonly File[]): string | undefined {
  const tooLarge = files.find((file) => file.size > MAX_FILE_BYTES);
  return tooLarge ? `${tooLarge.name || "A file"} is larger than 512 MB.` : undefined;
}

export async function uploadDroppedFiles(
  files: readonly File[], target: DropUploadTarget,
  options: UploadOptions & { insert: boolean },
  progress: (value: DropProgress) => void,
): Promise<void> {
  const problem = uploadProblem(files);
  if (problem) throw new Error(problem);
  const total = files.reduce((sum, file) => sum + file.size, 0);
  let stored = 0;
  const paths: string[] = [];
  for (const [index, file] of files.entries()) {
    options.signal?.throwIfAborted();
    progress({ stored, total, index, completed: paths.length });
    const path = await target.uploadFile(file, (received) => {
      progress({ stored: stored + received, total, index, completed: paths.length });
    }, options);
    paths.push(path);
    stored += file.size;
    progress({ stored, total, index, completed: paths.length });
  }
  options.signal?.throwIfAborted();
  if (options.insert && paths.length) target.paste(paths.map(quoteUploadPath).join(" "));
}
