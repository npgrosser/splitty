# Native GPUI development app

The development app opens an isolated workspace with real service-owned terminal
sessions, local Screens, split panes, tab groups and draggable dividers. Layouts
come from the shared mux tree; terminal views survive layout updates and Screen
switches.
Closing the window ends its owned service. Saved production workspaces and the
released Electron/Tauri apps remain separate.

The current terminal uses statically linked `libghostty-vt` 1.0.0 through a
UI-independent owned engine. The [terminal build instructions](development.md#native-terminal-library-for-gpui)
describe its pinned native dependency. GPUI handles input and drawing, caching
shaped content for unchanged visible rows. This branch evaluates a native client
alongside the working web client; replacement and long-term coexistence remain
open product choices.

`--probe` retains the original terminal/Markdown fixture, including the optional
CEF experiment. The [phase-0 evidence](#phase-0-evidence-and-limits-historical)
below describes the earlier Alacritty executable, not current performance.

## Build and run

The app pins [GPUI Kit 0.9.0](https://docs.rs/crate/gpui-kit/0.9.0/source/Cargo.toml)
(which pins `gpui-pre` 0.3.11) and
[`libghostty-vt` 1.0.0](https://github.com/ghostty-org/ghostty/releases/tag/vt-1.0.0).
The optional offscreen experiment pins the
[cef 156.3.0 crate](https://docs.rs/crate/cef/156.3.0+156.0.15).

On Ubuntu, install Rust stable, a C/C++ build toolchain, `pkg-config`, and the
Wayland, X11, keyboard, font and Vulkan development packages. For example:

```sh
sudo apt install build-essential pkg-config libwayland-dev libx11-dev libxcb1-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libfontconfig-dev libfreetype-dev \
  libegl-dev libvulkan-dev mesa-vulkan-drivers fonts-dejavu-core \
  fonts-noto-cjk fonts-noto-color-emoji
```

[GPUI Kit's Linux installation guide](https://gpui-kit.com/docs/installation/)
lists additional packages for its full feature set. Use its current package
list if the build enables more Kit features or reports a missing system header.

If system package installation is unavailable, Ubuntu packages can be unpacked
under a user-owned cache. This example supplies the keyboard and Vulkan files
missing from a host that already has the other development packages:

```sh
deps="${XDG_CACHE_HOME:-$HOME/.cache}/splitty-gpui-deps"
mkdir -p "$deps/debs" "$deps/prefix"
(cd "$deps/debs" && apt download libxkbcommon-x11-dev libxkbcommon-x11-0 \
  libxcb-xkb-dev libxcb-xkb1 libvulkan-dev)
for package in "$deps"/debs/*.deb; do dpkg-deb -x "$package" "$deps/prefix"; done
export PKG_CONFIG_PATH="$deps/prefix/usr/lib/x86_64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export CPATH="$deps/prefix/usr/include${CPATH:+:$CPATH}"
export LIBRARY_PATH="$deps/prefix/usr/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
export LD_LIBRARY_PATH="$deps/prefix/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
```

On a clean checkout, first build the existing frontend assets as described in
[development](development.md); the reused service crate still embeds them:

```sh
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend build
```

Prepare the pinned native terminal library using the explicit setup described in
[development](development.md#native-terminal-library-for-gpui). Keep
`GHOSTTY_VT_DIR` set to that installation prefix. Then run from the repository
root in a graphical Wayland or X11 session; use an optimized build to assess
interactive behavior:

```sh
cargo run --release -p splitty-gpui
cargo run --release -p splitty-gpui -- --workspace-smoke
cargo run --release -p splitty-gpui -- --probe
cargo run --release -p splitty-gpui -- --smoke
```

### Native Windows preview

Build the GUI with native Windows Rust/MSVC and the Windows Ghostty prefix from
[development](development.md#native-terminal-library-for-gpui). Build the matching
`splitty-server` binary in an existing WSL distribution. The window runs on Windows;
its owned service and terminal processes run in that named distribution.

```powershell
cargo build --locked --release -p splitty-gpui --no-default-features
& .\target\release\splitty-gpui.exe `
  --wsl-distro 'Ubuntu' `
  --wsl-backend '/absolute/path/to/preview/splitty' `
  --wsl-config-dir '/absolute/path/to/preview/config'
```

All three arguments are required. Use a separate preview configuration directory;
the process passes it directly to its owned backend. Startup does not install WSL,
choose a default distribution, discover a daemon or open a TCP listener. A local
preview package includes a launcher with those paths already filled in; keep its
matching backend at that WSL path. This is an evaluation build, not an installer.

Add `--workspace-smoke` for the real-window workflow described below. Normal
shutdown joins the two native streams and the owned helper. Failed or forced
helper cleanup is reported as an error; it is never treated as proof that Linux
sessions closed gracefully. The app never shuts down the distribution. Windows
input methods, clipboard combinations and interactive smoothness need platform
testing independently of compilation or the automated workspace smoke.

The Windows acceptance harness runs from WSL against already-built artifacts:

```sh
python3 scripts/testing/gpui-windows-smoke.py \
  --exe /mnt/c/path/to/splitty-gpui.exe \
  --backend /absolute/path/to/preview/splitty --distro Ubuntu \
  --output /tmp/gpui-win-check --execute
```

It owns a unique configuration, verifies the launched Windows process identity,
exercises the workspace, checks the seeded template library in a separate window,
and verifies helper/session cleanup while an unrelated WSL sentinel survives.
Use a short output path: the Linux control socket must fit the Unix socket path
limit. Screenshot inspection is separate evidence from the runtime log.

The default window offers **Split right**, **Split below**, **New tab** and
**Maximize/Restore** for the focused tab group.
**Float Tab** moves the focused tiled terminal into a floating tab group, leaving
at least one tiled pane. Drag the float header to move it or its lower-right grip
to resize it. **Dock** returns the whole group to the first tiled group, including
an empty root. Floats retain their terminals, history, search and selection;
new tabs and profile choices work within them. **Maximize/Restore** also works
for floating groups. Split controls and tab drag placement are tiled-only.
Clicking a float raises it locally without a layout operation. Geometry previews
commit once on release; Escape, window deactivation and viewport/zoom changes
cancel them. Out-of-viewport stored bounds are displayed within reach without
automatically rewriting their saved values.

**Templates** (Alt+P, Cmd+P on macOS) opens the local server's saved-template
library. Save current Screen first chooses an optional root for relative
directories, then reviews one server extraction: name, command selections,
directories and shell profiles. Saving does not extract again after review.
Overwrite and delete require an explicit choice for the selected file.
Unsupported template structures are reported before saving or application.

Apply chooses the current Screen or a new Screen and an optional root on the
target host. Every command is reviewed before **Run commands**; **Layout only**
omits commands and their restore modes. Replacing an existing Screen ends all
its terminals, including hidden/floating tabs, even for layout-only application.
The replacement review is checked again before submission. A new Screen uses
one apply operation without a throwaway terminal.

The intended layout appears as pending until the server result is established.
Its placeholders cannot receive terminal input or claim a terminal size.
Navigating elsewhere keeps that selection when application finishes. A launch
failure can leave a valid layout and usable sibling terminals; an uncertain
result is reported without replaying the operation. Escape dismisses template
dialogs, and Enter never approves command execution.

The arrow beside a tab group's **+** opens the local shell-profile picker.
Use Up/Down and Enter to choose an enabled, available profile; Escape or an outside
click dismisses it. A regular **+** or new-tab shortcut inherits the source shell
and directory from the service. An explicit choice changes only the profile;
commands and arguments remain service-owned. The picker shows unavailable reasons,
default and experimental markers, and a launch-time fallback is reported in status.
Profiles and appearance load independently over the owned auxiliary API stream;
neither read changes settings. Profile editing is still in the web settings UI.
Select tabs by clicking their labels, close panes with their tab close control,
and resize splits by dragging the divider. Right-click a tab or use its **⋯**
menu to move that running terminal to another local Screen or a **New Screen**.
Opening the menu preserves the selected tab; confirming a move follows its target
unless later navigation intervened. Terminal-body right-click stays available
to terminal applications. After closing the final pane,
**New terminal** repopulates the same Screen. Ctrl+Shift+C/V copies and pastes
in the focused terminal. Wheel input scrolls normal history; in the alternate
screen it sends up/down keys using the emulator's current cursor-key mode, with
at most five keys per event. Drag a terminal tab between header tabs to reorder it
or insert it into another group; the insertion marker shows the exact position.
Tab strips scroll horizontally and reveal keyboard-selected tabs. Dropping onto
a group body or its four edges retains center/edge movement without restarting
the shell; Escape cancels the drag.

Screen controls create, select, rename and close local Screens. Hidden Screens
keep their terminal output and scrollback. Closing the last Screen leaves
**New screen**; closing a Screen ends its sessions. Pane, Screen and normal window
close share one confirmation flow: busy sessions or incomplete session state
require confirmation, and cancelling preserves the work. Closing the owned window includes
terminals on hidden Screens. If the affected sessions change while a decision is
pending, request close again. Remote connections, saved workspace restoration, scratchpad and
`splitty open` integration remain subsequent work.

The Screens Drawer exposes the catalog, creation, rename, assessed close and
Templates. Its toolbar toggle saves visibility through the local settings
service; a new window restores that choice and the saved sidebar width.
Vertical wheel input over the Drawer header or current Screen name moves through
Screens. Over a tab header it selects a tab in that group, even when another
group has keyboard focus. Both stop at the ends. The Drawer list scrolls normally;
horizontal input and Shift+mouse-wheel browse overflowing tabs. Editing, close
prompts and dragging take priority over this navigation. Terminal wheel handling
stays inside the terminal view.

Each tab group has a **+** button for a new terminal and a profile picker beside
it. Split right/below, float/dock and maximize/restore controls act on that group,
including when a different terminal has keyboard focus. Narrow groups collect
the secondary actions in an icon menu. Screen navigation also has previous/next
buttons beside the current Screen name. The pinned GPUI Kit supplies the button
styling and icon assets; the canonical mux tree remains the only layout model.

**Hosts** shows known targets and aliases with their cached Screen names, pane
counts and last observation. It distinguishes an observed empty list from a host
with no Screen observation. These rows are informational; opening the view or
using **Refresh** does not connect to a host. A failed refresh keeps the last
valid directory and offers **Retry**. Use **R** to refresh and **Escape** or
**Screens** to return to the local workspace with terminal focus restored.

Workspace shortcuts use **Cmd** on macOS and **Alt** on other hosts as `Mod`:

| Shortcut | Action |
| --- | --- |
| Mod+T | New tab (or terminal on an empty Screen) |
| Mod+D / Mod+Shift+D | Split right / below |
| Mod+W | Close the focused pane |
| Mod+M | Maximize or restore the focused tab group |
| Mod+PageUp / Mod+PageDown | Previous / next tab of the focused group, stopping at its ends |
| Ctrl+Shift+Space | Create a local Screen, including from an empty workspace |
| Mod+Shift+1–9 | Select the corresponding local Screen |
| Mod+Shift+Left/Right/Up/Down | Focus the nearest visible split group in that direction |

These chords are handled before terminal input; Ctrl+C and clipboard keys keep
their terminal behavior. Rename editors and close prompts keep keyboard priority.
Tab selection remains available by clicking its label. On the pinned Linux
backend, shifted Screen numbers are verified for standard US and German XKB
layouts. The framework drops physical key codes; symbols from other layouts pass
through to the terminal rather than selecting a guessed Screen. Pointer Screen
selection remains available.

Ctrl+F (Cmd+F on macOS) opens find in the focused terminal. Enter searches toward
older matches; Shift+Enter goes toward newer matches; both wrap. Escape closes
find and returns typing to the terminal. Search is literal and ASCII
case-insensitive; other characters match exactly. It covers the retained screen
and history, including soft-wrapped text. Search highlights do not replace text
selection or change what Copy copies.

Hold Ctrl (Cmd on macOS) and click a terminal link to request the system browser.
Explicit OSC 8 links use their actual target, which can differ from the printed
label. Ordinary absolute HTTP(S) URLs are detected across soft wraps, including
retained output. Ctrl/Cmd-hover feedback shows the allowed target. Shift keeps local
selection; ordinary application mouse input is unchanged. Dragging, scrolling,
changing the target or leaving its cell before release cancels activation.
Only absolute HTTP(S) targets are accepted; editor and file schemes are not
generic terminal links. This local preview does not establish remote forwards.
The pinned terminal parser discards oversized OSC 8 commands (a 2,048-byte
fixed capture buffer). Their labels can still be ordinary detected URLs; no
shortened explicit target is opened.

Each terminal remembers its query and whether find is open. Only the focused
visible pane keeps an active index; changing tabs or Screens rebuilds it when
that pane returns. The query is limited to 1,024 UTF-8 bytes, native search
allocations to 16 MiB, and the active screen buffer to 20,000 matches. A limit
stops search with a refine-query message instead of showing partial results as
complete. Ordinary output does not retry a failed query. These limits do not
promise a fixed duration for each library call.

Maximization is a per-window, per-Screen reference to a canonical tab group.
It preserves tabs, terminal history and hidden output without changing the server
layout. New tabs and reorder within that group retain it; admitted splits and
cross-group moves restore the full view.
Hidden groups release visible PTY size claims, and restoration reports the
visible terminals' measured sizes. Screen and window close still include hidden
groups.

`--workspace-smoke` exercises distinct terminal sessions, both split axes, tab
switching, authoritative divider ratios, pane closure, keyboard routing and
preserved terminal identity. It also creates, switches, renames and closes local
Screens, checks hidden output and rename-editor focus, and creates a fresh Screen
after final closure. It requires fresh visible paints and joined service cleanup.
The smoke driver selects this workflow with `--workspace`; omit it for the legacy
fixture.

`--smoke` runs bounded terminal output, loads the Markdown fixture, creates an
annotation, reconnects to the same service and checks the rendered pane. It
prints one JSON result and exits with an error when its checks do not pass.
Interactive use lets you select Markdown text and click **Annotate selection**. Closing a window
with annotations asks whether to end the probe because this isolated service
keeps annotations only for that run. The terminal supports Ctrl+Shift+C/V for
copy and paste.

For a repeatable real-window run, pass the built binary to the smoke driver.
It writes stdout, stderr and `summary.json` for each backend. The default
`evidenceKind` is `runtime-log`; it does not claim a visual check. `both`
requires both display variables to be available;
an unavailable backend fails the run. The driver gives each run 45 seconds and
stops only the launched process group on timeout.

```sh
cargo build -p splitty-gpui
python3 scripts/testing/gpui-smoke.py --binary target/debug/splitty-gpui \
  --evidence-dir /tmp/splitty-gpui-evidence --backend both
```

The driver also accepts `--screenshot` for a binary built with `--features capture`
whose backend implements native capture. That mode requires a fresh nonempty PNG and labels successful
capture evidence `runtime-log-and-screenshot`. The pinned GPUI Linux backend does not implement
native `render_to_image`; capture Linux windows externally for visual review.

The optional CEF proof uses a fixed local HTML fixture and CPU offscreen frames
inside a GPUI pane, with a native overlay. It has no Splitty service handle or
command bridge. The `cef` dependency enables its sandbox feature; the probe
does not disable the sandbox at runtime. Its first build may download a CEF
distribution and needs CMake and a C/C++ build toolchain.

```sh
cargo run -p splitty-gpui --features web-spike -- --web-spike --smoke
```

For the optional CEF run, build with `--features web-spike` and pass
`--web-spike` to the smoke driver. The driver does not change CEF sandbox
settings. When launching the built CEF-enabled binary directly on Linux, its
loader must also find the downloaded `libcef.so` next to the binary, for example
with `LD_LIBRARY_PATH="$PWD/target/debug${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"`.
`cargo run` supplies the build library search paths automatically.

## Local appearance

The workspace reads the local service's saved appearance and resolved theme using
`home.get` and `theme.get`. These reads use one separate owned API stream, so an
API reply cannot consume terminal session events. Fixed themes use the selected
name; automatic mode requests the configured light or dark theme for the local
OS preference. Same-service settings changes and OS preference changes coalesce
into refreshes. Separate embedded service instances cache their own settings, so
an edit through another app instance becomes visible after restarting this
native workspace; cross-process live synchronization is not implemented.

Appearance font size and family apply to workspace chrome, matching the web
client. Empty/zero overrides use resolved UI theme defaults. The supported size
range is 8–24 px; a CSS font list selects an available concrete family or a native
UI generic. Terminal typography uses the existing monospace font at a 15 px
baseline with 21 px rows. A proportional UI font therefore does not alter
terminal cells. Display zoom scales that baseline; general terminal-font
settings remain separate work.

Native colors support checked `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`, decimal
`rgb(...)`/`rgba(...)` with comma-separated channels, and `transparent`. Alpha is
composed against the selected background. The terminal receives foreground,
background, cursor, selection and ANSI 0–15 colors; its standard indices 16–255
remain unchanged. Native cursor drawing uses an overlay, so `cursor-accent`
text inversion is not implemented. CSS expressions, layout/effect tokens and
the otherwise unused `terminal.theme` field are not activated by this adapter.
Unsupported color/font forms use native fallbacks with a status; failed reads
retain working defaults or the last valid appearance. Appearance refreshes do
not submit settings mutations; explicit zoom shortcuts use the same owned API
client for bounded ordered updates. Existing service startup may still persist
configuration defaults or migrations through the ordinary settings loader.

There is no native settings editor in this increment. The web client's settings
remain the reference interface; the native read path and terminal engine consume
the existing settings and theme contracts.

Native display zoom uses the saved `appearance.zoom` and `zoom_scope` settings.
Ctrl on Linux, or Cmd on macOS, with `+`/`=` and `-` selects the existing zoom
steps from 50 to 300 percent; `0` resets to 100 percent. These shortcuts are
handled before terminal and editor input. Window scope scales terminal content
and application controls together; terminal scope leaves chrome at its usual
size. System dialogs keep platform scaling. Zoom preserves running processes,
history and per-pane search text, including hidden panes.
The styled GPUI Kit Root applies chrome scaling from the component theme's base
font size. UI controls and terminal cells retain separate scaling paths.

Each shortcut requests one ordered settings action from the local client core.
The view shows the confirmed result; failures and uncertain saves are reported
without retrying the action. The queue admits up to 32 waiting actions beside
one running job and reports overflow. Each action reads current settings and
patches only zoom, preserving unrelated fields. This is not an atomic increment
across separate clients. The app remembers confirmed zoom through the existing
settings file; no terminal-font setting or OS DPI override is introduced.

The focused appearance checks pass: 45 client-core tests, 36 GPUI tests, 23 core
theme tests and 17 architecture checks, plus touched-target Clippy, formatting
and strict OpenSpec validation. In-memory service tests exercise initial and
later read failures while a real PTY continues producing output, recovery after
same-service settings events, OS theme choice, foreign-source rejection,
superseded reads and shutdown during setup. A setup failure during close remains
an optional appearance failure; actual stream/service cleanup errors still
propagate.

Four isolated X11 startup fixtures cover bundled dark/light themes, an 18 px
proportional chrome override and unsupported authored color/font values. All
retain shell identity and history through splits, tabs and maximization, and exit
normally without surviving children. The measured chrome ink height grows from
9 to 13 pixels; the same terminal marker remains 129×11 pixels with 21-pixel row
spacing. The fallback capture shows its persistent status alongside ordinary
workspace status. These pixel measurements validate the exercised font
installation, not universal font metrics. Live settings/OS refresh is verified
in-process; the window matrix uses separate startup fixtures and does not claim
cross-process synchronization. Hardware workspace smokes also pass under Wayland
and X11 with D3D12-backed OpenGL.

```sh
python3 scripts/testing/gpui-screens.py target/release/splitty-gpui \
  --output /tmp/splitty-appearance-evidence --appearance-matrix
```

## Native display-zoom acceptance

The accepted optimized binary has SHA-256
`25ea7af25a282901d144594af97d77a87ac28ed8c1fef1a5849ecd3bd0f685b7`.
The private X11 matrix covers both scopes at 50, 125 and 300 percent, plus two
restarts using the settings saved by real keyboard actions. Observed terminal
row pitches are 11, 26 and 63 pixels in both scopes. Terminal-only chrome stays
at the same 79-pixel content origin; window scope moves it to 40, 98 and 235
pixels. Glyph ink heights agree between scopes at each level.

Rapid zoom actions and Ctrl+0 produce no bytes in any owned PTY. Existing and
hidden tabs keep their process identity, query, history and background output;
hidden PTYs report no new size until revealed. Scaled OSC8 targets reach a private
opener spy, application mouse reports use scaled rows, and the find editor
retains its text. At 300 percent, the profile menu fits the viewport and the
toolbar scrolls to its controls. Zoom during a held divider gesture preserves
the settled split ratio after further movement and release. The tests wait for
rendered geometry as well as persistence: a saved setting can precede its UI
acknowledgement. Screenshots were inspected for retained history, both chrome
scopes, the menu, toolbar and split geometry. All eight runs exit normally with
no surviving children.

The same frozen binary passes the hardware workspace smoke under Wayland and
X11, reaching stage 18 with D3D12-backed graphics. Targeted checks include 72
client-core tests, 67 GPUI tests, all-target Clippy for both crates and 17
architecture checks. After the high-zoom toolbar/footer correction, all 14
workspace tests and native Clippy pass again. The interactive matrix uses a
private 1200×760 Linux Xvfb window; it does not establish macOS/Windows behavior,
manual IME composition, arbitrary window sizes or new latency measurements.
Full-workspace merge CI remains deferred for this evaluation branch.

```sh
python3 scripts/testing/gpui-display-zoom.py target/release/splitty-gpui \
  --output /tmp/splitty-zoom-evidence --matrix --xclip /path/to/xclip
```

The runner accepts private Xvfb, library-directory and xdotool overrides. It
records binary, harness and fixture hashes, exact PTY input, persisted settings,
process lifetimes and screenshots. Its opener and clipboard checks stay inside
the owned display and generated configuration.

## WSLg graphics

The default Mesa selection in the tested WSLg environment used CPU llvmpipe.
The same optimized application selected the hardware D3D12-backed OpenGL device
and passed its local-workspace smoke on both Wayland and X11 with these
**per-process** settings:

```sh
GALLIUM_DRIVER=d3d12 MESA_D3D12_DEFAULT_ADAPTER_NAME=NVIDIA \
  cargo run --release -p splitty-gpui
```

This is a host-specific development option, not an application default. Use the
adapter-name substring appropriate for the installed hardware. The native app
prints a `nativeGraphics` JSON diagnostic with the selected device and
`software` flag; the smoke driver records it. Check that result rather than
assuming `vulkaninfo` describes the selected backend. In this run, the selected
device was `D3D12 (NVIDIA GeForce RTX 4080)` with `software: false` and Mesa
26.0.8. No system or driver files were changed. See
[Microsoft's WSLg GPU selection guide](https://github.com/microsoft/wslg/wiki/GPU-selection-in-WSLg)
for the D3D12 adapter setting.

The matched performance table below deliberately retains its original software
renderer conditions. Hardware selection and functional smoke establish a usable
accelerated path; they do not establish new latency numbers. WSLg presentation
still adds a host-window transport, and a native Windows UI remains the target
Windows topology, with server and shells in WSL.

## Initial local workspace acceptance

For the initial single-Screen increment, repository checks passed: 1,247 workspace Rust tests (including the architecture
model), formatting, workspace Clippy with warnings denied, optional CEF Clippy
and all 19 main specifications. The 20 GPUI tests include actual Ghostty engine
behavior, render-cache invalidation, divider math and shared command admission
with complete paste framing and stale-generation rejection.

The local-workspace runtime smoke passed on Wayland and X11 with four distinct
session markers, both split axes, tab selection, authoritative resize ratios,
pane closure, retained original terminal identity, current visible paints and
joined service cleanup. The legacy reconnect/Markdown smoke also passed on both
backends with CEF disabled and enabled.

An independent X11 interaction run clicked the actual controls and typed shell
commands into the resulting terminals. It verified five distinct shell process
IDs, returning to an existing tab's original shell, dragging a divider from
599 to 780 pixels, closing hidden tabs, closing the final pane and opening a new
shell in the same empty Screen. The process exited normally with no surviving
children. External captures show [both split axes](images/gpui-workspace-splits.png)
and [ASCII, CJK, combining accents and emoji](images/gpui-workspace-unicode.png).
These are native window captures, separate from runtime-log smoke evidence.
Manual IME, cross-application clipboard, accessibility and other desktop
platforms still need target-specific acceptance.

## Multiple local Screens acceptance

The multi-Screen increment passes the focused client-core and GPUI tests, all
architecture checks, formatting, touched-crate Clippy with warnings denied and
OpenSpec validation. Broad release/CI checks are deferred for this development
increment. After moving the workspace worker into the native client runtime,
the optimized window passed the extended workspace smoke on Wayland and X11
with the hardware D3D12-backed OpenGL device described above. Both runs retained
hidden output and rename-editor focus and completed owned-service shutdown.

`scripts/testing/gpui-screens.py` drives the normal window through real X11 mouse
and keyboard events on its own Xvfb display. Its 12 checks create distinct
shells, rename with Enter and cancel with Escape, preserve shell PID and state
across switches, return to hidden output, cancel and confirm a busy Screen close,
close the final Screen and start a fresh shell from the empty workspace. It also
checks normal window closure and absence of surviving children. External captures
were reviewed separately from the runtime-log smoke. This is functional evidence,
not a latency benchmark or acceptance on other desktop platforms.

```sh
python3 scripts/testing/gpui-screens.py target/release/splitty-gpui \
  --output /tmp/splitty-screens-evidence --busy-close
```

The script requires Xvfb and xdotool; their executable paths and the isolated
display can be selected with its command-line options.

The alternate-screen wheel follow-up passes focused engine/input tests and GPUI
Clippy. An isolated X11 run used raw PTY fixtures: normal-history wheel input
changed viewport pixels and delivered no PTY bytes, while alternate-screen wheel
input delivered both up and down application-mode cursor sequences. The optimized
integrated binary also passed the Wayland/X11 hardware workspace smoke and exited
cleanly. This verifies the cursor-key fallback. Application mouse reporting is
covered separately below.

## Native terminal-link acceptance

Link interpretation stays in the terminal adapter, parsed HTTP(S) admission in
the UI-free client layer, and pointer gestures/platform opening in the view.
The adapter returns owned targets, including soft-wrapped text and wide-cell
hits, and retains no native grid references. It bounds complete logical-line
inspection to 16 KiB, 128 rows and 80,000 cells; exceeding a bound yields no
detected target. Explicit metadata retained by the terminal takes precedence.
The pinned parser's oversized OSC 8 behavior is described above.

The isolated X11 driver records exact PTY bytes and substitutes successful
process-local opener spies, including the WSL browser launcher. It never sends
test URLs to a real browser. The driver exercises actual pointer dispatch and
the platform handoff, rather than invoking a product test entry point:

```sh
python3 scripts/testing/gpui-terminal-links.py target/release/splitty-gpui \
  --output /tmp/splitty-link-evidence --display :95 \
  --xvfb /path/to/Xvfb --xdotool /path/to/xdotool
```

The frozen binary SHA-256
`52be3461770d5d52deb8defb8bdc4ab057a78848dc7ad858760623e01a9cc3dc`
passed all 12 window checks: no automatic opening, actual-target preview,
OSC 8/plain/wrapped activation, rejected schemes, changed-target and pointer
cancellation, modifier-release handling, application mouse reports and exact
Shift-selection copy/paste. The same binary passed the Wayland/X11 hardware
workspace smoke with joined cleanup. Screenshots confirm the actual-target
preview and selection. An initial candidate exposed a header exit/reentry bug;
the fixed view observes captured movement at window scope, including outside
the terminal element, while leaving its gesture local until completion.

The 64 client-core, 63 native and 17 architecture tests passed, as did affected
crates' all-target Clippy, formatting and the C ABI assertions. Link input
acceptance is X11-only; Wayland evidence covers the workspace/GPU workflow.
No macOS/Windows native interaction or new whole-window latency measurement is
claimed. The opener spy establishes the submitted target, not successful startup
of a particular installed browser. These checks do not establish remote forwarding.

## Native shell-profile acceptance

The new-tab arrow reads the local service's existing shell profiles. Ordinary
tabs and splits send no CWD/profile override; explicit selection sends only the
profile ID. The client admits at most 128 profiles with 256-byte IDs and 512-byte
names/reasons, retaining the charged snapshot through presentation. Oversized
reads fail atomically and leave the last valid choices available. Appearance and
profiles use independent generations on one auxiliary API stream.

The isolated window check creates private configuration and raw PTY profile
processes. It verifies explicit program/argument selection, plain tab/split
inheritance, unavailable choices, Escape/outside dismissal, terminal output,
keyboard and mouse-reporting isolation, and owned-process cleanup:

```sh
python3 scripts/testing/gpui-shell-profiles.py target/release/splitty-gpui \
  --output /tmp/splitty-profile-evidence --display :95 \
  --xvfb /path/to/Xvfb --xdotool /path/to/xdotool
```

The frozen candidate SHA-256
`f7ff32f4c8b286ebf538305533614a22a3a7d7c38320580ce0118d38566e76ec`
passed nine X11 window checks and the Wayland/X11 hardware workspace smoke.
The picker screenshots also confirm default, experimental and unavailable
presentation. In-process tests cover newer service CWD versus a stale client
snapshot, explicit profile precedence, availability-race fallback and rejection
of stale connection/Screen/pane/session captures. The window check does not force
an external source mutation while the picker stays open, and is not a latency
measurement. The 60 client-core tests use `--test-threads=4` to keep concurrent
real-service fixtures within the existing process-wide transport admission bound;
the production bound is unchanged. The 53 native and 17 architecture tests and
both affected crates' all-target Clippy checks also passed.

## Native terminal search

The find bar uses the pinned terminal search API. It searches the complete
retained buffer incrementally, without its synchronous full-history entry point.
Each scheduled turn performs at most one feed and one history tick; completed
searches wake only on a relevant change. Native calls still have variable cost.

A standalone C probe on the pinned static library used 5,000 lines and the
native adapter's 32 MiB/5,000-line retention settings. Sparse output contained
50 matches; dense output matched every occupied cell. These are isolated library
measurements, not window latency measurements:

| Case | Uncapped matches | Uncapped process peak | 16 MiB search budget |
| --- | ---: | ---: | --- |
| Sparse, 80×24 | 50 | 11.6 MiB | Completes; maximum feed 1.1 ms |
| Dense, 80×24 | 395,000 | 168.4 MiB | Stops at limit; process peak 22.5 MiB |
| Sparse, 400×200 | 50 | 38.1 MiB | Completes; maximum feed 2.8 ms |
| Dense, 400×200 | 1,995,000 | 813.6 MiB | Stops at limit; process peak 34.5 MiB |
| Dense, 400×200 after unobserved output | 2,034,900 | 842.0 MiB | Stops at limit; maximum feed 13.7 ms |

The uncapped hidden-output catch-up ranged from 526 to 821 ms in these runs.
With the budget, disposing the index took at most 0.75 ms and all charged search
allocations were freed. The native API returned success even when its custom
allocator refused memory, which is why the adapter must check its sticky failure
flag after every call. Memory figures use the current process's `/proc` VmHWM;
an inherited `getrusage` high-water value was unsuitable for this probe. The
budget covers search-owned storage, not the entire terminal or application.

The actual-window search driver uses an isolated display and a raw PTY fixture.
It records exact input bytes and window pixels, checking that find controls stay
local, results include history/soft wraps/Unicode, output refreshes counts, and
tab/Screen activation retains the query. Dense-query failure, refinement,
independent clipboard selection and VT reset are separate stages. Tesseract with
English data reads visible status labels; screenshots remain available for
review. The fixture never calls a product testing API.

```sh
python3 scripts/testing/gpui-terminal-search.py target/release/splitty-gpui \
  --output /tmp/splitty-search-evidence --display :95 \
  --tesseract "$(command -v tesseract)" --xclip "$(command -v xclip)" \
  --ocr-advisory
```

Xvfb, xdotool and xclip are required. Their paths, private library directory and
the OCR data directory can be supplied through the driver's options. The
clipboard belongs to the private display. `--ocr-advisory` continues byte/focus
checks after an OCR mismatch and records `visualReviewRequired`; those status
images require manual review even if the automated interaction checks pass.
Omit it for strict OCR assertions. `--setup-only` checks fixture startup and does
not establish search acceptance.

The final search candidate, SHA-256
`98db7d45534fafb750daccd62d67685e225637e8167daa58f6727b5c885fe0d1`,
passed 16 actual-window check groups, including exact Unicode clipboard input,
query restoration, output counts, VT reset, independent selection copy and
activation through an inactive visible split's find bar. A terminal-origin mouse
press now releases over the bar and the next motion reports an unheld hover;
the earlier candidate reproduced the missing-release failure. Search-control
presses deliver no PTY input. The dense-query limit label was reviewed in the
captured image because OCR missed it; automated interaction checks and that
manual visual check are distinct. The app exited normally with no surviving
owned children.

The implementation passed 53 native tests, native all-target Clippy, formatting
and all 17 architecture checks. The same final binary passed hardware-accelerated
Wayland and X11 workspace smoke checks. Search interaction evidence is X11;
those Wayland checks cover the workspace and graphics path, not find input.

## Native terminal pointer input acceptance

`gpui-pointer-input.py` drives a normal native window on a private Xvfb display.
Its companion raw-mode PTY fixture records the exact bytes sent to a terminal
application. The driver checks SGR press/release for left, middle, right and
navigation buttons, button and all-motion tracking, vertical/horizontal wheel
reports, high-coordinate legacy bytes, release outside the terminal,
Shift's local selection override during both kinds of gesture, private-display
copy/paste and tracking-off behavior. It saves per-stage byte traces and
screenshots, then requires a normal app exit with no surviving children.
The optional `--focus-loss` stage moves X focus to the private display's root
while an application press is held. It requires a release before physical
mouse-up, no duplicate release afterward, and a working gesture on return.

```sh
python3 scripts/testing/gpui-pointer-input.py target/release/splitty-gpui \
  --output /tmp/splitty-pointer-evidence --display :95 \
  --xvfb /path/to/Xvfb --xdotool /path/to/xdotool --focus-loss
```

The runner requires Xvfb, xdotool, libX11 and libXtst. `--xvfb-lib` can point
to a private Xvfb library directory. It creates an isolated app configuration,
server socket, PTY fixture and X11 clipboard; it never uses the desktop session.
`--setup-only` checks fixture startup before pointer support is available.
Linux X11 is the real-window acceptance target here. Wayland, macOS and Windows
pointer behavior, desktop-wide clipboard interaction and accessibility remain
separate checks.

On the pointer candidate with SHA-256
`1d983c7fc1cb89c49efbad8a8aacd501fa50df35dbedea479773df37f2c5be57`,
the isolated X11 run passed 15 check groups. Its legacy click carried coordinate
byte `0x98` and matched the SGR cell coordinate at the same pixel. Shift selection
produced no application mouse bytes and copied `POINTERMARK4826` back into the
fixture exactly. Focus loss settled the original press before physical mouse-up.
The app exited with status zero and no surviving child processes. These checks
establish X11 pointer routing for the tested modes; the engine's targeted tests
cover the remaining encoding formats and geometry cases.

## Local terminal close acceptance

The shared close flow is checked through model/runtime tests, a native approval
matching test and the existing real-window driver. On isolated X11,
`gpui-screens.py --close-confirmation` passed ten checks: busy pane button and
shortcut cancellation preserve shell identity and focus; repeated gestures keep
one prompt; confirmed pane closure preserves another tab; Screen confirmation
works; and normal window close detects busy hidden terminals, preserves their
output on cancellation and ends owned sessions on confirmation. The final prompt
capture explicitly describes ending the local workspace. Normal exit returned
zero with no surviving children. The previous twelve Screen checks also passed.

Focused checks passed: 32 client-core tests, 31 GPUI tests, 55 transport unit
tests and five transport integration tests. A deterministic transport regression
closes the owner while a valid frame finishes reading: the frame releases its
lease and shutdown succeeds. Malformed frames, foreign-budget transfers and
queue exhaustion remain errors. Targeted Clippy and the architecture check passed.

The optimized native-only build retains passing workspace smokes on Wayland and
X11 with hardware D3D12-backed OpenGL. The closure guard covers the existing owned
local terminal workspace. Forced termination and non-cancellable platform quit
callbacks are outside its confirmation guarantee. Remote detach, scratchpad and
viewer-loss policies remain separate work; the current implementation does not
claim full web-client close-policy parity.

```sh
python3 scripts/testing/gpui-screens.py target/release/splitty-gpui \
  --output /tmp/splitty-close-evidence --close-confirmation
```

## Local group maximization and navigation acceptance

The native viewport borrows the maximized group from the canonical mux snapshot.
One optional group ID per Screen is local view state; no layout rewrite or second
tree is introduced. Output subscriptions survive hiding, while visibility and
PTY size claims follow the displayed groups. A competing-client test exercises
the actual worker: the hidden terminal changes from the workspace's 73×19 claim
to the other client's 120×40 claim, then returns to 73×19 on restore. Delayed
pre-maximize visibility and resize reports cannot reclaim the hidden terminal.

Focused checks pass: 36 client-core tests, 32 GPUI tests, 17 architecture checks,
touched-target Clippy with warnings denied and formatting. The real-window
`gpui-screens.py --navigation` driver passes fourteen checks on isolated X11:
maximize/restore, retained tabs and shell identities, hidden output, Screen
switching, new/direct-Screen keys, unavailable index consumption, editor and modal
priority, busy hidden-group close cancellation, move/split restoration and new
Screen creation from an empty workspace. A raw PTY reader verifies that modal
navigation gestures do not leak input. A separate `--navigation-keys-only` run
applies the actual German XKB layout and checks shifted 1/2/3 and unavailable 9.
Both runs exit normally with no surviving children. The optimized final build
also passes the workspace smoke under hardware D3D12-backed OpenGL on Wayland
and X11.

The keyboard checks cover Linux XKB standard US and German layouts. They do not
establish physical-key support for arbitrary layouts or Windows/macOS parity.
Maximized view choice is not persisted; floating groups and popouts remain
outside this local increment.

```sh
python3 scripts/testing/gpui-screens.py target/release/splitty-gpui \
  --output /tmp/splitty-navigation-evidence --navigation --keyboard-layout us
python3 scripts/testing/gpui-screens.py target/release/splitty-gpui \
  --output /tmp/splitty-german-keys-evidence --navigation-keys-only --keyboard-layout de
```

## Workspace output wakeup measurements

The normal workspace now receives worker updates when they arrive instead of
waiting for its next 16 ms poll. The same task keeps a maintenance deadline for
visibility/resize retries and yields after bounded work; no second message queue
or terminal-emulator change is involved.

An optimized native-only baseline and the wakeup build were compared on isolated
Xvfb under WSL2 with Mesa 26.0.8 llvmpipe (software OpenGL). Each run sent 120
sequence-checked keys to the same raw-PTY fixture, with identical seeded 20–45 ms
pauses and a 320×110 response crop. No builds or other UI tests overlapped the runs.

| Metric | Fixed 16 ms poll | Update-driven receive |
| --- | ---: | ---: |
| Key request to changed X11 pixels, median | 21.82 ms | 17.17 ms |
| 95th percentile | 30.47 ms | 23.47 ms |
| 99th percentile | 38.85 ms | 24.44 ms |
| Samples at least 30 ms | 7 / 120 | 0 / 120 |
| Crop capture cost, median | 0.104 ms | 0.110 ms |
| CPU during 5 s without input, process tree | 2.00% | 2.20% |

This single matched pair supports lower observed response latency in that setup;
it does not establish physical display latency, hardware-GPU performance or a
universal percentile guarantee. CPU uses 100% for one logical core, excluding the
harness and X server. Both runs exited normally without surviving children.

A separate real-window check kept a foreground terminal writer active while
creating another Screen, opening and cancelling a busy-close prompt, and sending
Ctrl-C. Both shell identities survived those actions; final owned shutdown left
no children. The ten close-confirmation checks and 31 GPUI tests also passed.
The final build passed the workspace smoke on Wayland and X11 with hardware
D3D12-backed OpenGL. That smoke waits for the shell's known-idle close assessment
before exercising the no-prompt close shortcut; receiving output alone does not
prove that its later idle metadata has arrived.

The reusable driver records binary, harness and fixture hashes, raw samples,
inter-key delays, screenshots and shutdown evidence:

```sh
python3 scripts/testing/gpui-workspace-performance.py target/release/splitty-gpui \
  --output /tmp/splitty-workspace-latency
```

It requires Xvfb, xdotool and the existing native development dependencies. Use
`--xvfb`, `--xvfb-lib` and `--xdotool` for private dependency installations. The
companion `gpui-latency-fixture.py` runs only inside the test's owned terminal.

## Ghostty integration measurements

On 2026-11-07, the original Alacritty executable at `273f147` and the Ghostty
replacement were built with the optimized release profile and the optional CEF
feature. Both ran the same legacy terminal/Markdown fixture on an isolated Xvfb
X11 display under WSL2. Vulkan selected Mesa 26.0.8 llvmpipe (LLVM 21.1.8), a
software renderer; these are not hardware-GPU measurements. No builds or user
input overlapped the matched runs. Both variants closed normally with no
surviving owned child processes.

| Metric | Baseline, CEF off | Ghostty, CEF off | Baseline, CEF on | Ghostty, CEF on |
| --- | ---: | ---: | ---: | ---: |
| CPU during 5 s without input, process tree | 445.55% | 2.20% | 462.34% | 70.13% |
| Input to changed X11 pixels, median of 12 | 29.60 ms | 29.56 ms | 30.16 ms | 29.05 ms |
| Scroll to changed X11 pixels, median | 15.46 ms | 15.59 ms | 16.55 ms | 15.36 ms |
| Producer completion, 12,000 lines | 0.417 s | 0.403 s | 0.380 s | 0.400 s |
| Observed settling after output | 0.672 s | 0.629 s | 0.611 s | 0.651 s |

CPU uses 100% for one logical core and includes observed app/CEF/service child
processes; it excludes the harness and X server. The CEF-on fixture animates at
10 Hz even without input, so that column does not describe a quiescent browser.
Input/scroll measurements include X server round trips and capture sampling,
not physical display latency. Producer completion includes shell and transport;
settling includes a 150 ms stability interval and is not exact last-frame time.

Removing unconditional window invalidation and reusing shaped rows greatly
reduces the observed idle CPU workload. This run does **not** demonstrate a
material input-latency, scrolling or output-throughput improvement. Those metrics
need repeated measurements on hardware-accelerated target desktops before a
performance claim. The engine replacement and scheduling changes were measured
together, so the CPU difference cannot be attributed to Ghostty alone.

The comparison uses `scripts/testing/gpui-performance.py` with `--modes both`,
12 samples, 12,000 output lines and 5-second idle intervals. Pass
`--app-argument=--probe` for the current executable; the historical executable
opens that fixture by default. The harness records binary hashes, device/profile
labels, per-sample values and normal shutdown. Use a dedicated test display so
other typing or focus changes cannot invalidate the measurements.

## Phase-0 evidence and limits (historical)

| Check | Current evidence |
| --- | --- |
| Repository validation | 1,230 workspace Rust tests passed; seven tests passed in the optional CEF/capture variant. Workspace and optional-feature Clippy, formatting and all 19 main specifications passed. |
| Architecture model | Eight targeted model tests passed for the new crates and their module dependencies. |
| Linux graphics availability | WSLg exposed Wayland and X11 sockets; `vulkaninfo --summary` found Mesa llvmpipe, a software Vulkan device. |
| Native Wayland and X11 windows | Both runtime smokes passed: 1,200 output lines, terminal marker, two attachments to the same service session, a reread document and one server-stored Unicode annotation. Success requires current terminal and marked Markdown paint acknowledgements, then joined service cleanup. |
| CEF on Wayland and X11 | Both sandbox-enabled runtime smokes passed. The fixed fixture produced 35/38 real frames and 34/36 JavaScript timer updates; explicit engine closure and service cleanup completed. Wayland selects Chromium's Wayland Ozone backend explicitly. |
| Native composition | The [external X11 capture](images/gpui-spike-x11.png) shows terminal cells, native Markdown with its saved annotation, Chromium pixels clipped to the pane and a native overlay above them. Two routed pointer clicks reached the fixture button. Wayland has runtime/paint evidence, without an external visual capture. |
| Capture API | The pinned GPUI Linux platform does not implement `render_to_image`. Explicit `--screenshot` fails; runtime-only smoke is separately labeled and never invents an image. |
| Font fallback | External X11 review with DejaVu Sans Mono, Noto CJK and Noto Color Emoji shows correct terminal spacing, Japanese text and emoji fallback. The terminal's joined emoji sequence still needs refinement; this is not full grapheme/emoji fidelity. Without the optional host fonts, both native views showed missing glyphs. |
| Input and resize | Unit/in-process tests cover key encoding, replay suppression, split UTF-8, real PTY input and a 111×37 resize. Native manual IME/dead keys, clipboard, selection gestures, window resize and scrolling performance remain unverified. |
| Windows | Client adapter and GUI compilation are in scope for scheduled CI; this launcher has no owned WSL/native connection wired on Windows. |
| macOS | Runtime behavior has not been verified. |

The spike is a feasibility check. Its terminal and viewer cover fixed workflows,
not desktop feature parity. Manual IME and clipboard behavior still need a
human run on each target platform.

The terminal implements basic ANSI cell rendering, scrollback, selection,
navigation/control keys and native composition hooks. Mouse-reporting terminal
applications, image protocols and complete extended keyboard protocol parity
are outside this probe. Markdown uses exact quote/context anchoring and a fixed
annotation remark; full review threads and fuzzy re-anchoring remain migration
work. Links and document images deliberately remain inert.

Linux results here establish feasibility on WSLg with software Vulkan. They do
not establish hardware performance, production packaging, or Windows/macOS
runtime parity. The next migration step should extend the client core and
exercise native input on real target desktops before planning a release cutover.

The full workspace build enables JSON insertion-order preservation through a
transitive UI dependency. A CLI review test now compares parsed JSON and keeps
its single-line/newline checks, so it validates the protocol independently of
object key order. No CLI decision behavior was changed.

## Ghostty dependency check

On 2026-11-07, a separate dependency probe built the official
[`libghostty-vt` 1.0.0 release](https://github.com/ghostty-org/ghostty/releases/tag/vt-1.0.0)
with Zig 0.17.0 on Linux x86_64. The annotated tag resolves to commit
`0fa3d577a5614150ff8e2c152de5fb9d5c6d2dec`. The source and bundled dependency
archives matched the release's published SHA256 sums:

| Archive | SHA256 |
| --- | --- |
| `libghostty-vt-1.0.0.tar.gz` | `4a6ef758be721923e1969e7b81b443dcc92d8dd3fb5a39600c03a8817a753676` |
| `libghostty-vt-1.0.0-deps.tar.gz` | `4d5ea5e448e4c7ea8fb55ccbaa8668f2a97d44f438cee831e2b7ecd162d85c29` |

From the extracted release source, with the matching dependency archive
unpacked and Zig 0.17.0 available:

```sh
zig build -Doptimize=ReleaseFast --prefix /tmp/splitty-ghostty-vt-prefix
```

The release archive contains `LIB_VERSION` and builds the library by default.
A Git checkout instead needs `-Demit-lib-vt`; see the upstream
[installation instructions](https://github.com/ghostty-org/ghostty/blob/vt-1.0.0/dist/doxygen/pages/getting-started/installing.md).
The build installed shared and static libraries, matching headers, pkg-config
metadata and CMake metadata. Small C and Rust 2024 programs successfully linked
and ran with both library forms; the statically linked Rust program had no
runtime dependency on `libghostty-vt`. Header and runtime versions were 1.0.0.

The C functional probe passed fragmented UTF-8 ingestion (`A€`), a cursor-position
query/reply, resize to 12 columns by 4 rows, formatted text extraction and
iteration over four dirty render rows. These are isolated library checks, not
tests of the safe Rust wrapper, GPUI rendering, session replay, selection,
clipboard, image support or interactive performance. macOS and Windows builds
remain unverified locally; the native Windows client needs a Windows library,
independently of its server running in WSL. This dependency-only check preceded
the integration described above; it is not evidence for native Windows or
macOS runtime behavior.

## Native pane movement

The native workspace supports single-tab moves into another slot and onto all
four split edges using the existing `move_pane` operation. Captured layout and
route checks reject stale gestures while allowing terminal metadata updates.
Placement and focus follow the authoritative snapshot; terminal entities and
sessions remain attached. Surviving source siblings retain their proportions.

Focused client-core tests cover structural/connection/session invalidation,
metadata-only revisions, source collapse/proportions and selection confirmation.
A real-service test moves PTY-backed panes and verifies continued input on the
same session. The mouse-driven smoke in `scripts/testing/gpui-pane-drag.py` passed
center drops, all four split edges and Escape cancellation on isolated X11 with
software Vulkan rendering. The original shell process remained the same through
every move, screenshots retained its terminal history, and normal window closure
left no child processes. These are functional results, not latency or hardware
performance evidence. Exact header placement and same-group reordering now use
the additive `move_tab` operation, preserving canonical slot IDs and active
selection. It requires `tab-placement-v1`; body/edge moves retain the original
operation. Group moves and cross-window/screen moves remain later work. Broader release/CI and cross-platform cutover
validation remain separate.


## Exact native tab management acceptance

The native header supports same-group reorder and exact insertion before a tab
or at the end of another group. The whole tab chip is draggable; ordinary close
clicks retain their behavior. Overflow strips scroll horizontally, keep the add
button visible, and reveal selected tabs after keyboard navigation, Screen
switches and narrowing a previously maximized group. Mod+PageUp/PageDown stops
at the focused group's ends. Rename editors and close dialogs retain priority.

The real-window `gpui-screens.py --tab-management` run passed seven check groups
on the release binary with SHA-256
`4198a51f633c56e3dcaef87c43000f7ccc81c42ec878b6ea26e4988823e22dfb`.
It compares actual pointer and keyboard gestures against the embedded fixture's
existing owner-control `/api/mux/state` response and checks original pane/session
IDs and shell PIDs. Coverage includes first/middle/end reorder with selection and
maximization retained, cross-group insertion, Escape cancellation, overflowing
strips after Screen and viewport changes, clamped group navigation, editor/dialog
priority and ordinary tab closure. The private X11 fixture exited normally with
no surviving children. This harness additionally requires `curl` to read its own
service socket; it never discovers an unrelated daemon.

Targeted mux, protocol, session-handler, owner-control, client and GPUI tests,
touched-package Clippy, frontend type checks and the architecture checks passed.
Adversarial client tests cover the ordered catalog fence, earlier and malformed
snapshots, lower revisions, stale identities and navigation during settlement;
the real-service test retains terminal output through exact placement. The same
release binary passed the normal workspace smoke on Wayland and X11 with hardware
D3D12 rendering. These checks establish functionality, not new latency numbers or
native Windows/macOS release coverage.

```sh
python3 scripts/testing/gpui-screens.py target/release/splitty-gpui \
  --tab-management --output target/gpui-tab-management
```

## Native Screen-transfer acceptance

The optimized binary with SHA-256
`984922f1ad1231704ed6bdd74594c81e6b454444890914daf0d378fea38614c7`
passes 14 private-window checks. Both tab right-click and the visible **⋯**
affordance open a menu for a non-selected tab without selecting it; Escape
preserves the selection and sends no operation. Moving that captured tab to an
existing split Screen retains its pane/session IDs and process, follows its
target and reports the narrower PTY geometry. The same selection and search
query copy exactly, and the retained history query still has one match. The
visible menu also moves the terminal to a new Screen. Moving the last terminal
out of its source removes that Screen while all four fixture processes remain
alive. Terminal-body right-click still reaches the application as SGR input.

Continued-output checks wait for changed pixels in the terminal's expected
marker row after the fixture acknowledges its write; a PTY write alone is not
evidence that the UI painted it. Both resulting LIVE markers were visually
verified, along with the menu, retained search, target selection and source
removal. The run exits normally with no surviving children. Its harness and raw
PTY fixture use a private display, clipboard, configuration and owned control
socket; the generated evidence records their hashes and authoritative states.

Client checks cover 80 full-suite cases plus the added placement/refresh
regression, including local and external moves, unchanged attachment and PTY,
no replay, bounded failed-read recovery, later navigation and joined shutdown.
All 68 GPUI tests, strict all-target Clippy for both affected crates, formatting
and 17 architecture checks pass. The same frozen binary reaches workspace smoke
stage 18 on both Wayland and X11 with hardware D3D12 rendering and complete
cleanup. Interactive transfer acceptance is limited to a private 1200×760 Linux
X11 window; this does not establish macOS/Windows behavior or new latency
measurements. Full merge CI remains deferred on the evaluation branch.

```sh
python3 scripts/testing/gpui-screen-transfer.py target/release/splitty-gpui \
  --output /tmp/splitty-transfer-evidence --xclip /path/to/xclip \
  --visible-trigger-x 255
```

The runner accepts private Xvfb, library and xdotool overrides. The visible
trigger coordinate is calibrated for the fixture font and its second tab;
`--setup-only` captures the initial layout when another environment needs a
different coordinate.

## Native floating-group acceptance

The optimized binary with SHA-256
`013ec798baa3a0ee853071cbdecaf76adcd56c2474940408fdb09af51e2594ca`
passes 21 private-window checks. Float Tab, new tabs and cycling within a float,
header movement, grip resizing, maximize/restore, overlapping groups and whole-group
Dock into an empty tiled root retain the original pane/session identities and
processes. A floating tab also transfers to a new Screen; moving the last tiled
pane out leaves the source Screen alive while its floats remain.

Copy and find work immediately after Float and Dock without another terminal
click. Clipboard assertions replace the previous contents before copying, and
the retained history query still has one result. The floating profile picker
opens and dismisses with Escape without PTY input or layout changes. Its explicit
program-launch choices retain the earlier profile acceptance scope; this run
checks the floating picker and ordinary new-tab inheritance.

Move/resize headers send no terminal bytes. Escape, zoom, viewport changes and
window deactivation cancel captured gestures without committing stale bounds.
SGR clicks and wheel reports reach only the topmost terminal; raising an exposed
group changes no canonical layout. Hidden root output continues during float
maximization without a visible size claim, and paints after restore. Four output
checks wait for changed pixels in bounded terminal-content regions, with the
fixture cursor hidden. Screenshots verify the picker, retained search, stacking
and post-Dock output. The owned window exits normally with no surviving children.

The affected checks pass: 153 shared mux tests, 88 client tests, 74 GPUI tests,
strict all-target Clippy for all three crates, formatting and 17 architecture
checks. The same frozen binary reaches workspace smoke stage 18 on both Wayland
and X11 with hardware D3D12 rendering and complete cleanup. Interactive acceptance
is limited to private Linux X11 at 1200×760; Windows/macOS interaction and new
latency measurements remain unverified. No web UI code changed; valid existing
float operations retain their wire format, but browser interactions were not
rerun. Full merge CI remains deferred on the evaluation branch.

```sh
python3 scripts/testing/gpui-floating-groups.py target/release/splitty-gpui \
  --output /tmp/splitty-floating-evidence --xclip /path/to/xclip \
  --float-tab-x 884
```

The runner reuses the Screen-transfer PTY fixture and accepts private Xvfb,
library and xdotool overrides. `--setup-only` captures the initial layout for
coordinate calibration. The generated evidence records binary, harness and
fixture hashes alongside authoritative states, PTY input and screenshots.

## Native Windows workspace verification (2026-11-07)

The optimized Windows candidate has SHA-256
`2ba95dc60e87a34887fd9c2181dbedd2bca9f340393244b778c9bf49e301cfd6`;
its matching Linux backend has SHA-256
`6aac31a85b91312ea380e1549238704586e606e3f292564f0a239349f0b05636`.
The source manifest accompanying the preview records the exact build inputs.
The real Windows workspace smoke reached stage 18 and exited successfully with
hardware rendering. Separate window checks exercised keyboard input/output,
loaded a seeded template through the auxiliary API stream, and closed normally.
Owned WSL shells and helper exited; an unrelated WSL process survived.

The smoke now waits for observed idle session metadata before injecting its
four terminal commands. Attachment alone does not establish shell readiness;
early automated input can be consumed by shell startup terminal queries. This
check is a smoke admission rule, not a general prompt-readiness guarantee.
Normal interactive startup was checked separately without injected startup input.

Targeted checks cover shared API correlation/cancellation, owned helper lifecycle,
100 client-core regressions, catalog authority/inventory, the real owner reload
and API smoke, native startup parsing, strict Clippy and 17 architecture checks.
Five targeted native Windows release tests also passed. These establish supported
behavior and lifecycle, not latency measurements, complete input-method coverage,
or parity with every released web feature. Production installers and full merge
CI remain deferred on the evaluation branch.

The matching final Linux GUI candidate has SHA-256
`41756f51ec844b52322620c1579ef9bdbc47143f4fdb4536981b7cc3924925f8`.
It passed the complete private template window acceptance (13 checks, 20 captured
images) and stage-18 hardware workspace smoke on both Wayland and X11. The real
shell fixture proved selective saved command execution, layout-only omission,
replacement cancellation/confirmation, modal input isolation, preservation of
unrelated work and complete owned cleanup. Review/library transitions explicitly
restore workspace focus so Escape does not remain assigned to an old root editor.
These template and workspace checks use this exact binary; earlier feature and
latency evidence retain their original recorded binaries.

## Native Screens Drawer acceptance

The Drawer increment passes 108 client-core tests, 83 native UI tests and 17
architecture checks, touched-crate Clippy with warnings denied, formatting and
OpenSpec validation. The pinned Kit component feature adds styled controls; its
Root theme font now owns chrome zoom.

A private X11 window passed eight acceptance stages: create/rename/select,
hovered-group tab navigation, actual horizontal and Shift-wheel overflow,
Screen-header navigation, terminal input tied to live pane/session identities,
assessed busy-Screen closure, saved visibility with keyboard focus restored, and
a new window restoring the hidden Drawer without rewriting settings. The run
used five tabs in one split group and isolated owned shells; normal shutdown
left no owned children. Screenshots confirm the shown and restored layouts.

The zoom matrix also passes all eight cases: both scopes at 50, 125 and 300
percent plus persisted restarts. It checks real glyph/row geometry, pointer and
link targets, search focus, retained sessions, and high-zoom toolbar overflow.
The Windows release build and matching WSL-backend startup preflight pass.
Drawer interaction evidence is from private X11 windows; precision-touchpad
behavior and hardware smoothness need platform testing. In particular, Windows
precision Shift+vertical input is not covered by the ordinary Shift-wheel check.
Full merge CI remains separate from this experimental branch increment.

`gpui-screen-drawer.py` uses read-only mux queries and the isolated settings file;
all workspace and visibility mutations are actual window input. Its geometry
JSON must be calibrated from the candidate's screenshots, including a cropped
overflowing tab strip. `--setup-only` captures the initial window; supply
`window.confirm_close` coordinates when its normal shutdown needs confirmation.

## Native workspace chrome acceptance

The group-control increment passes 85 native UI tests, touched-target Clippy with
warnings denied, 17 architecture checks, formatting and OpenSpec validation.
The matching Linux candidate has SHA-256
`c78f864cefe7f87b163052c07a225ab6499d133f7cade18f350e7ade682d964a`.

`gpui-workspace-chrome.py` passes eight real-window checks with isolated shells:
new tabs and splits on an unfocused group, keyboard profile selection, compact
menu navigation isolation, maximize/restore with immediate terminal input,
float/dock, and selected-tab close. Pane/session identities prove unrelated
terminals remain intact. A separate 150-percent window checks rendered icons and
compact-menu splitting. Both runs close normally without owned children left
behind. The existing Drawer acceptance and all eight display-zoom cases also
pass with this binary, including the actual profile picker at 300-percent zoom.

The harness reads mux state through the owned control socket; workspace changes
are real pointer or keyboard input. Supply screenshot-calibrated geometry using
`--geometry`; setup switches capture the relevant controls for calibration.
This is private X11 functional evidence. Windows builds and hardware smoothness
testing remain separate from this UI increment.

## Passive native Hosts acceptance

The Hosts increment passes 126 client-core tests, 88 native UI tests, 17
architecture checks, focused shared-contract/server conformance tests and
touched-crate Clippy with warnings denied. Formatting and OpenSpec validation
pass. The matching Linux candidate has SHA-256
`8409b93379941dd54427c684535f9d93eaa3bed7c709c27c95fc10236b6a1914`.

`gpui-host-directory.py` passes six private-window acceptance stages at
135-percent window zoom. Its isolated fixtures include aliases, long targets
and Screen names, cached Screens, observed-empty and unobserved hosts, and enough
rows to exercise virtualized scrolling. A valid catalog larger than the native
API response ceiling produces a real failed refresh: the error and Retry remain
visible while the exact previously rendered rows are retained. Restoring the
small catalog and refreshing recovers. Terminal markers before and after the
failure and after returning to Screens retain the same PID, pane and session.
The child PATH's SSH spy records no launch. Normal window close exits successfully
without owned children left behind.

Screenshots verify text truncation with pane counts still visible; a separate
24-pixel UI-font capture checks three-line row height. The eight group-control
window checks also pass with this exact candidate. These are private X11
functional checks, not new hardware smoothness or Windows build evidence.
Remote connection, live browsing, attach/detach and forwarding controls remain
subsequent increments; this view only inspects the assigned service's directory.
