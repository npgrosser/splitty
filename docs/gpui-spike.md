# GPUI phase-0 spike

This is an experimental native UI probe. It starts an isolated embedded Splitty
service for its own terminal and Markdown viewer. It does not attach to a
production daemon, migrate saved state, or replace the Tauri or Electron app.
The probe ends its service when its window closes.

## Build and run

The app pins [GPUI Kit 0.9.0](https://docs.rs/crate/gpui-kit/0.9.0/source/Cargo.toml)
(which pins `gpui-pre` 0.3.11) and
[alacritty_terminal 0.26.0](https://docs.rs/crate/alacritty_terminal/0.26.0).
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

Run from the repository root in a graphical Wayland or X11 session:

```sh
cargo run -p splitty-gpui
cargo run -p splitty-gpui -- --smoke
```

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

## Evidence and limits

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
