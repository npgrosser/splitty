<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import { updateSetting, defaults, selectValue, inputNumber, type ThemeMode, type ZoomScope } from "../../../lib/settings.ts";
  import { ZOOM_STEPS, zoomLabel } from "../../../lib/zoom.ts";
  import { isMac } from "../../../lib/shortcuts.ts";
  import { reloadTheme, resolveActiveThemeName } from "../../../lib/theme.ts";
  import { apiFetch } from "../../../lib/protocol/http.ts";
  import { isDesktop, shellKind } from "../../../lib/platform.ts";
  import SettingRow from "./SettingRow.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
  }

  let { collapsed, onToggle }: Props = $props();

  let themeNames = $state<string[]>([]);

  $effect(() => {
    apiFetch("/api/home/themes")
      .then((r) => r.json())
      .then((names: string[]) => { themeNames = names; });
  });

  const s = $derived(settings());
  const isAuto = $derived(s.appearance.themeMode === "auto");

  async function onThemeChange(e: Event) {
    const name = selectValue<string>(e);
    updateSetting("appearance", "theme", name);
    await reloadTheme(name);
  }

  async function onModeChange(e: Event) {
    const mode = selectValue<ThemeMode>(e);
    updateSetting("appearance", "themeMode", mode);
    await reloadTheme(resolveActiveThemeName());
  }

  async function onThemeDarkChange(e: Event) {
    const name = selectValue<string>(e);
    updateSetting("appearance", "themeDark", name);
    if (isAuto) await reloadTheme(resolveActiveThemeName());
  }

  async function onThemeLightChange(e: Event) {
    const name = selectValue<string>(e);
    updateSetting("appearance", "themeLight", name);
    if (isAuto) await reloadTheme(resolveActiveThemeName());
  }
</script>

<SettingsSection title="APPEARANCE" {collapsed} {onToggle}>
  <SettingRow
    label="Theme Mode"
    showReset={s.appearance.themeMode !== defaults.appearance.themeMode}
    onReset={async () => {
      updateSetting("appearance", "themeMode", defaults.appearance.themeMode);
      await reloadTheme(resolveActiveThemeName());
    }}
  >
    <select value={s.appearance.themeMode} onchange={onModeChange}>
      <option value="fixed">Fixed</option>
      <option value="auto">Auto (follow system)</option>
    </select>
  </SettingRow>

  {#if isAuto}
    <SettingRow
      label="Dark Theme"
      showReset={s.appearance.themeDark !== defaults.appearance.themeDark}
      onReset={async () => {
        updateSetting("appearance", "themeDark", defaults.appearance.themeDark);
        if (isAuto) await reloadTheme(resolveActiveThemeName());
      }}
    >
      <select value={s.appearance.themeDark} onchange={onThemeDarkChange}>
        {#each themeNames as name}
          <option value={name}>{name}</option>
        {/each}
      </select>
    </SettingRow>

    <SettingRow
      label="Light Theme"
      showReset={s.appearance.themeLight !== defaults.appearance.themeLight}
      onReset={async () => {
        updateSetting("appearance", "themeLight", defaults.appearance.themeLight);
        if (isAuto) await reloadTheme(resolveActiveThemeName());
      }}
    >
      <select value={s.appearance.themeLight} onchange={onThemeLightChange}>
        {#each themeNames as name}
          <option value={name}>{name}</option>
        {/each}
      </select>
    </SettingRow>
  {:else}
    <SettingRow
      label="Theme"
      showReset={s.appearance.theme !== defaults.appearance.theme}
      onReset={async () => {
        updateSetting("appearance", "theme", defaults.appearance.theme);
        await reloadTheme(defaults.appearance.theme);
      }}
    >
      <select value={s.appearance.theme} onchange={onThemeChange}>
        {#each themeNames as name}
          <option value={name}>{name}</option>
        {/each}
      </select>
    </SettingRow>
  {/if}

  <SettingRow
    label="Font Size"
    hint={s.appearance.fontSize > 0 ? `${s.appearance.fontSize}px` : "theme default"}
    showReset={s.appearance.fontSize !== defaults.appearance.fontSize}
    onReset={() => updateSetting("appearance", "fontSize", defaults.appearance.fontSize)}
  >
    <input type="range" min="0" max="24" step="1" value={s.appearance.fontSize} onchange={(e) => updateSetting("appearance", "fontSize", inputNumber(e))} />
  </SettingRow>

  <SettingRow
    label="Zoom"
    hint={shellKind === "web" && s.appearance.zoomScope === "window"
      ? "In a browser, the browser's own zoom applies to the whole window"
      : `${isMac ? "⌘" : "Ctrl"} + / − / 0`}
    showReset={s.appearance.zoom !== defaults.appearance.zoom}
    onReset={() => updateSetting("appearance", "zoom", defaults.appearance.zoom)}
  >
    <select value={String(s.appearance.zoom)} onchange={(e) => updateSetting("appearance", "zoom", Number(selectValue<string>(e)))}>
      {#each ZOOM_STEPS as step}
        <option value={String(step)}>{zoomLabel(step)}</option>
      {/each}
    </select>
  </SettingRow>

  <SettingRow
    label="Zoom Applies To"
    hint="The whole window, or only the terminals' text"
    showReset={s.appearance.zoomScope !== defaults.appearance.zoomScope}
    onReset={() => updateSetting("appearance", "zoomScope", defaults.appearance.zoomScope)}
  >
    <select value={s.appearance.zoomScope} onchange={(e) => updateSetting("appearance", "zoomScope", selectValue<ZoomScope>(e))}>
      <option value="window">Whole window</option>
      <option value="terminals">Terminals only</option>
    </select>
  </SettingRow>

  <SettingRow
    label="Font Family"
    hint={s.appearance.fontFamily || "theme default"}
    showReset={s.appearance.fontFamily !== defaults.appearance.fontFamily}
    onReset={() => updateSetting("appearance", "fontFamily", defaults.appearance.fontFamily)}
  >
    <select value={s.appearance.fontFamily} onchange={(e) => updateSetting("appearance", "fontFamily", selectValue<string>(e))}>
      <option value="">Theme default</option>
      <option value="system-ui, sans-serif">System Sans</option>
      <option value='ui-monospace, "JetBrains Mono", monospace'>Monospace</option>
    </select>
  </SettingRow>

  {#if isDesktop}
    <SettingRow
      label="Border Radius"
      hint="{s.appearance.borderRadius}px"
      showReset={s.appearance.borderRadius !== defaults.appearance.borderRadius}
      onReset={() => updateSetting("appearance", "borderRadius", defaults.appearance.borderRadius)}
    >
      <input type="range" min="0" max="16" step="1" value={s.appearance.borderRadius} onchange={(e) => updateSetting("appearance", "borderRadius", inputNumber(e))} />
    </SettingRow>
  {/if}
</SettingsSection>
