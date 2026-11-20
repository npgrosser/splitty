# Install

Download the package for your system from the
[latest release](https://github.com/npgrosser/splitty/releases/latest), or with
the GitHub CLI, for example
`gh release download --repo npgrosser/splitty --pattern 'Splitty_*_aarch64.dmg'`.

| System | File | Notes |
|---|---|---|
| Windows 10/11 (x64) | `Splitty_<version>_x64-setup.exe` | Local terminals run in WSL 2 (Ubuntu or Debian): the app uses the default distribution and installs its backend there. To update, run the newer installer; it replaces an installed earlier version. |
| macOS | `Splitty_<version>_aarch64.dmg` (Apple silicon) or `Splitty_<version>_x64.dmg` (Intel) | Clear the quarantine flag after installing (below). |
| Linux (x86_64) | `splitty_<version>_amd64.deb`, `splitty-<version>-1.x86_64.rpm` or `splitty-<version>-linux-x86_64.tar.gz` | Install the deb (Debian, Ubuntu) or rpm (Fedora, openSUSE) with the system's package manager and start Splitty from the application menu or as `splitty-gpui`. The tar.gz runs from where it is unpacked: `./splitty-gpui`. |

The app draws its own interface on the GPU (Vulkan on Linux, Wayland or X11)
and needs no browser engine. The deb and rpm install to `/opt/splitty` and name
the libraries they need; with the tar.gz, keep the `splitty` command and the
`shell-integration/` folder next to `splitty-gpui`.

The binaries are not code-signed. On macOS, clear the quarantine flag after
installing:

```bash
xattr -dr com.apple.quarantine /Applications/Splitty.app
```

## The `splitty` command

Day to day, everything happens in the app. The `splitty` command adds a few
things on top:

| Command | What it does |
|---|---|
| `splitty attach` | Shows a session in a plain terminal, without the app |
| `splitty daemon start` | Starts an independent persistent local backend (macOS, Linux) |
| `splitty open <path>` | From inside a pane: shows a file in a viewer pane next to it; folders and `--editor` go to your editor |
| `splitty run`, `splitty logs` | Managed background processes, mostly for agents |

Everything about the command is in [The `splitty` command](cli.md).

Terminals of the app and on remote hosts have the command already. The app
does not put it on the `PATH` of other terminals; take it from the archive of
the same release:

```bash
# Linux x86_64; on other systems splitty-wsl-aarch64 or splitty-darwin-<arch>
mkdir -p ~/.local/share/splitty-cli
tar -xzf splitty-wsl-x86_64.tar.gz -C ~/.local/share/splitty-cli
export PATH="$HOME/.local/share/splitty-cli:$PATH"   # add this to your shell profile
```

Keep the `shell-integration/` folder next to the binary. On a Mac, if the
browser downloaded the archive, run
`xattr -dr com.apple.quarantine ~/.local/share/splitty-cli`.

## Configuration

The settings (Ctrl+,; macOS: Cmd+,) are saved in `config.toml` in
`~/.config/splitty/` (macOS: `~/Library/Application Support/splitty/`).
Themes go into a `themes/` folder next to it ([Themes](themes.md)), key
bindings into `keybindings.json` ([Keyboard](keyboard.md)).

## Building from source

You need Rust (stable), Node.js 24 with pnpm for the architecture check only,
Python 3 and a C toolchain for the terminal library and, on Linux, the
development packages listed in [Development](development.md#the-native-app).

```bash
make native-package       # deb, rpm and tar.gz (Linux) or dmg (macOS) in build/
make install-no-gui       # only splitty, to ~/.local/bin
```

Running the app from a checkout, the Windows installer and releases are
described in [Development](development.md). A build of your own installs
itself on remote hosts only with a payload: set `SPLITTY_REMOTE_PAYLOAD` as
[Remote development](remote.md#installing-splitty-on-hosts) explains, or put
`splitty` on the host's `PATH`.
