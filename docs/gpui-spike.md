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

Preview bundles are immutable snapshots. Create both the Downloads package and
its owned WSL runtime/configuration directory exclusively; refuse an existing
destination and choose a new name. Do not merge new binaries, metadata or evidence
into an older preview. Verify the frozen source and every packaged file before
marking the new bundle ready.

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
Select tabs by clicking their labels, close panes with their tab close control
or middle-click, and resize splits by dragging the divider. Right-click a local
tab to move that running terminal to another local Screen or a **New Screen**.
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

The Screens Drawer exposes the catalog, canonical pane entries, creation,
rename, assessed close, Hosts and Templates. Its pin command saves pinning through
the local settings service; a new window restores that choice, edge-reveal policy
and saved sidebar width. The unpinned Drawer slides over the workspace on an
intentional left-edge dwell and retracts without resizing terminals or writing
settings. Ctrl+B toggles its transient visibility. The redundant workspace
toolbar has been removed.

Ordinary vertical wheel input anywhere over the Drawer moves through Screens.
Over a tab header it selects a tab in that group, even when another group has
keyboard focus. Both stop at the ends. Modified wheel input and the scrollbar
retain Drawer overflow browsing; horizontal input and Shift+mouse-wheel browse
overflowing tabs. Editing, close prompts and dragging take priority over this
navigation. Terminal wheel handling stays inside the terminal view.

Each tab group has a **+** button and profile picker directly after its tabs.
Split right/below, float/dock and maximize/restore controls stay at the trailing
edge and act on that group, including when another terminal has keyboard focus.
Narrow groups collect secondary actions in an icon menu. Tab-specific menus open
on right-click; middle-click uses the same assessed close as the tab's close
button. Drag feedback retains the tab's title and appearance, and a valid center
drop highlights the whole pane even when returning to the same group changes no
layout. The pinned GPUI Kit supplies controls and icons; the canonical mux tree
remains the only layout model.

**Hosts** shows known targets and aliases with their cached Screen names, pane
counts and last observation. It distinguishes an observed empty list from a host
with no Screen observation. Opening the view or using **Refresh** does not
connect. Use a row's **Connect**, then **Open** for a Screen, or enter a new SSH
target in the **Connect** dialog. An optional display name keeps the target in
the directory independently of recent connection history. Confirmed names label
the Hosts list and Drawer groups without changing their SSH targets. The row menu
edits or clears that name and removes saved host metadata; removal keeps opened
Screens, terminal sessions and forwards running.

Current connection work remains visible separately from saved directory entries.
A failed unnamed attempt does not create a saved host. Cancelling the Connect
dialog withdraws its connection request; already submitted metadata may still
be saved. An uncertain metadata result is refreshed without automatic resubmission.
A failed refresh keeps the last valid directory and offers **Retry**. Use **R**
to refresh and **Escape** or **Screens** to return to the workspace with terminal
focus restored. Leaving Hosts releases browsing connections, while opened
Screens retain their independent connection demand.

Ports appear under their host in the same view. Detected suggestions, active
forwards and saved rules are joined by target and remote port; manual mappings
remain available without discovery or an open Screen. **Add port** accepts a
target, remote port, optional exact local port and label. **Forward** starts it;
opt-in **Remember** saves its actual local port after successful opening.
**Save** changes metadata without connecting. **Stop** pauses a saved rule;
**Pause saved port** also handles an inactive saved rule without connecting.
**Resume** starts it again, and **Forget** removes its saved preference while
leaving an active tunnel running. Editing a paused rule keeps Enter as metadata
Save; the separate **Resume with local port** action attempts an alternative
before updating its saved port. A conflicting local listener is left alone.

Only deliberate **Open** or remote terminal-link activation requests the browser.
The local worker confirms the exact mapping, and the window rechecks the captured
source and one-shot opening consent before dispatch. Changing the selected Screen
does not retarget an opening; closing its originating Screen, dismissing its form,
or stopping its mapping revokes that continuation. Closing the surface does not
claim to stop a submitted tunnel. Active/saved refresh failures retain their
separate last observed values, and uncertain requests are not replayed. Services
without the native forwarding update capability show an unavailable explanation.
Reverse sharing and automatic forwarding on discovery remain outside this view.

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
generic terminal links. Remote loopback links use the originating host's outbound
mapping and retain their scheme, path, query and fragment at the confirmed local
address. Local terminal links keep their direct platform-opening behavior.
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

## Native source-routing acceptance

Stable source-qualified Screen, pane and layout keys now separate retained view
identity from current connection/session authority. Worker commands and terminal
callbacks capture their original target; old output, attachment notices, close
outcomes and queued resize measurements cannot acquire a replacement's identity.
Same-source movement retains the emulator and search state. Local behavior and
the single local service remain unchanged; this is the prerequisite for remote
Screens, not remote connection support itself.

The increment passes 133 client-core tests and 91 native UI tests, including
charged-event release, stale document replies, startup-close publication races,
retained callbacks and saturated resize queues. Both touched crates pass
all-target Clippy with warnings denied; formatting, 17 architecture checks,
strict change validation and all main specs pass.

Private X11 window checks cover Drawer navigation and hidden output (9), group
controls and continuous divider dragging (9), window zoom with retained query
and hidden-session output (12), and floating groups (21). These checks use
candidate SHA-256
`be6f4be9785022c1346befc1aef20504c4bfd58e4a26c8bdc9725a316ddbf8ff`.
The divider check holds one mouse press through three confirmed canonical
revisions while retaining every pane/session route. It verifies the correction
to a Slot-only lookup that previously prevented split divider drags.

The final candidate has SHA-256
`c283b9eec3614f5b693c4fc0646720bde2f91b2233bd07d80f01586f58529894`.
Only the transfer module changed after the earlier window checks: the captured
context menu now accepts a nonselected tab belonging to the shown Screen. Its
real-model regression and all 14 Screen-transfer window checks pass, including
Escape dismissal without terminal input, same-session movement, retained text
selection/find query and continued output after the old Screen disappears.
Every window run closes normally without owned children left behind.

The existing harnesses perform actual keyboard/pointer interaction and read
authoritative state from isolated control sockets. These results are functional
software-rendered X11 evidence, not Windows or hardware-smoothness measurements.
Full workspace merge CI remains deferred until main integration, as does the next
Windows test build.


## Remote workspace integration checkpoint

