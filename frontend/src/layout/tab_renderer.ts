import type { ITabRenderer, TabPartInitParameters } from "dockview-core";
import { guardedClose } from "../lib/close_guard.ts";
import { showContextMenu } from "../stores/ui.svelte.ts";
import { tabMenuItems } from "../lib/context_menus.ts";
import { bindingForApi } from "../lib/connection/screen_binding.ts";
import { toggleScratchpad } from "../lib/scratchpad.ts";
import { scratchpadIcon } from "../panes/scratchpad_area.ts";

/**
 * Custom tab renderer with close guard, context menu, and wheel navigation.
 */
export class SplittyTabRenderer implements ITabRenderer {
  readonly element: HTMLElement;
  private cleanups: (() => void)[] = [];

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "dv-default-tab";
    this.element.style.display = "flex";
    this.element.style.alignItems = "center";
    this.element.style.height = "100%";
    this.element.style.padding = "0 8px";
    this.element.style.gap = "6px";
    this.element.style.cursor = "pointer";
    this.element.style.whiteSpace = "nowrap";
    this.element.style.overflow = "hidden";
  }

  init(params: TabPartInitParameters): void {
    const api = params.api;

    // Title element
    const titleEl = document.createElement("span");
    titleEl.className = "dv-default-tab-content";
    titleEl.textContent = api.title ?? "Terminal";
    this.element.title = api.title ?? "Terminal";
    this.element.appendChild(titleEl);

    // Shown while the pane's scratchpad holds text (session-scratchpad)
    const marker = document.createElement("span");
    marker.title = "The scratchpad holds text";
    marker.setAttribute("aria-label", "Scratchpad holds text");
    marker.innerHTML = scratchpadIcon(11);
    Object.assign(marker.style, { display: "none", lineHeight: "0", color: "var(--splitty-accent)" });
    marker.addEventListener("click", (e) => {
      e.stopPropagation();
      api.setActive();
      toggleScratchpad(api.id);
    });
    this.element.appendChild(marker);
    const scratchpads = bindingForApi(params.containerApi)?.conn.scratchpads;
    if (scratchpads) {
      const update = () => {
        const session = scratchpads.sessionOf(api.id);
        marker.style.display = session && scratchpads.text(session) ? "inline-flex" : "none";
      };
      update();
      this.cleanups.push(scratchpads.subscribe(update));
    }

    // Close button
    const closeBtn = document.createElement("div");
    closeBtn.className = "dv-default-tab-action";
    closeBtn.innerHTML = `<svg height="9" width="9" viewBox="0 0 28 28"><line x1="2" y1="2" x2="26" y2="26" stroke="currentColor" stroke-width="3" stroke-linecap="round"/><line x1="2" y1="26" x2="26" y2="2" stroke="currentColor" stroke-width="3" stroke-linecap="round"/></svg>`;
    closeBtn.addEventListener("mousedown", (e) => e.stopPropagation());
    closeBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      guardedClose(params.containerApi, api.id, () => api.close());
    });
    this.element.appendChild(closeBtn);

    // Title updates
    const titleDisposable = api.onDidTitleChange((e) => {
      titleEl.textContent = e.title;
      this.element.title = e.title;
    });
    this.cleanups.push(() => titleDisposable.dispose());

    // Middle-click close
    this.element.addEventListener("auxclick", (e: MouseEvent) => {
      if (e.button === 1) {
        e.preventDefault();
        e.stopPropagation();
        guardedClose(params.containerApi, api.id, () => api.close());
      }
    });

    // Right-click context menu
    this.element.addEventListener("contextmenu", (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      showContextMenu(tabMenuItems(api.id, api, params.containerApi), e.clientX, e.clientY);
    });

    // Mouse wheel to cycle tabs
    this.element.addEventListener("wheel", (e: WheelEvent) => {
      const group = api.group;
      const panels = group.panels;
      if (panels.length < 2) return;
      e.preventDefault();
      const active = group.activePanel;
      const idx = active ? panels.indexOf(active) : 0;
      const next = e.deltaY > 0
        ? Math.min(idx + 1, panels.length - 1)
        : Math.max(idx - 1, 0);
      if (next !== idx) panels[next].api.setActive();
    }, { passive: false });
  }

  dispose(): void {
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
  }
}
