import type { DockviewApi, DockviewGroupPanel, DockviewPanelApi } from "dockview-core";
import { toast } from "svelte-sonner";
import type { ContextMenuItem } from "../stores/ui.svelte.ts";
import { editorMenuItem, sshPathMenuItem } from "./remote_editor.ts";
import { shortcutKey } from "./shortcuts.ts";
import { addTerminalPane, addViewerPane } from "./panes.ts";
import { guardedClose, guardedCloseMultiple } from "./close_guard.ts";
import { bindingForApi, sessionForPane, pasteClipboardToPane } from "./connection/screen_binding.ts";
import { toggleDrawer, getDrawerState } from "../stores/ui.svelte.ts";
import { openExternalTarget } from "./window.ts";
import { getShellProfiles, shellProfileIcon } from "./shell_profiles.ts";
import { visibleShellProfiles } from "./shell_profile_model.ts";
import { emitUICommand } from "./client_commands.ts";
import { getScreens, type Screen } from "../stores/screens.svelte.ts";
import { scratchpadOf, toggleScratchpad } from "./scratchpad.ts";
import type { AnnotationKind } from "./protocol/types.ts";
import type { TerminalSession } from "./terminal/terminal_session.ts";
import { isViewerPanel } from "./viewer/panel.ts";
import { MAX_TEXT_BYTES, isViewerCwd, utf8Length, viewerTitle } from "./viewer/limits.ts";

// ── Pane Context Menu ──

export function paneMenuItems(
  paneId: string,
  api: DockviewApi,
): ContextMenuItem[] {
  const session = sessionForPane(api, paneId);
  const drawerOpen = getDrawerState() !== "closed";

  const items: ContextMenuItem[] = [
    {
      label: "Copy",
      action: () => {
        const sel = session?.emulator.getSelection?.();
        if (sel) navigator.clipboard.writeText(sel);
      },
    },
    {
      label: "Paste",
      action: () => pasteClipboardToPane(api, paneId, session),
    },
    ...openSelectionItem(api, paneId, session),
    { label: "", action: () => {}, separator: true },
    {
      label: "Clear Buffer",
      action: () => session?.emulator.clear(),
    },
    { label: "", action: () => {}, separator: true },
    {
      label: "Split Right",
      action: () => addTerminalPane(api, { direction: "right", referencePanel: paneId }),
      shortcut: shortcutKey("splitRight"),
    },
    {
      label: "Split Down",
      action: () => addTerminalPane(api, { direction: "below", referencePanel: paneId }),
      shortcut: shortcutKey("splitDown"),
    },
  ];

  // session-scratchpad: the pane's own control, also with the tab bar hidden.
  const scratchpad = scratchpadOf(paneId);
  if (scratchpad) {
    items.push({
      label: scratchpad === "open" ? "Hide Scratchpad" : "Show Scratchpad",
      action: () => toggleScratchpad(paneId),
      shortcut: shortcutKey("scratchpad"),
    });
  }

  items.push(...terminalDirectoryItems(api, paneId));

  const move = moveToScreenItem(api, paneId);
  if (move) items.push({ label: "", action: () => {}, separator: true }, move);

  if (!drawerOpen) {
    items.push(
      { label: "", action: () => {}, separator: true },
      { label: "Show Drawer", action: () => toggleDrawer(), shortcut: shortcutKey("drawer") },
    );
  }

  items.push(
    { label: "", action: () => {}, separator: true },
    {
      label: "Close Pane",
      action: () => guardedClose(api, paneId, () => api.getPanel(paneId)?.api.close()),
      danger: true,
      shortcut: shortcutKey("closePane"),
    },
  );

  return items;
}

/**
 * A terminal's selection as it goes to a viewer: trailing whitespace removed
 * from every line (`viewer-panes`, Terminal Selection In A Viewer).
 */
