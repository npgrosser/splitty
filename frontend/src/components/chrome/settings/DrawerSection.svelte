<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import { updateSetting, defaults, inputNumber } from "../../../lib/settings.ts";
  import SettingRow from "./SettingRow.svelte";
  import ToggleSwitch from "./ToggleSwitch.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
  }

  let { collapsed, onToggle }: Props = $props();

  const s = $derived(settings());
</script>

<SettingsSection title="DRAWER" {collapsed} {onToggle}>
  <!-- Edge Reveal -->
  <SettingRow
    label="Edge Reveal"
    hint="Show drawer when hovering screen edge"
    showReset={s.drawer.edgeReveal !== defaults.drawer.edgeReveal}
    onReset={() => updateSetting("drawer", "edgeReveal", defaults.drawer.edgeReveal)}
  >
    <ToggleSwitch
      checked={s.drawer.edgeReveal}
      onchange={() => updateSetting("drawer", "edgeReveal", !s.drawer.edgeReveal)}
    />
  </SettingRow>

  <!-- Resizable -->
  <SettingRow
    label="Resizable"
    hint="Allow resizing the drawer sidebar by dragging"
    showReset={s.drawer.resizable !== defaults.drawer.resizable}
    onReset={() => updateSetting("drawer", "resizable", defaults.drawer.resizable)}
  >
    <ToggleSwitch
      checked={s.drawer.resizable}
      onchange={() => updateSetting("drawer", "resizable", !s.drawer.resizable)}
    />
  </SettingRow>

  <!-- Sidebar Width -->
  {#if s.drawer.resizable}
    <SettingRow
      label="Sidebar Width"
      hint="Width of the drawer sidebar in pixels"
      showReset={s.drawer.sidebarWidth !== defaults.drawer.sidebarWidth}
      onReset={() => updateSetting("drawer", "sidebarWidth", defaults.drawer.sidebarWidth)}
    >
      <input type="range" min="150" max="400" step="10" value={s.drawer.sidebarWidth} onchange={(e) => updateSetting("drawer", "sidebarWidth", inputNumber(e))} />
    </SettingRow>
  {/if}

  <!-- Show Tips -->
  <SettingRow
    label="Show Tips"
    hint="Display helpful tips in the drawer"
    showReset={s.drawer.showTips !== defaults.drawer.showTips}
    onReset={() => updateSetting("drawer", "showTips", defaults.drawer.showTips)}
  >
    <ToggleSwitch
      checked={s.drawer.showTips}
      onchange={() => updateSetting("drawer", "showTips", !s.drawer.showTips)}
    />
  </SettingRow>
</SettingsSection>
