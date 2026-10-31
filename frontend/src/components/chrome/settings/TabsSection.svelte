<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import { updateSetting, defaults, selectValue, inputNumber, type CloseButtonMode } from "../../../lib/settings.ts";
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

<SettingsSection title="TABS" {collapsed} {onToggle}>
  <!-- Auto Hide -->
  <SettingRow
    label="Auto Hide"
    hint="Hide tab bar when only one tab"
    showReset={s.tabs.autoHide !== defaults.tabs.autoHide}
    onReset={() => updateSetting("tabs", "autoHide", defaults.tabs.autoHide)}
  >
    <ToggleSwitch
      checked={s.tabs.autoHide}
      onchange={() => updateSetting("tabs", "autoHide", !s.tabs.autoHide)}
    />
  </SettingRow>

  <!-- Auto Hide Multi -->
  <SettingRow
    label="Auto Hide Multi"
    hint="Hide tabs in multi-pane groups"
    showReset={s.tabs.autoHideMulti !== defaults.tabs.autoHideMulti}
    onReset={() => updateSetting("tabs", "autoHideMulti", defaults.tabs.autoHideMulti)}
  >
    <ToggleSwitch
      checked={s.tabs.autoHideMulti}
      onchange={() => updateSetting("tabs", "autoHideMulti", !s.tabs.autoHideMulti)}
    />
  </SettingRow>

  <!-- Sync Row -->
  <SettingRow
    label="Sync Row"
    hint="Synchronize tab row across groups"
    showReset={s.tabs.syncRow !== defaults.tabs.syncRow}
    onReset={() => updateSetting("tabs", "syncRow", defaults.tabs.syncRow)}
  >
    <ToggleSwitch
      checked={s.tabs.syncRow}
      onchange={() => updateSetting("tabs", "syncRow", !s.tabs.syncRow)}
    />
  </SettingRow>

  <!-- Locked -->
  <SettingRow
    label="Locked"
    hint="Prevent tab drag and drop"
    showReset={s.tabs.locked !== defaults.tabs.locked}
    onReset={() => updateSetting("tabs", "locked", defaults.tabs.locked)}
  >
    <ToggleSwitch
      checked={s.tabs.locked}
      onchange={() => updateSetting("tabs", "locked", !s.tabs.locked)}
    />
  </SettingRow>

  <!-- Close Button Mode -->
  <SettingRow
    label="Close Button"
    hint="When to show the tab close button"
    showReset={s.tabs.closeButton !== defaults.tabs.closeButton}
    onReset={() => updateSetting("tabs", "closeButton", defaults.tabs.closeButton)}
  >
    <select
      value={s.tabs.closeButton}
      onchange={(e) => updateSetting("tabs", "closeButton", selectValue<CloseButtonMode>(e))}
    >
      <option value="always">Always</option>
      <option value="active">Active Only</option>
    </select>
  </SettingRow>

  <!-- Min Width -->
  <SettingRow
    label="Min Width"
    hint="{s.tabs.minWidth}ch"
    showReset={s.tabs.minWidth !== defaults.tabs.minWidth}
    onReset={() => updateSetting("tabs", "minWidth", defaults.tabs.minWidth)}
  >
    <input type="range" min="4" max="40" step="1" value={s.tabs.minWidth} onchange={(e) => updateSetting("tabs", "minWidth", inputNumber(e))} />
  </SettingRow>

  <!-- Show Split -->
  <SettingRow
    label="Show Split Button"
    showReset={s.tabs.showSplit !== defaults.tabs.showSplit}
    onReset={() => updateSetting("tabs", "showSplit", defaults.tabs.showSplit)}
  >
    <ToggleSwitch
      checked={s.tabs.showSplit}
      onchange={() => updateSetting("tabs", "showSplit", !s.tabs.showSplit)}
    />
  </SettingRow>

  <!-- Show Float -->
  <SettingRow
    label="Show Float Button"
    showReset={s.tabs.showFloat !== defaults.tabs.showFloat}
    onReset={() => updateSetting("tabs", "showFloat", defaults.tabs.showFloat)}
  >
    <ToggleSwitch
      checked={s.tabs.showFloat}
      onchange={() => updateSetting("tabs", "showFloat", !s.tabs.showFloat)}
    />
  </SettingRow>

  <!-- Show Maximize -->
  <SettingRow
    label="Show Maximize Button"
    showReset={s.tabs.showMaximize !== defaults.tabs.showMaximize}
    onReset={() => updateSetting("tabs", "showMaximize", defaults.tabs.showMaximize)}
  >
    <ToggleSwitch
      checked={s.tabs.showMaximize}
      onchange={() => updateSetting("tabs", "showMaximize", !s.tabs.showMaximize)}
    />
  </SettingRow>

  <!-- Show Popout -->
  <SettingRow
    label="Show Popout Button"
    showReset={s.tabs.showPopout !== defaults.tabs.showPopout}
    onReset={() => updateSetting("tabs", "showPopout", defaults.tabs.showPopout)}
  >
    <ToggleSwitch
      checked={s.tabs.showPopout}
      onchange={() => updateSetting("tabs", "showPopout", !s.tabs.showPopout)}
    />
  </SettingRow>
</SettingsSection>