The shared source/window runtime passes 172 client-core tests, including
connection-reset focus retention with lower replacement revisions and old-token
rejection. The corresponding remote UI checkpoint passes 93 GPUI tests and
strict Clippy; architecture checks pass. The native host adapter's in-process
fixture verifies fixed target identity, caller budgets, revoked admission,
independent closure, and no forced update of a compatible older daemon.

The optimized baseline candidate has SHA-256
`e1f81d471ccb7f6a0a64a9e420d171caec4f0643b34e2cf45ae16dff76545867`.
Its isolated two-host real-window run passes nine stages: passive inspection,
deliberate Connect/Open, remote input and default Tab/Split inheritance, hidden
output and source-grouped navigation, Detach/reopen of the same session, explicit
Terminate cancel/confirm, and normal close preserving host sessions. The host
fixtures share raw Screen and Pane identifiers but use distinct actual sessions
and processes. The existing local Drawer run passes nine checks on this binary.

An earlier recovery candidate exposed lost split focus after replacement.
The corrected combined candidate, SHA-256
`dfe2f2fdbfc1ee66776d82b52daeeda9aaad9505da0e8a572f9b6ed3e55ec968`,
passes all eight stages of the actual recovery run. The test terminates only the
identity-verified private bridge; automatic replacement returns input to the same
right-pane PID, pane and session. Local and other-host terminals remain usable,
and the host inventories, live shells and one-time input are preserved. Normal
window close retains host sessions, and fixture cleanup completes. Broader remote
error, stale-consent and source-local feature coverage remain open.

A separately frozen Windows preview with executable SHA-256
`0040161c87051c699f9394aeef61ae31e47395a2ecaed0f4dafb2f31659ffd11`
passes native startup on hardware graphics and normal window shutdown, including
cleanup of its owned WSL backend and shell processes. It contains the earlier
remote baseline, not the subsequent recovery corrections or workspace-fidelity
changes. Linux remote-window checks do not establish Windows remote acceptance.
Full workspace CI remains deferred until main integration.

## Native workspace fidelity acceptance

The current workspace places new-tab/profile controls immediately after the tab
strip, keeps group actions trailing, and exposes the tab menu through right-click.
Middle-click closes the captured tab through the existing assessed-close flow.
Drag feedback carries the grabbed tab's title and icon; a valid center target
highlights the whole pane even when dropping there leaves the layout unchanged.

The Drawer follows the existing web hierarchy: compact Screen headings, monospace
pane entries, nested split/tab groups, hover actions, New screen below the list,
and Hosts/Templates at the bottom. The extra workspace toolbar is removed.
Port management, host creation, unread indicators and Screen reordering remain
separate increments; the Drawer does not present placeholder controls for them.
The Settings destination is described below.

The optimized Linux candidate has SHA-256
`dfe2f2fdbfc1ee66776d82b52daeeda9aaad9505da0e8a572f9b6ed3e55ec968`.
It passes all 95 native UI tests and all-target Clippy with warnings denied.
The shared client-core's 172 tests, the strengthened native host adapter test,
17 architecture checks and formatting also pass.

`gpui-workspace-fidelity.py` passes nine private-window stages on this candidate:
adjacent controls, inactive-tab right-click, assessed inactive-tab middle-close,
same-group center feedback without a revision change, an edge split retaining
session identities, canonical Drawer pane activation, whole-Drawer wheel
navigation, and unpinned edge reveal/retraction. A terminal with mouse reporting
enabled receives no leaked wheel bytes. Reveal and dismissal preserve saved pin
settings byte-for-byte and leave terminal rows/columns unchanged. The run exits
through normal window close with no surviving owned children.

A separate 15-Screen run verifies that the last created Screen is revealed in
the list and that clicking the visible final two rows selects their canonical
Screens. Shift-wheel scrolls the overflowing list without changing the selected
Screen. The run also closes normally with no surviving owned children.

Visual review covers short and overflowing tab strips, the one-line tab-shaped
drag preview, full-pane center marking, pane indentation and typography, and the
overlay with no redundant toolbar. An earlier candidate exposed incorrect
Drawer leave handling; the accepted candidate uses the Drawer hitbox's hover
transition and passes both retractions. These are software-rendered Linux
functional checks. The previously delivered Windows preview remains unchanged,
and full workspace merge checks remain deferred.

The 2026-11-08 follow-up aligns the host caption's weight and spacing, nested tab
indentation, and Templates/Settings icons with the web Drawer. The Linux binary
`3ef4bafe76b04a3c5f8ff08a8b01ffde4b26201e29bd9cd24022f1fa5bb57d5d`
passes the nine interaction stages again, strict native all-target Clippy and
formatting. The wheel-routing fixture now launches an owned script through a
short terminal command, avoiding dependence on a long synthetic keystroke stream.
The matching Windows binary
`c68a2d3cee284ea2e197acc406bdae23256c6a8dc481c9b686000deaaeeb19e8`
passes the integrated 18-stage workspace smoke on a hardware GPU and a separate
startup/WM_CLOSE check with an idle shell. Both exit with code 0 and no surviving
owned WSL processes. Pointer interaction evidence remains Linux-only; these
checks do not measure hardware smoothness.

## Native Settings

Open Settings from the Drawer footer. The editor replaces the workspace content
while retaining the selected Screen and its terminal sessions. Close or Escape
returns to the workspace; Escape dismisses an open selector first. Keyboard
focus returns to the Drawer Settings control. Selecting a Screen or creating a
new one also leaves the editor. Wheel navigation over the Drawer returns to
Screens, while scrolling inside the editor remains owned by its content.

The first editor exposes twelve effective settings:

| Group | Controls |
| --- | --- |
| Appearance | Theme mode, fixed theme, dark/light themes, interface font size and installed font family, display zoom and zoom scope |
| Terminal | Terminal colors and close-on-exit behavior |
| Workspace | Drawer pin state and width |

Interface font settings affect chrome rather than terminal text. The terminal
palette can follow the interface or use a separate existing Splitty theme; only
its terminal colors are applied. Palette changes retain emulator state,
scrollback, session identity and cell geometry. New terminals inherit the current
palette. The selector uses the service's theme catalog; native Kit preset names
are not additional persisted themes in this increment.

