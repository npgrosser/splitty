/**
 * Svelte action: detect mouse near the left edge and invoke a callback.
 * Port of React's useEdgeReveal hook.
 */
export function edgeReveal(
  _node: HTMLElement,
  params: { enabled: boolean; onReveal: () => void },
) {
  const EDGE_WIDTH = 6;
  const DWELL_MS = 200;
  const INTENT_SPEED = 800; // px/s max to count as intentional

  let timer: number | undefined;
  let lastX = -1;
  let lastY = -1;
  let lastTime = 0;

  function handleMove(e: MouseEvent) {
    if (!params.enabled || e.buttons !== 0) return;

    const now = performance.now();
    const dt = now - lastTime;
    const dx = Math.abs(e.clientX - lastX);
    const dy = Math.abs(e.clientY - lastY);
    const speed = dt > 0 ? (Math.sqrt(dx * dx + dy * dy) / dt) * 1000 : 0;
    lastX = e.clientX;
    lastY = e.clientY;
    lastTime = now;

    if (e.clientX <= EDGE_WIDTH && speed < INTENT_SPEED) {
      if (!timer) {
        timer = window.setTimeout(() => {
          timer = undefined;
          params.onReveal();
        }, DWELL_MS);
      }
    } else {
      if (timer) {
        clearTimeout(timer);
        timer = undefined;
      }
    }
  }

  function handleLeave() {
    if (timer) {
      clearTimeout(timer);
      timer = undefined;
    }
  }

  document.addEventListener("mousemove", handleMove);
  document.addEventListener("mouseleave", handleLeave);

  return {
    update(newParams: { enabled: boolean; onReveal: () => void }) {
      params = newParams;
      if (!params.enabled && timer) {
        clearTimeout(timer);
        timer = undefined;
      }
    },
    destroy() {
      document.removeEventListener("mousemove", handleMove);
      document.removeEventListener("mouseleave", handleLeave);
      if (timer) clearTimeout(timer);
    },
  };
}
