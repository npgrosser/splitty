/**
 * The dockview content renderer of viewer panes (`viewer-panes`), registered
 * as `viewer`.
 *
 * The pane's server is its screen's connection, found from the layout API like
 * a terminal pane's; it reads the document through that connection's reader
 * (`lib/viewer/documents.ts`) while its tab is shown on the active screen. A
 * pane of a server without viewer panes, or with a missing or malformed
 * record, says that it cannot be shown and reads nothing; it never starts a
 * terminal session.
 *
 * What the pane shows is `viewer_view.ts`. It is loaded with the window's
 * first viewer pane, and with it the libraries it renders with, while the
 * pane's document is read; later panes build their view at once. When it
 * cannot be loaded, for instance in a window opened before its server was
 * upgraded, the pane says so and asks for a reload of the window.
 *
 * Focus follows `TerminalPaneRenderer`: dockview's `focus()` only marks the
 * panel active, so the document takes the keyboard here, and only on the active
 * screen. A move to or from a popout window builds the view again in the new
 * window's document, from the reader's cached content and the view's state; a
 * panel built anew for the pane, as when a snapshot rebuilds its screen, starts
 * from the state the last one left (`viewer_view_state.ts`).
 */

import type { GroupPanelPartInitParameters, IContentRenderer, PanelUpdateEvent } from "dockview-core";
import { toast } from "svelte-sonner";
import { bindingForApi } from "../lib/connection/screen_binding.ts";
import { viewerMenuItems } from "../lib/context_menus.ts";
import { openViewerFile } from "../lib/panes.ts";
import type { ViewerRecord } from "../lib/protocol/types.ts";
import { getTheme, subscribeTheme } from "../lib/theme.ts";
import type { DocumentWatch } from "../lib/viewer/documents.ts";
import { MAX_FILE_BYTES, MAX_TEXT_BYTES } from "../lib/viewer/limits.ts";
import { sameViewerSource, viewerParams } from "../lib/viewer/panel.ts";
import { syntaxColors } from "../lib/viewer/syntax_colors.ts";
import { openExternalTarget } from "../lib/window.ts";
import { openPaneLink } from "../stores/ports.svelte.ts";
import { showContextMenu } from "../stores/ui.svelte.ts";
import type { ViewerView } from "./viewer_view.ts";
import { keepViewerState, takeViewerState, type ViewerViewState } from "./viewer_view_state.ts";

type ViewModule = typeof import("./viewer_view.ts");

/** The view's module once it has loaded. */
let viewModule: ViewModule | undefined;
let viewLoad: Promise<ViewModule> | undefined;

/** Load the view's module, once for the window; after a failure the next pane tries again. */
function loadView(): Promise<ViewModule> {
  viewLoad ??= import("./viewer_view.ts").then(
    (module) => (viewModule = module),
    (error: unknown) => {
      viewLoad = undefined;
      throw error;
    },
  );
  return viewLoad;
}

/** What a pane says when its view could not be loaded. */
export const VIEW_LOAD_FAILED = "The viewer could not be loaded. Reload the window to show this pane.";

/** Splitty's tokens and the syntax colors as custom properties of the pane, also in a popout window. */
function applyTheme(element: HTMLElement): void {
  const theme = getTheme();
  for (const [key, value] of Object.entries(theme.ui)) element.style.setProperty(`--splitty-${key}`, value);
  for (const [key, value] of Object.entries(syntaxColors(theme.terminal))) element.style.setProperty(key, value);
  if (theme.terminal["selection-bg"]) element.style.setProperty("--sv-selection", theme.terminal["selection-bg"]);
  element.style.background = theme.ui["surface-base"] ?? "";
  element.style.color = theme.ui["text-primary"] ?? "";
}

export class ViewerPaneRenderer implements IContentRenderer {
  readonly element: HTMLElement;
  private view: ViewerView | null = null;
  private watch: DocumentWatch | null = null;
  private record: ViewerRecord | undefined;
  /** Where the pane's last view was left, until this panel has a view. */
  private state: ViewerViewState | undefined;
  private paneId: string | undefined;
  private screen: string | undefined;
  private loading = false;
  private disposed = false;
  private cleanups: (() => void)[] = [];

  constructor() {
    this.element = document.createElement("div");
    // The rendered document has its own containment; this is a second one
    // around everything the pane shows.
    this.element.setAttribute("style", "width:100%;height:100%;overflow:hidden;position:relative;contain:layout paint;isolation:isolate");
  }