Settings always belong to the local service, including while a remote Screen is
selected. The editor sends one typed field update through the existing owned
local API stream. Opening it does not write config. Controls retain confirmed
values while saving, and Reset uses the default published by the service. An
unconfirmed write is reconciled through a fresh read without automatic replay.
Missing metadata or a missing theme catalog leaves effective values visible and
disables dependent choices. Other web Settings sections remain absent until the
native client implements their behavior.

`scripts/testing/gpui-settings.py` drives the production view with actual X11
input. Use `--setup-only` to capture an isolated window and calibrate a geometry
JSON, then run its `open`, `theme`, `typography`, `terminal`, `workspace` and
`rejection` phases against the same immutable optimized binary. Each phase owns
its config, service socket and shells, records the binary/harness hashes and
screenshots, and exits through normal window close. The rejection fixture blocks
only its private atomic-write temporary path. The remote-window harness has a
separate `settings-remote` phase for local settings ownership with a remote Screen
selected. These are functional checks; they do not measure hardware smoothness
or replace Windows/macOS acceptance.

For retained-view failure-state checks, run
`cargo test -p splitty-gpui --no-default-features --features ui-test-support workspace::settings::tests`.
This opt-in feature selects the production GPUI backend's test context and
isolated client-core presentation fixtures. It checks unavailable metadata/catalog
and rejected, uncertain or stale results without enabling the Kit-wide alternate
backend or adding fixture APIs to normal builds.

The optimized Linux candidate with SHA-256
`90dfd6cb2bafdb46b4a9f3ea7866598814ddd3e2fd99cdfa39b53f97f629c540`
passes all six Settings window phases. They verify no write on open, keyboard and
pointer navigation, selector-first Escape and trigger focus, content scrolling,
theme mode and reset, font family/size and zoom, Drawer width/pin, and visible
save rejection. Independent terminal colors preserve the original PID, pane,
session, indexed-color history marker and PTY size; a new tab inherits the
palette. The remote-selected run verifies local-only persistence, unchanged
remote configs, inventories and shell PIDs, and fresh input to the same remote
terminal after closing Settings.

The final candidate with SHA-256
`dd9da69abbfda98ee85c84fe2faafcf3135d5859f3531cee484ac3d918b00425`
adds Drawer wheel navigation out of Settings/Hosts. Its extended navigation run
returns from Settings to the preceding Screen and verifies input against that
Screen's original PID, pane and session. All window runs close normally with no
owned children remaining. The remaining Settings/palette implementation is
unchanged from the preceding candidate. The increment passes 184 client-core
tests, 98 normal GPUI tests, two additional retained-view tests with the opt-in
feature, strict targeted Clippy, formatting and 17 architecture checks.

A Windows preview built from the same final source has executable SHA-256
`abdb25de4b42f38fb0ba1fa9a1898cfcaa331a5cca21b78524cadb47c3fb30ae`
and matching WSL backend SHA-256
`fb3529baaff43b6dff21bedf75fccaf08386496a38b1eef469a8f59a0ad7f416`.
Its narrow startup check verifies hardware graphics, the exact owned backend,
normal GUI exit code 0 after a close request, and no surviving owned Linux
process. This check does not establish Windows terminal interaction or Settings
acceptance; the functional evidence above remains scoped to Linux windows.


## Native host management acceptance

The native Hosts surface now accepts typed SSH targets and optional display names,
edits or clears names, and removes saved host metadata while retaining opened
remote work. The local worker owns metadata preparation and connection continuation;
leaving Hosts or cancelling a pending form withdraws browsing intent. Confirmed
names label both Hosts and Drawer groups. Current connection work remains distinct
from saved directory entries, including after removal.

The increment passes 196 client-core tests, 105 GPUI tests with the selected
backend's UI test support, strict all-target Clippy for both changed crates,
formatting and 17 architecture checks. Existing server metadata and forwarding
conformance tests pass. Regression coverage includes lost acknowledgements,
partial server failure, cancelled preparation, immutable menu owners, saturated
cancellation queues, concurrent metadata writers, actual field typing, small-window
form bounds, keyboard focus return and visible uncertainty after a row disappears.

The optimized Linux candidate has SHA-256
`8f77bd4d54a15fc1d27a6e90217f0786f21c95e118bc1663f49eee2939d9d586`.
The existing remote-window harness's `host-management` phase passes ten checks:
passive inspection, explicit connection and original-session input, invalid target
submission without SSH, Enter/Escape focus handling, typed entry of an absent host,
name edit/clear, removal with open work, explicit re-add and normal window close.
It reads the private fixture's durable preferences, recents, order and catalog;
rendered screenshots confirm the Hosts and Drawer projections. Fresh shell markers
after removal and re-add retain the same PID, pane and session. Metadata edits
start no additional SSH process, remote configuration and inventories remain
unchanged, and all owned fixture processes are cleaned up.

A Windows preview built from the matching implementation sources has executable
SHA-256 `d6272de8d3455efe703ab31e436c0b9d3f633c86f2e4048711a74f734712a544`.
Its integrated local workspace smoke reaches stage 18 with distinct actual WSL
session markers, tab/split/Screen actions, hidden output and preserved editor
focus. Native hardware graphics is reported. A separate owned startup/close check
also exits normally; both runs leave no owned Windows or WSL process alive.

Host interaction evidence remains scoped to software-rendered Linux X11 windows;
Windows remote-host interaction, broader remote failures and outbound port controls
remain separate acceptance work. This increment does not close the wider remote
workspace matrix.

## Native outbound ports acceptance

The shared controller passes 220 client-core tests. The GPUI suite passes 113
tests, followed by an additional production-view regression for pausing both an
inactive saved rule and a failed enabled rule without starting either. Targeted
all-target Clippy passes for client-core, GPUI and server, together with formatting
and all 17 architecture checks. In-process service coverage verifies metadata-only
Save, omitted and cleared labels, durable pause, occupied-port refusal, explicit
alternate ports, Forget with a live forward, and a Remember storage failure that
leaves the verified tunnel usable. Runtime tests retain terminal and host-metadata
progress during gated forwarding and join owned cleanup without replaying requests.

The optimized Linux candidate has SHA-256
`94e2c7debd7f367b689217626005ffcfa6078227b485e20402744a4b76513386`.
The `ports` phase of `scripts/testing/gpui-remote-screens.py` passes 15 actual-window
checks with two private hosts exposing the same remote port. Manual Forward and
Remember, label edit/clear, Forget, Stop, detected Forward, saved-only Pause,
occupied-port refusal and explicit alternate-port recovery are checked against
durable metadata, owned listeners and distinct HTTP responses. Save and Pause
start no SSH work, and the competing listener survives conflict and recovery.

