# Milestone 1: Multiplexer Backend — Server als Source of Truth

## Status: ✅ Done

### Backend (45 Tests)
- ✅ Schritt 1: Datenmodell (`internal/mux/model.go`)
- ✅ Schritt 2: Reducer + Ops (`ops.go`, `reducer.go`, `validate.go`)
- ✅ Schritt 3: Effect-System (`effects.go`, `runtime.go`)
- ✅ Schritt 4: WS-Protokoll (`mux_handler.go`, `mux_messages.go`, HTTP API)
- ✅ Schritt 6: Ownership-Modell (client-owned/server-owned, Disconnect-Cleanup)
- ✅ 2D Grid-Projektion + Spatial Neighbor Queries (`grid.go`)

### Frontend-Integration
- ✅ Schritt 5: MuxSync — Shell.tsx verdrahtet: Split, Close, Tab, Move, ActivePanel, Webview senden Ops. `bridgeDockviewToMux` hooked dockview-Events. Snapshot→Dockview-Rebuild via `applySnapshot.ts`.
- ✅ Schritt 7: Attach/Detach — clientId-Tracking, lastClientId, Single-Attach, busy-Reject. Auto-Attach bei Welcome. Snapshot-Handler + Nudge→Resync.
- ✅ Schritt 8: CLI — split, close, focus, list-panes, list-screens, new-screen, switch-screen. Env-Vars auf allen Sessions.
- ✅ Schritt 9: Screens — screens.ts + Shell.tsx senden mux-Ops bei create/close/rename/switch.

### Architektur-Details
- **Shadow Mode (Phase A):** Dockview bleibt Source of Truth, Server trackt parallel. Client sendet Ops fire-and-forget. Kein Sync-Problem, weil Single-Attach.
- **Client-Set IDs:** Client erzeugt Panel-/Screen-IDs (dockview) und sendet sie in mux-Ops (`newPanelId`, `screenId`). Server übernimmt sie direkt. CLI-Ops ohne IDs → Server generiert.
- **CLI-Path:** Layout-Commands (split, close, focus) laufen über `POST /api/command` → ClientCommand → WS-Client → Client führt aus → mirrort als mux.op. Workspace-Commands (new-screen, switch-screen) laufen über `POST /api/mux/op`.
- **2D-Grid-Projektion:** `grid.go` projiziert den Layout-Baum auf relative 2D-Koordinaten. `splitty focus` nutzt spatial neighbor queries via `GET /api/mux/neighbor`.
- **PTY-Lifecycle:** Läuft über alten Protocol-Path (`create`/`close`). Client-initiierte mux-Ops skippen SpawnSessionEffect (keine doppelten PTYs). CLI-Ops führen Effects aus.
- **Connection.Close:** MuxManaged-Flag — wenn mux aktiv, killt Close() nur Streams, nicht PTYs. Ownership-basiertes Cleanup im MuxHandler.
- **Resize-Ops:** Noch nicht verdrahtet (dockview exponiert keine Split-Node-IDs). Sizes werden beim nächsten Snapshot korrigiert.

---

## Warum dieser Milestone

Aktuell lebt der gesamte Layout-State im Frontend (dockview + zustand). Der Server kennt nur PTY-Sessions — er weiß nicht, welche Panes existieren, wie sie angeordnet sind, oder welcher Pane aktiv ist. Das bedeutet:

- **Kein Attach/Detach.** Client disconnected → Sessions sterben. Kein Reconnect möglich.
- **Keine CLI-Steuerung.** `splitty split` kann nicht existieren, weil der Server das Layout nicht kennt.
- **Kein Ownership-Modell.** Keine Unterscheidung zwischen ephemeral und persistent Sessions.

All diese Features — Attach/Detach, CLI, Session Ownership — setzen dasselbe voraus: **der Server muss die Source of Truth für Layout und Sessions werden.**

