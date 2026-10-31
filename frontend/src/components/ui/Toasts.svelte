<script lang="ts">
  import { Toaster } from "svelte-sonner";

  let { theme }: { theme: "dark" | "light" } = $props();
</script>

<Toaster
  {theme}
  position="bottom-right"
  closeButton
  offset={16}
  gap={8}
  class="splitty-toaster"
  toastOptions={{ unstyled: true, classes: { toast: "splitty-toast" } }}
/>

<style>
  /* Keep Sonner's stacking, swipe gestures and live region; use app chrome for
     the cards so custom themes and appearance settings apply here as well. */
  :global(.splitty-toaster[data-sonner-toaster]) {
    font: inherit;
  }

  :global(.splitty-toast[data-sonner-toast]) {
    display: flex;
    align-items: center;
    gap: 8px;
    width: var(--width);
    max-width: calc(100vw - var(--mobile-offset-left) - var(--mobile-offset-right));
    padding: 12px;
    border: 1px solid var(--splitty-border-default);
    border-radius: var(--splitty-border-radius, 8px);
    background: var(--splitty-surface-overlay);
    color: var(--splitty-text-primary);
    box-shadow: 0 4px 16px var(--splitty-shadow);
    font: inherit;
    line-height: 1.4;
  }

  :global(.splitty-toast [data-content]) {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  :global(.splitty-toast[data-expanded="false"][data-front="false"] > *) {
    opacity: 0;
  }

  :global(.splitty-toast [data-title]) {
    font-weight: 500;
  }

  :global(.splitty-toaster .splitty-toast[data-sonner-toast] [data-description]) {
    color: var(--splitty-text-secondary);
    font-weight: 400;
  }

  :global(.splitty-toast [data-icon]) {
    --gray11: var(--splitty-text-secondary);
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 16px;
    height: 16px;
    color: var(--splitty-text-secondary);
  }

  :global(.splitty-toast[data-type="info"] [data-icon]) { color: var(--splitty-accent); }
  :global(.splitty-toast[data-type="success"] [data-icon]) { color: var(--splitty-success); }
  :global(.splitty-toast[data-type="warning"] [data-icon]) { color: var(--splitty-warning); }
  :global(.splitty-toast[data-type="error"] [data-icon]) { color: var(--splitty-danger); }

  :global(.splitty-toast [data-icon] svg) {
    flex-shrink: 0;
    width: 16px;
    height: 16px;
  }

  :global(.splitty-toast [data-button]),
  :global(.splitty-toast [data-close-button]) {
    flex-shrink: 0;
    min-height: 24px;
    padding: 3px 8px;
    border: 1px solid var(--splitty-border-default);
    border-radius: 5px;
    background: none;
    color: var(--splitty-text-primary);
    font: inherit;
    cursor: pointer;
    transition: color 80ms ease, background 80ms ease;
  }

  :global(.splitty-toast [data-cancel]) {
    color: var(--splitty-text-secondary);
  }

  :global(.splitty-toast [data-button]:hover),
  :global(.splitty-toast [data-close-button]:hover) {
    background: var(--splitty-hover-bg);
    color: var(--splitty-text-primary);
  }

  :global(.splitty-toast:has([data-close-button])) {
    padding-inline-end: 36px;
  }

  :global(.splitty-toast [data-close-button]) {
    position: absolute;
    top: 8px;
    inset-inline-end: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    min-height: 20px;
    padding: 0;
    border-color: transparent;
    color: var(--splitty-text-muted);
  }

  :global(.splitty-toast[data-sonner-toast]:focus-visible),
  :global(.splitty-toast [data-button]:focus-visible),
  :global(.splitty-toast [data-close-button]:focus-visible) {
    outline: 2px solid var(--splitty-accent);
    outline-offset: 2px;
  }
</style>
