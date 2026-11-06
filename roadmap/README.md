# Roadmap

Splittys Roadmap in empfohlener Reihenfolge. Leitprinzip: **Features vor Rewrites** — User-sichtbaren Value liefern bevor Infrastruktur umgebaut wird.

## Roadmap und OpenSpec

Seit Oktober 2026 läuft die Umsetzung über [OpenSpec](../openspec/). Die Rollen:

| Ort | Was dort steht |
|---|---|
| `roadmap/README.md` | Dieser Index: Reihenfolge, Leitprinzipien, Abhängigkeiten. |
| `roadmap/NN-*.md` | Historie der abgeschlossenen Milestones (1–5a) und noch nicht ausgearbeitete Ideen (102). |
| `openspec/changes/` | Ausgearbeitete Vorhaben. **Drafts** haben nur Proposal (und ggf. Design), Delta-Specs und Tasks kommen, wenn der Change dran ist. Laufende Changes haben alle Artefakte. |
| `openspec/specs/` | Was das System heute garantiert (Verhalten, Protokolle, Security-Invarianten). Aus dem Code abgeleitet. |
| `docs/architecture.md` | Wie es intern gebaut ist. Bewusst nicht als Spec. |

Ablauf:
- **Neue Idee** → direkt als Draft-Change (`/opsx:explore` zum Durchdenken, dann einen Change mit Draft-Kopf anlegen) und hier in die Tabelle eintragen. Kein neues Roadmap-Doc mehr.
- **Draft wird angegangen** → Proposal gegen die Specs prüfen, Delta-Specs, Design und Tasks ergänzen, umsetzen (`/opsx:apply`), archivieren (`/opsx:archive`). Danach verlinkt die Tabelle die betroffenen Specs.
- Bugs, veraltete Doku und Testlücken aus dem Aufbau der Baseline: [`docs/issues/openspec-baseline-findings.md`](../docs/issues/openspec-baseline-findings.md).

## Milestone 7, Remote Development (abgeschlossen)

Ziel: auf einer VM oder in einem DevPod-Workspace entwickeln, als wäre es lokal. Splitty läuft dort persistent weiter (Claude-Code-Sessions, Panes, Screens), der eigene Rechner zeigt nur an. Unter Windows auch ganz ohne WSL, wenn man nur remote arbeitet.

