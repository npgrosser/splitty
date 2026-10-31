/**
 * Svelte action: drag-to-resize handle for panels.
 */
export interface DragResizeParams {
  onDragStart: () => void;
  onDrag: (width: number) => void;
  onDragEnd: (width: number) => void;
  onReset: () => void;
  min: number;
  max: number;
  getWidth: () => number;
}

export function dragResize(node: HTMLElement, params: DragResizeParams) {
  let startX = 0;
  let startWidth = 0;
  let dragging = false;

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    dragging = true;
    startX = e.clientX;
    startWidth = params.getWidth();
    node.setPointerCapture(e.pointerId);
    params.onDragStart();
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return;
    const delta = e.clientX - startX;
    const clamped = Math.min(params.max, Math.max(params.min, startWidth + delta));
    params.onDrag(clamped);
  }

  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    node.releasePointerCapture(e.pointerId);
    const delta = e.clientX - startX;
    const clamped = Math.min(params.max, Math.max(params.min, startWidth + delta));
    params.onDragEnd(clamped);
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }

  function onDblClick() {
    params.onReset();
  }

  node.addEventListener("pointerdown", onPointerDown);
  node.addEventListener("pointermove", onPointerMove);
  node.addEventListener("pointerup", onPointerUp);
  node.addEventListener("dblclick", onDblClick);

  return {
    update(newParams: DragResizeParams) {
      params = newParams;
    },
    destroy() {
      node.removeEventListener("pointerdown", onPointerDown);
      node.removeEventListener("pointermove", onPointerMove);
      node.removeEventListener("pointerup", onPointerUp);
      node.removeEventListener("dblclick", onDblClick);
    },
  };
}
