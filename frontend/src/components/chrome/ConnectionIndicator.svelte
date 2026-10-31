<script lang="ts">
  import { localConnection } from "../../lib/connection/registry.ts";

  const hostParam = new URLSearchParams(window.location.search).get("host");

  let mode = $derived(localConnection().mux.serverMode);
  let label = $derived(
    hostParam
      ? `Remote: ${hostParam}`
      : mode === "daemon"
        ? "Daemon"
        : "",
  );
</script>

{#if label}
  <span class="connection-indicator" title={
    mode === "daemon"
      ? "Connected to daemon \u2014 sessions persist"
      : `Connected to ${hostParam}`
  }>
    <span class="indicator-dot"></span>
    {label}
  </span>
{/if}

<style>
.connection-indicator {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  color: var(--splitty-text-secondary);
  padding: 0 8px;
  white-space: nowrap;
  user-select: none;
}

.indicator-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--splitty-accent, #4fc3f7);
  flex-shrink: 0;
}
</style>
