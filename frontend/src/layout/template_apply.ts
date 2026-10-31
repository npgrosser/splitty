import type { DockviewApi } from "dockview-core";
import type { Template, TemplateLayoutNode, TemplatePane, TemplateFloatingEntry, RestoreMode } from "../lib/protocol/types.ts";
import { addTerminalPane } from "../lib/panes.ts";
import { bindingForApi } from "../lib/connection/screen_binding.ts";
import { getLaunchableProfile, getShellProfiles } from "../lib/shell_profiles.ts";
import { toast } from "svelte-sonner";

function isRelativeDir(dir?: string): boolean {
  if (!dir) return false;
  return dir.startsWith("./") || dir.startsWith("../") || !dir.startsWith("/");
}

function resolveRestoreMode(pane?: TemplatePane): RestoreMode | undefined {
  if (!pane?.command) return undefined;
  if (pane.restore_mode) return pane.restore_mode;
  return "replay";
}

const warnedMissingProfiles = new Set<string>();
const warnedSkippedWebviews = new Set<string>();

/**
 * Report a webview pane that an older template still carries.
 *
 * Splitty no longer renders foreign web content in-process (security review
 * SR-01/SR-12), so such panes are dropped on apply. Templates are
 * data that arrives from repositories, chats and downloads; silently loading a
 * URL they name would reintroduce exactly the path that was removed.
 */
function noteSkippedWebview(paneName: string): void {
  if (warnedSkippedWebviews.has(paneName)) return;
  warnedSkippedWebviews.add(paneName);
  toast.warning(`Skipped web pane “${paneName}”`, {
    description: "Splitty no longer embeds web pages. Open the URL in your browser instead.",
  });
}

function resolveShellProfileId(api: DockviewApi, pane?: TemplatePane): string | undefined {
  // Shell profiles are the local server's; another server's screen starts its own default shell.
  if (bindingForApi(api)?.conn.kind !== "local") return undefined;
  if (!pane?.shell_profile) return getLaunchableProfile()?.id;
  const exact = getShellProfiles().profiles.find((profile) => profile.id === pane.shell_profile);
  if (exact?.enabled && exact.available) return exact.id;
  if (!warnedMissingProfiles.has(pane.shell_profile)) {
    warnedMissingProfiles.add(pane.shell_profile);
    toast.warning(`Shell profile “${pane.shell_profile}” is unavailable`, {
      description: "Splitty is using the default shell profile for this template.",
    });
  }
  return getLaunchableProfile()?.id;
}

/**
 * Apply a template to the dockview layout.
 * Closes all existing panels, then walks the layout tree to create
 * panels with the correct split directions, commands, and size ratios.
 */
export function applyTemplate(api: DockviewApi, template: Template, root?: string): void {
  for (const panel of [...api.panels]) {
    panel.api.close();
  }

  const nodeToPane = new Map<TemplateLayoutNode, string>();

  const anchorId = seedNode(api, template, template.layout, undefined, undefined, true, root, nodeToPane);
  if (anchorId) {
    expandNode(api, template, template.layout, anchorId, root, nodeToPane);
  }

  for (const entry of template.floating ?? []) {
    createFloatingEntry(api, template, entry, root);
  }

  requestAnimationFrame(() => {
    applyRatios(api, template.layout, nodeToPane, api.width, api.height);
  });
}

function getLeafPanes(node: TemplateLayoutNode): string[] | null {
  if (!node.slot) return null;
  const slot = Array.isArray(node.slot) ? node.slot : [node.slot];
  if (slot.length > 0) return slot;
  return null;
}

function createLeafPanes(
  api: DockviewApi,
  template: Template,
  node: TemplateLayoutNode,
  refPaneId: string | undefined,
  direction: "right" | "below" | undefined,
  isFirst: boolean,
  root?: string,
  nodeToPane?: Map<TemplateLayoutNode, string>,
): string {
  const leafPanes = getLeafPanes(node);
  if (!leafPanes) return "";

  let firstId = "";
  for (let i = 0; i < leafPanes.length; i++) {
    const pane = template.panes[leafPanes[i]];
    // Webview panes are skipped, not rendered (security review
    // SR-01/SR-12). A template is data that arrives from a repository, a chat or
    // a download; honouring a `webview` pane would let it load an arbitrary URL
    // into a same-origin iframe on import. Older templates may still contain
    // them, so they are dropped with a visible notice rather than rejected.
    if (pane?.type === "webview") {
      noteSkippedWebview(leafPanes[i]);
      continue;
    }

    {
      const paneRoot = isRelativeDir(pane?.dir) ? root : undefined;
      const restoreMode = resolveRestoreMode(pane);
      if (i === 0) {
        firstId = addTerminalPane(api, {
          direction: isFirst ? undefined : direction,
          referencePanel: isFirst ? undefined : refPaneId,
          command: pane?.command,
          dir: pane?.dir,
          root: paneRoot,
          restoreMode,
          shellProfileId: resolveShellProfileId(api, pane),
          directSpawn: true,
        }) ?? "";
      } else {
        const group = api.getPanel(firstId)?.group;
        addTerminalPane(api, {
          group: group ?? undefined,
          command: pane?.command,
          dir: pane?.dir,
          root: paneRoot,
          restoreMode,
          shellProfileId: resolveShellProfileId(api, pane),
          directSpawn: true,
        });
      }
    }
  }

  nodeToPane?.set(node, firstId);
  return firstId;
}

