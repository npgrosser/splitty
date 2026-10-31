<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import { updateSetting, selectValue } from "../../../lib/settings.ts";
  import { editorPreferences, type AppEditor } from "../../../lib/editor_preferences.ts";
  import { isRemoteSession } from "../../../lib/platform.ts";
  import SettingsSection from "./SettingsSection.svelte";
  import SettingRow from "./SettingRow.svelte";

  let { collapsed, onToggle }: { collapsed: boolean; onToggle: () => void } = $props();
  const prefs = $derived(editorPreferences(settings().editors));
  let extension = $state("");
  let extensionEditor = $state<AppEditor>("zed");
  let connection = $state("local");
  let project = $state("");
  let projectEditor = $state<AppEditor>("vscode");
  let error = $state("");

  function addExtension() {
    const name = extension.trim().replace(/^\./, "").toLowerCase();
    if (!/^[a-z0-9_-]{1,32}$/.test(name)) { error = "Enter a file extension, such as rs or md."; return; }
    void updateSetting("editors", "extensions", [...prefs.extensions.filter((r) => r.extension.replace(/^\./, "").toLowerCase() !== name), { extension: name, editor: extensionEditor }]);
    extension = ""; error = "";
  }
  function addProject() {
    const rule = { connection: connection.trim(), path: project.trim().replace(/\/+$/, "") || "/", editor: projectEditor };
    if (!project.trim() || !rule.connection || !editorPreferences({ projects: [rule] }).projects.length) {
      error = "Enter local or host: followed by the exact SSH target and an absolute project folder."; return;
    }
    void updateSetting("editors", "projects", [...prefs.projects.filter((r) => r.connection !== rule.connection || r.path !== rule.path), rule]);
    project = ""; error = "";
  }
</script>

{#if !isRemoteSession}
  <SettingsSection title="EDITORS" {collapsed} {onToggle}>
    <p class="help">Open files and folders from a terminal with <code>splitty open</code>. Each request asks for confirmation on this computer.</p>
    {#each (["folder", "file"] as const) as kind}
      <SettingRow label={kind === "folder" ? "Folders" : "Files"}>
        <select aria-label={`Default editor for ${kind}s`} value={prefs[kind]} onchange={(e) => updateSetting("editors", kind, selectValue<AppEditor>(e))}>
          <option value="vscode">VS Code</option><option value="zed">Zed</option>
        </select>
      </SettingRow>
    {/each}
    <p class="help">File extensions</p>
    {#each prefs.extensions as rule, i}
      <div class="rule"><span>.{rule.extension.replace(/^\./, "")} → {rule.editor === "zed" ? "Zed" : "VS Code"}</span><button onclick={() => updateSetting("editors", "extensions", prefs.extensions.filter((_, n) => n !== i))} aria-label={`Remove ${rule.extension} rule`}>Remove</button></div>
    {/each}
    <form onsubmit={(e) => { e.preventDefault(); addExtension(); }}>
      <input aria-label="File extension" placeholder="rs" bind:value={extension} maxlength="33" />
      <select aria-label="Extension editor" bind:value={extensionEditor}><option value="vscode">VS Code</option><option value="zed">Zed</option></select>
      <button type="submit" disabled={prefs.extensions.length >= 100}>Add</button>
    </form>
    <p class="help">Project folders override extensions. Use <code>local</code> or <code>host:user@server</code> with the exact SSH target from Splitty. The most specific folder wins.</p>
    {#each prefs.projects as rule, i}
      <div class="rule"><span>{rule.connection}: {rule.path} → {rule.editor === "zed" ? "Zed" : "VS Code"}</span><button onclick={() => updateSetting("editors", "projects", prefs.projects.filter((_, n) => n !== i))} aria-label={`Remove project ${rule.path}`}>Remove</button></div>
    {/each}
    <form class="project" onsubmit={(e) => { e.preventDefault(); addProject(); }}>
      <input aria-label="Project connection" placeholder="local or host:user@server" bind:value={connection} maxlength="260" />
      <input aria-label="Project folder" placeholder="/home/me/project" bind:value={project} maxlength="4096" />
      <select aria-label="Project editor" bind:value={projectEditor}><option value="vscode">VS Code</option><option value="zed">Zed</option></select>
      <button type="submit" disabled={prefs.projects.length >= 100}>Add</button>
    </form>
    {#if error}<p role="alert" class="help">{error}</p>{/if}
    <p class="help">Install the selected editor on this computer. VS Code needs Remote - SSH for remote hosts or WSL support for Windows terminals.</p>
  </SettingsSection>
{/if}

<style>
  .help { font-size: 11px; opacity: 0.7; line-height: 1.5; margin: 8px 0; }
  form, .rule { display: flex; gap: 6px; align-items: center; margin: 6px 0; }
  form.project { flex-wrap: wrap; }
  input { min-width: 0; flex: 1; }
  form.project input { flex-basis: 100%; }
  .rule span { flex: 1; min-width: 0; overflow-wrap: anywhere; font-size: 11px; }
  select, input, button { background: var(--bg-secondary, transparent); color: inherit; border: 1px solid var(--border, #555); border-radius: 4px; padding: 4px 6px; font: inherit; font-size: 11px; }
  button { cursor: pointer; }
</style>