export function selectionForViewer(selection: string): string {
  return selection.split("\n").map((line) => line.replace(/\s+$/u, "")).join("\n");
}

/**
 * "Open Selection in Viewer", while the terminal holds a selection and its
 * server has viewer panes: a text document of the selection to the right of
 * the terminal, or as a tab of the terminal's first viewer when it has one
 * (`addViewerPane`), in its last known directory. A selection over the text
 * limit is refused with a notice, and nothing is sent.
 */
function openSelectionItem(api: DockviewApi, paneId: string, session: TerminalSession | undefined): ContextMenuItem[] {
  if (!session || !bindingForApi(api)?.conn.mux.viewers) return [];
  const text = selectionForViewer(session.emulator.getSelection?.() ?? "");
  if (!text.trim()) return [];
  return [{
    label: "Open Selection in Viewer",
    action: () => {
      if (utf8Length(text) > MAX_TEXT_BYTES) {
        toast.error("The selection is too large for a viewer", {
          description: `A viewer pane holds at most ${MAX_TEXT_BYTES / 1024} KB of text.`,
        });
        return;
      }
      const cwd = session.lastCwd;
      const paneTitle = api.getPanel(paneId)?.title || "Terminal";
      addViewerPane(api, {
        source: { kind: "text", title: viewerTitle(`Selection from ${paneTitle}`), text },
        anchor: paneId,
        placement: "right",
        ...(cwd && isViewerCwd(cwd) ? { cwd } : {}),
      });
    },
  }];
}

/** What a viewer pane's menu acts on, from its view. */
export interface ViewerMenuControls {
  /** The selected text of the document. */
  selection(): string;
  reload(): void;
  /** The mode of a document that switches between rendered and source; undefined for others. */
  mode(): "rendered" | "source" | undefined;
  toggleMode(): void;
  /** Whether the source view wraps lines; undefined while none is shown. */
  wrap(): boolean | undefined;
  toggleWrap(): void;
  /** The file, for a file document. */
  readonly path?: string;
  /** The pane's annotations, while it can hold them. */
  readonly annotations?: AnnotationMenuControls;
}

/** What a viewer pane's menu offers for its annotations (`viewer-annotations`). */
export interface AnnotationMenuControls {
  /** A selected passage, one too short or too long to annotate, or none. */
  readonly selection: "passage" | "short" | "long" | "none";
  annotate(kind: AnnotationKind): void;
  addComment(): void;
  readonly listShown: boolean;
  toggleList(): void;
  /** The document shows no marks of annotations, by the user's choice in this window. */
  readonly marksHidden: boolean;
  toggleMarks(): void;
  /** The passages of resolved annotations are marked as well. */
  readonly resolvedShown: boolean;
  toggleResolved(): void;
}

/** The two kinds (D1 of review-threads): a comment, which also asks, and a suggested change, which also deletes. */
const ANNOTATE_ITEMS: readonly { kind: AnnotationKind; label: string }[] = [
  { kind: "comment", label: "Comment on Selection" },
  { kind: "change", label: "Suggest Change" },
];

/** The annotation entries of a viewer pane's menu. */
function annotationItems(controls: AnnotationMenuControls): ContextMenuItem[] {
  const items: ContextMenuItem[] = [];
  const selection = controls.selection;
  if (selection !== "none") {
    const detail = selection === "short" ? "Select a longer passage" : selection === "long" ? "Over 16 KB" : undefined;
    for (const { kind, label } of ANNOTATE_ITEMS) {
      items.push({
        label,
        action: () => controls.annotate(kind),
        disabled: selection !== "passage",
        ...(detail ? { detail } : {}),
      });
    }
  }
  items.push(
    { label: "Add Comment", action: () => controls.addComment() },
    { label: controls.listShown ? "Hide Annotations" : "Show Annotations", action: () => controls.toggleList() },
    { label: controls.marksHidden ? "Show Marks" : "Hide Marks", action: () => controls.toggleMarks() },
    { label: controls.resolvedShown ? "Hide Resolved" : "Show Resolved", action: () => controls.toggleResolved() },
  );
  return items;
}

