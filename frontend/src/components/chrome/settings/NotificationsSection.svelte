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

<SettingsSection title="NOTIFICATIONS" {collapsed} {onToggle}>
  <!-- Enabled -->
  <SettingRow
    label="Enabled"
    hint="Master notification toggle"
    showReset={s.notifications.enabled !== defaults.notifications.enabled}
    onReset={() => updateSetting("notifications", "enabled", defaults.notifications.enabled)}
  >
    <ToggleSwitch
      checked={s.notifications.enabled}
      onchange={() => updateSetting("notifications", "enabled", !s.notifications.enabled)}
    />
  </SettingRow>

  <!-- Toasts -->
  <SettingRow
    label="Toasts"
    hint="In-app toast notifications"
    showReset={s.notifications.toasts !== defaults.notifications.toasts}
    onReset={() => updateSetting("notifications", "toasts", defaults.notifications.toasts)}
  >
    <ToggleSwitch
      checked={s.notifications.toasts}
      onchange={() => updateSetting("notifications", "toasts", !s.notifications.toasts)}
    />
  </SettingRow>

  <!-- Desktop -->
  <SettingRow
    label="Desktop"
    hint="OS-level desktop notifications"
    showReset={s.notifications.desktop !== defaults.notifications.desktop}
    onReset={() => updateSetting("notifications", "desktop", defaults.notifications.desktop)}
  >
    <ToggleSwitch
      checked={s.notifications.desktop}
      onchange={() => updateSetting("notifications", "desktop", !s.notifications.desktop)}
    />
  </SettingRow>

  <!-- Process Finished -->
  <SettingRow
    label="Process Finished"
    hint="Notify when a process finishes"
    showReset={s.notifications.processFinished !== defaults.notifications.processFinished}
    onReset={() => updateSetting("notifications", "processFinished", defaults.notifications.processFinished)}
  >
    <ToggleSwitch
      checked={s.notifications.processFinished}
      onchange={() => updateSetting("notifications", "processFinished", !s.notifications.processFinished)}
    />
  </SettingRow>

  <!-- Process Exited -->
  <SettingRow
    label="Process Exited"
    hint="Notify when a process exits normally"
    showReset={s.notifications.processExited !== defaults.notifications.processExited}
    onReset={() => updateSetting("notifications", "processExited", defaults.notifications.processExited)}
  >
    <ToggleSwitch
      checked={s.notifications.processExited}
      onchange={() => updateSetting("notifications", "processExited", !s.notifications.processExited)}
    />
  </SettingRow>

  <!-- Process Crashed -->
  <SettingRow
    label="Process Crashed"
    hint="Notify when a process crashes"
    showReset={s.notifications.processCrashed !== defaults.notifications.processCrashed}
    onReset={() => updateSetting("notifications", "processCrashed", defaults.notifications.processCrashed)}
  >
    <ToggleSwitch
      checked={s.notifications.processCrashed}
      onchange={() => updateSetting("notifications", "processCrashed", !s.notifications.processCrashed)}
    />
  </SettingRow>

</SettingsSection>