Dieser Milestone implementiert das Multiplexer-Backend aus `MULTIPLEXER_BACKEND_PLAN.md` Phase 1+2 in Go.

**Kanonische Referenzen:**
- [`docs/protocol.ts`](../docs/protocol.ts) — Protokoll-Spezifikation: Domain-Modell, alle Message-Types, Operations, Connection-Flows
- [`MULTIPLEXER_BACKEND_PLAN.md`](../MULTIPLEXER_BACKEND_PLAN.md) — Architektur-Prinzipien, Client-Autonomie, Reducer-Design

---

## Scope

### In Scope

- Backend-seitiges Multiplexer-Datenmodell (Screens, Splits, Slots, Panes, Sessions)
- Reducer mit pure State-Transitions + Effect-Schicht
- Operations-basierte Synchronisation über WebSocket (fire-and-forget, kein Realtime-Broadcast)
- Screen Ownership Model (client-owned vs server-owned Screens)
- **Single-Attach:** Nur ein Client pro Screen — kein Multi-Client-Sync nötig
- Attach/Detach (Sessions überleben Client-Disconnect bei server-owned Screens)
- CLI-Befehle für grundlegende Layout-Operationen
- Environment-Variablen für Pane-Kontext ($SPLITTY_PANE_ID etc.)

### Out of Scope (kommt danach)

- Multi-Attach / Multi-Client-Sync (siehe [Milestone 101](../openspec/changes/multi-attach/proposal.md))
- Rust-Rewrite (Phase 3 — erst wenn Protokoll stabil)
- Snapshot-Persistenz auf Disk (Layout nach Server-Restart wiederherstellen = Daemon-Feature)
- Edge-Constraints
- Multi-Window / Multi-Server
- Op-Log / Event-Sourcing

---

## Architektur-Überblick

```
                    ┌──────────────────────────────────┐
                    │         Frontend (React)          │
                    │                                   │
                    │  dockview ←→ MuxSync-Layer         │
                    │     ↑              ↓               │
                    │  Render     OperationEnvelope      │
                    │     ↑              ↓               │
                    └─────┼──────────────┼──────────────┘
                          │              │
                          │   WebSocket  │
                          │              ▼
                    ┌─────┴──────────────────────────────┐
                    │          Go Server                  │
                    │                                     │
                    │  Dispatcher → Reducer → Effects     │
                    │       ↓          ↓          ↓       │
                    │  Validate    ScreenState   PTY Mgr  │
                    │                  ↓                   │
                    │           Ownership Model            │
                    │    (client-owned / server-owned)     │
                    └─────────────────────────────────────┘
```

### Neues Modul: `internal/mux/`

```
internal/mux/
  model.go         // ScreenState, LayoutNode, PaneState, SessionState, Ownership
  ops.go           // Operation-Types (SplitPanelOp, ClosePanelOp, ...)
  reducer.go       // Pure state transitions: (State, Op) → (State, Effects)
  validate.go      // Op-Validation (Panel existiert? Sizes summe = 1.0?)
  effects.go       // Effect-Definitionen (SpawnSession, KillSession, ...)
  runtime.go       // Effect-Executor (PTY erzeugen, WS broadcasten)
  store.go         // In-Memory ScreenState Store + Revision-Tracking
  events.go        // WS-Event-Payloads (screen_snapshot, screen_op_applied, ...)
```

---

## Implementierungs-Schritte

### Schritt 1: Datenmodell definieren

**Ziel:** Go-Structs für das Multiplexer-Modell.

**Dateien:** `internal/mux/model.go`

Das Datenmodell folgt dem Design aus `MULTIPLEXER_BACKEND_PLAN.md`:

