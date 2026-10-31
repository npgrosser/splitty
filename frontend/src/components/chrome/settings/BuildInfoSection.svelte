<script lang="ts">
  import SettingsSection from "./SettingsSection.svelte";

  interface Props {
    collapsed: boolean;
    onToggle: () => void;
    buildInfo: { version?: string; commit?: string; date?: string } | null;
  }

  let { collapsed, onToggle, buildInfo }: Props = $props();
</script>

<SettingsSection title="BUILD INFO" {collapsed} {onToggle}>
  {#if buildInfo}
    <div class="build-info">
      {#if buildInfo.version}
        <div class="build-row">
          <span class="build-label">Version</span>
          <span class="build-value">{buildInfo.version}</span>
        </div>
      {/if}
      {#if buildInfo.commit}
        <div class="build-row">
          <span class="build-label">Commit</span>
          <span class="build-value">{buildInfo.commit}</span>
        </div>
      {/if}
      {#if buildInfo.date}
        <div class="build-row">
          <span class="build-label">Date</span>
          <span class="build-value">{buildInfo.date}</span>
        </div>
      {/if}
    </div>
  {:else}
    <div class="build-info">
      <span class="build-loading">Loading build info...</span>
    </div>
  {/if}
</SettingsSection>