| Schritt | Change | Was es bringt | Status |
|---|---|---|---|
| 7.0 | `architecture-model` | Architekturmodell plus Import-Test, damit die neuen Bausteine (Crate `splitty-remote`, Bridge auf dem Host) vorher im Modell stehen. Modell in [`architecture/`](../architecture/README.md), geprüft von `cargo test` | ✅ Done |
| 7.1a | `remote-connect` | `splitty connect` alltagstauglich: Tunnel baut sich selbst neu auf, Bind auf `127.0.0.1`, `--no-open`, Exit-Status, fragt den Host nach dem Daemon, Hostname im Fenster. Specs: `cli`, `access-control` | ✅ Done |
| 7.1b | `remote-install` | Auto-Install des passenden Servers auf dem Host über SSH, Crate `splitty-remote` (auch nativ unter Windows), Versionen nebeneinander. Specs: `remote-hosts`, `cli`, `access-control` | ✅ Done |
| 7.1c | `remote-handshake` | Handshake in einem SSH-Aufruf, Daemon-Start auf freiem Port (auch nach Reboot), Neuinstallation nach Container-Rebuild, Bridge ohne Credential für 7.3, Protokollversion. Specs: `remote-hosts`, `cli`, `server-modes`, `access-control` | ✅ Done |
| 7.1d | `remote-aarch64` | Statischer aarch64-Server im Release, eingebaut in den statischen x86_64-Server (der damit auf beiden Architekturen installiert), CI baut aarch64 mit. Spec: `remote-hosts` | ✅ Done |
| 7.2a | `remote-supervisor` | Tunnel-Supervisor in `splitty-remote` (für die Desktop-Apps), `ssh.exe` unter Windows, bekannte Hosts (zuletzt verbunden plus `~/.ssh/config`), `connect --json` für die Electron-Shell. Specs: `cli`, `remote-hosts` | ✅ Done |
| 7.2b | `remote-window` | Remote in der Desktop-App: ein Fenster pro Host, ohne lokalen Server, unter Windows ohne WSL (`ssh.exe`). Rechtloses Fenster mit eigenem Cookie-Speicher, Host-Auswahl mit Fortschritt (Command-Palette, Mod+Shift+P), Payloads für beide Architekturen in allen Apps. Specs: `desktop-remote`, `access-control`, `server-modes`, `remote-hosts` | ✅ Done |
| 7.2c | `remote-ports` | Ports von Dev-Servern auf dem Host lokal erreichbar: `connect --forward 3000,5173`, gleicher lokaler Port wenn frei, als `localhost` angekündigt (kein Splitty-Cookie für den Dev-Server), beim Reconnect mit aufgebaut. Specs: `cli`, `access-control` | ✅ Done |
| 7.3a | `connection-scoped-frontend` | Frontend hält mehrere Server-Verbindungen (je eigener Mux-, Session- und Attach-Client), Screens an ihre Verbindung gebunden, Nachrichten nur gegen die eigene Verbindung aufgelöst; Härtung: zufällige IDs, inaktive Screens `inert`, Server-Commands nur auf eigene Screens. Specs: `workspace-model`, `mux-protocol` | ✅ Done |
| 7.3b | `remote-screens` | Remote-Screens neben lokalen Screens im selben Fenster, der lokale Server als Transport-Hub (`/ws/remote`: Install, Handshake, Bridge, Relay ohne Umschreiben). Host-Auswahl, Fortschritt im Screen, Wiederöffnen laufender Screens, Badges, Reconnect pro Host, Zuklappen statt Beenden. Specs: `remote-screens`, `access-control` | ✅ Done |
| 7.3c | `remote-screens-persistence` | Remote-Screens überleben den Neustart (Liste beim lokalen Server, Verbinden erst beim Anzeigen), „Auf Hosts“, Update inkompatibler Hosts mit Warnung. Specs: `remote-screens`, `access-control` | ✅ Done |
| 7.4 | `remote-ports-auto` | Automatisch erkannte Ports (lauschende Prozesse der Sessions auf dem Host, `ports.changed`) mit Liste, Hinweis und klickbaren `localhost`-Links in Remote-Panes, wie VS Code; Forwards über den Hub (`/api/remote/forwards`), automatisches Forwarding per Einstellung (aus). Specs: `remote-screens`, `mux-protocol`, `access-control` | ✅ Done |
| 7.5 | `persistent-host-directory` | Host-Gruppen statt Badge pro Screen; zuletzt gesehene Screens bleiben nach Detach erhalten, Aufklappen verbindet ohne neuen Screen. | ✅ Done |

Milestone 7 ist fertig (Oktober 2026):
- **Browser:** `splitty connect user@vm` installiert den passenden Server auf dem Host (x86_64 und aarch64), startet den Daemon und baut den Tunnel nach Schlafmodus, Netzwechsel, Reboot oder Container-Rebuild selbst wieder auf. `--forward` reicht Dev-Server-Ports durch.
- **Desktop-Apps:** Sie öffnen einen Host als eigenes, rechteloses Fenster, unter Windows auch ganz ohne WSL.
- **Lokale Fenster:** Sie zeigen Remote-Screens neben lokalen Screens, über den lokalen Server als Hub. Die Remote-Screens überleben den Neustart, Zuklappen beendet nichts, und Ports der Sessions auf dem Host sind einen Klick entfernt.

Offen aus dem Milestone:
- die vorläufigen Entscheidungen in [`openspec/open-decisions.md`](../openspec/open-decisions.md);
- die Prüfung der Desktop-Apps unter Windows und macOS (bisher nur typgeprüft bzw. per CI-Build, CI läuft erst nach einem Push);
- der WSL-Installer auf `splitty-remote` (110).

## Milestone 8, Sessions unabhängig vom Server (abgeschlossen)

Ziel: Prozesse in Splitty überstehen Updates, Neustarts und Abstürze des Daemons, und man erreicht sie auch ohne Splitty-Fenster, etwa aus einem SSH-Terminal. Danach übernimmt Splitty sie wieder, ohne dass ein Prozess davon etwas merkt.