```go
type ScreenState struct {
    ID           string                  `json:"id"`
    Revision     int64                   `json:"revision"`
    Root         LayoutNode              `json:"root"`
    Floating     []FloatingEntry         `json:"floating"`
    Panes        map[string]*PaneState   `json:"panes"`
    Sessions     map[string]*SessionState `json:"sessions"`
    ActivePaneID string                  `json:"activePaneId,omitempty"`
    Name         string                  `json:"name,omitempty"`
    Ownership    OwnershipMode           `json:"ownership"`
    LastClientID string                  `json:"lastClientId,omitempty"` // Server-generiert, für Reconnect-Logik
}

type OwnershipMode string
const (
    OwnershipClient OwnershipMode = "client-owned"
    OwnershipServer OwnershipMode = "server-owned"
)

type LayoutNode struct {
    Type         string       `json:"type"` // "split" | "slot"
    ID           string       `json:"id"`
    // Split-Felder
    Axis         string       `json:"axis,omitempty"` // "horizontal" | "vertical"
    Children     []LayoutNode `json:"children,omitempty"`
    Sizes        []float64    `json:"sizes,omitempty"`
    // Slot-Felder
    PaneIDs      []string     `json:"paneIds,omitempty"`
    ActivePaneID string       `json:"activePaneId,omitempty"`
}
```

**Abnahmekriterien:**
- Structs können einen kompletten Screen-State abbilden (Split-Baum, Floating, Panes, Sessions)
- Ownership-Modus ist Teil des ScreenState
- JSON-Serialisierung funktioniert sauber
- Unit-Tests für Serialisierung/Deserialisierung

---

### Schritt 2: Operations und Reducer

**Ziel:** Pure State-Transitions ohne Side-Effects.

**Dateien:** `internal/mux/ops.go`, `internal/mux/reducer.go`, `internal/mux/validate.go`

Operations (Mindest-Set für MVP):

| Operation | Beschreibung |
|-----------|-------------|
| `split_panel` | Pane splitten (H/V), neues Pane erzeugen |
| `close_panel` | Pane schließen, Layout-Baum aufräumen |
| `resize_split` | Neue Sizes für einen Split-Node setzen |
| `set_active_panel` | Aktives Pane ändern |
| `create_screen` | Neuen Screen erzeugen |
| `close_screen` | Screen schließen |
| `move_panel` | Pane in anderen Slot/Split verschieben |

Jede Operation hat:
- `OperationEnvelope` mit `opId`, `clientId`, `screenId`, `baseRevision`
- Optionale `resulting_sizes` / Layout-Werte (Client-Autonomie)
- Server-Default-Policy wenn Werte fehlen (CLI-Szenario)

```go
type ReduceResult struct {
    State   ScreenState
    Effects []Effect
}

func Reduce(state ScreenState, op Operation) (ReduceResult, error) {
    // 1. Validate
    // 2. Apply (pure state transition)
    // 3. Return new state + effects (z.B. SpawnSession)
}
```

**Abnahmekriterien:**
- Reducer ist pure Function (kein I/O, kein PTY, kein WS)
- Jede Op hat Validation (ungültige Panel-ID → Error)
- Revision wird bei jeder erfolgreichen Op inkrementiert
- Umfangreiche Unit-Tests für jeden Op-Typ
- Default-Policies für Ops ohne Client-Werte (z.B. close_panel: Nachbar expandiert gleichmäßig)

---

### Schritt 3: Effect-System und Runtime

**Ziel:** Side-Effects sauber vom Reducer trennen.

**Dateien:** `internal/mux/effects.go`, `internal/mux/runtime.go`

Effect-Typen:

| Effect | Beschreibung |
|--------|-------------|
| `SpawnSession` | Neue PTY-Session starten |
| `KillSession` | PTY-Session beenden |

```go
type EffectRunner struct {
    ptyManager  *pty.Manager
    wsHub       *ws.Hub
}

func (r *EffectRunner) Execute(effects []Effect) error {
    for _, e := range effects {
        switch e := e.(type) {
        case SpawnSession:
            r.ptyManager.Create(e.SessionID, e.Command, e.Cwd)
        case BroadcastState:
            r.wsHub.Broadcast(e.Payload)
        // ...
        }
    }
}
```

