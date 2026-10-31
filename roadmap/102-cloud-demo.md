# Milestone 102: Cloud Demo Platform

## Ziel

Gehostete Splitty-Demo — User besucht URL, bekommt eigenen isolierten Container mit Splitty, kann sofort loslegen. Kein Install, kein Setup.

## Architektur

```
User → demo.example.com/s/<session-id>
         → Gateway (Go, lightweight)
              → Docker API: Container erstellen/starten
              → Reverse Proxy (Traefik): WebSocket-Routing zum Container
              → Idle-Timeout: Container nach X Min ohne Traffic stoppen/löschen
```

### Gateway

- ~150 LOC Go-Service
- Spawned pro User einen isolierten Splitty-Container (eigenes Docker-Network)
- Session-ID ↔ Container-ID + Token Mapping (in-memory)
- Token wird im HTML/JS injiziert oder Auth im Demo-Modus komplett deaktiviert (`SPLITTY_NO_AUTH=1`)
- Idle-Timeout tracked und Container killt

### Isolation

- Jeder Container in eigenem Network
- Nur über Gateway erreichbar
- Security-Hardening (no-new-privileges, cap_drop ALL, mem/pid limits)

## Verworfene Alternativen

| Ansatz | Problem |
|--------|---------|
| v86 (Linux im Browser) | Networking-Brücke zwischen emuliertem TCP und Browser-WebSocket zu aufwändig |
| WebContainers | Kein Go, kein echtes PTY, proprietär/closed-source |
| Play with Docker | Zu viel Anpassungsaufwand für Splitty-spezifischen Use Case |
| Fertige Plattformen (Coder, Gitpod) | Für diesen Umfang zu groß |

## Kontext

Erstmal nur als Idee festgehalten.