/** The context menu of a viewer pane. */
export function viewerMenuItems(paneId: string, api: DockviewApi, controls: ViewerMenuControls): ContextMenuItem[] {
  const selection = controls.selection();
  const mode = controls.mode();
  const wrap = controls.wrap();
  const items: ContextMenuItem[] = [
    {
      label: "Copy",
      action: () => { if (selection) void navigator.clipboard.writeText(selection); },
      disabled: !selection,
    },
    { label: "", action: () => {}, separator: true },
  ];
  if (controls.annotations) items.push(...annotationItems(controls.annotations), { label: "", action: () => {}, separator: true });
  items.push({ label: "Reload", action: () => controls.reload() });
  if (mode) items.push({ label: mode === "rendered" ? "Show Source" : "Show Rendered", action: () => controls.toggleMode() });
  if (wrap !== undefined) items.push({ label: wrap ? "Unwrap Lines" : "Wrap Lines", action: () => controls.toggleWrap() });
  const path = controls.path;
  if (path) items.push({ label: "Copy Path", action: () => void navigator.clipboard.writeText(path) });
  items.push(
    { label: "", action: () => {}, separator: true },
    {
      label: "Split Right",
      action: () => addTerminalPane(api, { direction: "right", referencePanel: paneId }),
      shortcut: shortcutKey("splitRight"),
    },
    {
      label: "Split Down",
      action: () => addTerminalPane(api, { direction: "below", referencePanel: paneId }),
      shortcut: shortcutKey("splitDown"),
    },
  );

  const move = moveToScreenItem(api, paneId);
  if (move) items.push({ label: "", action: () => {}, separator: true }, move);

  if (getDrawerState() === "closed") {
    items.push(
      { label: "", action: () => {}, separator: true },
      { label: "Show Drawer", action: () => toggleDrawer(), shortcut: shortcutKey("drawer") },
    );
  }

  items.push(
    { label: "", action: () => {}, separator: true },
    {
      label: "Close Pane",
      action: () => guardedClose(api, paneId, () => api.getPanel(paneId)?.api.close()),
      danger: true,
      shortcut: shortcutKey("closePane"),
    },
  );
  return items;
}

// ── Tab Context Menu ──

export function tabMenuItems(
  paneId: string,
  panelApi: DockviewPanelApi,
  api: DockviewApi,
): ContextMenuItem[] {
  const groupPanels = panelApi.group.panels;
  const currentIndex = groupPanels.findIndex((p) => p.id === paneId);
  const hasOtherTabs = groupPanels.length > 1;
  const hasTabsToRight = currentIndex < groupPanels.length - 1;

  const items: ContextMenuItem[] = [
    {
      label: "Close Tab",
      action: () => guardedClose(api, paneId, () => panelApi.close()),
      danger: true,
    },
    {
      label: "Close Other Tabs",
      action: () => {
        const toClose = groupPanels.filter((p) => p.id !== paneId);
        guardedCloseMultiple(api, toClose.map((p) => ({ id: p.id, close: () => p.api.close() })));
      },
      disabled: !hasOtherTabs,
    },
    {
      label: "Close Tabs to the Right",
      action: () => {
        const toClose = groupPanels.slice(currentIndex + 1);
        guardedCloseMultiple(api, toClose.map((p) => ({ id: p.id, close: () => p.api.close() })));
      },
      disabled: !hasTabsToRight,
    },
  ];

  items.push(...terminalDirectoryItems(api, paneId));

  const move = moveToScreenItem(api, paneId);
  if (move) items.push({ label: "", action: () => {}, separator: true }, move);

  return items;
}

