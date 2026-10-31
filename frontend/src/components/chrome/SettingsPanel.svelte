<script lang="ts">
  import { onMount } from "svelte";
  import { settings } from "../../stores/settings.svelte.ts";
  import { apiFetch } from "../../lib/protocol/http.ts";
  import { getDesktopPlatform, isDesktop, shellKind } from "../../lib/platform.ts";
  import "./settings/settings.css";

  import TerminalSection from "./settings/TerminalSection.svelte";
  import ShellsSection from "./settings/ShellsSection.svelte";
  import XtermSection from "./settings/XtermSection.svelte";
  import AppearanceSection from "./settings/AppearanceSection.svelte";
  import WindowSection from "./settings/WindowSection.svelte";
  import TabsSection from "./settings/TabsSection.svelte";
  import DrawerSection from "./settings/DrawerSection.svelte";
  import NotificationsSection from "./settings/NotificationsSection.svelte";
  import EditorsSection from "./settings/EditorsSection.svelte";
  import RemoteSection from "./settings/RemoteSection.svelte";
  import ExperimentalSection from "./settings/ExperimentalSection.svelte";
  import BuildInfoSection from "./settings/BuildInfoSection.svelte";
  import WslSection from "./settings/WslSection.svelte";

  // -- Fetched data --
  let colorThemes = $state<string[]>([]);
  let buildInfo = $state<{ version?: string; commit?: string; date?: string } | null>(null);

  // -- Collapsed sections --
  let collapsed = $state<Record<string, boolean>>({});

  function toggleSection(name: string) {
    collapsed = { ...collapsed, [name]: !collapsed[name] };
  }

  // -- Fetch themes and build info --
  onMount(() => {
    apiFetch("/api/themes")
      .then((r) => r.ok ? r.json() : [])
      .then((data: string[]) => { colorThemes = data; })
      .catch(() => {});

    apiFetch("/api/build")
      .then((r) => r.ok ? r.json() : null)
      .then((data) => { buildInfo = data; })
      .catch(() => {});
  });

  // -- Reactive settings reference (for conditional xterm section) --
  const s = $derived(settings());
</script>

<div class="settings-panel">
  <TerminalSection collapsed={collapsed["terminal"] ?? false} onToggle={() => toggleSection("terminal")} {colorThemes} />

  <ShellsSection collapsed={collapsed["shells"] ?? false} onToggle={() => toggleSection("shells")} />

  <XtermSection collapsed={collapsed["xterm"] ?? false} onToggle={() => toggleSection("xterm")} renderer={s.terminal.renderer} />

  <AppearanceSection collapsed={collapsed["appearance"] ?? false} onToggle={() => toggleSection("appearance")} />

  {#if isDesktop}
    <WindowSection collapsed={collapsed["window"] ?? false} onToggle={() => toggleSection("window")} />
  {/if}

  {#if shellKind === "tauri" && getDesktopPlatform() === "windows"}
    <WslSection collapsed={collapsed["wsl"] ?? false} onToggle={() => toggleSection("wsl")} />
  {/if}

  <TabsSection collapsed={collapsed["tabs"] ?? false} onToggle={() => toggleSection("tabs")} />
  <DrawerSection collapsed={collapsed["drawer"] ?? false} onToggle={() => toggleSection("drawer")} />
  <NotificationsSection collapsed={collapsed["notifications"] ?? false} onToggle={() => toggleSection("notifications")} />
  <EditorsSection collapsed={collapsed["editors"] ?? false} onToggle={() => toggleSection("editors")} />
  <RemoteSection collapsed={collapsed["remote"] ?? false} onToggle={() => toggleSection("remote")} />
  <ExperimentalSection collapsed={collapsed["experimental"] ?? false} onToggle={() => toggleSection("experimental")} />
  <BuildInfoSection collapsed={collapsed["build"] ?? false} onToggle={() => toggleSection("build")} {buildInfo} />
</div>