| Schritt | Change | Was es bringt | Status |
|---|---|---|---|
| 8.1 | `terminal-attach` | `splitty attach`: eine Session des Daemons im Terminal, z. B. per SSH, gleichzeitig mit dem Fenster; Auswahl aus einer Liste, Detach mit Ctrl+\, Größe wie ein Pane. Der Daemon bleibt Besitzer, „zurück zu Splitty“ heißt Detach. Spec: `cli` | ✅ Done |
| 8.2 | `session-keepers` | Ein Keeper-Prozess pro Session im Daemon-Modus hält PTY und Prozess. Neustart, Update und Absturz des Daemons beenden keine Session mehr, Scrollback und Exit-Codes bleiben; `daemon stop` lässt Sessions laufen (`--end-sessions` beendet sie), `daemon restart` als Update-Befehl; `splitty attach` auch ohne Daemon, der startende Daemon übernimmt wieder. Specs: `session-keepers`, `server-modes`, `terminal-sessions`, `cli`, `access-control`, `control-api` | ✅ Done |
| 8.3 | `keeper-host-updates` | Die Clients erfahren davon: Handshake, `remote.ready` und der `incompatible`-Fehler melden `keepsSessions`, die Update-Warnung nennt endende Panes nur noch bei Daemons ohne Keeper, `connect` aktualisiert einen älteren Daemon mit Keepern selbst (nie einen neueren), die Desktop-App startet nach einem Absturz den Daemon der wartenden Keeper statt eines leeren eingebetteten Servers. Specs: `remote-hosts`, `remote-screens`, `cli`, `server-modes` | ✅ Done |

## Als Nächstes

Der Nachtrag `remote-forwards-persistence` (manuelle und gespeicherte Dev-Port-Weiterleitungen mit Labels, dauerhaftem Stop/Resume und Wiederherstellung beim Verbinden des Hosts) ist seit 2026-10-28 archiviert. SSH-Lab und Browser-Abnahme liefen dafür in einem Linux-Container, da der Lab nicht in CI läuft; die übrigen kleineren Testlücken stehen in `docs/issues/openspec-baseline-findings.md`.

Aus einem Praxistest auf einem VPS folgten drei Nachträge, alle am 2026-10-14 archiviert: `remote-development-ux` räumt die Bedienung von Hosts und Ports auf; `remote-open-vscode` ergänzt „Open in VS Code“ für den aktuellen Ordner eines Remote-Terminals; `terminal-folder-actions` ergänzt „Copy SSH Path“ und „Open in VS Code“ für lokale Terminals im Browser. Live-Prüfungen auf Windows/macOS und mit installiertem Editor stehen aus.