**Abnahmekriterien:**
- Effects sind reine Daten-Structs (kein I/O in der Definition)
- EffectRunner führt sie sequenziell aus
- Fehler in einem Effect brechen nicht die ganze Kette ab (graceful degradation)

---

### Schritt 4: WebSocket-Protokoll erweitern

**Ziel:** Neue Message-Types für Layout-Synchronisation.

**Spec:** [`docs/protocol.ts`](../docs/protocol.ts) definiert alle Message-Types, Payloads und Connection-Flows.

**Dateien:** `internal/protocol/messages.go`, `internal/ws/handler.go`, Frontend `lib/protocol.ts`, `lib/ws.ts`

Neue Client→Server Messages:

| Message | Payload | Wann |
|---------|---------|------|
| `mux.connect` | `{ clientId?: string }` | Bei Connect/Reconnect — `clientId` aus localStorage, oder leer bei erstem Besuch |
| `mux.attach` | `{ screenId }` | Client will sich an einen Screen attachen |
| `mux.detach` | `{ screenId }` | Client detached von einem Screen |
| `mux.op` | `{ screenId, op }` | Nach jeder Frontend-Interaktion (fire-and-forget) |
| `mux.resync` | `{ screenId }` | Client will neuen Snapshot (z.B. nach CLI-Änderung) |

Neue Server→Client Messages:

| Message | Payload | Wann |
|---------|---------|------|
| `mux.welcome` | `{ clientId, screens: ScreenInfo[] }` | Antwort auf `mux.connect` — Liste aller Screens mit Attach-Status und `lastClientId` |
| `mux.screen_snapshot` | `{ screen: ScreenState }` | Nach `mux.attach` (wenn Client nicht der letzte war) oder nach `mux.resync` |
| `mux.nudge` | `{ reason, screenId }` | CLI hat Layout geändert → Client soll Resync machen |
| `mux.busy` | `{ screenId }` | Anderer Client ist schon an diesem Screen attached (Single-Attach) |

**Attach-Granularität:** Attach passiert **pro Screen**, nicht pro Workspace. Zwei Clients können gleichzeitig an verschiedenen Screens arbeiten. Nur ein Client pro Screen. Details und Flows in [`docs/protocol.ts`](../docs/protocol.ts).

**Warum so minimal:** Bei Single-Attach gibt es keinen zweiten Client am selben Screen, an den gebroadcastet werden muss. Der Client sendet Ops fire-and-forget an den Server. `mux.screen_snapshot` wird nur gebraucht wenn der Client seinen State nicht vertrauen kann (Erstbesuch, anderer Client war dazwischen, CLI-Änderung).

**Abnahmekriterien:**
- Bestehende PTY-Messages (`session_output`, `session_input`, `resize`) bleiben unverändert
- Neue Messages laufen über denselben WebSocket
- Codec unterstützt alle neuen Message-Types

---

### Schritt 5: Frontend MuxSync-Layer

**Ziel:** Frontend sendet Layout-Änderungen an den Server (fire-and-forget) und kann bei Attach den State vom Server laden.

**Dateien:** Neue Datei `frontend/src/lib/muxSync.ts`, Anpassungen in `Shell.tsx`, `TerminalPanel.tsx`

Der MuxSync-Layer ist bewusst simpel gehalten, weil **Single-Attach** gilt — es gibt keinen zweiten Client, der live synchronisiert werden muss.

**Zwei Flows:**

```
Flow 1: Normalbetrieb (Client arbeitet)
    dockview Event (Split, Close, Resize, Move, Tab-Wechsel)
        ↓
    MuxSync: Op erzeugen + an Server senden (fire-and-forget)
        ↓
    Server: Validate → Reduce → State aktualisieren (kein Response)

Flow 2: Attach (Client verbindet sich zu bestehendem Screen)
    Client connected
        ↓
    Server: sendet screen_snapshot
        ↓
    MuxSync: dockview aus Snapshot aufbauen
        ↓
    Weiter mit Flow 1
```

