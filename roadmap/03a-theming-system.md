# Milestone 3a: Theming System

> **Voraussetzung:** [Milestone 3 — Svelte-Migration](03-svelte-migration.md) (Svelte Scoped Styles + dockview-core stehen)
>
> Kann nicht sinnvoll vor der Svelte-Migration passieren: App.css (1.510 LOC, monolithisch) wird
> während der Migration ohnehin in Scoped Styles aufgelöst. Vorher ein Theming-System bauen
> heißt es danach nochmal umbauen.

## Context

Splitty hat aktuell zwei getrennte Theme-Systeme die sich gegenseitig nicht kennen:

1. **Terminal-Theme** (xterm.js): 16 ANSI-Farben + bg/fg/cursor/selection, geladen via `/api/theme`.
   Default ist One Dark. Optionale "System"-Variante liest aus Ghostty-Config.

2. **UI-Theme** (dockview): CSS-Klassen (`dockview-theme-splitty-modern`, etc.) mit `--dv-*`
   CSS-Variablen. 10 eingebaute Themes (8 aus dockview, 2 custom).

**Probleme:**

- **Dockview-Kopplung**: Drawer, Notifications, Context Menus, Empty State — alles referenziert
  `--dv-*` Variablen direkt, obwohl diese Komponenten keine dockview-Elemente sind. Wenn dockview
  jemals ersetzt wird, muss das gesamte CSS umgeschrieben werden.

- **Inkonsistente Farbdefinition**: ~100+ hardcoded `rgba(255, 255, 255, ...)` Werte im CSS
  statt benannter Variablen. Gleiche Farben werden an dutzenden Stellen wiederholt.

- **Keine Verbindung zwischen Terminal und UI**: Terminal kann dunkel sein während die UI hell ist
  (oder umgekehrt). `--splitty-terminal-bg` ist ein manueller Bridge-Hack um den Hintergrund
  zwischen beiden Systemen zu synchronisieren.

- **Ghostty-Abhängigkeit**: Die "System"-Theme-Option für Terminals liest aus Ghostty-Config.
  Das war anfangs praktisch (Ghostty-Themes lagen schon da), aber koppelt Splitty an einen
  spezifischen Terminal-Emulator.

## Ziel

Ein einheitliches, dockview-agnostisches Theming-System:

```
Splitty Theme
  ├── UI-Tokens (surfaces, borders, text, accents, ...)
  │     └──▶ Übersetzt sich zu dockview --dv-* Variablen (Adapter-Layer)
  ├── Terminal-Tokens (bg, fg, cursor, ANSI palette, ...)
  │     └──▶ Übersetzt sich zu xterm.js ITheme
  └── Semantische Tokens (danger, muted, hover, ...)
        └──▶ Drawer, Notifications, Context Menus etc.
```

### Design-Prinzipien

1. **Splitty-eigene CSS-Variablen** (`--splitty-*`) als Single Source of Truth.
   Kein UI-Element referenziert `--dv-*` direkt — ein Adapter-Modul übersetzt
   `--splitty-*` → `--dv-*` für dockview.

2. **Terminal-Theme = Optional Teil vom App-Theme**.
   Default: "Use Application Theme" — Terminal-Farben werden aus dem App-Theme abgeleitet.
   Custom: User kann weiterhin ein separates Terminal-Theme wählen (eigene ANSI-Palette).
   → Einfacher Schalter in Settings, nicht zwei unabhängige Theme-Picker.

3. **Keine externe Terminal-Emulator-Abhängigkeit** für Themes.
   Ghostty-Theme-Import raus. Eigenes Theme-Format (oder einfach CSS-Variablen).
   Ghostty/Alacritty/iTerm2 Theme-Import als optionaler Converter denkbar, aber nicht als
   Runtime-Dependency.

4. **Benannte Tokens statt hardcoded Werte**.
   `rgba(255, 255, 255, 0.08)` → `var(--splitty-border-subtle)`.
   Macht Themes konsistent und Light-Mode überhaupt erst realistisch.

## Token-Architektur (Entwurf)

### Primitive Tokens (Palette)

```css
--splitty-gray-50:  #fafafa;
--splitty-gray-900: #0f0f0f;
/* ... */
```

### Semantische UI-Tokens

