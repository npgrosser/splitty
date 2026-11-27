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
| NixOS, Nix (x86_64) | no file: the repository is a flake | See [Nix](#nix) below. |

The app draws its own interface on the GPU (Vulkan on Linux, Wayland or X11)
and needs no browser engine. The deb and rpm install to `/opt/splitty` and name
the libraries they need; with the tar.gz, keep the `splitty` command and the
`shell-integration/` folder next to `splitty-gpui`.

The binaries are not code-signed. On macOS, clear the quarantine flag after
installing:

```bash
xattr -dr com.apple.quarantine /Applications/Splitty.app
```

## Nix

The repository is a flake for x86_64 Linux with two packages:

| Package | What it is |
|---|---|
| `splitty-bin` (the default) | The published release: its Linux tar.gz, pointed at the libraries of the Nix store. Installs in seconds. |
| `splitty` | The app built from the flake's own revision. Takes a while to compile. |

```bash
nix run github:npgrosser/splitty                # the release, without installing
nix profile install github:npgrosser/splitty    # installs splitty-gpui and splitty
nix build github:npgrosser/splitty#splitty      # from source
```

In a NixOS configuration, add the flake as an input and its package to the
system's or a user's packages:

```nix
{
  inputs.splitty.url = "github:npgrosser/splitty";
  # in a module, with the flake's inputs passed to it:
  environment.systemPackages = [ inputs.splitty.packages.x86_64-linux.default ];
}
```

Both packages put `splitty-gpui` and `splitty` on the `PATH` and bring the
desktop entry. The app needs a Vulkan driver, which NixOS provides with
`hardware.graphics.enable`. `splitty-bin` follows the release named in
`packaging/nix/release.json`, which is updated after each release. `splitty`
has the servers for remote hosts built in only while its revision has the
version of that release (the archives are the release's); at another version
it is like any [build from source](#building-from-source).

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
make build                # the app and splitty, into target/release
make install              # Linux: installs them to ~/.local (PREFIX=/usr/local for another place)
make uninstall            # removes what make install put there

make native-package       # deb, rpm and tar.gz (Linux) or dmg (macOS) in build/
make install-no-gui       # only splitty, to ~/.local/bin
```

`make install` puts the app, the `splitty` command and `shell-integration/`
into `$PREFIX/lib/splitty`, links `splitty-gpui` and `splitty` into
`$PREFIX/bin` and installs the desktop entry and the icon under
`$PREFIX/share`. It installs what `make build` built and compiles nothing, so
`sudo make install PREFIX=/usr/local` builds nothing as root; `DESTDIR` stages
the files for a package. With Nix, `nix develop` opens a shell with everything
the build needs and `nix build .#splitty` builds the package ([Nix](#nix)).

Running the app from a checkout, the Windows installer and releases are
described in [Development](development.md). A build of your own installs
itself on remote hosts only with a payload: set `SPLITTY_REMOTE_PAYLOAD` as
[Remote development](remote.md#installing-splitty-on-hosts) explains, or put
`splitty` on the host's `PATH`.