**Warum kein Bidirektional-Sync:** Bei Single-Attach gibt es keinen zweiten Client. Der Server muss dem Client keine Ops zurücksenden. Kein Echo-Handling, kein Conflict-Resolution, keine Revision-Checks. Der Client ist die einzige Quelle für Layout-Änderungen (außer CLI — siehe Schritt 8).

**CLI-Ops (Schritt 8) sind der einzige Fall**, in dem der Server eine Layout-Änderung an den Client pushen muss. Das passiert über eine einfache `screen_update`-Message, die den Client zum Resync auffordert. Kein inkrementelles Op-Replay nötig — der Client holt sich einfach einen neuen Snapshot.

**Migrations-Strategie:**

1. **Phase A — Shadow-Modus:** Server trackt Layout mit, Frontend arbeitet wie bisher. Ops werden gesendet aber nicht konsumiert. Dient der Validierung, dass das Modell korrekt bleibt.

2. **Phase B — Attach funktioniert:** Bei Connect prüft der Client ob server-owned Screens existieren. Wenn ja: Layout aus `screen_snapshot` aufbauen statt leeren State. Wenn nein: normaler Start wie bisher.

**Abnahmekriterien:**
- Alle dockview-Interaktionen (Split, Close, Resize, Move, Tab-Wechsel) erzeugen Ops
- Ops werden fire-and-forget an den Server gesendet
- Attach zu bestehendem server-owned Screen baut Layout korrekt aus Snapshot auf
- CLI-initiierte Änderungen triggern Resync im Client

---

### Schritt 6: Screen Ownership Model

**Ziel:** Zwei Ownership-Modi bestimmen, ob ein Screen (und seine Sessions) einen Client-Disconnect überlebt.

**Dateien:** `internal/mux/model.go`, `internal/protocol/dispatcher.go`, `internal/mux/store.go`

**Ownership-Modi:**

| Modus | Default in | Verhalten bei Client-Disconnect |
|-------|-----------|--------------------------------|
| `client-owned` | Desktop-App | PTYs werden beendet (heutiges Verhalten via `Connection.Close()`) |
| `server-owned` | `splitty serve` / Remote | PTYs laufen weiter, Scrollback bleibt im RAM |

**Verhalten:**
- Ownership wird pro Screen gesetzt (nicht pro Session) — ein Screen ist entweder komplett ephemeral oder komplett persistent
- Desktop-App: User kann einzelne Screens als "persistent" markieren → werden `server-owned`
- `Connection.Close()` in `internal/protocol/dispatcher.go` prüft Ownership bevor Sessions gekillt werden
- Nur Sessions in `client-owned` Screens werden bei Disconnect beendet
- `server-owned` Screens + Sessions bleiben bestehen, können von neuem Client übernommen werden

**Neues Op-Set:**

| Operation | Beschreibung |
|-----------|-------------|
| `set_screen_ownership` | Ownership-Modus eines Screens ändern |

**Abnahmekriterien:**
- `client-owned` Screens verhalten sich wie heute (PTYs sterben bei Disconnect)
- `server-owned` Screens: Sessions überleben Client-Disconnect
- Ownership ist per Screen konfigurierbar
- Desktop-App defaultet zu `client-owned`, `splitty serve` defaultet zu `server-owned`
- Unit-Tests für beide Ownership-Modi

---

### Schritt 7: Attach/Detach (Single-Attach)

**Ziel:** Ein Client kann sich zu bestehenden server-owned Screens verbinden und trennen.

**Dateien:** `internal/protocol/dispatcher.go`, `internal/mux/events.go`, `internal/mux/store.go`, Frontend MuxSync-Layer