```css
/* Surfaces */
--splitty-surface-base:      var(--splitty-gray-900);
--splitty-surface-raised:    var(--splitty-gray-850);
--splitty-surface-overlay:   var(--splitty-gray-800);

/* Text */
--splitty-text-primary:      var(--splitty-gray-100);
--splitty-text-secondary:    var(--splitty-gray-400);
--splitty-text-muted:        var(--splitty-gray-600);

/* Borders */
--splitty-border-default:    var(--splitty-gray-700);
--splitty-border-subtle:     var(--splitty-gray-800);

/* Interactive */
--splitty-hover-bg:          rgba(255, 255, 255, 0.06);
--splitty-active-bg:         rgba(255, 255, 255, 0.10);

/* Accents */
--splitty-accent:            var(--splitty-blue-400);
--splitty-danger:            var(--splitty-red-400);
```

### Terminal-Tokens

```css
--splitty-term-bg:           var(--splitty-surface-base);
--splitty-term-fg:           var(--splitty-text-primary);
--splitty-term-cursor:       var(--splitty-text-primary);
--splitty-term-selection-bg: var(--splitty-surface-overlay);
/* ANSI colors — eigene Palette oder vom App-Theme abgeleitet */
--splitty-term-black:        ...;
--splitty-term-red:          ...;
/* ... */
```

### Dockview-Adapter

```typescript
// theme-adapter.ts
export function applySplittyToDockview(root: HTMLElement) {
  // Reads --splitty-* tokens, sets --dv-* variables
  root.style.setProperty('--dv-group-view-background-color',
    getComputedStyle(root).getPropertyValue('--splitty-surface-base'));
  // ...
}
```

Oder rein in CSS:
```css
.splitty-theme {
  --dv-group-view-background-color: var(--splitty-surface-base);
  --dv-tabs-and-actions-container-background-color: var(--splitty-surface-raised);
  --dv-separator-border: var(--splitty-border-subtle);
  --dv-activegroup-visiblepanel-tab-color: var(--splitty-text-primary);
  /* ... */
}
```

## Phasen

### Phase 1: Token-Definition + Adapter

- Splitty-eigene CSS Custom Properties definieren (Palette + semantische Tokens)
- Dockview-Adapter schreiben (`--splitty-*` → `--dv-*` Mapping)
- Alle `--dv-*` Referenzen in Nicht-dockview-Komponenten durch `--splitty-*` ersetzen
- Alle hardcoded `rgba(...)` Werte durch benannte Tokens ersetzen
- Ergebnis: visuell identisch, aber Farben kommen jetzt aus Splitty-Tokens

### Phase 2: Terminal-Theme-Unifikation

- "Use Application Theme" als Default-Option für Terminal-Theme
  - Terminal-Farben werden aus `--splitty-term-*` Tokens abgeleitet
  - ITheme-Objekt wird aus CSS-Variablen generiert statt aus `/api/theme`
- Custom Terminal-Themes weiterhin möglich (eigene ANSI-Palette)
- Ghostty "System"-Option entfernen
- Theme-Auswahl in Settings vereinfachen: Ein Theme-Picker, optional separates Terminal-Theme

### Phase 3: Theme-Authoring

- Theme-Format definieren (JSON/TOML mit Palette + Overrides)
- Bundled Themes: Splitty Dark (Modern), Splitty Light, evtl. 2-3 Community-Paletten
- Dockview-native Themes (dark, abyss, dracula, ...) als Legacy erhalten oder in Splitty-Format konvertieren
- User-Themes: Datei in Config-Dir, wird automatisch erkannt

## Scope-Grenze

Nicht Teil dieses Milestones:
- **Plugin-Theme-API** — kommt mit Plugin-System
- **Live-Theme-Editor** — Nice-to-have, aber nicht jetzt
- **OS-Level Dark/Light-Mode Sync** — sinnvoll, aber separates Feature

## Risiken

| Risiko | Schwere | Mitigation |
|--------|---------|------------|
| Dockview-Variablen-Mapping unvollständig | Mittel | Alle `--dv-*` systematisch inventarisieren |
| Visuelle Regression bei Token-Umstellung | Niedrig | Phase 1 ändert keine Farben, nur Indirektion |
| Terminal-Theme-Ableitung sieht schlecht aus | Mittel | Gute ANSI-Palette-Defaults, Custom bleibt Option |
| Light-Mode aufwändiger als gedacht | Niedrig | Nicht Teil von Phase 1-2, kann später kommen |

## Erwartetes Ergebnis

- **Dockview austauschbar**: Kein UI-Element referenziert `--dv-*` direkt
- **Ein Theme statt zwei**: App-Theme bestimmt Terminal-Look (optional überschreibbar)
- **Keine Ghostty-Abhängigkeit** für Themes
- **Konsistente Farben**: Benannte Tokens statt 100+ hardcoded rgba-Werte
- **Light-Mode möglich**: Token-Architektur macht es trivial (nur Palette tauschen)
- **Theme-Authoring**: User können eigene Themes erstellen
