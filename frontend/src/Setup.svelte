<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { remoteWindows } from "./lib/remoteWindows.ts";
  import HostPickerDialog from "./components/dialogs/HostPickerDialog.svelte";

  type Stage = "detecting" | "provisioning" | "starting" | "ready";

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

  interface SetupProgress {
    stage: Stage;
    message: string;
  }

  interface StartResult {
    distro: string;
    url: string;
  }

  const stages: Stage[] = ["detecting", "provisioning", "starting", "ready"];
  let stage = $state<Stage>("detecting");
  let message = $state("Detecting WSL distributions…");
  let distributions = $state<WslDistribution[]>([]);
  let selected = $state("");
  let error = $state("");
  let busy = $state(true);
  let checked = $state(false);
  let returning = $state(true);
  let pickerOpen = $state(false);
  /** The backend became ready while the host picker was open. */
  let pendingBackendUrl = "";
  let unlisten: UnlistenFn | undefined;

  const stageIndex = $derived(stages.indexOf(stage));
  const supported = $derived(distributions.filter((item) => item.supported));
  const installCommand = "wsl --install -d Ubuntu";

  onMount(() => {
    let disposed = false;
    void (async () => {
      const cleanup = await listen<SetupProgress>("wsl-setup-progress", (event) => {
        stage = event.payload.stage;
        message = event.payload.message;
      });
      if (disposed) {
        cleanup();
        return;
      }
      unlisten = cleanup;
      await bootstrap();
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  });

  async function bootstrap() {
    stage = "starting";
    message = "Starting Splitty…";
    error = "";
    busy = true;
    try {
      const state = await invoke<WindowsDesktopState>("wsl_setup_state");
      const preferred = state.pendingDistro ?? state.selectedDistro;
      if (preferred) {
        selected = preferred;
        await launch(preferred);
        return;
      }
    } catch (cause) {
      const startupError = readableError(cause);
      returning = false;
      await refresh(startupError);
      return;
    }
    returning = false;
    await refresh();
  }

  async function refresh(preservedError = "") {
    stage = "detecting";
    message = "Detecting WSL distributions…";
    error = "";
    busy = true;
    checked = false;
    try {
      const [state, found] = await Promise.all([
        invoke<WindowsDesktopState>("wsl_setup_state"),
        invoke<WslDistribution[]>("wsl_list_distributions"),
      ]);
      distributions = found;
      const preferred = state.pendingDistro ?? state.selectedDistro;
      const candidate = found.find((item) => item.name === preferred && item.supported)
        ?? found.find((item) => item.isDefault && item.supported)
        ?? found.find((item) => item.supported);
      selected = candidate?.name ?? "";
      checked = true;
      busy = false;
      error = preservedError;
    } catch (cause) {
      error = readableError(cause);
      checked = true;
      busy = false;
    }
  }

  async function start() {
    if (!selected) return;
    error = "";
    busy = true;
    try {
      await launch(selected);
    } catch (cause) {
      error = readableError(cause);
      busy = false;
    }
  }

  async function launch(distro: string) {
    const result = await invoke<StartResult>("wsl_start_backend", { distro });
    stage = "ready";
    message = `${result.distro} is ready`;
    // Leaving the page would drop the picker and an open in flight with it.
    if (pickerOpen) pendingBackendUrl = result.url;
    else window.location.replace(result.url);
  }

  function closePicker() {
    pickerOpen = false;
    if (pendingBackendUrl) window.location.replace(pendingBackendUrl);
  }

  function readableError(cause: unknown): string {
    if (typeof cause === "string") return cause;
    if (cause instanceof Error) return cause.message;
    return "An unexpected WSL setup error occurred.";
  }

  async function copyInstallCommand() {
    await navigator.clipboard.writeText(installCommand);
  }
</script>

<div class="window-title" data-tauri-drag-region>
  <span data-tauri-drag-region>{returning ? "Splitty" : "Splitty Setup"}</span>
  <div class="window-actions">
    <button aria-label="Minimize" onclick={() => getCurrentWindow().minimize()}>—</button>
    <button aria-label="Close" class="close" onclick={() => getCurrentWindow().close()}>×</button>
  </div>
</div>

{#if returning}
  <main class="launching">
    <img src="/icon-192.png" alt="" />
    <div class="launch-spinner" aria-hidden="true"></div>
    <h1>Starting Splitty</h1>
    <p>{message}</p>
    {#if remoteWindows.available}
      <button class="secondary" onclick={() => { pickerOpen = true; }}>Connect to host…</button>
    {/if}
  </main>
{:else}
<main>
  <section class="hero">
    <img src="/icon-192.png" alt="" />
    <div>
      <p class="eyebrow">WINDOWS + WSL 2</p>
      <h1>Run Splitty where your shell lives.</h1>
      <p class="intro">
        The interface stays native on Windows. The backend, terminals, working directories,
        and child processes run in the WSL distribution you choose.
      </p>
    </div>
  </section>

  <ol class="progress" aria-label="Setup progress">
    {#each stages as item, index}
      <li class:active={item === stage} class:complete={index < stageIndex}>
        <span>{index < stageIndex ? "✓" : index + 1}</span>
        {item}
      </li>
    {/each}
  </ol>

  <section class="card">
    <div class="status-line">
      <span class:spinner={busy} class="status-dot"></span>
      <strong>{message}</strong>
    </div>

    {#if checked && distributions.length > 0}
      <label for="distribution">WSL 2 distribution</label>
      <select id="distribution" bind:value={selected} disabled={busy}>
        {#each distributions as distro}
          <option value={distro.name} disabled={!distro.supported}>
            {distro.name} · WSL {distro.version}{distro.isDefault ? " · default" : ""}{distro.supported ? "" : " · unsupported"}
          </option>
        {/each}
      </select>

      {#each distributions.filter((item) => !item.supported) as distro}
        <p class="unsupported"><strong>{distro.name}:</strong> {distro.reason}</p>
      {/each}

      {#if supported.length > 0}
        <button class="primary" disabled={busy || !selected} onclick={start}>
          {busy ? "Working…" : `Use ${selected}`}
        </button>
      {/if}
    {:else if checked && !error}
      <p>No WSL distributions were found. Splitty does not install WSL automatically.</p>
      <div class="command-row">
        <code>{installCommand}</code>
        <button onclick={copyInstallCommand}>Copy</button>
      </div>
    {/if}

    {#if error}
      <div class="error" role="alert">{error}</div>
      {#if stage === "detecting"}
        <div class="command-row">
          <code>{installCommand}</code>
          <button onclick={copyInstallCommand}>Copy</button>
        </div>
      {/if}
    {/if}

    {#if checked && (!busy || error)}
      <button class="secondary" disabled={busy} onclick={() => refresh()}>Check again</button>
    {/if}
  </section>

  {#if remoteWindows.available}
    <section class="remote-row">
      <p>Working on a remote host? Connect over SSH, no WSL needed.</p>
      <button class="secondary" onclick={() => { pickerOpen = true; }}>Connect to host…</button>
    </section>
  {/if}

  <p class="footnote">Supported in this release: Windows x64, WSL 2, Ubuntu and Debian x86_64.</p>
</main>
{/if}

{#if remoteWindows.available}
  <!-- The launcher stays open after a remote window opened (desktop-remote). -->
  <HostPickerDialog
    open={pickerOpen}
    title="Connect to host"
    colors={{ bg: "#1a191e", fg: "#f2f0f5", danger: "#e06c75" }}
    hosts={() => remoteWindows.hosts()}
    opener={remoteWindows}
    onClose={closePicker}
  />
{/if}
