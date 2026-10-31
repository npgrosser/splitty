# Milestone 3: Svelte-Migration — DONE

> **Voraussetzung:** [Milestone 1 — Multiplexer Backend](01-multiplexer-backend.md) (MuxSync-Layer existiert, dockview-Integration ist stabil)
>
> Die Svelte-Migration ist Frontend-only und kann technisch parallel zu Milestone 1 starten, aber der MuxSync-Layer sollte zuerst stehen — sonst wird er in React gebaut und sofort wieder in Svelte neu geschrieben.

## Context

The web frontend is the primary product — not a temporary stepping stone to native.
A native renderer may exist as an optional performance tier for users who want it,
but the web UI serves the broad user base and will be maintained long-term.

This means the frontend framework choice is a permanent architectural decision.
It needs to scale with the roadmap: plugin system, command palette, agent tracking,
worktree management, notifications, and more reactive UI surface over time.

## Why not React

React's model does not fit the architecture of this project. The codebase already uses
Svelte-like patterns (module-level state, manual `subscribe()`/`notify()`, imperative DOM
work in effects) — but wrapped in React glue code that adds complexity.

**What React forces us to write that Svelte doesn't need:**

| Pattern | React | Svelte |
|---------|-------|--------|
| Reactive module state | `subscribe()` + `useEffect` + `forceUpdate(n=>n+1)` | `writable()` / `$state` |
| Derived values | `useMemo` with dependency arrays | `$derived` |
| Stable callbacks | `useCallback` with dependency arrays | just functions |
| Ref-boxing for closures | `useRef` + `.current` | not needed (no stale closures) |
| DOM cleanup | `useEffect` return + dependency tracking | `onMount`/`onDestroy` |
| Conditional subscriptions | `useEffect` with guards + disposal | reactive blocks |

**Real bugs this has caused:**

- **3 GB memory leak**: HMR re-evaluated `sessions.ts`, creating a new module-level
  `Map`. React effect cleanup closures captured the old module bindings, so
  `getSession()` checked the new (empty) Map → `session.dispose()` was skipped →
  terminals, WebGL contexts, and scrollback buffers leaked on every save.
- **Linked sashes feedback loop**: `requestAnimationFrame` guard in an effect closure
  created an async gap where `onDidLayoutChange` could re-trigger propagation at 60fps.
- **Dependency array drift**: Multiple effects with `[views, viewApis]` dependencies
  that trigger cascading state updates — hard to reason about, easy to break.

## Why not Vanilla

For the current scope (a few dialogs, a view switcher), vanilla JS with a small signal
helper would be sufficient. But the roadmap changes the calculus:

- **Plugin system**: Plugins need to render UI. Without a component model, every plugin
  author builds their own DOM management, which amounts to a custom framework.
- **Command palette**: Fuzzy search, keyboard navigation, dynamic filtering, live list
  updates — tedious in vanilla, trivial in Svelte.
- **Agent tracking / notifications**: Dynamic status badges, progress indicators,
  reactive updates from backend events across many UI surfaces.
- **Worktree management**: Lists with state, actions, branch status — form-heavy UI
  that vanilla makes verbose.

The inflection point: once you need list diffing, conditional rendering, reactive
bindings, and a plugin component API, you're building a framework. Svelte is that
framework, already built and compiled away.

## Why Svelte specifically

- **Compiled away**: No runtime framework code shipped to the browser. Output is
  direct DOM manipulation — closest to vanilla while having a component model.
- **Fine-grained reactivity**: No virtual DOM diffing. State changes update exactly
  the DOM nodes that depend on them. Matters for a terminal app where layout changes
  are frequent and re-render cost must be minimal.
- **Natural fit**: The existing module-level state patterns (`subscribe`/`notify`)
  are hand-rolled Svelte stores. Migration is renaming, not rewriting.
- **Plugin story**: Svelte components can be compiled independently. A plugin that
  ships a `.svelte` component gets scoped CSS, reactivity, and lifecycle for free.
  No framework boilerplate for plugin authors.
- **HMR**: Svelte's HMR preserves component state without re-evaluating modules.
  The entire class of HMR-related leaks we just fixed doesn't exist.

## What changes

### dockview-core replaces dockview (React wrapper)

`dockview-core` (`npm install dockview-core`) is the vanilla TypeScript engine.
The React wrapper is a thin bridge — dropping it gives us direct API control:

```typescript
const dockview = new DockviewComponent(element, {
  createComponent: ({ name }) => ({
    init(params) {
      // mount terminal into params.containerElement — same imperative code as today
    },
    dispose() {
      // cleanup
    },
  }),
});
```

This is **better** for our use case:
- Terminal mounting is imperative anyway (`terminal.open(container)`)
- Sash linking needs direct access to DOM elements and layout events
- No React reconciliation sitting between us and the layout engine
- `setSize()`, `getBoundingClientRect()`, event subscriptions — synchronous, no
  effect-closure indirection

