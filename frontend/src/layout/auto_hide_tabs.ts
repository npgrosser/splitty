import type { DockviewApi, DockviewGroupPanel } from "dockview-core";

/**
 * Auto-hide tab bars per dockview instance.
 *
 * Uses `data-single-tab` on group elements to mark groups whose tab bar
 * should be hidden, and `data-tab-reveal` for mousemove hover reveal.
 * Row-sync ensures groups sharing a visual row stay consistent.
 *
 * Returns a cleanup function.
 */
export function setupAutoHideTabs(
  api: DockviewApi,
  settings: { autoHide: boolean; autoHideMulti: boolean; syncRow: boolean },
): () => void {
  const REVEAL_DELAY = 30;
  const { autoHide, autoHideMulti: hideMulti, syncRow } = settings;

  const syncAllGroups = () => {
    for (const group of api.groups) {
      const el = group.element;
      if (!autoHide) {
        el.removeAttribute("data-single-tab");
        el.removeAttribute("data-tab-reveal");
        continue;
      }
      const shouldHide = hideMulti || group.panels.length <= 1;
      el.toggleAttribute("data-single-tab", shouldHide);
    }

    if (!autoHide || !syncRow) return;

    const TOLERANCE = 2;
    const rows: DockviewGroupPanel[][] = [];
    for (const group of api.groups) {
      const top = group.element.getBoundingClientRect().top;
      const row = rows.find(
        (r) => Math.abs(r[0].element.getBoundingClientRect().top - top) < TOLERANCE,
      );
      if (row) row.push(group);
      else rows.push([group]);
    }

    for (const row of rows) {
      if (row.length < 2) continue;
      if (row.some((g) => g.panels.length > 1)) {
        for (const g of row) g.element.removeAttribute("data-single-tab");
      }
    }
  };

  const groupCleanups = new Map<DockviewGroupPanel, () => void>();

  const setupGroup = (group: DockviewGroupPanel) => {
    const el = group.element;
    let revealTimer: ReturnType<typeof setTimeout> | null = null;

    const getHoverZone = () => {
      const header = el.querySelector<HTMLElement>(".dv-tabs-and-actions-container");
      return (header?.offsetHeight ?? 28) + 100;
    };

    const cancelReveal = () => {
      if (revealTimer) {
        clearTimeout(revealTimer);
        revealTimer = null;
      }
      el.removeAttribute("data-tab-reveal");
    };

    const onMove = (e: MouseEvent) => {
      if (!el.hasAttribute("data-single-tab")) return;
      const rect = el.getBoundingClientRect();
      const nearTop = e.clientY - rect.top < getHoverZone();

      if (nearTop && !el.hasAttribute("data-tab-reveal") && !revealTimer) {
        revealTimer = setTimeout(() => {
          el.setAttribute("data-tab-reveal", "");
          revealTimer = null;
        }, REVEAL_DELAY);
      } else if (!nearTop) {
        cancelReveal();
      }
    };

    el.addEventListener("mousemove", onMove);
    el.addEventListener("mouseleave", cancelReveal);

    const d1 = group.model.onDidAddPanel(syncAllGroups);
    const d2 = group.model.onDidRemovePanel(syncAllGroups);

    groupCleanups.set(group, () => {
      cancelReveal();
      el.removeEventListener("mousemove", onMove);
      el.removeEventListener("mouseleave", cancelReveal);
      el.removeAttribute("data-single-tab");
      el.removeAttribute("data-tab-reveal");
      d1.dispose();
      d2.dispose();
    });
  };

  for (const group of api.groups) {
    setupGroup(group);
  }

  const d1 = api.onDidAddGroup((group) => {
    setupGroup(group);
    syncAllGroups();
  });
  const d2 = api.onDidRemoveGroup((group) => {
    groupCleanups.get(group)?.();
    groupCleanups.delete(group);
    syncAllGroups();
  });
  const d3 = api.onDidMovePanel(syncAllGroups);
  const d4 = api.onDidLayoutChange(syncAllGroups);

  syncAllGroups();

  return () => {
    d1.dispose();
    d2.dispose();
    d3.dispose();
    d4.dispose();
    for (const cleanup of groupCleanups.values()) cleanup();
    groupCleanups.clear();
  };
}
