<script lang="ts">
  import { tick } from "svelte";
  import { getContextMenu, hideContextMenu, type ContextMenuItem } from "../../stores/ui.svelte.ts";

  const menu = $derived(getContextMenu());
  let menuEl: HTMLDivElement | undefined = $state();
  let submenuEl: HTMLDivElement | undefined = $state();
  let expanded: ContextMenuItem | undefined = $state();
  let anchor: HTMLButtonElement | undefined = $state();

  function groupItems(items: ContextMenuItem[]): ContextMenuItem[][] {
    const groups: ContextMenuItem[][] = [];
    let current: ContextMenuItem[] = [];
    for (const item of items) {
      if (item.separator) {
        if (current.length) groups.push(current);
        current = [];
      } else current.push(item);
    }
    if (current.length) groups.push(current);
    return groups;
  }

  $effect(() => {
    // A fresh menu must not retain a submenu from a previous terminal.
    if (menu) { expanded = undefined; anchor = undefined; }
  });

  $effect(() => {
    if (!menuEl || !menu) return;
    const rect = menuEl.getBoundingClientRect();
    menuEl.style.left = `${Math.max(8, Math.min(menu.x, window.innerWidth - rect.width - 8))}px`;
    menuEl.style.top = `${Math.max(8, Math.min(menu.y, window.innerHeight - rect.height - 8))}px`;
    menuEl.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  });

  $effect(() => {
    if (!submenuEl || !expanded || !anchor || !menuEl) return;
    const parent = menuEl.getBoundingClientRect();
    const row = anchor.getBoundingClientRect();
    const rect = submenuEl.getBoundingClientRect();
    const x = parent.right + rect.width + 4 <= window.innerWidth - 8 ? parent.right + 4 : parent.left - rect.width - 4;
    submenuEl.style.left = `${Math.max(8, Math.min(x, window.innerWidth - rect.width - 8))}px`;
    submenuEl.style.top = `${Math.max(8, Math.min(row.top, window.innerHeight - rect.height - 8))}px`;
  });

  function expand(item: ContextMenuItem, button: HTMLButtonElement) {
    expanded = item.children ? item : undefined;
    anchor = item.children ? button : undefined;
  }

  function handleItemClick(item: ContextMenuItem, button: HTMLButtonElement) {
    if (item.disabled) return;
    if (item.children) {
      expand(item, button);
      return;
    }
    item.action();
    hideContextMenu();
  }

  function handleKey(event: KeyboardEvent, sub = false) {
    const container = sub ? submenuEl : menuEl;
    const buttons = Array.from(container?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
        : (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
      buttons[next]?.focus();
      if (!sub && document.activeElement !== anchor) expanded = undefined;
    } else if (event.key === "ArrowRight" && !sub) {
      const item = menu?.items.filter((item) => !item.separator && !item.disabled)[index];
      if (item?.children && buttons[index]) {
        event.preventDefault();
        expand(item, buttons[index]);
        // Wait for Svelte to mount the submenu, without dispatching an editor.
        void tick().then(() => submenuEl?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus());
      }
    } else if ((event.key === "ArrowLeft" || event.key === "Escape") && sub) {
      event.preventDefault();
      expanded = undefined;
      anchor?.focus();
    } else if (event.key === "Escape" || event.key === "Tab") {
      if (event.key === "Escape") event.preventDefault();
      hideContextMenu();
    }
  }
</script>

{#snippet itemContent(item: ContextMenuItem)}
  <span class="splitty-context-menu-copy">
    <span>{item.label}</span>
    {#if item.detail}<span class="splitty-context-menu-detail" title={item.detail}>{item.detail}</span>{/if}
  </span>
  <span class="splitty-context-menu-meta">
    {#if item.badge}<span class="splitty-context-menu-badge">{item.badge}</span>{/if}
    {#if item.shortcut}<span class="splitty-context-menu-shortcut">{item.shortcut}</span>{/if}
    {#if item.children}<span aria-hidden="true">›</span>{/if}
  </span>
{/snippet}

{#if menu}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="splitty-context-menu-backdrop" onmousedown={hideContextMenu} oncontextmenu={(event) => { event.preventDefault(); hideContextMenu(); }}></div>
  <div class="splitty-context-menu" role="menu" tabindex="-1" aria-label="Terminal actions" bind:this={menuEl}
    style="left: {menu.x}px; top: {menu.y}px;" onkeydown={(event) => handleKey(event)}>
    {#each groupItems(menu.items) as group, gi}
      <div class="splitty-context-menu-group" class:separator-before={gi > 0} role="group">
        {#each group as item}
          <button class="splitty-context-menu-item" class:danger={item.danger} class:expanded={expanded === item}
            role="menuitem" disabled={item.disabled} aria-haspopup={item.children ? "menu" : undefined}
            aria-expanded={item.children ? expanded === item : undefined}
            onmouseenter={(event) => expand(item, event.currentTarget)}
            onclick={(event) => handleItemClick(item, event.currentTarget)}>
            {@render itemContent(item)}
          </button>
        {/each}
      </div>
    {/each}
  </div>
  {#if expanded?.children}
    <div class="splitty-context-menu splitty-context-submenu" role="menu" tabindex="-1" aria-label={expanded.label}
      bind:this={submenuEl} onkeydown={(event) => handleKey(event, true)}>
      {#each groupItems(expanded.children) as group, gi}
        <div class="splitty-context-menu-group" class:separator-before={gi > 0} role="group">
          {#each group as item}
            <button class="splitty-context-menu-item" role="menuitem" class:danger={item.danger}
              disabled={item.disabled} onclick={(event) => handleItemClick(item, event.currentTarget)}>
              {@render itemContent(item)}
            </button>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
{/if}
