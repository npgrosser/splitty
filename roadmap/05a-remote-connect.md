# Milestone 5a: Remote Connect

> **Status:** ✅ Done
>
> **Voraussetzung:** [Milestone 5 — Persistent Daemon](05-persistent-daemon.md)

## Ziel

Splitty-Client kann sich zu einem Remote-Daemon verbinden. Remote fühlt sich an wie lokal — gleiche UI, gleiche Features, gleiche Shortcuts. Kein Terminal-in-Terminal, kein nested Keybinding-Problem.

## Implementiert

### SSH Tunnel CLI

```
splitty connect user@host
```

Ursprünglich (5a) las der Befehl `daemon.json` per `cat` und gab den Token des Remote-Daemons in der URL weiter. Beides ist ersetzt; das Verhalten beschreiben die Specs `cli` (Remote Connect Tunnel, Session Handoff, Reconnect) und `access-control` (Remote Access Through SSH Tunnel). Heute:

1. Fragt das `splitty` auf dem Host nach seinem Daemon (`splitty daemon status --json`)
2. Öffnet den SSH-Tunnel auf demselben Port, lokal explizit an `127.0.0.1` gebunden (`ssh -L 127.0.0.1:P:127.0.0.1:P -N`), und prüft ihn
3. Holt auf dem Host einen Einmal-Code (`splitty issue-bootstrap`) und öffnet `http://127.0.0.1:P/?bootstrap=<code>&host=<target>`; kein langlebiges Credential geht über die Leitung
4. Baut den Tunnel neu auf, wenn er abreißt (seit `remote-connect`, Milestone 7); Ctrl+C beendet ihn

### Remote Indicator

- `ConnectionIndicator.svelte` im Titlebar
- Zeigt "Daemon" bei Daemon-Verbindung, "Remote: {host}" bei `?host=`-Parameter
- Embedded Mode: kein Indicator

### Keine direkte Verbindung mehr

Anfangs war ein Remote-Server auch direkt erreichbar (`http://remote:9800/?token=…`, im Desktop per `?host=remote:9800`). Das ist bewusst entfernt (Security-Review 2026, siehe `SECURITY.md`): der Server lauscht nur auf Loopback, Remote-Zugriff geht ausschließlich über SSH, und `host` in der URL ist nur noch Anzeige für den Indicator.

## Design-Entscheidungen

- **Ein Client = ein Remote** — kein Multi-Host-Mixing in einem Fenster
- **Kein per-Screen Ownership** — Persistenz ist Server-Eigenschaft, nicht per-Screen
- **SSH-Tunnel wird vom CLI verwaltet** — nicht vom Frontend (Webseite kann kein SSH)
- **System-SSH** — kein eingebetteter SSH-Client, nutzt `~/.ssh/config` und SSH-Agent
- **Server-Bookmarks** — bewusst auf später verschoben, CLI reicht für den Anfang

## Dateien

- `crates/splitty-server/src/tunnel/` — SSH-Tunnel-Logik und Supervisor (Neuaufbau)
- `crates/splitty-server/src/main.rs` — `connect` Subcommand
- `frontend/src/components/chrome/ConnectionIndicator.svelte` — Remote-Badge