A gated terminal link keeps its original host after another Screen is selected;
the emitted localhost URL retains `/app?x=1#frag`, and the intercepted opener's
HTTP fetch reaches the original host's distinct process with `/app?x=1`.
Form Open also dispatches once, while dismissing a second gated Open dispatches
nothing. The original remote terminal PID, pane and session survive the workflow.
Normal window close preserves the private host sessions, and fixture cleanup
leaves no owned process. The run records 26 screenshots, including the new Pause
menu, conflict, alternate-port, link and dismissal states.

This window evidence uses software-rendered X11, the real service/controller,
an exact-command SSH forwarding shim and an intercepted platform opener.
The shim binds a real owned loopback listener and relays to the private HTTP
processes; it does not exercise OpenSSH. A recorded opener plus an HTTP fetch
does not establish navigation in an installed OS browser. The bounded fixture
can use `SPLITTY_TEST_TMPDIR` when temporary storage is limited; keep its root
short enough for the private Unix control socket.

### Windows endpoint follow-up

The matching Windows GUI has SHA-256
`4b4cdc4e55f1baae2d83eb5f3fd60db47df4e5e2015eefba55454ed545780c51`.
The final preview uses a static Linux x86_64 WSL backend with SHA-256
`e09ff8f2a7d63f27c8adee49b50e15c39b5a92c25847b1a069d8ca3a174d1815`,
built from the same server sources with verified frontend assets and matching
shell integration. This corrects the earlier dynamic preview backend's missing
remote installation payload: the existing installer can package the static
server itself for a matching Linux x86_64 host. No single-architecture runtime
payload override is installed, and other host architectures are not established
by this preview.

An owned fake-SSH installation probe, with all payload overrides absent, reaches
the production install script and verifies the uploaded archive's checksum,
installed binary, version 0.2.4 and shell-integration files. It deliberately
refuses the subsequent handshake; that probe proves installation rather than a
real SSH connection or a remotely running daemon. A separate owned backend
fixture verifies normal service readiness and session behavior. The final
Windows GUI starts with this dedicated backend on hardware graphics, exits with
code 0 and leaves no owned Windows or WSL process. The preceding candidate's
stage-18 Windows workspace smoke is retained as separate, earlier evidence.
The Linux Ports window run above used the previous compatible backend; it is
not relabeled as a run with the new static artifact.

The Windows native-app forwarding workflow is **unrun** for this increment.
Its prepared follow-up uses the selected WSL service, two distinguishable remote
HTTP responses and an independently owned Windows listener: request a forward
from the native window, record the emitted localhost URL and reached response,
then attempt a conflicting saved port and explicitly choose an alternative.
The competing listener must survive, and closing the originating Screen must
prevent any still-pending browser dispatch. An observed wrong endpoint requires
correction or withholding the affected path before claiming acceptance.

`scripts/testing/native-outbound-wsl-probe.py` provides a narrower network probe.
On the observed mirrored WSL network, an IPv4-only WSL loopback HTTP listener
was reached by Windows `curl.exe` through `localhost` with the default resolver
and forced IPv4; both returned its distinct identity and requested path/query.
IPv6-only requests could not reach that listener. With owned Windows IPv4/IPv6
listeners started first on another port, the WSL bind returned address-in-use;
requests still reached the Windows listeners, which survived until owned cleanup.
A separate PowerShell HTTP client timed out for `localhost` while succeeding for
the IPv4 literal. These are client-specific observations, not a browser result.

This probe starts neither Splitty nor its SSH forward and does not establish the
native app's `local_port_conflict`, alternative-port or OS-browser behavior.
It found no successful WSL bind followed by a wrong Windows endpoint in this
configuration. It does not establish other WSL networking modes or justify a
new proxy or a blanket Windows opening restriction. The existing desktop
localhost contract remains in use; shared exact-mapping and source-consent
tests have a different scope from endpoint reachability.

## Drawer interaction and shell-menu corrections (2026-11-08)

Local Screen names enter the existing rename editor by double-click, F2 or
context menu; the hover pencil is removed. Hover covers the complete Screen
block, while individual terminal rows brighten only their text. An admitted pin
action changes presentation immediately. Identified write results and accepted
settings observations retain the confirmed baseline for rejection rollback.

The shell picker is anchored to its group's arrow with the existing Kit Popover.
Four production-view UI integration tests pass: three cover Drawer interactions
and pin rollback, and one uses actual arrow clicks at two window widths, checks
popup geometry, then verifies Escape dismissal and Enter reopening from restored
arrow focus. Strict GPUI Clippy with UI test support and formatting checks pass.
Validation used an isolated source snapshot, matching the six changed source
files byte-for-byte, while independent host-update work continued. This does not
claim an updated Windows package or a real-window animation run; motion and the
startup/theme corrections remain follow-up work.

### Windows appearance startup and workspace feedback (2026-11-08)

The Windows workspace now uses the Kit TitleBar with the application's projected
palette. The initial window stays hidden until the assigned backend supplies
appearance, reports a matching read failure, or a one-second fallback expires.
A deferred public GPUI draw prepares the current scene before activation; this
is not a claim that the GPU has presented while the window is hidden. Subsequent
theme changes never reactivate the window.

The optimized Windows candidate (SHA-256
`7715ea25dbab9aed6f955f790788dd1feef1365f7393f326409c1293b087b66b`)
passed the owned 18-stage workspace smoke using hardware rendering, ordinary
workspace/template interaction, normal close and owned backend/session cleanup.
Startup capture was added to the existing harness, with isolated theme seeding
and three frames beginning at the first observed visible window. Cold and warm
light-theme captures show the configured palette and matching title bar; the
first captures also contain transient window compositing, so they do not prove
absence of every compositor transition. An unavailable-backend run still
produced a visible fallback window instead of waiting indefinitely.

Drawer feedback uses Kit transitions for overlay translation, pinned workspace
reservation, hover colors and measured split-drop bounds. Six production-view
tests and strict targeted GPUI Clippy passed. During a pinned transition the
retained terminal emitted ten integer-grid resize commands over fifteen sampled
frames, one pin write and no terminal input. Screen slides and Screen drag
ordering were not covered by this candidate. The new host update controls have
seven passing production-view tests; real-window remote maintenance acceptance
remains open and is not implied by this Windows workspace smoke.

