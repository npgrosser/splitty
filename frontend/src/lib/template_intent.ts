/** Pure projection of apply_template; rendering uses the ordinary screen snapshot path. */
import type { ApplyTemplateOp, LayoutNode, ScreenState, Template, TemplateLayoutNode } from "./protocol/types.ts";
import { newPaneId } from "./ids.ts";

export function templateIntent(template: Template, screenId: string, root = "", createScreen = false): { op: ApplyTemplateOp; screen: ScreenState } {
  if (!template.name || Object.keys(template.panes).length > 256) throw Error("Invalid template name or pane count");
  // The serving server drops legacy web panes before a template reaches the window.
  const paneIds = Object.fromEntries(Object.keys(template.panes).map(name => [name, newPaneId()]));
  const op: ApplyTemplateOp = { type: "apply_template", template, paneIds, root, createScreen };
  const seen = new Set<string>();
  let nodeId = 0;
  const project = (node: TemplateLayoutNode, depth = 0): LayoutNode => {
    if (depth > 32 || !Number.isFinite(node.ratio ?? 0) || (node.ratio ?? 0) < 0) throw Error("Invalid template layout");
    const id = `template-node-${nodeId++}`;
    if (node.slot !== undefined) {
      if (node.split || node.children?.length) throw Error("Invalid template slot");
      const names = Array.isArray(node.slot) ? node.slot : [node.slot];
      if (!names.length) throw Error("Empty template slot");
      const ids = names.map(name => {
        if (!template.panes[name] || seen.has(name)) throw Error("Missing or repeated template pane");
        seen.add(name);
        return paneIds[name];
      });
      return { type: "slot", id, paneIds: ids, activePaneId: ids[0] };
    }
    if (!["horizontal", "vertical"].includes(node.split ?? "") || (node.children?.length ?? 0) < 2) throw Error("Invalid template split");
    const kept = node.children!.map(child => ({ layout: project(child, depth + 1), size: child.ratio && child.ratio > 0 ? child.ratio : 1 }));
    const sum = kept.reduce((n, child) => n + child.size, 0);
    if (!Number.isFinite(sum)) throw Error("Invalid template ratios");
    return { type: "split", id, axis: node.split, children: kept.map(c => c.layout), sizes: kept.map(c => c.size / sum) };
  };
  const layout = project(template.layout);
  const floating = (template.floating ?? []).map((entry, index) => {
    if (entry.layout.slot === undefined) throw Error("Floating layouts must be slots");
    const { x, y, w, h } = entry.bounds;
    if (![x, y, w, h].every(Number.isFinite) || w <= 0 || h <= 0) throw Error("Invalid floating bounds");
    return { id: `template-floating-${index}`, root: project(entry.layout), bounds: entry.bounds };
  });
  if (Object.keys(paneIds).some(name => !seen.has(name))) throw Error("Template contains unused terminals");
  return {
    op,
    screen: { id: screenId, name: template.name, revision: 0, root: layout, floating,
      panes: Object.fromEntries(Object.values(paneIds).map(id => [id, { id, role: "terminal" as const }])), sessions: {} },
  };
}