function seedNode(
  api: DockviewApi,
  template: Template,
  node: TemplateLayoutNode,
  refPaneId: string | undefined,
  direction: "right" | "below" | undefined,
  isFirst: boolean,
  root?: string,
  nodeToPane?: Map<TemplateLayoutNode, string>,
): string {
  if (getLeafPanes(node)) {
    return createLeafPanes(api, template, node, refPaneId, direction, isFirst, root, nodeToPane);
  }

  const firstChild = node.children?.[0];
  if (!firstChild) return "";

  return seedNode(api, template, firstChild, refPaneId, direction, isFirst, root, nodeToPane);
}

function expandNode(
  api: DockviewApi,
  template: Template,
  node: TemplateLayoutNode,
  anchorPanelId: string,
  root?: string,
  nodeToPane?: Map<TemplateLayoutNode, string>,
): void {
  if (getLeafPanes(node)) return;

  const children = node.children ?? [];
  if (children.length === 0) return;

  const branchDirection = node.split === "horizontal" ? "right" as const : "below" as const;
  const childAnchors: string[] = [anchorPanelId];

  for (let i = 1; i < children.length; i++) {
    const previousAnchor = childAnchors[i - 1];
    childAnchors[i] = seedNode(api, template, children[i], previousAnchor, branchDirection, false, root, nodeToPane);
  }

  for (let i = 0; i < children.length; i++) {
    if (!childAnchors[i]) continue;
    expandNode(api, template, children[i], childAnchors[i], root, nodeToPane);
  }
}

function applyRatios(
  api: DockviewApi,
  node: TemplateLayoutNode,
  nodeToPane: Map<TemplateLayoutNode, string>,
  availableWidth: number,
  availableHeight: number,
): void {
  const children = node.children;
  if (!children || children.length < 2) return;

  const isHorizontal = node.split === "horizontal";
  const totalAvailable = isHorizontal ? availableWidth : availableHeight;

  for (let i = 0; i < children.length - 1; i++) {
    const child = children[i];
    if (child.ratio == null) continue;

    const paneId = findFirstPaneId(child, nodeToPane);
    if (!paneId) continue;

    const panel = api.getPanel(paneId);
    if (!panel?.group) continue;

    const targetSize = Math.round(totalAvailable * child.ratio);
    if (isHorizontal) {
      panel.group.api.setSize({ width: targetSize });
    } else {
      panel.group.api.setSize({ height: targetSize });
    }
  }

  for (const child of children) {
    const childRatio = child.ratio ?? (1 / children.length);
    const childWidth = isHorizontal ? availableWidth * childRatio : availableWidth;
    const childHeight = isHorizontal ? availableHeight : availableHeight * childRatio;
    applyRatios(api, child, nodeToPane, childWidth, childHeight);
  }
}

function findFirstPaneId(node: TemplateLayoutNode, nodeToPane: Map<TemplateLayoutNode, string>): string | undefined {
  const id = nodeToPane.get(node);
  if (id) return id;
  if (node.children) {
    for (const child of node.children) {
      const childId = findFirstPaneId(child, nodeToPane);
      if (childId) return childId;
    }
  }
  return undefined;
}

function createFloatingEntry(
  api: DockviewApi,
  template: Template,
  entry: TemplateFloatingEntry,
  root?: string,
): void {
  const leafPanes = getLeafPanes(entry.layout);
  if (!leafPanes || leafPanes.length === 0) return;

  const floatingPos = {
    x: entry.bounds.x * api.width,
    y: entry.bounds.y * api.height,
    width: entry.bounds.w * api.width,
    height: entry.bounds.h * api.height,
  };

  let firstId: string | undefined;

  for (let i = 0; i < leafPanes.length; i++) {
    const pane = template.panes[leafPanes[i]];

    // See the note in the tiled path above: webview panes are dropped.
    if (pane?.type === "webview") {
      noteSkippedWebview(leafPanes[i]);
      continue;
    }

    const paneRoot = isRelativeDir(pane?.dir) ? root : undefined;
    const restoreMode = resolveRestoreMode(pane);

    if (firstId === undefined) {
      firstId = addTerminalPane(api, {
        command: pane?.command, dir: pane?.dir, root: paneRoot, restoreMode, floating: floatingPos,
        shellProfileId: resolveShellProfileId(api, pane),
        directSpawn: true,
      });
    } else {
      const group = api.getPanel(firstId)?.group;
      addTerminalPane(api, {
        group: group ?? undefined, command: pane?.command, dir: pane?.dir, root: paneRoot, restoreMode,
        shellProfileId: resolveShellProfileId(api, pane),
        directSpawn: true,
      });
    }
  }
}