[Milestone 8](#milestone-8-sessions-unabhängig-vom-server-abgeschlossen) ist abgeschlossen: `terminal-attach`, `session-keepers` und `keeper-host-updates` sind seit 2026-10-28 archiviert, ebenso der Quick Win `session-scratchpad` (108): ein Scratchpad pro Terminal-Session unter dem Pane (`Cmd/Alt+J`), mit der Session gespeichert und für Agents in Panes unsichtbar. Vor **112** vorgezogen und seit 2026-11-06 archiviert: **105** (`splitty open` mit Viewer-Panes) und **111** (Annotationen und Review an ein Terminal). Ihre Folge-Drafts sind 115 bis 119. Als Nächstes wird die Architektur geradegezogen: **112**, erst die vier Server-Changes (unabhängig voneinander), dann der Client-Kern, der die Viewer-Teile mit aufnimmt. Danach kommt [Milestone 6, Terminal Intelligence](../openspec/changes/terminal-intelligence/proposal.md) in Frage.

## Empfohlene Reihenfolge

| # | Milestone | Status | Voraussetzung |
|---|-----------|--------|---------------|
| 1 | [Multiplexer Backend](01-multiplexer-backend.md) | ✅ Done | — |
| 2 | [Notification System](02-notifications.md) | ✅ Done | Milestone 1 |
| 3 | [Svelte-Migration](03-svelte-migration.md) | ✅ Done | Milestone 1 |
| 3a | [Theming System](03a-theming-system.md) | ✅ Done | Milestone 3 |
| 4 | [Rust Backend](04-rust-backend.md) | ✅ Done | Milestone 1 |
| 5 | [Persistent Daemon](05-persistent-daemon.md) | ✅ Done | Milestone 1 + 4 |
| 5a | [Remote Connect](05a-remote-connect.md) | ✅ Done (Ausbau: Milestone 7) | Milestone 5 |
| 6 | [Terminal Intelligence](../openspec/changes/terminal-intelligence/proposal.md) | Draft (Ebene 1 teils umgesetzt) | Milestone 2 + 4 |
| 7 | Remote Development: Schritte 7.0–7.4 siehe [oben](#milestone-7-remote-development-abgeschlossen) | ✅ Done | Milestone 5 + 5a |
| 8 | Sessions unabhängig vom Server: Schritte 8.1–8.3 siehe [oben](#milestone-8-sessions-unabhängig-vom-server-abgeschlossen) | ✅ Done | Milestone 5 |

```
Milestone 1: Multiplexer Backend  ✅
    │
    ├──→ Milestone 2: Notification System  ✅
    │         └──→ Milestone 6: Terminal Intelligence
    │
    ├──→ Milestone 3: Svelte-Migration  ✅
    │         └──→ Milestone 3a: Theming System  ✅
    │
    └──→ Milestone 4: Rust Backend  ✅
              └──→ Milestone 5: Persistent Daemon  ✅
                        ├──→ Milestone 5a: Remote Connect  ✅
                        │         └──→ Milestone 7: Remote Development  ✅
                        └──→ Milestone 8: Sessions unabhängig vom Server  ✅
```

## Milestone-Beschreibungen

**Milestone 1** macht den Server zur Source of Truth für Layout und Sessions. Shadow Mode: Client ist autonom, Server spiegelt. Fire-and-forget Ops, client-generierte IDs.

**Milestone 2** bringt ein vollständiges Notification-System: Process-Lifecycle-Events (Exit, Crash), generische Title-Icon-Animation-Detection, Drawer-Badges, Toast-Popups und Desktop-Notifications.

**Milestone 3** migriert das Frontend von React nach Svelte 5. Löst die architektonischen Probleme die mit React entstanden sind (Memory Leaks, HMR-State-Leaks, Hook-Drift).

**Milestone 3a** baut ein dockview-agnostisches Theming-System: Splitty-eigene CSS-Tokens (`--splitty-*`), Ghostty-kompatible Themes, palette-basierte Token-Derivation.

**Milestone 4** portiert das Go-Backend nach Rust. Gleiche Types, gleicher Reducer, gleiches Protokoll. `portable-pty` für Cross-Platform-PTY, `vte` für die Parser-Pipeline in M6.

**Milestone 5** macht den Server zum eigenständigen Hintergrund-Daemon mit State-Persistenz. Persistenz ist eine Server-Eigenschaft (Daemon vs. Embedded), nicht per-Screen. `mux.welcome` enthält `serverMode`. Sessions werden beim Neustart aus persistiertem State respawnt.

**Milestone 5a** ermöglicht Remote-Verbindungen über SSH-Tunnel (`splitty connect user@host`). Die Architektur war schon immer netzwerk-transparent — dieser Milestone liefert CLI-Convenience und einen visuellen Remote-Indicator im Titlebar.

**Milestone 6** gibt Splitty semantisches Verständnis für Terminal-Inhalte: Shell-State via OSC 133, app-spezifische Output-Parser, und native Agent-Integration. Nutzt das Notification-System aus Milestone 2 als Delivery-Kanal. OSC-133-Parsing und bash/zsh-Integration existieren bereits; der Draft-Change trennt Erledigtes und Offenes.

**Milestone 7** macht Splitty zum Werkzeug für Entwicklung auf entfernten Rechnern (VM, DevPod-Workspace). Splitty läuft auf dem Host als Daemon und hält dort Screens, Panes und Prozesse am Leben; der eigene Rechner zeigt nur an. Zugang ausschließlich über SSH, der Server bleibt auf dem Host an Loopback gebunden, über die Leitung geht kein langlebiges Credential. Drei Wege bauen aufeinander auf: Schritt 7.1 (`remote-connect`, `remote-install`, `remote-handshake`, `remote-aarch64`) macht `splitty connect` samt Browser robust und installiert Splitty automatisch auf dem Host; `remote-window` bringt einen Host als eigenes, rechteloses Fenster in die Desktop-Apps, auch unter Windows ohne WSL; `remote-screens` mischt Remote- und lokale Screens in einem Fenster.

**Milestone 8** trennt den Besitz der Prozesse vom Server. Heute hält der Daemon jedes PTY selbst; endet er (Stop, Update, Absturz), legt der Kernel alle Terminals auf, und der Neustart startet nur frische Shells. `terminal-attach` macht zuerst ein beliebiges Terminal zu einem weiteren Client einer Session (wie `tmux attach`), über die schon bestehende Authentisierung der Remote-Bridge. `session-keepers` gibt dann jeder Session im Daemon-Modus einen kleinen Keeper-Prozess (wie dtach oder shpool), der Prozess und PTY hält; der Daemon ist nur noch sein Controller und übernimmt laufende Keeper beim Start. `keeper-host-updates` sagt es schließlich den Clients: Update-Warnungen nennen endende Panes nur noch bei Daemons ohne Keeper, `splitty connect` aktualisiert einen älteren Host ohne Verlust, und die Desktop-App startet nach einem Absturz den Daemon, der die wartenden Sessions übernimmt. Embedded-Server bleiben, wie sie sind: Persistenz ist weiterhin eine Eigenschaft des Modus.

---

## In Vorbereitung (100er)

Ideen und Features, nicht fest eingeplant, aber vorsortiert. Ausgearbeitete Vorhaben sind Draft-Changes in `openspec/changes/`.

| # | Vorhaben | Status | Voraussetzung |
|---|-----------|--------|---------------|
| 101 | [Multi-Attach](../openspec/changes/multi-attach/proposal.md) | Draft | Fix für den PTY-Reader (Fundliste) |
| 102 | [Cloud Demo Platform](102-cloud-demo.md) | Idee | Milestone 1 |
| 103 | [Notes & Tags](../openspec/changes/notes-and-tags/proposal.md) | Draft | Milestone 1 |
| 104 | [MCP Server](../openspec/changes/mcp-server/proposal.md) | Draft | Fixes für Session-Output (Fundliste) |
| 105 | [splitty open](../openspec/specs/viewer-panes/spec.md) | ✅ Done (2026-11-06): Viewer-Panes | Milestone 1 |
| 106 | [CLI Control](../openspec/specs/managed-jobs/spec.md) | ✅ Done: Managed Jobs, Logs, Agent-Rechte | Milestone 1 + 5 |
| 107 | Remote Screens | → aufgegangen in Milestone 7 | |
| 108 | Session-Scratchpad | ✅ Done (2026-10-28) | Milestone 1 + 5 |
| 109 | [Workspace-Restore](../openspec/changes/workspace-restore/proposal.md) | Draft | Milestone 1 + 5, Session-Status und Pane-cwd im Modell (Fundliste) |
| 110 | [WSL-Installer teilen](../openspec/changes/wsl-install-shared/proposal.md) | Draft | Windows-Tests (CI oder Windows-Rechner) |
| 111 | [Review-Pane](../openspec/specs/viewer-annotations/spec.md) | ✅ Done (2026-11-06): Annotationen | 105 |
| 112 | [Client-Kern und Fachlogik im Server](../openspec/changes/frontend-client-core/proposal.md) | Draft | — |
| 113 | [Mikrofon für Hosts](../openspec/changes/remote-microphone/proposal.md) | Draft | Milestone 7 + 8 |
| 114 | [Terminal-Bell](../openspec/changes/terminal-bell/proposal.md) | Draft | — |
| 115 | [Weitere Renderer](../openspec/changes/viewer-renderers/proposal.md) | Draft | 105 |
| 116 | [Dateien schnell bearbeiten](../openspec/changes/viewer-quick-edit/proposal.md) | Draft | 105 |
| 117 | [Diffs reviewen](../openspec/changes/review-diffs/proposal.md) | Draft | 105 + 111 |
| 118 | [HTML mit Skripten in einer Sandbox](../openspec/changes/viewer-sandboxed-html/proposal.md) | Draft | 105 |
| 119 | [Web-View für Dev-Server](../openspec/changes/viewer-webview/proposal.md) | Draft | 105 + Milestone 7 |

**101** macht mehrere gleichzeitige Clients am selben Screen verlässlich. Verbinden können sie sich schon heute (Single-Attach wurde im März 2026 entfernt); es fehlen Layout-Sync für alle Ops, Reihenfolge und Input-Routing. Geteilte Output-Streams (`fix-pty-reader`) und die Resize-Policy (`pty-size-policy`: kleinste Größe der aktiven Clients, sichtbare Markierung, Übernahme) sind erledigt.

**102** gehostete Splitty-Demo-Plattform: User besucht URL, bekommt eigenen ephemeren Container.

**103** leichtgewichtiges Notiz-System mit Tag-System das Notes, Screens und Templates lose verbindet.

**104** MCP Server: Splitty als Tool für AI Agents exponieren. Agents können Terminals lesen, Input senden, Workspaces verwalten.

**105** `splitty open <file>` zeigt eine Datei in einem Viewer-Pane neben dem Terminal: Markdown und HTML gerendert, ohne Skripte und ohne Frames, Code und Text mit Syntax-Highlighting, Bilder. Gelesen wird über die Verbindung des Fensters, also genauso auf Hosts, und das Pane folgt Änderungen der Datei. Eine Terminal-Auswahl öffnet sich über das Kontextmenü als Text im Viewer. Ordner, `--editor` und `--with` gehen weiter an VS Code oder Zed. Weitere Formate und ein Overlay sind 115.

**106** ergänzt die bestehende CLI um sichtbare Hintergrund-Jobs (`run`, `jobs`, `logs`, `inspect`, `input`, `stop`, `forget`) und begrenzte Agent-Zugänge. Prozesse und Output bleiben beim Splitty-Server, wenn Claude neu startet. MCP kann dieselbe API später nutzen. Weitere CLI-Conveniences (`--command`-Sugar, `-v`, Uptime, Completions) bleiben Folgearbeit. Anleitung: [Managed Jobs](../docs/cli-jobs.md).

**107** ist in Milestone 7 (Remote Development) aufgegangen, siehe oben.

**108** ein Notizfeld pro Terminal-Session, im Pane einklappbar: vorbereitete Prompts und Merkzettel für eine laufende Agent-Session, ohne Texteditor daneben. Lebt so lange wie die Session (im Daemon-Modus auch über einen Neustart), Markierung im Tab, Rückfrage beim Schließen. Bewusst getrennt von 103: dort dauerhafte Notizen mit Tags, hier ein Zwischenspeicher, der fest an die Session gebunden ist.

**109** stellt beim Start der App den letzten Workspace wieder her, wie klassische Terminals ihre Fenster und Tabs: Screens, Layout, Arbeitsverzeichnisse und Claude-Code-Sessions per `claude --resume <id>` samt Berechtigungsmodus. Andere Befehle werden nur vorgetippt, nicht ausgeführt. Funktioniert auch ohne Daemon, also auch in der Windows-App. Die Conversation-ID meldet ein opt-in Claude-Code-Hook über den Control-Socket. Ergänzt Daemon und 107, die Prozesse weiterlaufen lassen, statt sie neu zu starten.

**110** installiert das WSL-Backend der Windows-App mit dem Installer aus `splitty-remote` (über `wsl.exe` statt `ssh`), in dieselben Versionsverzeichnisse mit Aufräumen. Zurückgestellt aus `remote-install` (D7), weil es sich hier nur typprüfen, nicht unter Windows testen lässt.

**111** Annotationen in Viewer-Panes: Passagen im gerenderten Text oder in der Quelle markieren und kommentieren (Kommentar, Frage, Änderungsvorschlag, Löschen), auch auf Terminal-Auswahlen wie der Antwort eines Agents. Die Annotationen liegen mit dem Pane auf dem Server, überstehen Reload und Daemon-Neustart und werden nach Änderungen der Datei über ihr Zitat wiedergefunden. Gesammelt gehen sie als Markdown-Review an ein Terminal, standardmäßig das, aus dem der Viewer geöffnet wurde: als Paste ohne Enter, als Datei mit eingefügtem Pfad oder ins Scratchpad; Kopieren und Speichern gehen auch. Agents sehen Annotationen erst, wenn der User sie schickt. Ohne Integration in ein Agent-Werkzeug; Diffs sind 117, Bearbeiten ist 116.

**112** trennt das Web-Frontend in einen UI-freien Client-Kern (`frontend/src/core/`, ohne DOM, Svelte, Dockview, Emulator und Shell-API, vom Build erzwungen), Plattform-Ports und Views. Vorher ziehen vier unabhängige Changes Fachlogik aus dem Fenster in den Server: `server-templates` (Templates anwenden und extrahieren), `server-derived-pane-state` (Screen-Namen, Pane-Titel, Busy-Flag, Close-on-Exit, Replay-Filter), `settings-authority` (Defaults, Grenzen, Validierung, Änderungs-Event) und `hub-host-decisions` (Update-Angebot, Retry, Host-Verzeichnis, Katalog, Forward-Feed). Danach kann eine zweite UI (Mobile-Renderer, native Desktop-UI) nur noch Views schreiben. Bestandsaufnahme und Reihenfolge stehen im Design.

**113** macht Spracheingabe wie Claude Codes `/voice` in Panes auf Hosts möglich, ohne PulseAudio, SoX oder eigenes SSH-Forwarding. Pro Host schaltet man das Mikrofon einmal frei. Splitty-Sessions auf dem Host finden dann ein SoX-kompatibles `rec` im `PATH`. Das ist nur ein Link auf das schon installierte Splitty-Binary, installiert wird nichts. Dieses `rec` fordert das Audio über die bestehende Verbindung beim Fenster an. Das Mikrofon öffnet nur für das fokussierte Pane eines fokussierten Fensters und nur, solange aufgenommen wird; das Pane zeigt das an. Ein Wechsel zwischen zwei Agent-Panes braucht keinen Schritt, das neu fokussierte Pane übernimmt das Mikrofon. Lokale Panes nehmen schon heute selbst auf (unter Windows über WSLg).

**114** reagiert auf die Terminal-Bell (BEL), mit der Werkzeuge wie Claude Code „fertig“ oder „wartet“ melden. Der Server erkennt sie schon und schickt `bell` an die Fenster, das Fenster loggt sie bisher nur. Geplant sind eine Markierung am Tab (wie bei ungelesenen Benachrichtigungen) und optional ein Ton oder eine Benachrichtigung. Alles passiert im Fenster selbst, darum funktioniert es für jeden Host ohne Audio über die Verbindung. Ob auch OSC-Benachrichtigungen (9, 99, 777) dazukommen, wird entschieden, wenn der Change dran ist.

**115** weitere Renderer für Viewer-Panes: Mermaid- und Graphviz-Diagramme (als Fence in Markdown und als Datei), PDF über pdf.js, CSV und TSV als Tabelle, `--format` für mehrdeutige Endungen und `splitty open --overlay` als schnelle Vorschau über dem aktuellen Pane. Ohne Renderer, die den Code eines Dokuments ausführen (dafür legt 118 die Sandbox an), und ohne URLs und Dev-Server (119).

**116** einfaches Bearbeiten kleiner Textdateien im Viewer-Pane (Agent-Anweisungen, Konfiguration, Prompts), gespeichert über die Verbindung des Fensters. Ein Speichern wird abgelehnt, wenn sich die Datei seit dem Laden geändert hat, da der Agent sie gleichzeitig bearbeiten kann. Bewusst ohne Language Server, Extensions und projektweite Suche; dafür bleibt `splitty open --editor`.

**117** Diffs des Repositorys eines Panes im Viewer (Working Tree, Index, gegen einen Commit), mit Annotationen pro Seite und Zeile und demselben Review an ein Terminal wie in 111. Der Server ruft dafür nur lesende `git`-Befehle mit festen Argumenten auf und schaltet Repository-Einstellungen ab, die Befehle ausführen könnten.

**118** HTML-Dokumente mit laufenden Skripten im Viewer-Pane: Diagramme und Reports mit eingebettetem JavaScript, als HTML exportierte Notebooks, klickbare Mock-ups. Das Dokument läuft in einem Frame mit eigenem, opakem Origin, ohne Netzwerk und ohne Weg aus dem Frame (keine Navigation des Fensters, keine Popups, Formulare oder Berechtigungen), und spricht mit seinem Pane nur über wenige geprüfte Nachrichten. Fenster, Session-Cookie, API, Terminals und die nativen Befehle der Desktop-Shell erreicht es nicht; die bestehenden Prüfungen des Servers weisen seine Anfragen zusätzlich ab. Standard bleibt die inerte Darstellung, Skripte laufen erst, wenn man sie für das Pane einschaltet. Annotationen gehen auch dort.

**119** Web-View-Panes für Dev-Server: `splitty open http://localhost:5173` oder „Open in Pane“ an einem erkannten Port zeigt die laufende App neben dem Terminal, auf Hosts über die vorhandene Port-Weiterleitung. Anders als bei den entfernten Web-App-Panes (SR-01) läuft die Seite nie auf Splittys Origin, und es gibt keinen Proxy: Sie wird direkt von `http://localhost:<port>` geladen, einer anderen Site als das Fenster auf `127.0.0.1`, und die Origin-, Fetch-Metadata- und Cookie-Regeln weisen ab, was sie an Splitty schickt. Vorerst nur `localhost`-Adressen. Seiten, die ihren Login in einem Cookie halten, gehen im Frame nicht und bleiben bei „Open in browser“. Ob die Desktop-Apps stattdessen eine eigene native Web-View bekommen, entscheidet der Change.
