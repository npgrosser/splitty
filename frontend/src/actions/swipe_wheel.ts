/**
 * Svelte action: velocity-based wheel gesture detection.
 *
 * Treats each swipe as a single gesture — fires onSwipe("up"|"down")
 * on the first peak, then ignores momentum until velocity drops or
 * a time gap appears. Works well with both touchpads and mouse wheels.
 *
 * Usage:
 *   <div use:swipeWheel={{ onSwipe: (dir) => ... }}>
 */
export function swipeWheel(
  node: HTMLElement,
  params: { onSwipe: (direction: "up" | "down") => void },
) {
  let switched = false;
  let lastTime = 0;
  let lastDelta = 0;
  let onSwipe = params.onSwipe;

  function handleWheel(e: WheelEvent) {
    e.preventDefault();

    const now = performance.now();
    const gap = now - lastTime;
    const deltaAbs = Math.abs(e.deltaY);
    lastTime = now;

    // New gesture: time gap OR velocity increasing (new swipe after momentum died)
    if (gap > 120 || (switched && deltaAbs > lastDelta * 1.5)) {
      switched = false;
    }

    lastDelta = deltaAbs;

    if (switched || deltaAbs < 2) return;

    switched = true;
    onSwipe(e.deltaY > 0 ? "down" : "up");
  }

  node.addEventListener("wheel", handleWheel, { passive: false });

  return {
    update(newParams: { onSwipe: (direction: "up" | "down") => void }) {
      onSwipe = newParams.onSwipe;
    },
    destroy() {
      node.removeEventListener("wheel", handleWheel);
    },
  };
}
