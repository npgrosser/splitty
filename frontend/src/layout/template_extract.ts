import type { DockviewApi } from "dockview-core";
import type { Template, TemplateLayoutNode, TemplatePane, RestoreMode } from "../lib/protocol/types.ts";
import { sessionForPane } from "../lib/connection/screen_binding.ts";

interface SerializedNode {
  type: "leaf" | "branch";
  data: unknown;
  size?: number;
}

interface LeafData {
  id: string;
  views: string[];
  activeView?: string;
}

/**
 * Extract pane definitions from all panels in a dockview group.
 * Returns the list of pane names added to the panes record.
 */
function extractGroupPanes(
  api: DockviewApi,
  group: { panels: Iterable<{ id: string; title?: string; params: unknown }> },
  panes: Record<string, TemplatePane>,
  startCounter: number,
  root?: string,
): string[] {
  const names: string[] = [];
  let paneCounter = startCounter;

  // No webview branch here (security review SR-01/SR-12): there
  // are no webview panes left to extract, and emitting `type: "webview"` into a
  // saved template would produce a file that a future reader might act on.
  for (const panel of group.panels) {
    const session = sessionForPane(api, panel.id);
    let command: string | undefined;
    let dir: string | undefined;
    let restoreMode: RestoreMode | undefined;

    if (session) {
      if (session.lastTitle) {
        if (!session.lastIsBaseShell) {
          command = session.lastCmdline || session.lastTitle;
          restoreMode = session.restoreMode ?? "replay";
        }
      } else if (session.command) {
        command = session.command;
        restoreMode = session.restoreMode;
      }
      if (session.lastCwd) {
        dir = root ? relativize(session.lastCwd, root) : session.lastCwd;
      }
    }

    const paneName = command ? paneLabelFromCommand(command) : `shell${paneCounter > 0 ? paneCounter : ""}`;
    let finalName = paneName;
    let suffix = 2;
    while (finalName in panes) finalName = `${paneName}${suffix++}`;

    panes[finalName] = {};
    if (command) panes[finalName].command = command;
    if (dir) panes[finalName].dir = dir;
    if (restoreMode) panes[finalName].restore_mode = restoreMode;
    if (session?.shellProfileId) panes[finalName].shell_profile = session.shellProfileId;

    names.push(finalName);
    paneCounter++;
  }

  return names;
}

function paneLabelFromCommand(cmd: string): string {
  const parts = cmd.split(/\s+/);
  const prog = parts[0];
  const base = prog.split("/").pop() ?? prog;
  if (parts.length > 1 && /^(node|python3?|ruby|perl|deno|bun|ts-node)$/.test(base)) {
    return parts[parts.length - 1].split("/").pop() ?? base;
  }
  if (parts.length > 1) return cmd;
  return base;
}

function relativize(abs: string, root: string): string {
  const r = root.endsWith("/") ? root.slice(0, -1) : root;
  if (abs === r) return ".";
  if (abs.startsWith(r + "/")) return "./" + abs.slice(r.length + 1);
  return abs;
}

/**
 * Extract a Template from the current dockview layout.
 * Uses api.toJSON() to get the grid tree, then maps each leaf (group)
 * to pane definitions based on the session's current foreground process.
 */
export function extractTemplate(api: DockviewApi, name: string, root?: string): Template {
  const json = api.toJSON();
  const grid = json.grid;

  const panes: Record<string, TemplatePane> = {};
  const groupToPaneNames = new Map<string, string[]>();
  let paneCounter = 0;

  const gridGroups: typeof api.groups = [];
  const floatingGroups: typeof api.groups = [];
  for (const group of api.groups) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if ((group as any).api?.location?.type === "floating") {
      floatingGroups.push(group);
    } else {
      gridGroups.push(group);
    }
  }

  for (const group of gridGroups) {
    const names = extractGroupPanes(api, group, panes, paneCounter, root);
    paneCounter += names.length;
    if (names.length > 0) {
      groupToPaneNames.set(group.id, names);
    }
  }

  const rootOrientation = grid.orientation === "HORIZONTAL" ? "horizontal" : "vertical";
  const layout = convertNode(grid.root as SerializedNode, rootOrientation, groupToPaneNames);

  const floating: { layout: TemplateLayoutNode; bounds: { x: number; y: number; w: number; h: number } }[] = [];
  const containerWidth = api.width;
  const containerHeight = api.height;

  for (const group of floatingGroups) {
    const names = extractGroupPanes(api, group, panes, paneCounter, root);
    paneCounter += names.length;
    if (names.length === 0) continue;

    const rect = group.element.getBoundingClientRect();
    const containerRect = group.element.closest(".dv-dockview")?.getBoundingClientRect();
    const offsetX = containerRect ? rect.left - containerRect.left : rect.left;
    const offsetY = containerRect ? rect.top - containerRect.top : rect.top;

    floating.push({
      layout: { slot: names },
      bounds: {
        x: Math.round((offsetX / containerWidth) * 1000) / 1000,
        y: Math.round((offsetY / containerHeight) * 1000) / 1000,
        w: Math.round((rect.width / containerWidth) * 1000) / 1000,
        h: Math.round((rect.height / containerHeight) * 1000) / 1000,
      },
    });
  }

  const tmpl: Template = { name, panes, layout };
  if (floating.length > 0) tmpl.floating = floating;
  return tmpl;
}

function convertNode(
  node: SerializedNode,
  parentOrientation: "horizontal" | "vertical",
  groupMap: Map<string, string[]>,
): TemplateLayoutNode {
  if (node.type === "leaf") {
    const data = node.data as LeafData;
    const names = groupMap.get(data.id) ?? ["unknown"];
    return { slot: names };
  }

  const children = node.data as SerializedNode[];
  const childOrientation = parentOrientation === "horizontal" ? "vertical" : "horizontal";

  if (children.length === 1) {
    return convertNode(children[0], childOrientation, groupMap);
  }

  const totalSize = children.reduce((sum, c) => sum + (c.size ?? 0), 0);
  const converted = children.map((child) => {
    const layoutNode = convertNode(child, childOrientation, groupMap);
    if (totalSize > 0 && child.size != null) {
      layoutNode.ratio = Math.round((child.size / totalSize) * 1000) / 1000;
    }
    return layoutNode;
  });

  return { split: parentOrientation, children: converted };
}