**Bereits teilweise implementiert:** Der Dispatcher hat `detach`/`attach` Messages (siehe `internal/protocol/messages.go`: `MsgAttach`, `MsgDetach`, `MsgAttached`). Sessions überleben einen WebSocket-Disconnect — nur `Connection.Close()` killt sie aktuell.

**Single-Attach Modell:**
- Pro Screen kann maximal ein Client attached sein
- Zweiter Client bekommt Reject (`mux.busy`) oder übernimmt den Screen (konfigurierbares Verhalten, Default: Reject)
- Kein Multi-Client-Sync, kein Resize-Konflikt, kein Input-Routing

**Warum Single-Attach:** Multi-Attach (mehrere Clients am selben Screen gleichzeitig) bringt massive Komplexität: Realtime-Sync, Resize-Konflikte (welche Terminalgröße bei 120x40 vs 80x24?), Input-Routing (wessen Tastatur?). Der Use-Case (Pair-Programming, Remote-Zuschauen) ist real aber selten. Single-Attach deckt 99% der Fälle ab und hält die Architektur einfach. Multi-Attach kann als [Milestone 101](../openspec/changes/multi-attach/proposal.md) nachkommen.

**Client-Identität:**
- `clientId` wird vom **Server generiert** beim ersten Connect (UUID)
- Client speichert `clientId` in `localStorage` — überlebt Tab-Reload, aber nicht Browser-Wechsel (korrekt: anderer Browser = anderer Client)
- Server trackt pro Screen: `lastClientId` (welcher Client zuletzt attached war)
- Client sendet `clientId` bei jedem Connect/Reconnect mit

**Reconnect-Logik (entscheidend):**

Der Server entscheidet anhand von `clientId` und `lastClientId`, ob der Client seinen lokalen State behalten darf oder den Server-State laden muss:

```
Client reconnected mit clientId
    ↓
Server: clientId == screen.lastClientId ?
    ├── JA  → "du warst der letzte, trust your state"
    │         → Kein Snapshot, nur WS-Streams wieder aufbauen
    │         → Client arbeitet mit seinem lokalen State weiter
    │
    └── NEIN → "jemand anderes war da, dein State ist stale"
              → Server sendet screen_snapshot
              → Client verwirft lokalen State, baut Layout aus Snapshot auf
```

**Warum das wichtig ist:** Ohne diese Prüfung müsste der Client bei jedem Reconnect (auch bei kurzem Netzwerk-Dropout) den kompletten State vom Server laden und dockview neu aufbauen. Mit `lastClientId`-Check ist ein Reconnect nach Connection-Loss nahezu unsichtbar — WS-Streams werden wieder aufgebaut, kein Layout-Flicker.

**Verhalten:**
- Client connected (erster Besuch) → Server generiert `clientId`, sendet `screen_snapshot`
- Client reconnected (gleicher Client, kein anderer dazwischen) → kein Snapshot, nur Streams reconnecten
- Client reconnected (anderer Client war dazwischen) → Server sendet `screen_snapshot`, Client lädt State
- Client disconnected → `server-owned` Sessions laufen weiter, Scrollback bleibt im RAM
- `client-owned` Screens: Detach killt Sessions (heutiges Verhalten)
- `server-owned` Screens: Detach lässt Sessions laufen

**Scrollback-Handling:**
- Server hält Scrollback-Buffer im RAM für `server-owned` Sessions (Ring-Buffer existiert bereits in `internal/pty/ringbuffer.go`)
- Bei Reconnect mit Snapshot: Server sendet gespeicherten Scrollback als initialen Output
- Bei Reconnect ohne Snapshot (gleicher Client): Scrollback ist noch im Client-Terminal vorhanden, kein Replay nötig — nur neuer Output seit Disconnect wird nachgeliefert
- Buffer-Größe konfigurierbar (Default: gleiche Größe wie Terminal-Scrollback)