A separate owned X11 window run on optimized candidate
`a62edcf4868162f15134c860f1d9c0a550e04f1f81f73803e3f3d1a1abc43e4f`
captured Drawer unpin, overlay close/reveal and pin, then clicked the retained
terminal and verified a fresh shell-output marker. Both the application and
Xvfb exited normally. Settled captures show the expected full-width overlay and
reserved pinned layout without clipping or overlap. The intermediate captures
had already settled, so the in-process geometry tests, rather than those
screenshots, establish the transition trajectory.

### Workspace clarity corrections (2026-11-08)

The normal workspace no longer reserves a prototype shortcut/status strip or
padding above its tab bars. Errors and transient feedback remain visible when
present. Drawer Screen numbers have a fixed-width index and an explicit gap
before the name; enabled controls expose pointer and close-button hover feedback.
A maximized group now shows `Group maximized` beside a labeled, outlined
`Restore layout` control in the existing tab bar. The production-view test
`maximized_group_shows_labeled_restore_and_restores_canonical_slots` passes: it
starts from the maximized canonical projection, clicks the actual restore
control, observes its captured command and restores the two-slot layout. It
does not establish compact-window visual acceptance.

### Relative Screen order and Drawer swipe acceptance (2026-11-08)

Owned Xvfb windows exercised local relative Screen placement with concurrent
creation and Escape cancellation. Selection, every existing session and the
original shell PID remained unchanged; a fresh terminal marker after the move
confirmed input and output still used that pane and session. A second run used
two private host fixtures: dragging an opened reference changed the Drawer
order while the host catalog, host-group order and original live terminal
identity stayed unchanged. Normal window close preserved the private remote
sessions, and fixture cleanup completed. These use optional phases in the
existing Drawer and remote-Screen harnesses, not a new process framework.

The optimized native candidate was
`2f27065632921afb3b6eda21de68a1e04c42bf32c9d2f60207ed23e2a1b5a48f`,
with matching backend
`4a3d5de13a10037a2968e7146f2fe04595ba500df6f91001a25414c1ae720078`.
All 376 source inputs remained unchanged during the build. Windows/macOS
ordering acceptance is not covered. A separate 560×420 pinned-Drawer run kept
terminal input and keyboard maximize/restore working, but revealed clipping of
the labeled restore control. The subsequent compact `Maximized`/`Restore` copy
fix passes a production-view test that checks its bounds and clicks the control;
normal widths retain the full labels.

The subsequent Drawer swipe correction follows the web action's 120 ms idle
and renewed-impulse gate, retaining its state across selection acknowledgements.
The targeted Drawer suite passes 22 tests, including a production-view stream
longer than 120 ms that admits only one change until input actually pauses.
Strict GPUI all-target Clippy passes. The frozen real-window candidate above
predates this swipe correction.

### Screen motion, terminal Tab and live titles (2026-11-08)

Screen transitions now retain one passive outgoing projection for 200 ms.
Production-view tests verify current-only input/resize ownership, focus after
clicking the outgoing region, reversal from the displayed position, bounded
retention, removal/owner invalidation and reduced motion. A private Xvfb run
using software rendering captured adjacent red and blue terminal surfaces
sliding vertically, including reversal and settled content, without clipping or
overlap. Candidate SHA-256:
`903def7fbf90d845660739c64ba9f9ce98d9b576a2cc8d5dfd7f426ed7f97f78`.
This is visual acceptance of a debug candidate, not a hardware-performance or
Windows timing result.

The terminal's focused key context now suppresses Kit's global Tab traversal
binding before raw key dispatch. A production Root/Terminal test uses actual
pointer focus, verifies `\t` and `ESC[Z` reach the PTY for Tab/Shift+Tab, and
verifies search controls retain traversal without terminal input. Tab context
menus also reuse the captured-pane close assessment; three view tests cover
inactive tabs, keyboard activation, obsolete owners and remote close-only menus.

Titles remain server-derived. The existing metadata observer updates canonical
pane titles and nudges each affected Screen; native snapshot reconciliation
updates both Drawer and tab text, including hidden Screens. An owned OSC title
probe on the optimized reorder candidate above verified that full path. A
production-view test additionally verifies replacement of a hidden Screen's
label and leading icon without navigating to it. The Drawer matches the web
non-ASCII prefix rule while also supporting single Unicode emoji; there is no
second title parser. Shared live/passive tab labels stay on one line with
ellipsis.

Final integrated validation: all 150 native GPUI tests and all 17 architecture
checks pass. Strict GPUI all-target Clippy with UI test support passes. The full
workspace merge gate and real-window Windows/macOS acceptance remain deferred.

### Windows Interaction preview (2026-11-08)

The Interaction preview packages frozen source commit `c35b1e4` with matching
Windows and Linux source manifests (1,188 inputs, SHA-256
`600b5c08296ab30dd1b8a12694340d5a4f8d1b49f5b351d64e1fcfe7df78af75`).
Both manifests were reverified after compilation. The Windows executable is
`a35457e66d791c617bd0448b42b3d3ae16b3e341db49d12e543f615be37747b0`;
its matching static Linux backend is
`a84e49684f26adf557c63e3c22b64d53b71334b85e8ec5b89cb3c85762c33d83`
and advertises relative Screen placement. An owned fake-SSH probe verified the
backend's self-install payload without an override. The archive includes sibling
shell integration.

The owned Windows smoke passes its 18 workspace stages on a hardware GPU,
normal close, owned WSL/backend/session cleanup and preservation of an unrelated
sentinel. This generic smoke does not cover every new drag, swipe, animation or
Tab input case; their focused evidence is recorded above. Real-host update
acceptance remains open. The package carries its build manifest, logs, evidence,
licenses and launchers in a new Downloads folder, preserving prior previews.

A separate owned Windows probe launches that packaged executable with a private
configuration and its matching backend. After pointer focus, a five-second raw
PTY reader receives byte `09` from Windows Tab input. Normal window close then
completes with no owned WSL processes remaining. This proves the terminal Tab
fix through the Windows input path; Drawer wheel bursts remain a separate
platform acceptance check.

### Drawer pane-list feedback (2026-11-08)

