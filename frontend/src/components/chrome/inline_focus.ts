/** Focus an explicitly opened editor, and return to its trigger on completion. */
export function focusInlineForm(node: HTMLFormElement, cancel: () => void) {
  const previous = node.ownerDocument.activeElement;
  let active = true;
  let oncancel = cancel;
  queueMicrotask(() => {
    if (active) node.querySelector<HTMLInputElement>("input:not([disabled])")?.focus();
  });
  function keydown(event: KeyboardEvent) {
    if (!unclaimedEscape(event)) return;
    event.preventDefault();
    event.stopPropagation();
    oncancel();
  }
  node.addEventListener("keydown", keydown);
  return {
    update(next: () => void) { oncancel = next; },
    destroy() {
      active = false;
      node.removeEventListener("keydown", keydown);
      if (node.contains(node.ownerDocument.activeElement) && previous instanceof HTMLElement && previous.isConnected) {
        // Wait until Svelte has re-enabled the trigger after a completed action.
        queueMicrotask(() => {
          if (!previous.isConnected) return;
          const closed = previous.closest<HTMLDetailsElement>("details:not([open])");
          if (closed) closed.querySelector("summary")?.focus();
          else previous.focus();
        });
      }
    },
  };
}

/** Native disclosure keyboard activation plus Escape, without a menu focus trap. */
export function escapeDisclosure(node: HTMLDetailsElement) {
  function keydown(event: KeyboardEvent) {
    if (!unclaimedEscape(event) || !node.open) return;
    event.preventDefault();
    event.stopPropagation();
    node.open = false;
    node.querySelector("summary")?.focus();
  }
  node.addEventListener("keydown", keydown);
  return { destroy() { node.removeEventListener("keydown", keydown); } };
}
/** Editors claim Escape before the drawer's bubbling listener sees it. */
export function unclaimedEscape(event: KeyboardEvent): boolean {
  return event.key === "Escape" && !event.defaultPrevented && !event.isComposing;
}