  init(params: GroupPanelPartInitParameters): void {
    const api = params.api;
    const binding = bindingForApi(params.containerApi);
    if (!binding) {
      console.warn(`pane ${api.id}: its layout belongs to no screen, not mounted`);
      return;
    }
    this.element.dataset.paneId = api.id;
    this.paneId = api.id;
    this.screen = binding.key;
    const conn = binding.conn;
    const viewer = viewerParams(params.params);
    this.record = viewer?.record;
    const supported = !!viewer?.supported;
    this.state = takeViewerState(api.id);
    applyTheme(this.element);
    this.cleanups.push(subscribeTheme(() => applyTheme(this.element)));

    // A host's links to its loopback open at their forward (`remote-screens`).
    const openExternal = (url: string) => {
      if (conn.kind === "remote") openPaneLink(conn.key, url);
      else openExternalTarget({ url });
    };
    // The view in the element's current document, from the state the last one left.
    const mount = (module: ViewModule) => {
      const state = this.view?.viewState() ?? this.state;
      this.view?.destroy();
      this.state = undefined;
      this.view = new module.ViewerView(this.element.ownerDocument, {
        record: this.record,
        supported,
        state,
        openExternal,
        openFile: (path) => openViewerFile(params.containerApi, path, api.id),
        refuseLink: () => toast.error("This link cannot be opened", {
          description: "Its path leaves the root folder or holds characters a viewer pane does not take.",
        }),
        reload: () => this.watch?.reload(),
      });
      this.element.replaceChildren(this.view.element);
      if (this.watch) this.view.update(this.watch.state);
    };
    const focusIfActive = () => {
      if (!binding.suppressFocus && api.isActive && binding.isActive()) this.view?.focus();
    };
    const build = () => {
      if (viewModule) {
        mount(viewModule);
        return;
      }
      if (this.loading) return;
      this.loading = true;
      loadView().then((module) => {
        this.loading = false;
        if (this.disposed) return;
        mount(module);
        // The pane became the active one while its view was loading.
        focusIfActive();
      }, (error: unknown) => {
        this.loading = false;
        if (this.disposed) return;
        console.warn(`pane ${api.id}: the viewer could not be loaded`, error);
        this.showLoadFailure();
      });
    };

    if (this.record && supported) {
      this.watch = conn.viewers.watch(api.id, {
        poll: this.record.source.kind === "file",
        limit: this.record.source.kind === "file" ? MAX_FILE_BYTES : MAX_TEXT_BYTES,
        onChange: (state) => this.view?.update(state),
      });
    }
    build();

    // Read only while the tab is shown on the active screen.
    const syncShown = () => this.watch?.setShown(api.isVisible && binding.isActive());
    syncShown();
    const visibility = api.onDidVisibilityChange(syncShown);
    this.cleanups.push(() => visibility.dispose(), binding.onActivate(syncShown), binding.onDeactivate(syncShown));

    this.element.addEventListener("mousedown", () => {
      if (!api.isActive) api.setActive();
    }, true);
    const active = api.onDidActiveChange(() => {
      if (api.isActive) requestAnimationFrame(focusIfActive);
    });
    this.cleanups.push(() => active.dispose(), binding.onActivate(() => requestAnimationFrame(focusIfActive)));

    this.element.addEventListener("contextmenu", (event: MouseEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const view = this.view;
      const source = this.record?.source;
      showContextMenu(viewerMenuItems(api.id, params.containerApi, {
        selection: () => view?.selectionText() ?? "",
        reload: () => this.watch?.reload(),
        mode: () => view?.switchableMode,
        toggleMode: () => view?.toggleMode(),
        wrap: () => view?.wrapping,
        toggleWrap: () => view?.toggleWrap(),
        path: source?.kind === "file" ? source.path : undefined,
      }), event.clientX, event.clientY);
    });

    // Grid ↔ popout: the view is built again in the new window's document; a
    // view still loading is built there once it has loaded.
    let location = api.location.type;
    const moved = api.onDidLocationChange(() => {
      if (api.location.type === location) return;
      location = api.location.type;
      if (this.view && viewModule) mount(viewModule);
      syncShown();
    });
    this.cleanups.push(() => moved.dispose());
  }

  /**
   * A snapshot changed the pane's record in place (`layout/screen_snapshot.ts`):
   * its origin or its revision. A record of another document is not taken;
   * the snapshot builds such a pane anew.
   */
  update(event: PanelUpdateEvent): void {
    const record = viewerParams(event.params)?.record;
    if (!record || !this.record || !sameViewerSource(record.source, this.record.source)) return;
    this.record = record;
    this.view?.setRecord(record);
  }

  dispose(): void {
    this.disposed = true;
    for (const cleanup of this.cleanups) cleanup();
    this.cleanups = [];
    // For the pane's next view, also one a snapshot builds in a moment.
    const state = this.view?.viewState() ?? this.state;
    if (state && this.paneId && this.screen) keepViewerState(this.paneId, this.screen, state);
    this.watch?.close();
    this.watch = null;
    this.view?.destroy();
    this.view = null;
  }

  /** The view could not be loaded: the pane says so as the view's notices do, and reads nothing. */
  private showLoadFailure(): void {
    this.watch?.close();
    this.watch = null;
    const doc = this.element.ownerDocument;
    const pane = doc.createElement("div");
    pane.className = "viewer-pane";
    const body = doc.createElement("div");
    body.className = "viewer-body";
    const notice = doc.createElement("div");
    notice.className = "viewer-notice";
    const text = doc.createElement("p");
    text.className = "viewer-notice-text";
    text.textContent = VIEW_LOAD_FAILED;
    notice.append(text);
    body.append(notice);
    pane.append(body);
    this.element.replaceChildren(pane);
  }
}
