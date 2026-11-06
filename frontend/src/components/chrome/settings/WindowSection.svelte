<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import {
    updateSetting, defaults, numericLimit, isAllowed, inputNumber, selectValue, type TitleBarColorMode,
  } from "../../../lib/settings.ts";
  import { syncNativeTitleBarAppearance } from "../../../lib/theme.ts";
  import { getDesktopPlatform } from "../../../lib/platform.ts";
  import SettingRow from "./SettingRow.svelte";
  import ToggleSwitch from "./ToggleSwitch.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
  }

  let { collapsed, onToggle }: Props = $props();

  const s = $derived(settings());
  const isMac = getDesktopPlatform() === "mac";
  const isWindows = getDesktopPlatform() === "windows";

  async function updateTitleBarColorMode(mode: TitleBarColorMode) {
    await updateSetting("window", "titleBarColorMode", mode);
    syncNativeTitleBarAppearance();
  }
</script>

<SettingsSection title="WINDOW" {collapsed} {onToggle}>
  <!-- Native Title Bar -->
  {#if !isWindows}
    <SettingRow
      label="Native Title Bar"
      hint="Use OS title bar instead of custom (requires restart)"
      showReset={s.window.nativeTitleBar !== defaults.window.nativeTitleBar}
      onReset={() => updateSetting("window", "nativeTitleBar", defaults.window.nativeTitleBar)}
    >
      <ToggleSwitch
        checked={s.window.nativeTitleBar}
        onchange={() => updateSetting("window", "nativeTitleBar", !s.window.nativeTitleBar)}
      />
    </SettingRow>
  {/if}

  {#if isMac && s.window.nativeTitleBar}
    <SettingRow
      label="macOS Title Bar Color"
      hint="Inactive tab color is currently identical to tab bar base color, so it is omitted"
      showReset={s.window.titleBarColorMode !== defaults.window.titleBarColorMode}
      onReset={() => updateTitleBarColorMode(defaults.window.titleBarColorMode)}
    >
      <select
        value={s.window.titleBarColorMode}
        onchange={(e) => updateTitleBarColorMode(selectValue<TitleBarColorMode>(e))}
      >
        {#if isAllowed("window.titleBarColorMode", "tabBar")}<option value="tabBar">Follow Tab Bar Base</option>{/if}
        {#if isAllowed("window.titleBarColorMode", "native")}<option value="native">Use Native Default</option>{/if}
        {#if isAllowed("window.titleBarColorMode", "terminal")}<option value="terminal">Use Terminal Background</option>{/if}
        {#if isAllowed("window.titleBarColorMode", "activeTab")}<option value="activeTab">Use Active Tab</option>{/if}
      </select>
    </SettingRow>
  {/if}

  <!-- Title Bar Height -->
  <SettingRow
    label="Title Bar Height"
    hint="{s.window.titleBarHeight || 'auto'}px"
    showReset={s.window.titleBarHeight !== defaults.window.titleBarHeight}
    onReset={() => updateSetting("window", "titleBarHeight", defaults.window.titleBarHeight)}
  >
    <input type="range" min={numericLimit("window.titleBarHeight", {min: 0, max: 60, step: 1}).min} max={numericLimit("window.titleBarHeight", {min: 0, max: 60, step: 1}).max} step={numericLimit("window.titleBarHeight", {min: 0, max: 60, step: 1}).step} value={s.window.titleBarHeight} onchange={(e) => updateSetting("window", "titleBarHeight", inputNumber(e))} />
  </SettingRow>

  <!-- Title Bar Scroll -->
  <SettingRow
    label="Title Bar Scroll"
    hint="Scroll wheel on title bar switches screens"
    showReset={s.window.titleBarScroll !== defaults.window.titleBarScroll}
    onReset={() => updateSetting("window", "titleBarScroll", defaults.window.titleBarScroll)}
  >
    <ToggleSwitch
      checked={s.window.titleBarScroll}
      onchange={() => updateSetting("window", "titleBarScroll", !s.window.titleBarScroll)}
    />
  </SettingRow>
</SettingsSection>
