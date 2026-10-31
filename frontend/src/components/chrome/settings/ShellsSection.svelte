<script lang="ts">
  import { onMount } from "svelte";
  import SettingsSection from "./SettingsSection.svelte";
  import {
    getShellProfiles,
    loadShellProfiles,
    saveShellProfiles,
    shellProfileIcon,
    subscribeShellProfiles,
    type ShellProfile,
    type ShellProfilesState,
    type ShellTarget,
  } from "../../../lib/shell_profiles.ts";
  import { validDefaultProfileId } from "../../../lib/shell_profile_model.ts";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
  }

  let { collapsed, onToggle }: Props = $props();
  let draft = $state<ShellProfilesState>(structuredClone(getShellProfiles()));
  let dirty = $state(false);
  let saving = $state(false);
  let error = $state("");
  let editingId = $state<string | null>(null);
  let editor = $state({ name: "", command: "", argsText: "", target: "local" as ShellTarget });

  onMount(() => {
    if (draft.profiles.length === 0) {
      loadShellProfiles().then(resetDraft);
    }
    return subscribeShellProfiles(() => {
      if (!dirty) resetDraft(getShellProfiles());
    });
  });

  function resetDraft(state = getShellProfiles()): void {
    draft = structuredClone(state);
    dirty = false;
    error = "";
    editingId = null;
  }

  function markChanged(): void {
    dirty = true;
    error = "";
    ensureDefault();
  }

  function ensureDefault(): void {
    draft.defaultProfileId = validDefaultProfileId(draft);
  }

  function setDefault(event: Event): void {
    draft.defaultProfileId = (event.target as HTMLSelectElement).value;
    markChanged();
  }

  function toggleProfile(profile: ShellProfile): void {
    profile.enabled = !profile.enabled;
    draft = { ...draft, profiles: [...draft.profiles] };
    markChanged();
  }

  function move(index: number, delta: number): void {
    const target = index + delta;
    if (target < 0 || target >= draft.profiles.length) return;
    const profiles = [...draft.profiles];
    [profiles[index], profiles[target]] = [profiles[target], profiles[index]];
    draft = { ...draft, profiles };
    markChanged();
  }

  function edit(profile: ShellProfile): void {
    editingId = profile.id;
    editor = {
      name: profile.name,
      command: profile.command,
      argsText: profile.args.join("\n"),
      target: profile.target,
    };
  }

  function addProfile(): void {
    const id = `custom-${crypto.randomUUID()}`;
    const profile: ShellProfile = {
      id,
      name: "Custom Shell",
      command: "",
      args: [],
      enabled: true,
      target: "local",
      source: "custom",
      available: true,
      experimental: false,
    };
    draft = { ...draft, profiles: [...draft.profiles, profile] };
    edit(profile);
    dirty = true;
  }

  function applyEditor(): void {
    const profile = draft.profiles.find((candidate) => candidate.id === editingId);
    if (!profile) return;
    const name = editor.name.trim();
    const command = editor.command.trim();
    if (!name || !command) {
      error = "Name and executable are required.";
      return;
    }
    profile.name = name;
    profile.command = command;
    profile.args = editor.argsText === ""
      ? []
      : editor.argsText.split("\n").map((arg) => arg.replace(/\r$/, ""));
    profile.target = editor.target;
    profile.experimental = editor.target === "windowsInterop";
    profile.available = true;
    profile.reason = undefined;
    draft = { ...draft, profiles: [...draft.profiles] };
    editingId = null;
    markChanged();
  }

  function cancelEditor(): void {
    const profile = draft.profiles.find((candidate) => candidate.id === editingId);
    if (profile?.source === "custom" && profile.command === "") {
      draft = { ...draft, profiles: draft.profiles.filter((candidate) => candidate.id !== profile.id) };
      ensureDefault();
    }
    editingId = null;
  }

  function resetDetected(profile: ShellProfile): void {
    const defaults = profile.detectedDefaults;
    if (!defaults) return;
    profile.name = defaults.name;
    profile.command = defaults.command;
    profile.args = [...defaults.args];
    profile.target = defaults.target;
    profile.experimental = defaults.target === "windowsInterop";
    draft = { ...draft, profiles: [...draft.profiles] };
    markChanged();
    edit(profile);
  }

  function removeProfile(profile: ShellProfile): void {
    if (profile.source !== "custom") return;
    draft = { ...draft, profiles: draft.profiles.filter((candidate) => candidate.id !== profile.id) };
    if (editingId === profile.id) editingId = null;
    markChanged();
  }

  async function save(): Promise<void> {
    saving = true;
    error = "";
    try {
      resetDraft(await saveShellProfiles(structuredClone(draft)));
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    } finally {
      saving = false;
    }
  }
