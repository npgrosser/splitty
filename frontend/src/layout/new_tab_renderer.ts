import type { IHeaderActionsRenderer, IGroupHeaderProps, DockviewGroupPanel } from "dockview-core";
import { addTerminalPane } from "../lib/panes.ts";
import { showContextMenu } from "../stores/ui.svelte.ts";
import { newTabMenuItems } from "../lib/context_menus.ts";
import { shortcutTitle } from "../lib/shortcuts.ts";

/**
 * Left-side header action: "+" new tab button with right-click context menu.
 */
export class NewTabButtonRenderer implements IHeaderActionsRenderer {
  readonly element: HTMLElement;

  constructor() {
    this.element = document.createElement("div");
    this.element.style.display = "flex";
    this.element.style.alignItems = "center";
    this.element.style.height = "100%";
  }

  init(params: IGroupHeaderProps): void {
    const { containerApi, group } = params;

    const btn = document.createElement("button");
    btn.className = "group-action-btn new-tab-btn new-tab-main";
    btn.title = shortcutTitle("newTab");
    btn.innerHTML = `<svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="8" y1="2" x2="8" y2="14"/><line x1="2" y1="8" x2="14" y2="8"/></svg>`;

    const arrow = document.createElement("button");
    arrow.className = "group-action-btn new-tab-btn new-tab-arrow";
    arrow.title = "Choose shell profile";
    arrow.innerHTML = `<svg width="9" height="9" viewBox="0 0 10 10" fill="currentColor"><path d="M1.5 3.25 5 6.75l3.5-3.5z"/></svg>`;

    const openMenu = (x: number, y: number) => {
      showContextMenu(newTabMenuItems(containerApi, group as DockviewGroupPanel), x, y);
    };

    btn.addEventListener("click", () => {
      addTerminalPane(containerApi, { group: group as DockviewGroupPanel });
    });

    btn.addEventListener("contextmenu", (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      openMenu(e.clientX, e.clientY);
    });

    arrow.addEventListener("click", (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const rect = arrow.getBoundingClientRect();
      openMenu(rect.left, rect.bottom + 2);
    });

    arrow.addEventListener("contextmenu", (e: MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      openMenu(e.clientX, e.clientY);
    });

    this.element.append(btn, arrow);
  }

  dispose(): void {}
}