Pane lists now collapse and reveal over 150 ms through a per-Screen Kit
transition. Reversal starts at the displayed height; reduced motion settles
immediately. Groups retain fixed row geometry while the outer list clips, and
collapsed pane buttons cannot activate or participate in Tab traversal.
The 15 production-view Drawer tests and strict GPUI all-target Clippy pass,
including intermediate spacing between two groups and no commands from motion.
Pane spacing, running title-icon emphasis and full-title tooltips also follow
the web presentation. These later Drawer changes are not in the frozen
Interaction preview.

### Native terminal context actions (2026-11-08)

The terminal body now opens a captured Kit context menu with Copy, Paste, Find,
Split Right/Below, Show Drawer and assessed Close Pane actions. Application mouse
reporting keeps priority; Shift chooses the local menu for that gesture. Local
Copy and Find remain available on retained offline terminals, while writes need
separately captured live authority. Both keyboard and menu actions reuse the
same terminal implementations.

Focus carries an exact terminal token into the shared worker and updates only
window presentation state, including offline retained views. Queue rejection
does not advance optimistic focus. Search ownership follows retained displayed
identity independently from live activity reporting. Delayed callbacks validate
source, Screen, session, entity and terminal generation before acting.

Five production-view menu tests cover pointer and keyboard activation, an
initially unfocused split, exact bracketed paste bytes, Copy/Find offline, busy
close assessment, gesture ownership, failed admission and 15 obsolete-action
cases. Shared model/worker tests cover retained focus and source rejection.
After integration with Drawer motion, all 156 GPUI tests and all 17 architecture
checks pass. Strict GPUI/client-core Clippy and strict change validation pass.
A private Linux X11 window also passes center and lower-right menu placement,
Escape dismissal, terminal typing after dismissal and pointer activation of Find.
The captured menu remains inside the window, and normal shutdown leaves no owned
children. The frozen candidate has SHA-256
`32bcf139c7e99af235dff8f957214184c8333e6acdad72356bcd382b9a1d7d89`.
These actions are not in the frozen Interaction preview.

The additional Windows Drawer wheel probe remained inconclusive because the
synthetic input setup could not establish its starting visible selection. The
server's active Screen field does not represent each native window's selection.
All owned probe processes were cleaned up; the delivered preview was unchanged.

### Native unread activity projection (2026-11-08)

The native Drawer shows a per-Screen unread count with newest-title summary and
a per-pane severity dot. Its policy lives in the UI-free client core: at most
100 bounded records, source-scoped deduplication, exact captured session binding
and read history across reconnects without requiring notification replay.
Current-token facts can arrive after command readiness is revoked; stale-token
facts cannot update the ledger. Fresh canonical mapping restores a retained
binding, while conclusive removal or session replacement prunes it.

An independent settings read on the existing auxiliary stream gates admission.
It starts closed, keeps the last good switches after failure and purges disabled
categories on successful refresh. Actual window activation and visibility, plus
active-pane selection, determine what is watched and read. Read-state changes
produce no server command or settings write.

Seven shared-policy tests and three production-view integration tests cover
source collisions, offline delivery, stale owners, reconnects, bounds, settings
refresh and failure, focus/visibility return, deliberate pane activation and
count/severity projection. All 159 native tests and touched-crate strict Clippy
pass. A private Linux X11 window with two owned shells also verifies the
server-to-Drawer path: a delayed OSC 133 command-end marker in the first shell
produces two admitted process activity facts while Screen 2 is displayed. The
Drawer shows count 2 and its pane severity dot; returning to Screen 1 clears
both. Normal close leaves no owned child or display process. The frozen GPUI
candidate has SHA-256
`834c19f9fbc39689e9da03aadcd31e93214c34493f85bfc6eedd90c48a93dcfc`.
These additions are not in the frozen Interaction preview; Windows badge
acceptance and full workspace merge checks remain separate.

### Windows Activity preview (2026-11-08)

The Activity preview freezes source `9252c22`, including the Interaction baseline,
terminal body context menu, Drawer collapse and spacing, unread activity and full
live tab-title tooltips. The existing hidden-Screen production-view test now also
selects that Screen, checks its complete updated tab title and verifies fallback
labels after a canonical snapshot removes the title. Focused view tests, strict
GPUI Clippy and formatting pass.

The native MSVC build has SHA-256
`8f167bc6d37b907c273f344dbe65c12fd6ff9724e8a5b2dad91fda9e42ef9bc7`;
its matching static Linux x86_64 backend has SHA-256
`75824e34d27ac9d0d160299c78c0ac5f03ffc25e9663ef9ee07aa21e9843f18f`.
The owned Windows workspace smoke passes all 18 stages using hardware graphics,
normal window close and owned backend/shell cleanup while an unrelated sentinel
survives. A separate owned fake-SSH probe verifies the static self-install
payload. The package preserves earlier previews in a separate Downloads folder.

An additional Windows Drawer probe verifies selection through distinct terminal
input logs, rather than the server's active Screen hint: three equal Win32 wheel
events in 40 ms move exactly one Screen, one event after idle moves again, and
the reverse direction moves back. This establishes wheel behavior only. That
probe's normal-close step times out with three running shell workloads and its
overall result remains failed; targeted termination of its verified owned
Windows process is followed by complete owned backend/shell cleanup. The
separate 18-stage smoke supplies normal-close acceptance.

Windows unread-badge and terminal-context-menu interaction acceptance, real-host
update/install and remote payloads for other architectures remain separate.

### Native window context title (2026-11-08)

The OS window title and existing Windows Kit title bar now follow the selected
Screen, focused pane title and accepted remote host alias. The projection reads
the current source-qualified workspace model; the shell observes presentation
changes and only updates the OS title when the resulting text changes. Control
characters are rendered as plain replacement characters, and long title-bar
text uses ellipsis with a full tooltip. Probe windows retain their own title.

Three focused tests verify metadata and Screen rename, pane focus, colliding
local/remote identifiers and aliases, plus Root-to-OS title propagation. The
four-test title filter and strict GPUI Clippy pass. This later source change is
not included in the frozen Activity preview; Windows title-bar interaction
acceptance remains separate.

### Native notification controls (2026-11-08)

