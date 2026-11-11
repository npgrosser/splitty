/**
 * Svelte action: debounced wheel gesture detection.
 *
 * Treats each continuous scroll/swipe as a single gesture — fires
 * onSwipe("up"|"down") once the noise floor is exceeded, then ignores
 * subsequent events (including touchpad acceleration and momentum) until
 * an idle gap > 120 ms appears.
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
  let onSwipe = params.onSwipe;

  function handleWheel(e: WheelEvent) {
    e.preventDefault();

    const now = performance.now();
    const gap = now - lastTime;
    const deltaAbs = Math.abs(e.deltaY);
    lastTime = now;

    if (gap > 120) {
      switched = false;
    }

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