/**
 * "Move to Screen": the other screens of the terminal's server and a new one.
 * The terminal keeps running; another host is never offered, as its process
 * lives on its own host (workspace-model, Moving a Pane to Another Screen).
 */
export function moveToScreenItem(
  api: DockviewApi,
  paneId: string,
  screens: readonly Screen[] = getScreens(),
): ContextMenuItem | undefined {
  // A managed job's pane stays where its job placed it.
  if ((api.getPanel(paneId)?.params as { jobId?: string } | undefined)?.jobId) return undefined;
  const binding = bindingForApi(api);
  if (!binding) return undefined;
  const move = (target: string | null) => () =>
    emitUICommand({ type: "pane.move-to-screen", screenKey: binding.key, paneId, target });
  const others = screens.filter((screen) => screen.conn === binding.conn.key && screen.id !== binding.key);
  return {
    label: "Move to Screen",
    action: () => {},
    children: [
      ...others.map((screen) => ({ label: screen.name, action: move(screen.id) })),
      ...(others.length > 0 ? [{ label: "", action: () => {}, separator: true }] : []),
      { label: "New Screen", action: move(null) },
    ],
  };
}

/** The directory actions belong to the selected terminal's connection. */
function terminalDirectoryItems(api: DockviewApi, paneId: string): ContextMenuItem[] {
  // The pane's kind is its component; ids carry no meaning (workspace-model).
  if (isViewerPanel(api.getPanel(paneId))) return [];
  const cwd = sessionForPane(api, paneId)?.lastCwd;
  const editor = editorMenuItem(api, paneId);
  const ssh = sshPathMenuItem(api, paneId);
  if (!cwd && !editor) return [];
  const items: ContextMenuItem[] = [{ label: "", action: () => {}, separator: true }];
  if (cwd) items.push({ label: "Copy Working Directory", action: () => navigator.clipboard.writeText(cwd) });
  if (ssh) items.push(ssh);
  if (editor) items.push(editor);
  return items;
}

// ── New Tab Context Menu ──

export function newTabMenuItems(
  containerApi: DockviewApi,
  group: DockviewGroupPanel,
): ContextMenuItem[] {
  // Shell profiles are the local server's; another server's screen offers only
  // a plain terminal, which starts that server's default shell.
  const local = bindingForApi(containerApi)?.conn.kind === "local";
  const state = getShellProfiles();
  const profiles: ContextMenuItem[] = !local ? [] : visibleShellProfiles(state)
    .map((profile) => ({
      label: `${shellProfileIcon(profile)}  ${profile.name}`,
      detail: profile.available ? undefined : profile.reason,
      badge: [
        profile.id === state.defaultProfileId ? "Default" : "",
        profile.experimental ? "Experimental" : "",
      ].filter(Boolean).join(" · ") || undefined,
      disabled: !profile.available,
      shortcut: profile.id === state.defaultProfileId ? shortcutKey("newTab") : undefined,
      action: () => addTerminalPane(containerApi, { group, shellProfileId: profile.id }),
    }));

  if (profiles.length === 0) {
    profiles.push({
      label: "New Terminal",
      action: () => addTerminalPane(containerApi, { group }),
      shortcut: shortcutKey("newTab"),
    });
  }

  // Ports and pages would be the local machine's, not the host's.
  if (!local) return profiles;

  return [
    ...profiles,
    { label: "", action: () => {}, separator: true },
    {
      label: "Open Web Page in Browser",
      action: () => {
        const input = prompt("URL or port number:");
        if (!input) return;
        const trimmed = input.trim();
        const portMatch = trimmed.match(/^(\d+)(\?.*)?$/);
        const port = portMatch ? parseInt(portMatch[1], 10) : 0;
        if (port > 1024 && port <= 65535) {
          openExternalTarget({ port, query: portMatch![2] ?? "" });
        } else {
          openExternalTarget({ url: trimmed });
        }
      },
      shortcut: shortcutKey("openWebPage"),
    },
  ];
}
