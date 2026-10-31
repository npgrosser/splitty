# Milestone 2: Notification System — DONE

> **Voraussetzung:** Mux-Backend (Milestone 1), Session-Persistence
>
> Baut auf dem bestehenden Title-Tracking und `baseShell`-Detection auf. Legt die Infrastruktur für [Milestone 6 — Terminal Intelligence](../openspec/changes/terminal-intelligence/proposal.md) — das Notification-System ist der Delivery-Kanal für alle dort geplanten Parser-Events und App-State-Changes.

### Implementierungs-Status

**Vollständig implementiert:**
- Protokoll-Erweiterung (`notification` Message-Typ in `docs/protocol.ts`, `frontend/src/lib/protocol.ts`, Go `internal/protocol/messages.go`)
- Backend NotificationEngine (`internal/protocol/notify.go`) mit TitleAnimationTracker und ProcessLifecycleTracker
- Alle 4 Notification-Typen: `process.finished`, `session.exited`, `session.crashed`, `app.idle`
- `userInitiated`-Flag und Frontend-Filterlogik
- Frontend Notification Store (`lib/notifications.ts`) mit subscribe-Pattern, Dedup, read/unread/dismiss
- Drawer-Badges (Severity-Farben, Puls-Animation für `app.idle`, Screen-Level Badge-Count)
- Toast-Notifications (bottom-right, 5s auto-dismiss, click-to-navigate, Stacking)
- Desktop/System-Notifications (document.hidden Check, Permission-Request, Tag-Dedup)
- Settings UI im SettingsDialog (Master-Switch, per-Typ Toggles)
- Navigation (Klick auf Badge/Toast → zum Terminal navigieren)
- Backend Config (`internal/config/config.go` mit `NotificationSettings` + `AppIdleRules`)
- NotificationBuffer (Ring-Buffer pro Screen, max 50 Einträge, 1h TTL, Replay bei `mux.attach`)
- OSC Realtime-Timestamps (`oscScanner.changeAt` → exakte Idle-Detection-Timestamps)
- app_idle Prozessname-Glob-Matching (`AppIdleRules` mit Match-Pattern + pro-Regel Threshold, Default: `claude*` / 1500ms)

**Design-Abweichung (bewusst):**
- `desktopSeverity: string[]` aus dem Milestone wurde durch per-Typ Toggles ersetzt (`processFinished`, `processCrashed`, `appIdle`). Per-Typ ist granularer als Severity-basierte Filterung und macht `desktopSeverity` redundant.

## Motivation

Splitty verwaltet viele Terminals gleichzeitig. Der User hat nicht alle im Blick. Wenn ein Build fertig ist, ein Prozess crasht, oder Claude Code auf Input wartet, muss Splitty das aktiv mitteilen — nicht nur passiv im Drawer-Status anzeigen.

Bisheriger Stand: eine einfache Welcome-Tooltip-Notification (`Notifications.tsx`). Kein echtes Notification-System.

---

## Architektur-Entscheidung: Backend generiert, Frontend managed

### Backend-Verantwortung
- **Detection**: Erkennt notifizierbare Events (Prozess-Exit, Title-Animation → Idle, etc.)
- **Generierung**: Erzeugt Notification-Objekte mit Typ, Severity, Ziel (Session/Screen/Panel)
- **Versand**: Sendet via WebSocket-Protokoll an alle verbundenen Clients
- **Buffer**: Hält einen begrenzten Ring-Buffer pro Screen für Reconnect-Szenarien

### Frontend-Verantwortung
- **Store**: Verwaltet alle empfangenen Notifications (read/unread, dismiss)
- **Rendering**: Drawer-Badges, Toast-Popups, Notification-Center
- **Navigation**: Klick auf Notification → Spring zum Ziel-Terminal
- **Desktop-Notifications**: System-Notification-API wenn Splitty nicht im Fokus
- **UI-Preferences**: Welche Typen anzeigen, wie anzeigen

### Warum nicht Frontend-only?

| | Backend | Frontend |
|---|---|---|
| Milestone 2 Alignment | Notifications = Output der Parser-Pipeline | Würde parallele Interpretation in JS erfordern |
| Stateful Detection | Trivial (OSC-Timestamps, Process-State) | Timer + Visibility-API-Quirks |
| Multi-Client | Eine Engine → alle Clients | Jeder Client detektiert separat |
| Config | Prozessname + Threshold neben Prozess-Info | Müsste Mapping separat pflegen |