The native Settings editor adds notification enablement and the three process
categories. Each control writes only its explicit field on the assigned local
service, including while a remote Screen is selected. Reset uses the exact
published service default. Missing reset metadata leaves valid boolean edits
available; appearance/theme failures do not block notification observation.
Toast and desktop preferences remain preserved, without offering unsupported
native delivery controls.

Common editor contracts now live outside appearance. One existing mutation FIFO
routes acknowledgement, required readback generation and uncertainty release by
field domain. The retained Settings view observes appearance and notification
values independently even while closed. Pending controls keep confirmed values;
authoritative notification readback alone updates the unread ledger. Captured
owner and Reset-default checks reject obsolete control actions.

All 165 native tests pass, including three added production-view tests using
actual Switch/Reset input, the local Settings command path while viewing a
remote Screen, native key down/up activation, independent domain completion,
last-good reads and badge policy after authoritative readback. The core slice
passes 32 scheduler, one field-contract and ten appearance tests; strict
GPUI/client-core Clippy, formatting, 17 architecture checks and strict change
validation pass. An owned 1200×760 Linux X11 window also passes actual Switch
and Reset interaction: the complete parsed configuration changes exactly
`notifications.enabled`, and Reset restores the exact baseline while retaining
toast, desktop and process preferences. Captures show unclipped controls at that
size; Escape, trigger-focus/reopen, terminal typing, normal close and cleanup
pass. The frozen candidate has SHA-256
`fe46efb4be12be4fefa3d1246df9f7301b7a3ef472a63065948a43cc686c5376`.
Windows controls and the full merge gate remain separate. These changes are not
in the frozen Activity preview.

### Narrow Settings and bounded port feedback (2026-11-08)

The Settings navigation moves above its independently scrollable content in
narrow windows, with wrapping section buttons and header actions. A production
view reproduced the previous 560×420 failure: with the default pinned Drawer,
the fixed side navigation pushed Close beyond the window. The resize test now
keeps controls inside the viewport, preserves the wide layout, scrolls to the
fourth notification switch and submits only the deliberate field change.

An owned 560×420 Linux X11 window with a 200-pixel pinned Drawer confirms visible
navigation and keyboard focus, reachable Close and the final process switch
with its Reset action after scrolling. Layout/focus/scroll alone make no config
change; normal close leaves no owned children or display socket. Candidate
SHA-256: `ad29b99cedebe32f78379a42f240a8b43d614c52fc56796203b78bfc1c0b0d0b`.

Port action admission now checks room in the 64-entry UI feedback table before
queueing a command or retaining browser consent. The reproducer failed before
the fix because untracked open IDs could outlive exhausted feedback capacity.
The fixed test proves refusal without a command or registry growth, permits
Stop for an existing key and recovers capacity after an identified terminal
result. Ten focused Ports tests, the combined 167 GPUI tests and strict GPUI
Clippy pass. These later fixes are not in the frozen Activity preview; full
workspace CI remains deferred.


### Native Drawer host and port actions (2026-11-08)

Remote host captions now navigate to their exact current Hosts row. Their Ports
menus project the same detected, active and saved mappings as Hosts, with at
most twelve actions, a total known count and honest missing/partial discovery.
Popup inspection is passive. An ephemeral menu capture is consumed before
submission; admitted browser intent instead belongs to the retained host group
and survives ordinary navigation/reconnect until that group is removed. Both
Up and inactive mappings use the existing controller and verified one-shot
browser result. No new server operation or forwarding policy was added.

All 179 GPUI tests, strict GPUI and client-core all-target Clippy, 17 architecture
tests, formatting and strict change validation pass. Production-view input tests
cover stale cached menu rows, owner replacement, duplicate activation, final
reference removal before acknowledgement, autohide and keyboard focus. The
Escape-to-Enter regression failed before the focus fix: the Kit dropdown restored
its wrapper rather than the actual Button. Cancellation now returns to the
trigger, while management navigation retains its destination focus.

An owned Linux X11 window verifies two fixture hosts with colliding aliases and
remote IDs while the local Screen is selected. Inspect/dismiss/reopen and exact
B management navigation cause no forwarding mutation; deliberate B port selection
produces one accepted B forward and a successful opener-spy HTTP response from
B's owned endpoint, with no A forward. Escape then Enter reopens the menu;
Shift-Tab then Enter activates the host caption. The caption is left aligned,
the popup fits a 560×420 window, and unpinned autohide/reveal remains usable.
Normal window close preserves host sessions, followed by complete owned fixture
cleanup. This uses a private exact-shape SSH shim and opener spy; real OpenSSH,
external hosts, a real browser and other platforms are separate acceptance.

Frozen source: `fe5d325a83a96420e95027dfd31ee714300d8f44`;
1221-file source/asset manifest SHA-256:
`b177a73a76e2da68d76fb5dc284a2b84538e9ec0e598d718fe016cd2caae1e04`.
GUI binary SHA-256:
`9bb116ee9595b7bfa9e948d85781f60002d156bda925b9a0be68b7824d768cf3`.
Actual-window result SHA-256:
`2b10cd1454d6cd500c3fda07245de41af12460c3d538c97cefd17c305ced9769`.
The Activity Windows preview is unchanged; full workspace CI remains the deferred
pre-merge checkpoint.


### Windows Drawer snapshot (2026-11-08)

A separate Windows Drawer preview freezes source `a33d0b6` with 1221 source/asset
files, including 183 generated frontend assets. Its native Rust/MSVC executable
is SHA-256 `bbe5fa8f11786c0d3bc95f35eb46a16e47cbe13b6c8c1ff5a04922426fa1e055`;
the source manifest is
`a38ff17eb373a38453f283e45c668f9220d70daad5e69ed7235bdfec4ae3c127`.
The Activity static Linux backend and self-install payload are reused only after
verifying unchanged server/core/remote/transport and shell-integration sources.
The package has its own new WSL runtime and configuration; earlier demo settings
are not reused.

The packaged executable and exact backend path pass the real Windows 18-stage
workspace probe, hardware rendering on an RTX 4080, template-library pixel
interaction, normal close and owned WSL process cleanup while an unrelated
sentinel remains. The secondary smoke title assertion now accepts the live
Screen/pane title; PID, process start time and executable path still establish
ownership. The actual-window result SHA-256 is
`f578672afd92f15f52bf68a2a79ec247f959534f2b932aa888b4ec882b7f6051`.
All 39 checksum-listed package files were independently verified. Drawer quick
ports and notification controls retain their separate Linux input acceptance;
individual Windows interactions for those additions remain unverified. Remote
Screen creation is not part of this frozen preview.

