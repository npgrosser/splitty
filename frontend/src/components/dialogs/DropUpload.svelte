<script lang="ts">
  import { onMount } from "svelte";
  import { getTheme } from "../../lib/theme.ts";
  import { formatBytes } from "../../lib/terminal/image_paste.ts";
  import { uploadDroppedFiles, uploadProblem, type DropProgress, type DropUploadTarget } from "../../lib/terminal/drop_upload.ts";
  import type { DropContext } from "../../lib/terminal/drop_upload_view.ts";
  import "./dialog.css";

  let { files, target, context, controller, onClose }: {
    files: File[]; target: DropUploadTarget; context: DropContext;
    controller: AbortController; onClose: () => void;
  } = $props();
  let dialog = $state<HTMLDialogElement>();
  let phase = $state<"choice" | "uploading" | "done" | "cancelled" | "failed">("choice");
  let mode = $state("temporary");
  let directory = $state("");
  let insert = $state(false);
  let error = $state("");
  const total = $derived(files.reduce((sum, file) => sum + file.size, 0));
  let progress = $state<DropProgress>({ stored: 0, total: 0, index: 0, completed: 0 });
  const problem = $derived(uploadProblem(files));
  const theme = $derived(getTheme());
  const colors = $derived(`--dialog-bg:${theme.ui['surface-base']};--dialog-fg:${theme.ui['text-primary']};--dialog-border:${theme.ui['text-primary']}33`);
  const valid = $derived(context.supported && !problem && (mode === "temporary" || (directory.startsWith("/") && !/[\x00-\x1f\x7f]/.test(directory))));
  const percent = $derived(progress.total ? Math.floor(progress.stored / progress.total * 100) : phase === "done" ? 100 : 0);

  onMount(() => {
    directory = context.cwd.startsWith("/") ? context.cwd : "";
    dialog?.showModal();
  });

  async function start(e: SubmitEvent) {
    e.preventDefault();
    if (!valid || phase !== "choice") return;
    dialog?.close();
    phase = "uploading";
    try {
      await uploadDroppedFiles(files, target, {
        signal: controller.signal,
        dir: mode === "folder" ? directory : undefined,
        insert: mode === "temporary" || insert,
      }, (value) => { progress = value; });
      phase = "done";
    } catch (err) {
      phase = controller.signal.aborted ? "cancelled" : "failed";
      error = err instanceof Error ? err.message : String(err);
    }
  }
</script>

{#if phase === "choice"}
  <dialog bind:this={dialog} style={colors} aria-labelledby="upload-title" oncancel={onClose}>
    <form class="dialog-panel upload-choice" onsubmit={start}>
      <h2 id="upload-title" class="dialog-title">Upload files</h2>
      <p class="dialog-text">To <strong>{context.host}</strong> · {files.length} {files.length === 1 ? "file" : "files"} · {formatBytes(total)}</p>
      <ul class="upload-files">
        {#each files as file}<li><span>{file.name}</span><small>{formatBytes(file.size)}</small></li>{/each}
      </ul>
      <label class="upload-option"><input type="radio" bind:group={mode} value="temporary" /> Temporarily store and insert paths</label>
      {#if mode === "temporary"}<p class="dialog-text detail">Temporary cache on {context.host}, normally kept for up to 24 hours.</p>{/if}
      <label class="upload-option"><input type="radio" bind:group={mode} value="folder" /> Upload into a folder</label>
      {#if mode === "folder"}
        <label class="template-field">Destination folder on {context.host}
          <input class="dialog-input" bind:value={directory} placeholder="/absolute/path/to/folder" aria-label="Destination folder" spellcheck="false" />
        </label>
        <p class="dialog-text detail">{context.cwd ? "Check the suggested working directory before uploading." : "Enter an existing absolute directory."} Existing files will not be replaced. Spaces and special characters in filenames become underscores.</p>
        <label class="upload-option"><input type="checkbox" bind:checked={insert} /> Insert paths after upload</label>
      {/if}
      {#if !context.supported}<p class="dialog-error">Update Splitty on {context.host} to choose upload destinations and cancel uploads.</p>{/if}
      {#if problem}<p class="dialog-error">{problem}</p>{/if}
      <div class="dialog-actions">
        <button type="button" class="dialog-btn" onclick={onClose}>Cancel</button>
        <button type="submit" class="dialog-btn-primary" disabled={!valid}>Upload</button>
      </div>
    </form>
  </dialog>
{:else}
  <section class="upload-progress" style={colors} aria-label="File upload">
    <div class="upload-heading">
      <strong>{phase === "uploading" ? "Uploading" : phase === "done" ? "Upload complete" : phase === "cancelled" ? "Upload cancelled" : "Upload failed"}</strong>
      {#if phase === "uploading"}
        <button type="button" class="dialog-btn" onclick={() => controller.abort()}>Cancel</button>
      {:else}
        <button type="button" class="dialog-btn" onclick={onClose}>Dismiss</button>
      {/if}
    </div>
    <p class="destination">{context.host} · {mode === "temporary" ? "Temporary storage" : directory}</p>
    {#if phase === "uploading"}
      <p aria-live="polite">File {progress.index + 1} of {files.length} · {percent}% · {formatBytes(progress.stored)} / {formatBytes(total)}</p>
      <progress max={total || 1} value={progress.stored} aria-label="Upload progress"></progress>
    {:else}
      <p role="status">{progress.completed} of {files.length} files saved.{phase !== "done" ? " Completed files remain; no paths were inserted." : ""}</p>
      {#if phase === "failed"}<p class="dialog-error">{error}</p>{/if}
    {/if}
  </section>
{/if}

<style>
  dialog { padding: 0; border: 0; border-radius: 10px; color: var(--dialog-fg); background: var(--dialog-bg); max-width: min(480px, 94vw); max-height: 90vh; }
  dialog::backdrop { background: #0008; }
  .upload-choice { width: 460px; max-width: 94vw; box-sizing: border-box; }
  .upload-files { list-style: none; margin: 0; padding: 0; max-height: 150px; overflow: auto; font-size: 13px; }
  .upload-files li { display: flex; justify-content: space-between; gap: 16px; padding: 4px 0; }
  .upload-files span { overflow-wrap: anywhere; }
  .upload-files small { white-space: nowrap; opacity: .65; }
  .upload-option { display: flex; align-items: center; gap: 8px; font-size: 13px; cursor: pointer; }
  .detail { font-size: 12px; opacity: .75; }
  button:disabled { opacity: .4; cursor: default; }
  .upload-progress { position: absolute; bottom: 12px; right: 12px; z-index: 20; width: min(370px, calc(100% - 24px)); box-sizing: border-box; padding: 12px; border: 1px solid var(--dialog-border); border-radius: 8px; background: var(--dialog-bg); color: var(--dialog-fg); box-shadow: 0 4px 16px #0004; font-size: 12px; }
  .upload-heading { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .upload-heading button { padding: 4px 8px; font-size: 12px; }
  .upload-progress p { margin: 8px 0 0; overflow-wrap: anywhere; }
  .destination { opacity: .7; }
  progress { width: 100%; height: 6px; margin-top: 8px; accent-color: var(--dialog-fg); }
</style>