**Abnahmekriterien:**
- `clientId` wird server-generiert und im Client in `localStorage` gespeichert
- Server trackt `lastClientId` pro Screen
- Reconnect desselben Clients (kein anderer dazwischen) ist nahtlos — kein Snapshot, kein Layout-Rebuild
- Reconnect nach fremdem Client → Server-State wird geladen
- Scrollback wird nach Reconnect korrekt wiederhergestellt (Replay bei Snapshot, Delta bei nahtlosem Reconnect)
- Laufende Prozesse in `server-owned` Screens werden nicht unterbrochen
- Zweiter Client auf denselben Screen bekommt sauberen Reject (`mux.busy`)

---

### Schritt 8: CLI-Steuerung

**Ziel:** Layout-Operationen von der Kommandozeile.

**Dateien:** Neue CLI-Commands in `main.go` / `cmd/`

Minimales Command-Set:

```bash
# Pane-Operationen
splitty split [--horizontal|--vertical]    # Aktuelles Pane splitten
splitty close                              # Aktuelles Pane schließen
splitty focus [left|right|up|down]         # Fokus verschieben

# Informationen
splitty list-panes                         # Alle Panes mit IDs auflisten
splitty list-screens                       # Alle Screens auflisten

# Screen-Operationen
splitty new-screen [--name NAME]           # Neuen Screen erzeugen
splitty switch-screen <name|id>            # Screen wechseln
```

**Kontext-Erkennung via Environment:**

```bash
# Wird automatisch in jeder Splitty-Shell gesetzt
SPLITTY_PANE_ID=pane-abc123
SPLITTY_SESSION_ID=sess-xyz789
SPLITTY_SCREEN_ID=screen-001
SPLITTY_SERVER=127.0.0.1:9800
SPLITTY_TOKEN=...
```

Damit weiß `splitty split` (ohne Argumente), welches Pane gesplittet werden soll.

**Transport:** CLI sendet Op über HTTP POST an den Server (kein WebSocket nötig für einmalige Befehle).

```
POST /api/mux/op
Authorization: Bearer <token>
Content-Type: application/json

{
  "screenId": "screen-001",
  "op": { "type": "split_panel", "panelId": "pane-abc123", "axis": "vertical" }
}
```

**Abnahmekriterien:**
- `splitty split` innerhalb eines Splitty-Terminals splittet das richtige Pane
- `splitty list-panes` zeigt alle Panes mit IDs, cwd, aktivem Prozess
- CLI-Ops laufen durch denselben Reducer wie Frontend-Ops
- Fehlermeldungen bei ungültigen Operationen (z.B. close auf letztem Pane)

---

### Schritt 9: Screens-Integration

**Ziel:** Screens als Server-seitiges Konzept mit Ownership pro Screen.

**Dateien:** Erweiterung von `internal/mux/store.go`, Anpassungen an der Screen-Verwaltung im Frontend

Der Mux-Store verwaltet mehrere Screens:

```go
type MuxStore struct {
    screens      map[string]*ScreenState
    activeScreen string
    mu           sync.RWMutex
}
```

**Verhalten:**
- Jeder Screen hat einen eigenen ScreenState mit eigenem Layout-Baum
- Jeder Screen hat seinen eigenen Ownership-Modus (`client-owned` oder `server-owned`)
- Screen-Wechsel ist eine Op (`set_active_screen`)
- Templates erzeugen neue Screens (statt das bestehende Layout zu überschreiben)

**Integration mit bestehendem Screens-Feature:**
- Das Frontend nutzt bereits mehrere dockview-Instanzen (eine pro Screen, per CSS versteckt)
- Der Server wird zur Source of Truth für die Screen-Liste und den aktiven Screen
- Screen-Operationen (create, close, switch, rename) laufen als Ops durch den Reducer

