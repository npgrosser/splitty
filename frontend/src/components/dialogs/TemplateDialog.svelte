<script lang="ts">
  import { getTheme } from "../../lib/theme.ts";
  import { listTemplates, getTemplate, saveTemplate, deleteTemplate } from "../../lib/templates.ts";
  import type { Template, TemplateSummary } from "../../lib/protocol/types.ts";
  import "./dialog.css";

  interface Props {
    open: boolean;
    onClose: () => void;
    onApply: (template: Template, root?: string) => void;
    onApplyNewScreen: (template: Template, root?: string) => void;
    onExtract: (name: string, root?: string) => Template;
  }

  let { open, onClose, onApply, onApplyNewScreen, onExtract }: Props = $props();

  const theme = $derived(getTheme());

  // ── State ──

  type Mode = "list" | "save" | "root-prompt";

  let mode = $state<Mode>("list");
  let templates = $state<TemplateSummary[]>([]);
  let loading = $state(false);
  let error = $state("");
  let selectedIndex = $state(0);

  // Save mode
  let saveName = $state("");
  let saveUseRoot = $state(false);
  let saveRoot = $state("");
  let confirmOverwrite = $state(false);

  // Save flow: extracted template + per-pane command toggles
  let saveTemplate_ = $state<Template | null>(null);
  let paneToggles = $state<Record<string, boolean>>({});

  // Root prompt mode (for applying templates with relative paths)
  let pendingTemplate = $state<Template | null>(null);
  let rootInput = $state("");
  let applyInNewScreen = $state(false);

  // Delete confirmation
  let confirmDeleteSlug = $state("");

  let listEl: HTMLDivElement | undefined = $state();
  let nameInputEl: HTMLInputElement | undefined = $state();
  let rootInputEl: HTMLInputElement | undefined = $state();

  // Panes that have commands (for toggle UI in save mode)
  let commandPanes = $derived(
    saveTemplate_
      ? Object.entries(saveTemplate_.panes).filter(([, p]) => p.command)
      : [],
  );

  // ── Fetch on open ──

  $effect(() => {
    if (open) {
      mode = "list";
      error = "";
      confirmDeleteSlug = "";
      confirmOverwrite = false;
      pendingTemplate = null;
      applyInNewScreen = false;
      saveTemplate_ = null;
      paneToggles = {};
      fetchTemplates();
    }
  });

  $effect(() => {
    if (open && mode === "list") {
      // Focus the list container for keyboard nav
      requestAnimationFrame(() => listEl?.focus());
    }
  });

  $effect(() => {
    if (mode === "save") {
      requestAnimationFrame(() => nameInputEl?.focus());
    }
  });

  $effect(() => {
    if (mode === "root-prompt") {
      requestAnimationFrame(() => rootInputEl?.focus());
    }
  });

  async function fetchTemplates() {
    loading = true;
    try {
      templates = await listTemplates();
      selectedIndex = 0;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  // ── Apply ──

  async function applySelected(newScreen = false) {
    const item = templates[selectedIndex];
    if (!item) return;
    try {
      const tmpl = await getTemplate(item.slug);
      // Check if template has relative paths
      const hasRelative = Object.values(tmpl.panes).some(
        (p) => p.dir && (p.dir.startsWith("./") || p.dir.startsWith("../") || !p.dir.startsWith("/")),
      );
      if (hasRelative) {
        pendingTemplate = tmpl;
        rootInput = "";
        applyInNewScreen = newScreen;
        mode = "root-prompt";
      } else {
        if (newScreen) {
          onApplyNewScreen(tmpl);
        } else {
          onApply(tmpl);
        }
        onClose();
      }
    } catch (e) {
      error = String(e);
    }
  }

  function applyWithRoot(newScreen?: boolean) {
    if (!pendingTemplate) return;
    const root = rootInput.trim() || undefined;
    const useNewScreen = newScreen ?? applyInNewScreen;
    if (useNewScreen) {
      onApplyNewScreen(pendingTemplate, root);
    } else {
      onApply(pendingTemplate, root);
    }
    pendingTemplate = null;
    onClose();
  }

  // ── Save ──

  function startSave() {
    mode = "save";
    saveName = "";
    saveUseRoot = false;
    saveRoot = "";
    confirmOverwrite = false;

    // Extract now so we can show per-pane command toggles
    try {
      const tmpl = onExtract("", undefined);
      saveTemplate_ = tmpl;
      // Default: all commands enabled
      const toggles: Record<string, boolean> = {};
      for (const [name, pane] of Object.entries(tmpl.panes)) {
        if (pane.command) toggles[name] = true;
      }
      paneToggles = toggles;
    } catch {
      saveTemplate_ = null;
      paneToggles = {};
    }
  }

  async function doSave() {
    const name = saveName.trim();
    if (!name || !saveTemplate_) return;
    const slug = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
    if (!slug) return;

    // Check for overwrite
    if (!confirmOverwrite && templates.some((t) => t.slug === slug)) {
      confirmOverwrite = true;
      return;
    }

    try {
      // Clone the pre-extracted template
      const tmpl = structuredClone($state.snapshot(saveTemplate_));

      // Re-extract with root if needed
      if (saveUseRoot && saveRoot.trim()) {
        const reExtracted = onExtract(name, saveRoot.trim());
        tmpl.panes = reExtracted.panes;
        tmpl.layout = reExtracted.layout;
      }

      // Strip commands from toggled-off panes
      for (const [paneName, pane] of Object.entries(tmpl.panes)) {
        if (pane.command && !paneToggles[paneName]) {
          delete pane.command;
          delete pane.restore_mode;
        }
      }

      tmpl.name = name;

      await saveTemplate(slug, tmpl);
      mode = "list";
      confirmOverwrite = false;
      saveTemplate_ = null;
      paneToggles = {};
      await fetchTemplates();
    } catch (e) {
      error = String(e);
    }
  }

  // ── Delete ──

  async function doDelete(slug: string) {
    try {
      await deleteTemplate(slug);
      confirmDeleteSlug = "";
      await fetchTemplates();
    } catch (e) {
      error = String(e);
    }
  }

  // ── Keyboard ──

  function handleListKeydown(e: KeyboardEvent) {
    if (mode !== "list") return;

    if (e.key === "Escape") {
      e.preventDefault();
      if (confirmDeleteSlug) {
        confirmDeleteSlug = "";
      } else {
        onClose();
      }
      return;
    }

    if (e.key === "ArrowDown" || e.key === "j") {
      e.preventDefault();
      if (templates.length > 0) {
        selectedIndex = (selectedIndex + 1) % templates.length;
      }
      return;
    }

    if (e.key === "ArrowUp" || e.key === "k") {
      e.preventDefault();
      if (templates.length > 0) {
        selectedIndex = (selectedIndex - 1 + templates.length) % templates.length;
      }
      return;
    }

    if (e.key === "Enter") {
      e.preventDefault();
      applySelected();
      return;
    }
  }

  function handleSaveKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      if (confirmOverwrite) {
        confirmOverwrite = false;
      } else {
        mode = "list";
        saveTemplate_ = null;
        paneToggles = {};
      }
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      doSave();
    }
  }

  function handleRootKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      pendingTemplate = null;
      mode = "list";
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      applyWithRoot();
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) onClose();
  }
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="dialog-backdrop"
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33;--dialog-danger:{theme.ui['danger'] ?? '#e06c75'}"
    onclick={handleBackdropClick}
  >
    <div class="dialog-panel dialog-panel-compact">
      {#if mode === "list"}
        <h2 class="dialog-title dialog-title-sm">
          Templates
        </h2>

        {#if error}
          <p class="dialog-error">{error}</p>
        {/if}

        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          bind:this={listEl}
          tabindex="-1"
          onkeydown={handleListKeydown}
          class="template-list"
        >
          {#if loading}
            <p class="dialog-text dialog-text-center dialog-text-muted">
              Loading...
            </p>
          {:else if templates.length === 0}
            <p class="dialog-text dialog-text-center dialog-text-muted">
              No templates saved yet.
            </p>
          {:else}
            {#each templates as tmpl, i (tmpl.slug)}
              {#if confirmDeleteSlug === tmpl.slug}
                <div class="template-item template-item-confirm">
                  <span class="template-confirm-text">Delete "{tmpl.displayName}"?</span>
                  <span class="template-confirm-actions">
                    <button
                      class="template-btn-sm template-btn-ghost"
                      onclick={() => { confirmDeleteSlug = ""; }}
                    >Cancel</button>
                    <button
                      class="template-btn-sm template-btn-danger-sm"
                      onclick={() => doDelete(tmpl.slug)}
                    >Delete</button>
                  </span>
                </div>
              {:else}
                <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
                <div
                  class="template-item"
                  class:template-item-selected={i === selectedIndex}
                  onmouseenter={() => { selectedIndex = i; }}
                  onclick={() => applySelected()}
                >
                  <span class="template-item-name">
                    {tmpl.displayName}
                  </span>
                  <span class="template-item-actions">
                    <button
                      class="template-btn-screen"
                      onclick={(e: MouseEvent) => { e.stopPropagation(); applySelected(true); }}
                      title="Apply in new screen"
                    >+ Screen</button>
                    <button
                      class="template-btn-delete"
                      onclick={(e: MouseEvent) => { e.stopPropagation(); confirmDeleteSlug = tmpl.slug; }}
                      title="Delete template"
                    >&times;</button>
                  </span>
                </div>
              {/if}
            {/each}
          {/if}
        </div>

        <!-- Save button -->
        <button
          class="template-btn-save"
          onclick={startSave}
        >
          Save current layout as template...
        </button>

      {:else if mode === "save"}
        <h2 class="dialog-title dialog-title-sm">
          Save Template
        </h2>

        {#if error}
          <p class="dialog-error">{error}</p>
        {/if}

        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div onkeydown={handleSaveKeydown} class="template-form">
          <div class="template-field">
            <label class="template-label" for="tmpl-name">Name</label>
            <input
              bind:this={nameInputEl}
              id="tmpl-name"
              type="text"
              bind:value={saveName}
              placeholder="My Layout"
              spellcheck="false"
              autocomplete="off"
              class="dialog-input dialog-input-sm"
            />
          </div>

          <div class="template-segmented">
            <button
              class="template-segmented-btn"
              class:template-segmented-btn-active={!saveUseRoot}
              onclick={() => { saveUseRoot = false; }}
            >Absolute</button>
            <button
              class="template-segmented-btn"
              class:template-segmented-btn-active={saveUseRoot}
              onclick={() => { saveUseRoot = true; }}
            >Relative</button>
          </div>

          {#if saveUseRoot}
            <input
              type="text"
              bind:value={saveRoot}
              placeholder="/home/user/project"
              spellcheck="false"
              autocomplete="off"
              class="dialog-input dialog-input-sm"
            />
          {/if}

          {#if commandPanes.length > 0}
            <div class="template-commands-section">
              <p class="template-commands-heading">
                Include commands
              </p>
              {#each commandPanes as [name, pane] (name)}
                <label
                  class="template-command-toggle"
                  class:template-command-toggle-on={paneToggles[name]}
                  class:template-command-toggle-off={!paneToggles[name]}
                >
                  <input
                    type="checkbox"
                    checked={paneToggles[name] ?? false}
                    onchange={(e: Event) => { paneToggles = { ...paneToggles, [name]: (e.target as HTMLInputElement).checked }; }}
                  />
                  <span class="template-command-text">
                    {pane.command}
                    {#if pane.dir}
                      <span class="template-command-dir">{pane.dir}</span>
                    {/if}
                  </span>
                </label>
              {/each}
            </div>
          {/if}

          {#if confirmOverwrite}
            <p class="dialog-warning">
              A template with this name already exists. Save again to overwrite.
            </p>
          {/if}

          <div class="dialog-actions">
            <button
              class="template-btn-sm template-btn-ghost"
              onclick={() => { mode = "list"; confirmOverwrite = false; saveTemplate_ = null; paneToggles = {}; }}
            >Cancel</button>
            <button
              class="template-btn-sm template-btn-fill"
              onclick={doSave}
            >{confirmOverwrite ? "Overwrite" : "Save"}</button>
          </div>
        </div>

      {:else if mode === "root-prompt"}
        <h2 class="dialog-title dialog-title-sm">
          Root Directory
        </h2>

        <p class="dialog-text">
          This template uses relative paths. Enter the project root directory:
        </p>

        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div onkeydown={handleRootKeydown} class="template-form">
          <input
            bind:this={rootInputEl}
            type="text"
            bind:value={rootInput}
            placeholder="/home/user/project"
            spellcheck="false"
            autocomplete="off"
            class="dialog-input dialog-input-sm"
          />

          <div class="template-checkbox-row">
            <label class="template-checkbox-label">
              <input type="checkbox" bind:checked={applyInNewScreen} />
              Apply in new screen
            </label>
          </div>

          <div class="dialog-actions">
            <button
              class="template-btn-sm template-btn-ghost"
              onclick={() => { pendingTemplate = null; mode = "list"; }}
            >Back</button>
            <button
              class="template-btn-sm template-btn-fill"
              onclick={() => applyWithRoot()}
            >Apply</button>
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}