### Native remote Screen creation candidate (2026-11-08)

An opened, ready host group now offers a quiet New Screen action alongside
Ports. Shared client-core admission captures the exact host and connection,
reserves Screen/pane capacity, sends the existing unnamed creation once and
adopts its host-assigned name and sessions only through coherent canonical
observation. An unconfirmed reference remains explicit after connection loss;
recovery observes its identity without replaying creation or inventing success.
Detaching withdraws the reference without terminating host work.

Follow is separately revocable through final selection publication. Newer
Screen/destination navigation, accepted exact detach or window close, and a
new local Screen/template creation prevent late focus or selection changes.
Per-host enqueue latches are independent of the latest visible outcome, so an
older host's rejection cannot strand its action or cancel newer host intent.

Frozen source `34bedd5250281e75531c744dd4ec58d313d4b472` passes 267 client-core
tests, 187 GPUI tests, strict all-target Clippy for both crates, 17 architecture
tests, formatting and strict validation of the affected changes. This includes
existing local creation/template and assessed remote close regressions. The
1229-file source/asset manifest is SHA-256
`92c9ea8fa9a6620e3ab779116ecf7547879f672bb00efbe6ed1dc2095603bcc8`;
the Linux GUI executable is
`c7c2610712b1335d2b853311c3f7eddcddacc3b204a8a7e5e12c492e3378fb7e`.
The previously frozen Linux fixture backend is reused after verifying unchanged
server/core/remote/transport/shell-integration sources and lockfile; its hash is
`5880d40578ee539f5d0e635a7d7fb2e1a9c7b7aa7f7dd5797cc9ea94e79ca3ba`.
This initial candidate exposed the platform event-dispatch failure described
below. No new Windows package or smoothness claim is implied by these checks;
full workspace CI remains deferred.

### Linux event fairness and complete remote creation acceptance

The initial candidate reproducibly lost a suffix of terminal input during
remote creation. A private payload-free trace measured a 1.995-second GUI poll
gap while 144 keyboard callbacks continued at a median 12.919 ms interval. The
worker waited 1.073 seconds on its bounded update channel; 15 later inputs were
refused by the full command queue. Polling then consumed 64 updates in 755 µs.
No slow reconciliation or poll-task termination was observed. The pinned X11
drain-until-empty loop starved foreground runnables; this was not a remote
collector or terminal-emulator workaround.

The explicit `gpui-pre-linux` patch bounds each native pass to 32 raw events,
retains trailing repeat/keymap state and schedules one explicitly woken weak
continuation for buffered XCB events. Application queue sizes, source authority,
input ordering and replay policy are unchanged. The published archive checksum,
Apache-2.0 license and original file hashes are retained; exactly one upstream
source file is patched. Four added regressions cover finite dispatch,
repeat/keymap/client-message ordering and real calloop continuation fairness and
wakeup. Actual IME-server acceptance is separate from the unchanged XIM handler
and its ordered-message unit coverage.

Frozen source `b78f81086532248209223aaebe4a86a31c793746` (equivalent application
and SDK files in integration commit `a4621ce`) uses GUI SHA-256
`2c60541c6d109195e71be02264c5aa32848feac734e64a4cff26322611628122`,
source-manifest SHA-256
`dd12888f8fa0f4aebf8d84660d2dd494450d414f5f6aed1efc2cc0bee7e1a932`
and the unchanged fixture backend above. Although the lockfile now selects the
patched GUI dependency, all 271 resolved packages reachable from the backend
remain identical to its earlier freeze.

The owned X11 fixture keeps the same 25-ms xdotool setting (measured callbacks
about 13 ms apart). While B's private bridge is paused, deliberate creation adds
only its pending reference and A accepts terminal input. After release, exactly
one host-named B Screen/session appears, existing local/A/B work is unchanged and
A still accepts input without selection takeover. After opening the new Screen,
the fixture renders the hover-only Detach action before clicking, requires the
window title to switch to the old B Screen, verifies the new Drawer row is gone
and runs a marker in the old B session. Hosts Reopen returns to the new Screen's
original PID/pane/session. Normal window close preserves all host sessions,
then owned fixture cleanup completes.

This strengthened gate avoids the earlier harness false positive, where a
premature hover-action click could leave the Screen open and make Reopen a no-op.
Result SHA-256:
`d46f8af214a9da5ed6316fa1863d9e057a96529f8868631b688610cdce1dd28f`;
harness SHA-256:
`aef7d9f5fa15da16cda3d05034872de09707ceb5da0655ac21c842ef71ebb369`.
The fixture uses an owned SSH-shape shim and software rendering; real OpenSSH,
IME servers, other platforms and smoothness are not established by this run.

Integration checks after the patch pass: 267 client-core and 187 GPUI tests,
8 targeted platform tests plus 2 matching GPUI tests, strict Core/GPUI/platform
all-target Clippy, 17 architecture tests, formatting and both affected strict
OpenSpec validations. Full workspace CI remains the later checkpoint.

### Native remote Screen rename, in-process checkpoint (2026-11-08)

The existing inline Drawer editor now captures the window owner, exact source
connection and Screen key for local and opened remote Screens. Core admission
requires a ready, canonically present remote reference and the existing bounded
name; the remote runtime sends the existing `rename_screen` operation through
that captured source. The old label stays until a catalog observation reports
the effective name. No new naming store, reconnect replay or wire operation was
added.

Focused checks pass: 18 Drawer production-view tests, the client-core rename
admission test, and the in-process remote stream test extended with colliding
local/remote Screen IDs, stale connection and pending-close refusal, and the
canonical renamed name. Strict all-target client-core/GPUI Clippy, 17
architecture tests, formatting, LikeC4 validation and strict OpenSpec change
validation pass. The view tests exercise a real double-click, context menu/F2
entry, Enter and Escape, retained input/focus through unrelated reconciliation,
invalid-name and full-queue refusal, and disconnect cancellation. A 200px
Drawer with a long remote title and Loading status retains a stable select
hitbox and keeps both text and status outside its fixed action lane; the full
title remains available as a tooltip. Blur uses the same retained editor event
subscription as Enter, but a separate active-window Blur input test has not yet
been run. Actual remote-window acceptance and the deferred full workspace CI
remain separate checkpoints.