Das Backend sendet weiterhin `title`-Events für Tab-Labels und Drawer-Darstellung. Notifications sind ein **zusätzlicher Kanal** — keine Ersetzung.

---

## Notification-Typen

### Phase 1 (dieses Milestone)

| Kind | Trigger | Severity | Beispiel |
|---|---|---|---|
| `process.finished` | Foreground-Prozess kehrt zur Shell zurück (`baseShell` false→true) | `info` | "make build finished" |
| `session.exited` | PTY-Session beendet, Exit-Code 0 | `info` | "npm run dev exited" |
| `session.crashed` | PTY-Session beendet, Exit-Code ≠ 0 | `error` | "npm test failed (exit 1)" |
| `app.idle` | Title-Icon war animiert, ist jetzt statisch | `warning` | "Claude Code is waiting for input" |

### Phase 2 (mit Milestone 6 — Terminal Intelligence)

| Kind | Trigger | Severity | Beispiel |
|---|---|---|---|
| `app.state` | App-Parser erkennt Zustandswechsel | variabel | "Claude Code: tool approval needed" |
| `command.finished` | OSC 133;D mit Exit-Code (Shell-Integration) | `info`/`error` | "make build failed (exit 2)" |

### Filter-Logik & `userInitiated`-Flag

Alle Session-Close-Notifications tragen ein `userInitiated`-Flag:

- **`userInitiated: true`**: Client hat `close`-Message geschickt (Pane geschlossen) oder User hat `exit` getippt (Prozess beendet sich während der Pane aktiv ist und Input vom Client kam)
- **`userInitiated: false`**: Prozess hat sich von selbst beendet ohne Client-Aktion

Das Frontend entscheidet basierend auf dem Flag:
- `userInitiated: true` → Default: keine Toast/Desktop-Notification (User weiß was er getan hat), aber Badge im Drawer wenn auf anderem Screen
- `userInitiated: false` → Immer notifizieren (Toast + Badge + Desktop wenn nicht im Fokus)

Für `process.finished` (baseShell false→true): Immer notifizieren — der Foreground-Prozess ist zurück zur Shell, egal ob user-initiated oder nicht. Der User will wissen dass `make build` fertig ist.

---

## Detection-Mechanismen

### 1. Session Lifecycle (process.finished / session.exited / session.crashed)

**Bereits vorhanden im Backend:**
- `session.exitCode` wird in `pty/session.go:103-114` erfasst
- `closed`-Message mit `exitCode` wird in `dispatcher.go:222` gesendet
- `baseShell`-Flag wird bei jedem Title-Poll ermittelt (`session.go:187-205`)

**Neu:**
- `baseShell`-Transitionen tracken: wenn `false→true`, Notification `process.finished` mit dem vorherigen Prozessnamen
- Bei `closed`-Event: Notification basierend auf Exit-Code und ob Session ein explizites Command hatte

### 2. Title-Icon-Animation → Idle (app.idle)

**Beobachtung:** Moderne CLI-Tools (Claude Code, Aider, etc.) nutzen animierte Unicode-Icons im Terminal-Titel als Spinner. Beispiel:

```
⠋ Claude Code → ⠙ Claude Code → ⠹ Claude Code → ... → ✳ Claude Code (fertig)
```

Das Drawer-Frontend extrahiert dieses Icon bereits (`splitTitleIcon()` in `Drawer.tsx:24`).

**Detection-Ansatz: OSC-Title-Timestamps (Realtime)**

Der `oscScanner` (`pty/osc.go`) erkennt OSC 0/2 Title-Changes bereits in Echtzeit aus dem PTY-Stream. Statt auf den 1s-Title-Poll zu warten, tracken wir den exakten Zeitpunkt jeder OSC-Title-Änderung:

```
oscScanner erkennt neuen Title
    │
    ├── Leading-Icon extrahieren
    │
    ├── Icon anders als vorher?
    │       → ja: lastIconChange = now(), wasAnimating = true, notified = false
    │       → nein: (nichts, Timer läuft weiter)
    │
    └── Periodischer Check (z.B. auf dem bestehenden 1s-Tick von watchTitle):
            if wasAnimating && !notified && now() - lastIconChange >= threshold:
                → Notification "app.idle" feuern
                → notified = true (kein Re-Fire bis nächster Animations-Zyklus)
```

**Warum Timestamps statt Poll-Counting:**
- OSC-Changes werden exakt getimed, nicht auf 1s-Intervalle quantisiert
- Threshold-Genauigkeit: 1200ms heißt wirklich 1200ms, nicht "2 Polls à 1s"
- Robuster: auch wenn der 1s-Poll mal verzögert wird, stimmt der Timestamp

