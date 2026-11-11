<script lang="ts">
  import { onMount, tick } from "svelte";
  import { apiFetch } from "../../lib/protocol/http.ts";
  import { rootAssignment, browserAssignment } from "../../lib/protocol/connection.ts";
  import { plainLines, plainText } from "../../lib/protocol/untrusted.ts";
  import { getTheme } from "../../lib/theme.ts";
  import "./dialog.css";

  interface Prompt { id: string; target: string; prompt: string; confirm: boolean }
  let prompt = $state<Prompt | null>(null);
  let password = $state("");
  let pending = $state(false);
  let error = $state("");
  let panel: HTMLDivElement | undefined = $state();
  let input: HTMLInputElement | undefined = $state();
  const theme = $derived(getTheme());

  $effect(() => {
    const id = prompt?.id;
    password = "";
    error = "";
    if (id) void tick().then(() => (prompt?.confirm ? panel : input)?.focus());
  });

  onMount(() => {
    if ((rootAssignment?.role ?? browserAssignment?.role) === "remote") return;
    let closed = false;
    let timer: ReturnType<typeof setTimeout>;
    const controller = new AbortController();
    async function poll() {
      try {
        const response = await apiFetch("/api/remote/ssh-prompts", { signal: controller.signal });
        if (response.status === 404) return;
        if (response.ok) {
          const values: unknown = await response.json();
          if (Array.isArray(values)) {
            const questions = values.slice(0, 16).filter((p): p is Prompt =>
              p && typeof p.id === "string" && /^[a-f0-9]{64}$/.test(p.id)
              && typeof p.target === "string" && typeof p.prompt === "string" && typeof p.confirm === "boolean");
            const next = questions.find(p => p.id === prompt?.id) ?? questions[0] ?? null;
            if (!closed && next?.id !== prompt?.id) prompt = next;
          }
        }
      } catch { /* Connection retry owns its own error display. */ }
      if (!closed) timer = setTimeout(poll, 750);
    }
    void poll();
    return () => { closed = true; clearTimeout(timer); controller.abort(); password = ""; };
  });

  async function answer(value: string | null) {
    if (!prompt || pending) return;
    const id = prompt.id;
    pending = true;
    error = "";
    password = "";
    try {
      const response = await apiFetch("/api/remote/ssh-prompts", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id, answer: value }),
      });
      if (!response.ok) throw Error("SSH response could not be sent. Try again.");
      if (prompt?.id === id) prompt = null;
    } catch (cause) { error = cause instanceof Error ? cause.message : "SSH response could not be sent."; }
    finally { pending = false; }
  }

  function keydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.key === "Escape") { event.preventDefault(); void answer(null); }
    if (event.key === "Tab") {
      const elements = [...(panel?.querySelectorAll<HTMLElement>("input:not(:disabled), button:not(:disabled)") ?? [])];
      const index = elements.indexOf(document.activeElement as HTMLElement);
      event.preventDefault();
      elements[(index + (event.shiftKey ? -1 : 1) + elements.length) % elements.length]?.focus();
    }
  }
</script>

{#if prompt}
  <div class="dialog-backdrop" role="dialog" aria-modal="true" aria-labelledby="ssh-auth-title" tabindex="-1" onkeydown={keydown}
    style="--dialog-bg:{theme.ui['surface-base']};--dialog-fg:{theme.ui['text-primary']};--dialog-border:{theme.ui['text-primary']}33">
    <div bind:this={panel} class="dialog-panel dialog-panel-compact ssh-auth" tabindex="-1">
      <h2 class="dialog-title dialog-title-sm" id="ssh-auth-title">{prompt.confirm ? "Confirm SSH host key" : "SSH authentication"}</h2>
      <p class="dialog-text">{plainText(prompt.target, 4096)}</p>
      <pre class="ssh-question">{plainLines(prompt.prompt, 16384)}</pre>
      <form onsubmit={(event) => { event.preventDefault(); if (!prompt?.confirm) void answer(password); }}>
        {#if !prompt.confirm}
          <label for="ssh-password">Password / passphrase</label>
          <input bind:this={input} bind:value={password} id="ssh-password" type="password" autocomplete="off" disabled={pending} maxlength="16384" />
        {/if}
        {#if error}<p class="dialog-error" role="alert">{error}</p>{/if}
        <div class="dialog-actions">
          <button class="dialog-btn" type="button" disabled={pending} onclick={() => answer(null)}>Cancel</button>
          {#if prompt.confirm}
            <button class="dialog-btn" type="button" disabled={pending} onclick={() => answer("yes")}>Accept fingerprint</button>
          {:else}
            <button class="dialog-btn" type="submit" disabled={pending}>Continue</button>
          {/if}
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .ssh-auth { width: 560px; outline: none; }
  .ssh-question { white-space: pre-wrap; overflow-wrap: anywhere; overflow-y: auto; max-height: 35vh; font-size: 13px; }
  form { display: flex; flex-direction: column; gap: 12px; }
  input { color: var(--dialog-fg); background: var(--dialog-bg); border: 1px solid var(--dialog-border); padding: 8px; border-radius: 4px; }
</style>
