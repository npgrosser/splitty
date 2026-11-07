# Native GPUI development app

The development app opens an isolated local Screen with real service-owned
terminal sessions, split panes, tab groups and draggable dividers. Its layout
comes from the shared mux tree; existing terminal views survive layout updates.
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

The default window offers **Split right**, **Split below** and **New tab**.
Select tabs by clicking their labels, close panes with their tab close control,
and resize splits by dragging the divider. After closing the final pane,
**New terminal** repopulates the same Screen. Ctrl+Shift+C/V copies and pastes
in the focused terminal. This increment has one local Screen; hosts, saved
workspace restoration, drag-and-drop, scratchpad and `splitty open` integration
remain subsequent migration work.

Workspace shortcuts use **Cmd** on macOS and **Alt** on other hosts as `Mod`:

| Shortcut | Action |
| --- | --- |
| Mod+T | New tab (or terminal on an empty Screen) |
| Mod+D / Mod+Shift+D | Split right / below |
| Mod+W | Close the focused pane |
| Mod+Shift+Left/Right/Up/Down | Focus the nearest visible split group in that direction |

These chords are handled before terminal input; Ctrl+C and clipboard keys keep
their terminal behavior. Tab selection remains available by clicking its label.

`--workspace-smoke` exercises four distinct terminal sessions, both split axes,
tab switching, authoritative divider ratios, pane closure, keyboard routing and
preserved terminal identity. It requires fresh visible paints and joined service cleanup. The smoke
driver selects this workflow with `--workspace`; omit it for the legacy fixture.

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

## Local workspace acceptance

Repository checks passed: 1,247 workspace Rust tests (including the architecture
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