**Konfiguration — pro Prozessname:**

```toml
# ~/.config/splitty/config.toml

[notifications]
enabled = true

[notifications.process]
finished = true    # baseShell false→true
exited = true      # session clean exit (nur bei explizitem command)
crashed = true     # session non-zero exit

[[notifications.app_idle]]
match = "claude*"
threshold_ms = 1500

# Weitere können vom User ergänzt werden:
# [[notifications.app_idle]]
# match = "aider*"
# threshold_ms = 2500
```

Die `claude*`-Regel ist **built-in default** — auch ohne User-Config aktiv. Claude Code ist der primäre Use-Case und wird aktiv mit Splitty demonstriert. Weitere Rules (aider, etc.) kann der User in seiner Config ergänzen.

**Match-Logik:** Glob auf den Prozessnamen (`session.Title()` liefert `proc`). Wenn der Foreground-Prozess matcht UND einen OSC-Title mit Leading-Icon hat → Tracking aktivieren.

---

## Protokoll-Erweiterung

### Neuer Message-Typ: `notification` (Server → Client)

Die Types unten landen bei Implementierung in `docs/protocol.ts` — das ist die offizielle Protokoll-Spezifikation. TypeScript übernimmt die Types 1:1. Go implementiert sie so sauber wie es das Typsystem hergibt — fehlende Type-Features (kein Discriminated Union, kein Literal-Narrowing) ändern nichts am Protokoll. Das Wire-Format ist durch die TS-Spec definiert, nicht durch die Go-Implementierung.

```typescript
// ── docs/protocol.ts — Section: Notifications ──

// --- Notification Messages (new in Milestone 2) ---

type NotificationSeverity = "info" | "success" | "warning" | "error";

/** Targeting: which terminal a notification refers to. */
interface TerminalTarget {
  sessionId: string;
  screenId: string;
  panelId: string;
}

/**
 * Base shape of every notification.
 *
 * `kind`, `severity`, `target`, and `payload` are declared here so the
 * full structure is visible in one place. Concrete kinds narrow each of
 * them to exact types — when you `extends NotificationBase` the compiler
 * forces you to specify all four.
 */
interface NotificationBase {
  type: "notification";
  kind: string;
  id: string;
  severity: NotificationSeverity;
  title: string;
  body?: string;
  timestamp: number;
  target: TerminalTarget;
  payload: Record<string, unknown>;
}

// Future: global notifications (e.g. "server shutting down") may use a
// different target type. At that point, NotificationBase.target becomes
// a union (TerminalTarget | ScreenTarget | null) or a generic.

/** Foreground process returned to shell (baseShell false→true). */
interface NotifyProcessFinished extends NotificationBase {
  kind: "process.finished";
  severity: "info";
  payload: {
    process: string;
    cmdline?: string;
  };
}

/** PTY session ended cleanly (exit code 0). */
interface NotifySessionExited extends NotificationBase {
  kind: "session.exited";
  severity: "info";
  payload: {
    exitCode: 0;
    process?: string;
    command?: string;
    userInitiated: boolean;
  };
}

/** PTY session ended with non-zero exit code. */
interface NotifySessionCrashed extends NotificationBase {
  kind: "session.crashed";
  severity: "error";
  payload: {
    exitCode: number;
    process?: string;
    command?: string;
    userInitiated: boolean;
  };
}

/** Title icon stopped animating — app is waiting for input. */
interface NotifyAppIdle extends NotificationBase {
  kind: "app.idle";
  severity: "warning";
  payload: {
    process: string;
    icon: string;
    appName: string;
  };
}

/** Discriminated union — narrow on `kind`. */
type NotificationMessage =
  | NotifyProcessFinished
  | NotifySessionExited
  | NotifySessionCrashed
  | NotifyAppIdle;
```

**Hinweise:**
- `session.exited` hat `exitCode: 0` als Literal — der Typ allein unterscheidet clean exit von crash.
- Zukünftige globale Notifications (z.B. "server shutting down") brauchen einen anderen `target`-Typ — dann wird `NotificationBase.target` zur Union oder zum Generic.

**Kein Client→Server nötig (Phase 1).** Read/Unread/Dismiss ist rein client-seitig. Falls später Cross-Device-Sync gewünscht, kann `notification.dismiss` ergänzt werden.

### Auf `mux.attach`: Pending Notifications senden

Bei Reconnect/Attach sendet der Server alle gebufferten Notifications für den Screen. Client dedupliziert per `id`.