</script>

<SettingsSection title="SHELLS" {collapsed} {onToggle}>
  <div class="shell-settings-toolbar">
    <label for="default-shell-profile">Default profile</label>
    <select id="default-shell-profile" value={draft.defaultProfileId} onchange={setDefault}>
      {#each draft.profiles.filter((profile) => profile.enabled && profile.available) as profile}
        <option value={profile.id}>{profile.name}</option>
      {/each}
    </select>
  </div>

  <div class="shell-profile-list">
    {#each draft.profiles as profile, index (profile.id)}
      <div class="shell-profile-card" class:unavailable={!profile.available}>
        <div class="shell-profile-summary">
          <button
            class="shell-enable-btn"
            class:enabled={profile.enabled}
            aria-label={profile.enabled ? `Disable ${profile.name}` : `Enable ${profile.name}`}
            onclick={() => toggleProfile(profile)}
          >{profile.enabled ? "●" : "○"}</button>
          <span class="shell-profile-icon">{shellProfileIcon(profile)}</span>
          <span class="shell-profile-name">{profile.name}</span>
          {#if profile.id === draft.defaultProfileId}<span class="shell-profile-badge">DEFAULT</span>{/if}
          {#if profile.experimental}<span class="shell-profile-badge experimental">EXPERIMENTAL</span>{/if}
          <span class="shell-profile-actions">
            <button title="Move up" disabled={index === 0} onclick={() => move(index, -1)}>↑</button>
            <button title="Move down" disabled={index === draft.profiles.length - 1} onclick={() => move(index, 1)}>↓</button>
            <button onclick={() => edit(profile)}>Edit</button>
            {#if profile.source === "custom"}
              <button class="danger" onclick={() => removeProfile(profile)}>Delete</button>
            {/if}
          </span>
        </div>
        <div class="shell-profile-command">{profile.command} {profile.args.join(" ")}</div>
        {#if !profile.available}<div class="shell-profile-error">{profile.reason}</div>{/if}

        {#if editingId === profile.id}
          <div class="shell-profile-editor">
            <label>Name <input value={editor.name} oninput={(e) => { editor.name = e.currentTarget.value; }} /></label>
            <label>Executable <input value={editor.command} oninput={(e) => { editor.command = e.currentTarget.value; }} spellcheck="false" /></label>
            <label>Arguments <textarea rows="3" value={editor.argsText} oninput={(e) => { editor.argsText = e.currentTarget.value; }} placeholder="One argument per line"></textarea></label>
            <label>Execution target
              <select value={editor.target} onchange={(e) => { editor.target = e.currentTarget.value as ShellTarget; }}>
                <option value="local">Local</option>
                <option value="windowsInterop">Windows interoperability (WSL)</option>
              </select>
            </label>
            <div class="shell-editor-actions">
              {#if profile.source === "detected" && profile.detectedDefaults}
                <button onclick={() => resetDetected(profile)}>Reset detected values</button>
              {/if}
              <span></span>
              <button onclick={cancelEditor}>Cancel edit</button>
              <button class="primary" onclick={applyEditor}>Apply</button>
            </div>
          </div>
        {/if}
      </div>
    {/each}
  </div>

  <div class="shell-settings-footer">
    <button onclick={addProfile}>+ Add profile</button>
    <span></span>
    <button disabled={!dirty || saving} onclick={() => resetDraft()}>Cancel</button>
    <button class="primary" disabled={!dirty || saving} onclick={save}>{saving ? "Saving…" : "Save"}</button>
  </div>
  {#if error}<div class="shell-settings-error">{error}</div>{/if}
</SettingsSection>
