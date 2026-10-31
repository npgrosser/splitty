<!--
  Headless sortable list with smooth reorder animation.

  Uses pointer events (not HTML5 DnD) to give a dnd-kit-like feel:
  - Dragged item follows the pointer on the Y axis only
  - Other items animate smoothly into the gap via CSS transitions
  - Drag activates only after a configurable pixel threshold

  Usage:
    <SortableList items={list} key={(x) => x.id} onReorder={handleReorder}>
      {#snippet item(entry, { index, dragging })}
        <div class:dragging>…</div>
      {/snippet}
    </SortableList>
-->
<script lang="ts" generics="T">
  import type { Snippet } from "svelte";

  interface Props {
    items: T[];
    key: (item: T) => string | number;
    onReorder: (fromIndex: number, toIndex: number) => void;
    item: Snippet<[T, { index: number; dragging: boolean }]>;
    class?: string;
    /** Pixels the pointer must move before drag activates (default 5). */
    threshold?: number;
    /**
     * A selector for the part of an item that starts a drag; the whole item
     * without it. A list nested in an item then drags only its own items.
     */
    handle?: string;
  }

  let {
    items,
    key,
    onReorder,
    item,
    class: className = "",
    threshold = 5,
    handle,
  }: Props = $props();

  // ── State ──
  let containerEl = $state<HTMLElement>();
  let dragIdx = $state<number | null>(null);
  let overIdx = $state<number | null>(null);
  let deltaY = $state(0);
  let active = $state(false);

  let startY = 0;
  let rects: { top: number; height: number }[] = [];

  // ── Helpers ──

  function measure() {
    if (!containerEl) return;
    rects = Array.from(containerEl.children, (child) => {
      const r = (child as HTMLElement).getBoundingClientRect();
      return { top: r.top, height: r.height };
    });
  }

  function suppressClick(e: Event) {
    e.stopPropagation();
    e.preventDefault();
  }

  // ── Pointer handlers ──

  function handlePointerDown(e: PointerEvent, idx: number) {
    if (e.button !== 0) return;
    if (handle && !(e.target instanceof Element && e.target.closest(handle))) return;
    startY = e.clientY;
    dragIdx = idx;
    overIdx = idx;
    deltaY = 0;
    active = false;
    measure();
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
  }

  function handlePointerMove(e: PointerEvent) {
    if (dragIdx === null) return;
    const dy = e.clientY - startY;

    if (!active) {
      if (Math.abs(dy) < threshold) return;
      active = true;
      document.body.style.userSelect = "none";
      document.body.style.webkitUserSelect = "none";
    }

    e.preventDefault();
    deltaY = dy;

    // Find target index by checking which item centers we crossed
    const center = rects[dragIdx].top + rects[dragIdx].height / 2 + dy;
    let target = dragIdx;

    for (let i = dragIdx + 1; i < items.length; i++) {
      if (center > rects[i].top + rects[i].height / 2) target = i;
      else break;
    }
    for (let i = dragIdx - 1; i >= 0; i--) {
      if (center < rects[i].top + rects[i].height / 2) target = i;
      else break;
    }

    overIdx = target;
  }

  function handlePointerUp() {
    window.removeEventListener("pointermove", handlePointerMove);
    window.removeEventListener("pointerup", handlePointerUp);
    document.body.style.userSelect = "";
    document.body.style.webkitUserSelect = "";

    const didDrag = active;

    if (didDrag && dragIdx !== null && overIdx !== null && overIdx !== dragIdx) {
      onReorder(dragIdx, overIdx);
    }

    dragIdx = null;
    overIdx = null;
    deltaY = 0;
    active = false;

    // Suppress the click browsers synthesize after pointerup so it doesn't
    // accidentally trigger onclick handlers on the item.
    if (didDrag && containerEl) {
      const el = containerEl;
      el.addEventListener("click", suppressClick, { capture: true, once: true });
      setTimeout(() => el.removeEventListener("click", suppressClick, { capture: true }), 0);
    }
  }

  // ── Per-item style computation ──

  function itemTransform(i: number): string {
    if (dragIdx === null || overIdx === null || !active) return "";
    if (i === dragIdx) return `translateY(${deltaY}px)`;

    const h = rects[dragIdx]?.height ?? 0;
    if (dragIdx < overIdx && i > dragIdx && i <= overIdx) return `translateY(${-h}px)`;
    if (dragIdx > overIdx && i < dragIdx && i >= overIdx) return `translateY(${h}px)`;
    return "";
  }

  function itemTransition(i: number): string {
    if (!active) return "";
    if (i === dragIdx) return "none";
    return "transform 200ms cubic-bezier(0.25, 1, 0.5, 1)";
  }
</script>

<div bind:this={containerEl} class={className}>
  {#each items as it, i (key(it))}
    <div
      style:transform={itemTransform(i)}
      style:transition={itemTransition(i)}
      style:z-index={active && dragIdx === i ? 1 : undefined}
      style:position="relative"
      onpointerdown={(e) => handlePointerDown(e, i)}
    >
      {@render item(it, { index: i, dragging: active && dragIdx === i })}
    </div>
  {/each}
</div>