---

## Backend-Implementierung

### Notification Engine

Neue Komponente die an der Session-Lifecycle partizipiert:

```
Session Events (exit, baseShell change)  ──┐
                                            ├──→ NotificationEngine ──→ Broadcast
OSC Title Changes (realtime timestamps)  ──┘
```

**Bestandteile:**

1. **`TitleAnimationTracker`** (pro Session)
   - Empfängt OSC-Title-Changes vom Scanner
   - Extrahiert Leading-Icon, trackt Timestamps
   - Matcht Prozessname gegen `app_idle`-Config
   - Wird vom `watchTitle`-Ticker periodisch geprüft

2. **`ProcessLifecycleTracker`** (pro Session)
   - Trackt `baseShell`-Transitionen
   - Merkt sich letzten Foreground-Prozessnamen
   - Generiert `process.finished` bei false→true Transition

3. **`NotificationBuffer`** (pro Screen)
   - Ring-Buffer, z.B. 50 Einträge, max 1h TTL
   - Bei `mux.attach` → pending Notifications senden
   - Einträge verfallen automatisch

### Integration in bestehenden Code

- **`osc.go`**: `oscScanner` bekommt Callback/Channel für Title-Changes mit Timestamp
- **`dispatcher.go`**: `watchTitle` prüft zusätzlich den `TitleAnimationTracker`
- **`dispatcher.go`**: Session-Close-Handler generiert Lifecycle-Notifications
- **Mux-Layer**: `NotificationBuffer` pro Screen, Flush bei Attach

---

## Frontend-Implementierung

### 1. Notification Store

```typescript
// lib/notifications.ts
interface Notification {
  id: string;
  kind: string;
  severity: "info" | "success" | "warning" | "error";
  title: string;
  body?: string;
  sessionId?: string;
  screenId?: string;
  panelId?: string;
  timestamp: number;
  read: boolean;       // client-side state
  dismissed: boolean;   // client-side state
}
```

- In-Memory Store mit `subscribe()`-Pattern (wie Settings)
- Empfängt `notification`-Messages vom WebSocket
- Dedupliziert per `id` (für Reconnect-Szenarien)
- Bietet Queries: `getByScreen(id)`, `getBySession(id)`, `getUnreadCount()`

### 2. Drawer-Badges

**Das wichtigste UI-Element.** Direkt neben dem Pane-Titel im Drawer.

Aktuell sieht ein Pane-Eintrag im Drawer so aus:
```
[icon/dot] Claude Code
```

Mit Notification-Badge:
```
[icon/dot] Claude Code                 [●]
```

**Implementierung:**
- `DrawerPaneInfo` bekommt kein neues Feld — der Drawer fragt direkt den Notification Store
- Badge-Farbe nach Severity: gelb (warning/idle), rot (error/crash), blau (info)
- Badge verschwindet wenn der User den Pane aktiviert (navigiert dorthin)
- Screen-Level: aggregierter Badge-Count neben dem Screen-Namen

**CSS-Konzept:**
```css
.drawer-pane-badge {
  /* Kleiner Punkt, rechts im Pane-Eintrag */
  width: 6px;
  height: 6px;
  border-radius: 50%;
  margin-left: auto;  /* pushed nach rechts via flexbox */
}
.drawer-pane-badge.warning { background: var(--notification-warning); }
.drawer-pane-badge.error   { background: var(--notification-error); }
.drawer-pane-badge.info    { background: var(--notification-info); }
```

Für `app.idle` speziell: subtile Puls-Animation auf dem Badge, damit es auffällt ohne zu nerven.

### 3. Toast-Notifications

Kurze Popup-Nachricht für neue Notifications. Erweitert die bestehende `Notifications.tsx`-Infrastruktur.

- Position, Stacking, Auto-Dismiss, Action-Button ("Go to terminal") — UX-Design-Details werden beim Implementieren finalisiert
- Grundprinzip: nicht-invasiv, klickbar, auto-dismiss nach ~5s

### 4. Desktop/System-Notifications

Wenn das Splitty-Fenster nicht im Fokus ist:

```typescript
if (document.hidden && Notification.permission === "granted") {
  new Notification(notification.title, {
    body: notification.body,
    tag: notification.id,  // verhindert Duplikate
  });
}
```

- Permission-Request beim ersten Mal (oder in Settings aktivierbar)
- Klick auf System-Notification → Splitty-Fenster fokussieren + zum Ziel navigieren
- Nur für bestimmte Severities (konfigurierbar, default: warning + error)

