<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { runningOnQuit } from "../../../lib/close_guard.ts";
  import SettingsSection from "./SettingsSection.svelte";
  import SettingRow from "./SettingRow.svelte";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
  }

  interface WslDistribution {
    name: string;
    version: number;
    isDefault: boolean;
    supported: boolean;
    reason?: string;
  }

  interface WindowsDesktopState {
    selectedDistro?: string;
    pendingDistro?: string;
  }

  let { collapsed, onToggle }: Props = $props();
  let distributions = $state<WslDistribution[]>([]);
  let desktopState = $state<WindowsDesktopState>({});
  let loading = $state(true);
  let error = $state("");

  onMount(async () => {
    try {
      [desktopState, distributions] = await Promise.all([
        invoke<WindowsDesktopState>("wsl_setup_state"),
        invoke<WslDistribution[]>("wsl_list_distributions"),
      ]);
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  });

  async function switchDistribution(name: string) {
    if (!name || name === desktopState.selectedDistro) return;
    const target = distributions.find((item) => item.name === name);
    if (!target?.supported) return;

    const running = runningOnQuit().length;
    const warning = running
      ? `Switch to ${name}? ${running} running process${running === 1 ? "" : "es"} will be stopped when Splitty restarts.`
      : `Switch to ${name}? Splitty will restart.`;
    if (!window.confirm(warning)) return;

    error = "";
    try {
      await invoke("wsl_switch_distribution", { distro: name });
    } catch (cause) {
      error = String(cause);
    }
  }
</script>

<SettingsSection title="WSL DISTRIBUTION" {collapsed} {onToggle}>
  <SettingRow
    label="Distribution"
    hint={desktopState.pendingDistro
      ? `${desktopState.pendingDistro} will be used after restart`
      : "Backend, shells, and terminal data run inside this WSL 2 distribution"}
  >
    <select
      value={desktopState.pendingDistro ?? desktopState.selectedDistro ?? ""}
      disabled={loading}
      onchange={(event) => switchDistribution((event.currentTarget as HTMLSelectElement).value)}
    >
      {#if loading}
        <option value="">Detecting…</option>
      {/if}
      {#each distributions as distro}
        <option value={distro.name} disabled={!distro.supported}>
          {distro.name}{distro.isDefault ? " (default)" : ""}{distro.supported ? "" : " — unsupported"}
        </option>
      {/each}
    </select>
  </SettingRow>

  {#each distributions.filter((distro) => !distro.supported) as distro}
    <div class="wsl-note"><strong>{distro.name}:</strong> {distro.reason}</div>
  {/each}
  {#if error}
    <div class="wsl-note wsl-error">{error}</div>
  {/if}
</SettingsSection>

<style>
  .wsl-note {
    padding: 4px 14px;
    color: var(--splitty-text-secondary);
    font-size: 10px;
    line-height: 1.4;
  }

  .wsl-error {
    color: var(--splitty-danger);
  }
</style>
