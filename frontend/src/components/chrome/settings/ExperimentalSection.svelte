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

<SettingsSection title="EXPERIMENTAL" {collapsed} {onToggle}>
  <!-- Linked Sashes -->
  <SettingRow
    label="Linked Sashes"
    hint="Resize adjacent panes together"
    showReset={s.experimental.linkedSashes !== defaults.experimental.linkedSashes}
    onReset={() => updateSetting("experimental", "linkedSashes", defaults.experimental.linkedSashes)}
  >
    <ToggleSwitch
      checked={s.experimental.linkedSashes}
      onchange={() => updateSetting("experimental", "linkedSashes", !s.experimental.linkedSashes)}
    />
  </SettingRow>

  <!-- Resize Debounce -->
  <SettingRow
    label="Resize Debounce"
    hint={`${s.experimental.resizeDebounce}ms`}
    showReset={s.experimental.resizeDebounce !== defaults.experimental.resizeDebounce}
    onReset={() => updateSetting("experimental", "resizeDebounce", defaults.experimental.resizeDebounce)}
  >
    <input
      type="range"
      min="0"
      max="200"
      step="5"
      value={s.experimental.resizeDebounce}
      onchange={(e) => updateSetting("experimental", "resizeDebounce", inputNumber(e))}
    />
  </SettingRow>
</SettingsSection>
