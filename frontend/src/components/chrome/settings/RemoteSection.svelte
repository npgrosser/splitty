<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import { updateSetting, defaults } from "../../../lib/settings.ts";
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

<SettingsSection title="REMOTE" {collapsed} {onToggle}>
  <!-- Forward new ports -->
  <SettingRow
    label="Forward New Ports"
    hint="Forward a host's new ports without asking; every program here can reach them"
    showReset={s.remote.autoForwardPorts !== defaults.remote.autoForwardPorts}
    onReset={() => updateSetting("remote", "autoForwardPorts", defaults.remote.autoForwardPorts)}
  >
    <ToggleSwitch
      checked={s.remote.autoForwardPorts}
      onchange={() => updateSetting("remote", "autoForwardPorts", !s.remote.autoForwardPorts)}
    />
  </SettingRow>
</SettingsSection>