**Abnahmekriterien:**
- Screens werden server-seitig verwaltet
- `splitty new-screen` und `splitty switch-screen` funktionieren
- Ownership-Modus ist pro Screen konfigurierbar
- `server-owned` Screens überleben Client-Disconnect

---

## Migrations-Risiken und Mitigations

### Risiko: dockview-State und Server-State laufen auseinander

**Mitigation:** Shadow-Modus (Schritt 5, Phase A). Server trackt mit, Frontend arbeitet wie bisher. Automatisierte Vergleiche zwischen dockview.toJSON() und ScreenState decken Divergenzen auf, bevor sie User betreffen.

### Risiko: Performance-Regression durch Op-Roundtrips

**Mitigation:** Ops sind fire-and-forget — der Client wartet nicht auf eine Server-Antwort. Kein Roundtrip, kein Latenz-Impact. Resize-Drags senden nur beim Loslassen, nicht per Frame.

### Risiko: Bestehende Features brechen (Templates, Webview-Panes)

**Mitigation:** Inkrementelle Migration. Templates arbeiten zunächst weiter wie bisher (dockview-basiert). Erst in Schritt 9 werden sie auf das Backend-Modell umgestellt. Webview-Panes sind ein Pane-Typ im neuen Modell — der Panel-Renderer bleibt gleich.

### Risiko: Komplexität der Revision/Conflict-Handling

**Mitigation:** Bei Single-Attach gibt es keine konkurrierenden Clients → keine Revision-Konflikte. Der einzige Fall ist CLI-Ops parallel zum Frontend — dafür reicht ein einfacher Resync-Trigger. Echtes Conflict-Handling wird erst mit Multi-Attach relevant (Milestone 101).

---

## Reihenfolge und Abhängigkeiten

```
Schritt 1: Datenmodell
    ↓
Schritt 2: Reducer + Ops          (braucht Datenmodell)
    ↓
Schritt 3: Effect-System          (braucht Reducer)
    ↓
Schritt 4: WS-Protokoll           (braucht Ops + Effects)
    ↓
Schritt 5: Frontend MuxSync       (braucht WS-Protokoll)
    ↓
Schritt 6: Screen Ownership        (braucht funktionierenden Mux-Store)
    ↓
Schritt 7: Attach/Detach          (braucht Ownership-Modell)
    ↓
Schritt 8: CLI                    (braucht Reducer + HTTP-Endpoint)
    ↓
Schritt 9: Screens                (braucht alles oben)
```

Schritte 6+7 und 8 sind voneinander unabhängig und können parallel bearbeitet werden.

---

## Was danach kommt

Wenn dieser Milestone abgeschlossen ist, hat Splitty:

- ✅ Server als Source of Truth für Layout + Sessions
- ✅ Screen Ownership Model (client-owned vs server-owned)
- ✅ Single-Attach mit Detach/Reattach (Sessions überleben Client-Disconnect bei server-owned Screens)
- ✅ CLI-Steuerung (`splitty split`, `splitty close`, `splitty focus`)
- ✅ Environment-Kontext ($SPLITTY_PANE_ID etc.)
- ✅ Screens als Server-seitiges Konzept mit Ownership

Die nächsten Milestones bauen darauf auf (in empfohlener Reihenfolge):

1. [**Milestone 2: Notification System**](02-notifications.md) — Process-Lifecycle-Events, Idle-Detection, Drawer-Badges, Toasts
2. [**Milestone 3: Svelte-Migration**](03-svelte-migration.md) — Frontend-Framework tauschen, dockview-core direkt nutzen
3. [**Milestone 4: Rust Backend**](04-rust-backend.md) — Backend + PTY in Rust reimplementieren, gleiches Protokoll
4. [**Milestone 5: Persistent Daemon**](05-persistent-daemon.md) — Server als Hintergrund-Daemon (systemd/launchd)
5. [**Milestone 6: Terminal Intelligence**](../openspec/changes/terminal-intelligence/proposal.md) — Shell-Integration, App-Parser, ACP
