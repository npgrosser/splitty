<script lang="ts">
  import TokenDialog from "./TokenDialog.svelte";

  let { submit }: { submit: (token: string) => Promise<boolean> } = $props();
  let error = $state(false);
  let pending = false;
  async function onSubmit(token: string): Promise<void> {
    if (pending) return;
    pending = true;
    try { error = !await submit(token); } finally { pending = false; }
  }
</script>

<TokenDialog open {error} {onSubmit} />