### State management

Current module-level stores become Svelte stores:

```
screens.ts      → writable<Screen[]>      (currently: manual notify/subscribe + forceUpdate)
settings.ts     → writable<SplittySettings>  (currently: manual notify/subscribe + useSettings hook)
linkedSashes.ts → writable<SashLink[]>     (currently: manual notify/subscribe + Map)
sessions.ts     → Map (keep as-is, not reactive UI state)
```

### Component mapping

| Current (React) | After (Svelte) | Notes |
|-----------------|----------------|-------|
| `App.tsx` (1141 lines) | `App.svelte` (~400 lines) | Most lines are hooks, effects, callbacks |
| `ScreenLayout` (memo + hooks) | `ScreenLayout.svelte` | `onMount`/`onDestroy` for dockview lifecycle |
| `TerminalPanel` (useLayoutEffect) | Imperative in `createComponent` | Not a Svelte component — pure DOM |
| `ScreenSwitcher` | `ScreenSwitcher.svelte` | Cleanest migration, pure UI |
| `SettingsDialog` | `SettingsDialog.svelte` | Cleanest migration, pure UI |
| `TemplateDialog` | `TemplateDialog.svelte` | Cleanest migration, pure UI |
| `useAutoHideTabs` hook | `autoHideTabs.ts` action/module | Imperative DOM, drops the useEffect wrapper |
| `useLinkedSashes` hook | `linkedSashes.ts` module | Direct dockview-core event subscriptions |

### Future components (roadmap)

| Feature | Implementation |
|---------|---------------|
| Command Palette | `CommandPalette.svelte` — fuzzy search, keyboard nav, reactive list |
| Plugin UI | Svelte components compiled independently, mounted into dockview panels |
| Agent Tracking | `AgentPanel.svelte` — reactive status, progress, live updates from WS |
| Notifications | `NotificationBadge.svelte` — reactive badges on tabs/panes |
| Worktree Manager | `WorktreePanel.svelte` — list with branch state, actions |

### What stays the same

Framework-agnostic code migrates 1:1 with zero changes:

- `sessions.ts` — terminal lifecycle, PTY communication, batched writes
- `ws.ts` — WebSocket client
- `theme.ts`, `config.ts` — configuration loading
- `closeGuard.ts` — process detection
- `fit.ts`, `shortcuts.ts` — utilities
- `applyTemplate.ts`, `extractTemplate.ts` — template logic

## Migration path

### Phase 1: Scaffold + dialogs

Set up Vite + Svelte alongside the existing React app. Migrate the isolated UI
components: `SettingsDialog`, `TemplateDialog`, `ConfirmCloseDialog`, `TokenDialog`,
`WindowControls`. These have no dockview dependency — easiest wins, proves the setup.

### Phase 2: Core layout

Replace `DockviewReact` with `dockview-core`. Wire `createComponent` to mount
terminals imperatively (same code as `TerminalPanel`'s `mountTerminal`, minus the
React wrapper). Migrate `ScreenLayout` and `ScreenSwitcher`.

### Phase 3: App shell

Migrate `App.tsx` → `App.svelte`. Convert hook-based features (`useAutoHideTabs`,
`useLinkedSashes`, keyboard shortcuts) to plain modules with dockview-core event
subscriptions. Biggest line-count reduction happens here.

### Phase 4: Cleanup

Remove React, `dockview` (React wrapper), and all React-specific tooling. Update
Vite config. Verify HMR works cleanly with Svelte's built-in HMR.

## Risk assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| dockview-core API gaps vs React wrapper | Low | Same engine, wrapper is thin. Inspect source if needed. |
| Svelte learning curve | Low | Simpler mental model than React hooks. |
| Migration duration | Medium | ~2–3 focused sessions. Most code is framework-agnostic. |
| Regression in edge cases (popouts, DnD) | Medium | Manual QA per phase. |
| Svelte ecosystem (fewer UI libs) | Low | Zero React UI libraries used. All custom. |
| Plugin API design | Medium | Svelte's compile model is an advantage — research ahead of Phase 4. |

## Expected outcome

- **~40–50% fewer lines** in UI layer (App, dialogs, switcher)
- **No dependency arrays** — eliminates stale closure bugs entirely
- **No HMR state leaks** — Svelte HMR preserves state without module re-evaluation
- **Direct dockview-core access** — simpler sash linking, layout constraints
- **Faster runtime** — compiled to direct DOM updates, no virtual DOM
- **Plugin-ready component model** — plugins ship `.svelte` files with scoped CSS,
  reactivity, and lifecycle built in
- **Scales with roadmap** — command palette, agent tracking, worktree UI all benefit
  from reactive templating without framework overhead growing
