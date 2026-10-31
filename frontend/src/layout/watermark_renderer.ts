import type { IWatermarkRenderer, WatermarkRendererInitParameters } from "dockview-core";
import { addTerminalPane } from "../lib/panes.ts";
import { shortcutKey } from "../lib/shortcuts.ts";

/**
 * Shown when a dockview group has no panels.
 */
export class WatermarkRenderer implements IWatermarkRenderer {
  readonly element: HTMLElement;

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "empty-state";
  }

  init(params: WatermarkRendererInitParameters): void {
    this.element.innerHTML = `
      <div class="empty-state-content">
        <h1 class="empty-state-title">splitty</h1>
        <p class="empty-state-subtitle">no open terminals</p>
        <div class="empty-state-actions">
          <button class="empty-state-btn primary" data-action="new">+ new terminal</button>
        </div>
        <div class="empty-state-shortcuts" style="display:grid;grid-template-columns:auto auto;gap:4px 16px;font-size:10px;color:rgba(255,255,255,0.15)">
          <kbd style="text-align:right;opacity:0.7">${shortcutKey("newTab")}</kbd><span>new terminal</span>
          <kbd style="text-align:right;opacity:0.7">${shortcutKey("splitRight")}</kbd><span>split right</span>
          <kbd style="text-align:right;opacity:0.7">${shortcutKey("splitDown")}</kbd><span>split down</span>
        </div>
      </div>
    `;

    this.element.querySelector('[data-action="new"]')?.addEventListener("click", () => {
      addTerminalPane(params.containerApi);
    });
  }

  dispose(): void {}
}
