# Native client fixes

## Why

The native window departed from the web client and from its specs in five places: an editor request for a file opened a folder URI, and SSH targets were checked less strictly; an offered file's name kept direction characters; on Windows a local pane got Windows paths its Linux shell cannot use; a review went to a terminal with its final line break, without the bracketed-paste check, and was marked as handed over before its file was stored; and settings edits were held to built-in limits instead of the server's.

## What Changes

- Editor and SFTP URIs are built in client-core by the web client's rules, checked against the shared vectors in `testdata/`; a file is handed over as a file.
- Offered file names lose control and bidirectional formatting characters in the client too; current servers already refuse such paths.
- Files pasted or dropped into a local pane are inserted by their own paths only where the local server reads this app's paths; a local server in WSL gets them stored, as a paste to a host's pane does.
- A review sent to a terminal follows the web client's policy, and a review stored as a file is handed over only once the store succeeded.
- A settings edit is checked against the server's metadata; the former built-in limits apply only to a server without metadata.

## Capabilities

### New Capabilities

### Modified Capabilities
- `terminal-sessions`: files into a local pane (added requirement); an offered name with direction characters (Saving An Offered File).
- `viewer-annotations`: a review stored as a file is handed over once stored (Handing A Review To A Terminal).
- `settings`: edits follow the server's limits, and built-in ones only without metadata (Server Settings and Metadata).

## Impact

`splitty-client-core`: `native_workspace/{editor_uri,files}.rs`, `annotations.rs`, `settings.rs`, `appearance.rs`. `splitty-gpui`: the editor notices and terminal menu, file paste and drop, the viewer's review handover, the terminal engine's bracketed-paste query, the settings editor. `splitty-server`: a router test only. No protocol change.

## Architecture impact

none (new modules stay inside `native_workspace`; `docs/architecture.md` names the settings fallback)

## Decisions

- **NCF-1 (provisional, tbd):** files for a local pane whose server runs in WSL are stored on that server (temporary storage, as a paste) without a question, since they do not leave this computer. The alternative is translating the Windows path through the backend (`wslpath`), which needs a new backend operation; it would avoid the copy and the 512 MiB limit.
- **NCF-2 (provisional, tbd):** without settings metadata the native window edits only the fields every server knows, within the built-in limits; fields that need the server's default to show that it knows them (font family, switches, tab bar options, the terminal palette per mode, hosts to reconnect) stay disabled. The web client offers every field and leaves a refusal to the server.
