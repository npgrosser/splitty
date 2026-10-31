import { DockviewComponent, type DockviewApi, type AddPanelOptions, type DockviewTheme } from "dockview-core";
import { TerminalPaneRenderer, type TerminalPaneParams } from "../panes/terminal_pane.ts";
import { getSplittyTheme } from "./dockview_theme.ts";
import { SplittyTabRenderer } from "./tab_renderer.ts";
import { GroupControlsRenderer } from "./group_controls_renderer.ts";
import { NewTabButtonRenderer } from "./new_tab_renderer.ts";
import { WatermarkRenderer } from "./watermark_renderer.ts";

let panelCounter = 0;

function nextPanelId(): string {
  return `panel-${++panelCounter}`;
}

/**
 * Manages a single DockviewComponent instance.
 *
 * Wraps the framework-agnostic dockview-core API, registers the terminal
 * component factory, and provides convenience methods for adding panes.
 *
 * There is deliberately no "webview" component (security review
 * SR-01/SR-02): an unknown component name throws, so a stale layout or
 * template that still names one fails loudly instead of silently rendering an
 * iframe. Callers strip such panes before they reach dockview.
 */
export class DockviewManager {
  private dockview: DockviewComponent;

  constructor(container: HTMLElement) {
    this.dockview = new DockviewComponent(container, {
      createComponent: (options) => {
        switch (options.name) {
          case "terminal":
            return new TerminalPaneRenderer();
          default:
            throw new Error(`Unknown component: ${options.name}`);
        }
      },
      defaultTabComponent: "splitty",
      createTabComponent: () => new SplittyTabRenderer(),
      createRightHeaderActionComponent: () => new GroupControlsRenderer(),
      createLeftHeaderActionComponent: () => new NewTabButtonRenderer(),
      createWatermarkComponent: () => new WatermarkRenderer(),
      theme: getSplittyTheme(),
      defaultRenderer: "always",
    });
  }

  get api(): DockviewApi {
    return this.dockview.api;
  }

  /** Add a terminal pane. */
  addTerminalPane(options?: {
    params?: TerminalPaneParams;
    position?: AddPanelOptions["position"];
    title?: string;
    id?: string;
    inactive?: boolean;
  }): string {
    const id = options?.id ?? nextPanelId();
    this.dockview.api.addPanel({
      id,
      component: "terminal",
      title: options?.title ?? "Terminal",
      params: options?.params,
      position: options?.position,
      inactive: options?.inactive,
      renderer: "always",
    });
    return id;
  }

  /** Set a raw DockviewTheme. */
  setThemeRaw(theme: DockviewTheme): void {
    this.dockview.updateOptions({ theme });
  }

  /** Dispose the dockview instance and all panels. */
  dispose(): void {
    this.dockview.dispose();
  }
}
