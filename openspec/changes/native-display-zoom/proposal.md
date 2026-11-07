# Proposal

## Why

The native workspace ignores the existing persisted zoom setting and cannot enlarge terminal text or its controls through the established shortcuts. Honoring both zoom scopes makes the local client usable at different display densities while keeping settings authority and presentation geometry separate.

## What Changes

- Apply the existing `appearance.zoom` and `appearance.zoom_scope` to the whole native workspace or only its terminals.
- Handle the established Ctrl/Cmd zoom shortcuts before terminal input, persist minimal local-service updates, and report confirmed or uncertain outcomes.
- Give native terminal rendering and hit testing one coherent metrics value, preserving live sessions, retained output and hidden-pane visibility rules.
- Scale native controls and their hit targets together without overriding the display's DPI.

Out of scope: new terminal font settings, reinterpreting chrome font fields, porting xterm-specific options, changing the web client, remote UI, native OS dialog scaling, packaging and merge CI. This is another increment of the native local-desktop evaluation; no separate roadmap milestone is introduced.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `display-zoom`: preserve existing shortcuts and persisted settings while defining native whole-UI scaling, ordered local mutations and coherent terminal geometry.

## Impact

The UI-free client layer gains pure zoom-step policy and bounded mutation jobs on its existing owned auxiliary API stream. GPUI projects confirmed zoom into chrome and terminal metrics. Existing `home.get`, `home.update` and settings notifications are reused without wire or persisted-format changes. The web implementation remains the behavior reference; this does not introduce a browser-portable Rust/WASM boundary or imply a product cutover.
