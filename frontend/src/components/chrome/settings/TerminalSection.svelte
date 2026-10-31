<script lang="ts">
  import { settings } from "../../../stores/settings.svelte.ts";
  import {
    updateSetting,
    defaults,
    selectValue,
    inputNumber,
    type TerminalRenderer,
    type CloseOnExit,
    type SizePolicy,
  } from "../../../lib/settings.ts";
  import SettingRow from "./SettingRow.svelte";
  import ToggleSwitch from "./ToggleSwitch.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
    colorThemes: string[];
  }

  let { collapsed, onToggle, colorThemes }: Props = $props();

  const s = $derived(settings());
</script>

<SettingsSection title="TERMINAL" {collapsed} {onToggle}>
  <!-- Renderer -->
  <SettingRow
    label="Renderer"
    hint="Terminal rendering engine"
    showReset={s.terminal.renderer !== defaults.terminal.renderer}
    onReset={() => updateSetting("terminal", "renderer", defaults.terminal.renderer)}
  >
    <select
      value={s.terminal.renderer}
      onchange={(e) => updateSetting("terminal", "renderer", selectValue<TerminalRenderer>(e))}
    >
      <option value="xterm">xterm.js</option>
      <option value="ghostty">Ghostty</option>
    </select>
  </SettingRow>

  <!-- Color Theme -->
  <SettingRow
    label="Color Theme"
    hint="Terminal color scheme"
    showReset={s.terminal.theme !== defaults.terminal.theme}
    onReset={() => updateSetting("terminal", "theme", defaults.terminal.theme)}
  >
    <select
      value={s.terminal.theme}
      onchange={(e) => updateSetting("terminal", "theme", selectValue<string>(e))}
    >
      <option value="default">Default</option>
      {#each colorThemes as theme}
        <option value={theme}>{theme}</option>
      {/each}
    </select>
  </SettingRow>

  <!-- Ligatures -->
  <SettingRow
    label="Ligatures"
    hint="Enable font ligatures"
    showReset={s.terminal.ligatures !== defaults.terminal.ligatures}
    onReset={() => updateSetting("terminal", "ligatures", defaults.terminal.ligatures)}
  >
    <ToggleSwitch
      checked={s.terminal.ligatures}
      onchange={() => updateSetting("terminal", "ligatures", !s.terminal.ligatures)}
    />
  </SettingRow>

  <!--
    "Link Target" and "Webview Mode" removed (security review
    SR-01/SR-02). Both selected how foreign web content was rendered inside the
    application; that path no longer exists, and a user-facing setting would not
    have been a security boundary anyway since any credential holder can write
    settings via /api/home/settings.
  -->

  <!-- Clipboard write via OSC 52 -->
  <SettingRow
    label="Allow Clipboard Writes from Terminal"
    hint="Lets programs set the system clipboard (OSC 52). On by default for copying in terminal apps. Programs can replace what you paste. Reading the clipboard this way is never allowed."
    showReset={s.terminal.allowOsc52Write !== defaults.terminal.allowOsc52Write}
    onReset={() => updateSetting("terminal", "allowOsc52Write", defaults.terminal.allowOsc52Write)}
  >
    <ToggleSwitch
      checked={s.terminal.allowOsc52Write}
      onchange={() => updateSetting("terminal", "allowOsc52Write", !s.terminal.allowOsc52Write)}
    />
  </SettingRow>

  <!-- Close on Exit -->
  <SettingRow
    label="Close on Exit"
    hint="When to close pane after process exits"
    showReset={s.terminal.closeOnExit !== defaults.terminal.closeOnExit}
    onReset={() => updateSetting("terminal", "closeOnExit", defaults.terminal.closeOnExit)}
  >
    <select
      value={s.terminal.closeOnExit}
      onchange={(e) => updateSetting("terminal", "closeOnExit", selectValue<CloseOnExit>(e))}
    >
      <option value="always">Always</option>
      <option value="graceful">Graceful</option>
      <option value="never">Never</option>
    </select>
  </SettingRow>

  <!-- PTY size with several clients (this computer's server; a host has its own config.toml) -->
  <SettingRow
    label="Size With Several Clients"
    hint="Which clients set the size of a terminal that several windows or devices show: the smallest of those in use, the last one used, or the smallest of all. Applies to this computer's terminals; a host uses its own config.toml."
    showReset={s.terminal.sizePolicy !== defaults.terminal.sizePolicy}
    onReset={() => updateSetting("terminal", "sizePolicy", defaults.terminal.sizePolicy)}
  >
    <select
      value={s.terminal.sizePolicy}
      onchange={(e) => updateSetting("terminal", "sizePolicy", selectValue<SizePolicy>(e))}
    >
      <option value="active">Smallest in use</option>
      <option value="latest">Last used</option>
      <option value="smallest">Smallest of all</option>
    </select>
  </SettingRow>

  {#if s.terminal.sizePolicy === "active"}
    <SettingRow
      label="Client Idle After"
      hint={`A client stops counting as in use ${Math.round(s.terminal.sizeIdleSeconds / 60 * 10) / 10} min after its last key, click or window change.`}
      showReset={s.terminal.sizeIdleSeconds !== defaults.terminal.sizeIdleSeconds}
      onReset={() => updateSetting("terminal", "sizeIdleSeconds", defaults.terminal.sizeIdleSeconds)}
    >
      <input
        type="range"
        min="10"
        max="600"
        step="10"
        value={s.terminal.sizeIdleSeconds}
        onchange={(e) => updateSetting("terminal", "sizeIdleSeconds", inputNumber(e))}
      />
    </SettingRow>
  {/if}

  <!-- Balanced Padding -->
  <SettingRow
    label="Balanced Padding"
    hint="Balance whitespace around terminal content"
    showReset={s.terminal.windowPaddingBalance !== defaults.terminal.windowPaddingBalance}
    onReset={() => updateSetting("terminal", "windowPaddingBalance", defaults.terminal.windowPaddingBalance)}
  >
    <ToggleSwitch
      checked={s.terminal.windowPaddingBalance}
      onchange={() => updateSetting("terminal", "windowPaddingBalance", !s.terminal.windowPaddingBalance)}
    />
  </SettingRow>

  <!-- Scroll to Bottom -->
  <SettingRow
    label="Scroll to Bottom"
    hint="Auto-scroll on new output"
    showReset={s.terminal.scrollToBottom !== defaults.terminal.scrollToBottom}
    onReset={() => updateSetting("terminal", "scrollToBottom", defaults.terminal.scrollToBottom)}
  >
    <ToggleSwitch
      checked={s.terminal.scrollToBottom}
      onchange={() => updateSetting("terminal", "scrollToBottom", !s.terminal.scrollToBottom)}
    />
  </SettingRow>
</SettingsSection>
