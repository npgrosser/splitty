<script lang="ts">
  import { getTheme } from "../../lib/theme.ts";
  import type { TemplateCommand } from "../../lib/template_guard.ts";
  import "./dialog.css";

  /**
   * Shows exactly what a template would execute before it runs.
   *
   * security review SR-12: templates are shared like documents
   * but can carry shell commands, so applying one used to be able to execute
   * code with no prior indication. Every command is listed verbatim — commands
   * are rendered as text, never as markup, so a template cannot dress up what it
   * is about to run.
   */
  interface Props {
    open: boolean;
    templateName: string;
    commands: TemplateCommand[];
    onRunAll: () => void;
    onLayoutOnly: () => void;
    onCancel: () => void;
  }

  let { open, templateName, commands, onRunAll, onLayoutOnly, onCancel }: Props = $props();

  const theme = $derived(getTheme());

  function handleKeydown(e: KeyboardEvent) {
    if (!open) return;
    // No Enter-to-confirm here, unlike the other dialogs: the safe default is
    // *not* running the commands, and a reflexive Enter should not execute code.
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <div
    class="dialog-backdrop"
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33;--dialog-danger:{theme.ui['danger'] ?? '#e06c75'}"
    role="dialog"
  >
    <div class="dialog-panel template-approval">
      <h2 class="dialog-title">This template runs commands</h2>

      <p class="dialog-text">
        <strong>{templateName}</strong>
        will run {commands.length}
        {commands.length === 1 ? "command" : "commands"} when applied.
      </p>

      <ul class="command-list">
        {#each commands as cmd (cmd.paneName)}
          <li class="command-entry">
            <div class="command-line">{cmd.command}</div>
            <div class="command-meta">
              pane {cmd.paneName}{#if cmd.dir} · in {cmd.dir}{/if}{#if cmd.shellProfile} ·
                {cmd.shellProfile}{/if}
            </div>
          </li>
        {/each}
      </ul>

      <div class="dialog-actions">
        <button onclick={onCancel} class="dialog-btn">Cancel</button>
        <button onclick={onLayoutOnly} class="dialog-btn">Layout only</button>
        <button onclick={onRunAll} class="dialog-btn-danger">Run commands</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .template-approval {
    max-width: 560px;
  }

  .command-list {
    list-style: none;
    margin: 12px 0 4px;
    padding: 0;
    max-height: 260px;
    overflow-y: auto;
  }

  .command-entry {
    border: 1px solid var(--dialog-border);
    border-radius: 6px;
    padding: 8px 10px;
    margin-bottom: 6px;
  }

  .command-line {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    color: var(--dialog-fg);
    word-break: break-all;
    white-space: pre-wrap;
  }

  .command-meta {
    font-size: 11px;
    opacity: 0.6;
    margin-top: 4px;
  }
</style>
