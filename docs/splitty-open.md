# `splitty open` — Dateien direkt in Splitty anzeigen

> Konzept für die spätere interne Vorschau. `splitty open` öffnet zunächst nach lokaler Bestätigung im externen Editor (siehe README und Change `cli-editor-open`). Die Vorschau muss später als explizites Ziel, etwa `--in splitty`, in denselben Einstieg integriert werden.

## Konzept

Ein CLI-Befehl `splitty open <file>` der beliebige unterstützte Dateiformate direkt in Splitty rendert — PDF, Webseiten, Bilder, etc. Funktioniert als natürlicher Entry Point für das Plugin-System: jedes Plugin registriert seine unterstützten MIME-Types, und `splitty open` dispatcht automatisch an den passenden Renderer.

```
$ splitty open report.pdf        # PDF-Viewer als Pane
$ splitty open preview.html      # Web-Renderer als Pane
$ splitty open photo.png         # Image-Viewer als Pane
```

## Design-Entscheidungen

### Default: neuer Pane (nicht Overlay)

Ein `splitty open` öffnet standardmäßig einen neuen Pane/Tab. Das ist konsistent mit der bestehenden UI — man kann den Pane normal schließen, splitten und resizen, ohne neue UX-Konzepte einzuführen.

### Optional: Overlay-Modus für schnelle Previews

Über `--float` / `--overlay` kann eine Datei on top of the current Pane geöffnet werden. Schließen per `Escape` oder `q` — UI-Konventionen statt Shell-Konventionen, da es sich um einen Viewer handelt, nicht um ein Terminal. `Ctrl+C` wäre hier verwirrend.

### Plugin-basierte Renderer

Jedes Plugin registriert seine unterstützten Formate (MIME-Types). `splitty open` dispatcht automatisch an den passenden Renderer. Damit ist das System beliebig erweiterbar, ohne dass Splitty selbst jeden Dateityp kennen muss.

## Abhängigkeiten

- Plugin-System
- WebView-Infrastruktur (für HTML/Web-basierte Renderer)
- Pane-System muss Non-Terminal-Content-Types unterstützen
