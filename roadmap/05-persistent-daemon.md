# Milestone 5: Persistent Daemon

> **Status:** ✅ Done
>
> **Voraussetzung:** [Milestone 1 — Multiplexer Backend](01-multiplexer-backend.md), [Milestone 4 — Rust Backend](04-rust-backend.md)

## Ziel

Splitty-Server läuft als Hintergrund-Daemon, unabhängig von der Desktop-App oder einem Browser-Tab. Vergleichbar mit tmux's Server-Modell, aber mit GUI-First UX.

Persistenz ist eine Server-Eigenschaft (Daemon vs. Embedded), nicht per-Screen konfigurierbar.

## Implementiert

### Daemon-Lifecycle

- **`splitty-server daemon start`** — Server im Hintergrund starten (Double-Fork + Detach)
- **`splitty-server daemon stop`** — Daemon sauber beenden (SIGTERM)
- **`splitty-server daemon status`** — PID, Port, Startzeit, Version anzeigen

### State-Persistenz

- **`~/.config/splitty/state.json`** — WorkspaceState wird nach jedem Dispatch atomar gespeichert
- **`~/.config/splitty/daemon.json`** — Daemon-Info (PID, Port, Token) für Discovery
- **Session-Restore:** Beim Daemon-Neustart werden PTY-Sessions aus dem persistierten State neu gespawnt (gleiche Session-IDs, gleiche Commands/CWDs)
- **Scrollback wird nicht persistiert** — Sessions starten frisch, wie bei tmux

### Client-Anbindung

- **Desktop-App:** Prüft beim Start ob ein Daemon läuft (`daemon.json` + PID-Check). Falls ja, verbindet sich zum Daemon statt eigenen Server zu starten
- **Fallback:** Kein Daemon → Desktop-App startet eingebetteten Server wie bisher

### Server-Mode im Protokoll

- `mux.welcome` enthält `serverMode: "embedded" | "daemon"`
- Frontend kann darauf reagieren (z.B. ConnectionIndicator im Titlebar)

## Design-Entscheidungen

- **Kein per-Screen Ownership** — Persistenz ist binär: Daemon = alles persistent, Embedded = nichts persistent
- **Kein Attach/Detach** — Client bekommt Screen-State via `mux.resync` on demand
- **Ein Handshake** — Server sendet `mux.welcome` mit Screen-Liste und `serverMode`, kein `hello`/`mux.connect`-Split
- **Kein systemd/launchd** — Double-Fork reicht. Service-Manager-Integration kann später kommen
- **State-Persistenz nach jedem Dispatch** — WorkspaceState ist klein (<10KB), kein Batching/Debouncing nötig

## Dateien

### Rust Backend
- `crates/splitty-core/src/daemon/mod.rs` — DaemonInfo, read/write, is_running, restore_effects
- `crates/splitty-core/src/daemon/persistence.rs` — save/load WorkspaceState
- `crates/splitty-core/src/mux/store.rs` — `on_change` Callback, `with_state()` Constructor
- `crates/splitty-server/src/main.rs` — Daemon CLI Commands, Daemonization

### Frontend
- `frontend/src/lib/mux/mux_sync.ts` — `getServerMode()` Getter
- `frontend/src/components/chrome/ConnectionIndicator.svelte` — Daemon/Remote Badge
