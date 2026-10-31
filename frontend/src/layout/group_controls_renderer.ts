import type { IHeaderActionsRenderer, IGroupHeaderProps, DockviewApi, DockviewGroupPanel } from "dockview-core";
import { addTerminalPane } from "../lib/panes.ts";
import { getSettings, subscribe as subscribeSettings } from "../lib/settings.ts";
import { shortcutTitle } from "../lib/shortcuts.ts";
import { bindingForApi } from "../lib/connection/screen_binding.ts";
import { toggleScratchpad } from "../lib/scratchpad.ts";
import { scratchpadIcon } from "../panes/scratchpad_area.ts";

// ── SVG Icons ──

const splitRightSvg = `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="1" width="14" height="14" rx="1.5"/><line x1="8" y1="1" x2="8" y2="15"/></svg>`;
const splitDownSvg = `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="1" width="14" height="14" rx="1.5"/><line x1="1" y1="8" x2="15" y2="8"/></svg>`;
const floatSvg = `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="4" width="10" height="11" rx="1.5"/><path d="M5 4V2.5A1.5 1.5 0 0 1 6.5 1H13.5A1.5 1.5 0 0 1 15 2.5V9.5A1.5 1.5 0 0 1 13.5 11H11"/></svg>`;
const dockSvg = `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="1" width="14" height="14" rx="1.5"/><rect x="4" y="4" width="8" height="8" rx="0.5"/></svg>`;
const popoutSvg = `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="3" width="11" height="12" rx="1.5"/><polyline points="9,1 15,1 15,11"/></svg>`;
const maximizeSvg = `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="1" width="14" height="14" rx="1.5"/></svg>`;
const restoreSvg = `<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="1" y="3" width="10" height="10" rx="1.5"/><polyline points="5,3 5,1 15,1 15,11 13,11"/></svg>`;

function makeBtn(svg: string, title: string): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.className = "group-action-btn";
  btn.title = title;
  btn.innerHTML = svg;
  return btn;
}

/**
 * Right-side header actions: Split Right, Split Down, Float/Dock, Popout, Maximize/Restore.
 */
export class GroupControlsRenderer implements IHeaderActionsRenderer {
  readonly element: HTMLElement;
  private cleanups: (() => void)[] = [];

  constructor() {
    this.element = document.createElement("div");
    this.element.style.display = "flex";
    this.element.style.alignItems = "center";
    this.element.style.gap = "2px";
    this.element.style.padding = "0 4px";
    this.element.style.height = "100%";
  }

  init(params: IGroupHeaderProps): void {
    const { api, containerApi, group } = params;

    let isMaximized = containerApi.hasMaximizedGroup();
    let isPopout = api.location.type === "popout";
    let isFloating = api.location.type === "floating";

    const splitRightBtn = makeBtn(splitRightSvg, shortcutTitle("splitRight"));
    const splitDownBtn = makeBtn(splitDownSvg, shortcutTitle("splitDown"));
    const floatBtn = makeBtn(floatSvg, "Float Panel");
    const popoutBtn = makeBtn(popoutSvg, "Open in New Window");
    const maxBtn = makeBtn(maximizeSvg, shortcutTitle("maximizePane"));
    const scratchpadBtn = makeBtn(scratchpadIcon(14), shortcutTitle("scratchpad"));

    // The active pane's scratchpad (session-scratchpad)
    scratchpadBtn.addEventListener("click", () => {
      const paneId = (group as any).activePanel?.id;
      if (paneId) toggleScratchpad(paneId);
    });

    // Split Right
    splitRightBtn.addEventListener("click", () => {
      addTerminalPane(containerApi, { direction: "right", group: group as DockviewGroupPanel });
    });

    // Split Down
    splitDownBtn.addEventListener("click", () => {
      addTerminalPane(containerApi, { direction: "below", group: group as DockviewGroupPanel });
    });

    // Float/Dock
    floatBtn.addEventListener("click", () => {
      const activePaneId = (group as any).activePanel?.id;
      const binding = bindingForApi(containerApi);

      if (!isFloating) {
        containerApi.addFloatingGroup(group as DockviewGroupPanel, { width: 600, height: 400 });
        if (binding && activePaneId) {
          const w = containerApi.width || 1;
          const h = containerApi.height || 1;
          binding.sendOp({ type: "float_pane", paneId: activePaneId, bounds: { x: 0.25, y: 0.25, w: 600 / w, h: 400 / h } });
        }
      } else {
        const groupId = String((group as any).id ?? "");
        const floatingId = binding?.floatingId(groupId);
        api.moveTo({ position: "center" });
        binding?.unregisterFloatingId(groupId);
        if (binding && floatingId) {
          binding.sendOp({ type: "dock_pane", floatingId, paneId: containerApi.activePanel?.id });
        }
      }
    });

    // Popout
    popoutBtn.addEventListener("click", () => {
      if (api.location.type !== "popout") {
        containerApi.addPopoutGroup(group as DockviewGroupPanel);
      } else {
        api.moveTo({ position: "right" });
      }
    });

    // Maximize/Restore
    maxBtn.addEventListener("click", () => {
      if (containerApi.hasMaximizedGroup()) {
        containerApi.exitMaximizedGroup();
      } else {
        (group as any).activePanel?.api.maximize();
      }
    });

    this.element.append(scratchpadBtn, splitRightBtn, splitDownBtn, floatBtn, popoutBtn, maxBtn);

    // ── Update visibility based on settings + state ──

    const update = () => {
      const s = getSettings();
      scratchpadBtn.style.display = bindingForApi(containerApi)?.conn.mux.scratchpads ? "" : "none";
      splitRightBtn.style.display = s.tabs.showSplit ? "" : "none";
      splitDownBtn.style.display = s.tabs.showSplit ? "" : "none";
      floatBtn.style.display = s.tabs.showFloat && !isPopout ? "" : "none";
      popoutBtn.style.display = s.tabs.showPopout ? "" : "none";
      maxBtn.style.display = s.tabs.showMaximize && !isPopout ? "" : "none";

      // Float button state
      const gridGroups = containerApi.groups.filter((g) => (g as any).api?.location?.type !== "floating").length;
      floatBtn.disabled = !isFloating && gridGroups < 2;
      floatBtn.innerHTML = isFloating ? dockSvg : floatSvg;
      floatBtn.title = isFloating ? "Dock Panel" : gridGroups < 2 ? "Split first to float a panel" : "Float Panel";

      // Popout button title
      popoutBtn.title = isPopout ? "Move Back to Main Window" : "Open in New Window";

      // Maximize button state
      maxBtn.innerHTML = isMaximized ? restoreSvg : maximizeSvg;
      maxBtn.title = isMaximized
        ? shortcutTitle("maximizePane").replace("Maximize", "Restore")
        : shortcutTitle("maximizePane");
    };

    update();

    // Subscribe to state changes
    const d1 = containerApi.onDidMaximizedGroupChange(() => {
      isMaximized = containerApi.hasMaximizedGroup();
      update();
    });
    const d2 = api.onDidLocationChange(() => {
      isPopout = api.location.type === "popout";
      isFloating = api.location.type === "floating";
      update();
    });
    const d3 = containerApi.onDidAddGroup(() => update());
    const d4 = containerApi.onDidRemoveGroup(() => update());
    const unsubSettings = subscribeSettings(() => update());

    this.cleanups.push(
      () => d1.dispose(), () => d2.dispose(),
      () => d3.dispose(), () => d4.dispose(),
      unsubSettings,
    );
  }

  dispose(): void {
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
  }
}