### 5. Navigation

Klick auf Badge, Toast, oder System-Notification:
1. Screen aktivieren (`setActiveScreen`)
2. Panel aktivieren (`onActivatePane`)
3. Notification als `read` markieren

Die Infrastruktur dafür existiert bereits — `onActivatePane` im Drawer macht genau das.

### 6. Settings-Erweiterung

```typescript
// settings.ts
export interface NotificationSettings {
  enabled: boolean;             // Master-Switch
  toasts: boolean;              // Toast-Popups zeigen
  desktop: boolean;             // System-Notifications
  desktopSeverity: string[];    // ["warning", "error"]
  processFinished: boolean;     // Notify when foreground process returns to shell
  processExited: boolean;       // Notify on session clean exit
  processCrashed: boolean;      // Notify on session crash
  appIdle: boolean;             // Notify on title-icon-idle detection
}
```

Im SettingsDialog unter eigenem Abschnitt "Notifications".

---

## Persistenz: Bounded Buffer mit TTL

**Kein vollständiges Message-Queue-System.** Stattdessen:

- Backend hält pro Screen einen Ring-Buffer (50 Einträge, 1h TTL)
- Bei `mux.attach`: alle noch gültigen Einträge senden
- Client dedupliziert per Notification-`id`
- Einträge verfallen server-seitig nach TTL — kein Cleanup durch Client nötig

**Warum das reicht:**
- User schließt Laptop, kommt nach 30min zurück → sieht "Claude Code finished" von vor 20min
- User refresht Browser → Notifications bleiben da (solange < 1h)
- Kein Overhead eines echten Persistence-Layers
- Aligns mit Session-Persistence: solange die Session lebt, leben die Notifications

**Bewusst nicht:** Disk-Persistenz, Cross-Session-History, Notification-Log. Notifications sind ephemeral. Wer wissen will was passiert ist, schaut in den Terminal-Output.

---

## Datenfluss-Übersicht

```
PTY Output Stream
    │
    ├──→ oscScanner ──→ Title Change + Timestamp
    │                         │
    │                         └──→ TitleAnimationTracker
    │                                   │
    │                                   └──→ "app.idle" Notification
    │
    └──→ Process Exit (cmd.Wait)
              │
              └──→ ProcessLifecycleTracker
                        │
                        ├──→ "session.exited" / "session.crashed"
                        └──→ (via baseShell transition) "process.finished"


NotificationEngine
    │
    ├──→ NotificationBuffer (pro Screen, Ring-Buffer + TTL)
    │
    └──→ WebSocket Broadcast ──→ Client(s)
                                      │
                                      ├──→ Notification Store
                                      │         │
                                      │         ├──→ Drawer Badges
                                      │         ├──→ Toast Popups
                                      │         └──→ Desktop Notifications
                                      │
                                      └──→ User klickt → Navigation zum Terminal
```

---

## Implementierungs-Reihenfolge

1. **Protokoll + Store**: `notification` Message-Typ, Frontend Notification Store
2. **Session Lifecycle Notifications**: `session.exited`, `session.crashed` (einfachster Fall — Exit-Event existiert bereits)
3. **Drawer Badges**: Badge-Rendering, Click-to-Navigate, Mark-as-Read
4. **Process Finished**: `baseShell` false→true Tracking, `process.finished` Notification
5. **Toast Component**: Popup-UI für neue Notifications
6. **App Idle Detection**: OSC-Title-Timestamp-Tracking, `app.idle` Notification
7. **Desktop Notifications**: System Notification API Integration
8. **Settings UI**: Notification-Preferences im SettingsDialog
9. **Persistenz-Buffer**: Ring-Buffer pro Screen, Replay bei Attach

---

## Offene Punkte

- **Toast UX-Design**: Position, Animation, Stacking — wird beim Implementieren finalisiert
- **Notification-Center**: Ob und wo ein vollständiges Panel für Notification-History sinnvoll ist — kann nachträglich ergänzt werden, Drawer-Badges sind Prio
- **Sound**: Bewusst ausgeklammert, als Future-Feature vorgesehen
- **`app_idle` Default-Config**: Welche Prozesse sind default-aktiviert? Mindestens `claude*`
- **Rate-Limiting**: Verhindern dass ein wild flackernder Titel Notification-Spam erzeugt (z.B. max 1 `app.idle` pro Session pro 30s)
- **Notification-Grouping**: Wenn 5 Terminals gleichzeitig fertig werden — einzeln oder gruppiert?
