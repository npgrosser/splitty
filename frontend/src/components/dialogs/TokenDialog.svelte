<script lang="ts">
  import { getTheme } from "../../lib/theme.ts";
  import "./dialog.css";

  interface Props {
    open: boolean;
    error?: boolean;
    onSubmit: (token: string) => void;
  }

  let { open, error = false, onSubmit }: Props = $props();
  let value = $state("");
  let inputEl: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (open) {
      inputEl?.focus();
    }
  });

  function handleSubmit(e: Event) {
    e.preventDefault();
    const trimmed = value.trim();
    if (trimmed) onSubmit(trimmed);
  }

  const theme = $derived(getTheme());
</script>

{#if open}
  <div
    class="dialog-backdrop"
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33;--dialog-danger:{theme.ui['danger'] ?? '#e06c75'}"
  >
    <form
      onsubmit={handleSubmit}
      class="dialog-panel"
    >
      <h2 class="dialog-title">
        Connection Token Required
      </h2>

      <p class="dialog-text">
        Paste the token from
        <code class="dialog-code">
          ~/.config/splitty/auth-token
        </code>
      </p>

      {#if error}
        <p class="dialog-error">
          Invalid token. Please try again.
        </p>
      {/if}

      <input
        bind:this={inputEl}
        type="text"
        bind:value
        placeholder="Paste token here"
        spellcheck="false"
        autocomplete="off"
        class="dialog-input"
      />

      <button
        type="submit"
        class="dialog-btn-primary"
      >
        Connect
      </button>
    </form>
  </div>
{/if}
