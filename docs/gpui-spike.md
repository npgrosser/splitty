# Native GPUI development app

The development app opens an isolated workspace with real service-owned terminal
sessions, local Screens, split panes, tab groups and draggable dividers. Layouts
come from the shared mux tree; terminal views survive layout updates and Screen
switches.
Closing the window ends its owned service. Saved production workspaces and the
released Electron/Tauri apps remain separate.

The [native migration proposal](../openspec/changes/native-ui-gpui/proposal.md#terminal-engine-decision)
selects Ghostty's native terminal core. The current terminal uses statically
linked `libghostty-vt` 1.0.0 through a UI-independent owned engine. GPUI handles
input and drawing, caching shaped content for unchanged visible rows.

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

The default window offers **Split right**, **Split below**, **New tab** and
**Maximize/Restore** for the focused tab group.
Select tabs by clicking their labels, close panes with their tab close control,
and resize splits by dragging the divider. After closing the final pane,
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
pending, request close again. Hosts, saved workspace restoration, scratchpad and
`splitty open` integration remain subsequent work.

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
UI generic. Terminal typography remains the existing monospace font at 15 px
with 21 px rows. A proportional UI font therefore does not alter terminal cells.
Terminal-font settings and zoom are separate work.

Native colors support checked `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`, decimal
`rgb(...)`/`rgba(...)` with comma-separated channels, and `transparent`. Alpha is
composed against the selected background. The terminal receives foreground,
background, cursor, selection and ANSI 0–15 colors; its standard indices 16–255
remain unchanged. Native cursor drawing uses an overlay, so `cursor-accent`
text inversion is not implemented. CSS expressions, layout/effect tokens and
the otherwise unused `terminal.theme` field are not activated by this adapter.
Unsupported color/font forms use native fallbacks with a status; failed reads
retain working defaults or the last valid appearance. The appearance reader
never submits a settings mutation. Existing service startup may still persist
configuration defaults or migrations through the ordinary settings loader.

There is no native settings editor in this increment. The web client's settings
remain the reference interface; the native read path and terminal engine consume
the existing settings and theme contracts.

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
cleanly. This verifies the cursor-key fallback; native application mouse reporting
remains unimplemented.

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
