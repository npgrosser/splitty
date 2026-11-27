# Install

Every release has the desktop app for these systems; the commands below
install the latest one. The files themselves are on the
[latest release](https://github.com/npgrosser/splitty/releases/latest) page.

| System | How |
|---|---|
| [Windows 10/11](#windows) (x64, with WSL 2) | Installer |
| [macOS](#macos) (Apple silicon, Intel) | Disk image |
| [Debian, Ubuntu](#debian-ubuntu) (x86_64) | deb |
| [Fedora, openSUSE](#fedora-opensuse) (x86_64) | rpm |
| [Any other Linux](#any-other-linux) (x86_64) | tar.gz with an installer |
| [NixOS, Nix](#nix) (x86_64) | Flake |
| [Everything else](#not-supported) | Not supported |

The app draws its own interface on the GPU (Vulkan on Linux, on Wayland or
X11) and needs no browser engine. The binaries are not code-signed.

## Windows

Windows 10 or 11 on x64. Local terminals run in WSL 2 (Ubuntu or Debian): the
app uses the default distribution and installs its backend there. Without a
distribution, install one first with `wsl --install` and restart.

In PowerShell:

```powershell
$release = Invoke-RestMethod https://api.github.com/repos/npgrosser/splitty/releases/latest
$setup = $release.assets | Where-Object name -like '*_x64-setup.exe'
Invoke-WebRequest $setup.browser_download_url -OutFile "$env:TEMP\$($setup.name)"
Start-Process "$env:TEMP\$($setup.name)"
```

To update, run the same commands: the installer replaces an installed earlier
version. To remove Splitty, use "Installed apps" in the Windows settings.

## macOS

Apple silicon and Intel. In a terminal:

```bash
arch=$(uname -m | sed 's/arm64/aarch64/; s/x86_64/x64/')
version=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/npgrosser/splitty/releases/latest | sed 's|.*/v||')
curl -fLO "https://github.com/npgrosser/splitty/releases/download/v$version/Splitty_${version}_$arch.dmg"
hdiutil attach "Splitty_${version}_$arch.dmg" -nobrowse -mountpoint /Volumes/splitty-install
cp -R /Volumes/splitty-install/Splitty.app /Applications/
hdiutil detach /Volumes/splitty-install
xattr -dr com.apple.quarantine /Applications/Splitty.app
```

The last line clears the quarantine flag, which macOS sets on apps that are
not signed by a registered developer; run it also after dragging the app from
a disk image the browser downloaded. To update, move the old
`/Applications/Splitty.app` to the Trash and run the commands again; to remove
Splitty, move it to the Trash.

## Debian, Ubuntu

x86_64, Ubuntu 22.04 or Debian 12 and later:

```bash
version=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/npgrosser/splitty/releases/latest | sed 's|.*/v||')
curl -fLO "https://github.com/npgrosser/splitty/releases/download/v$version/splitty_${version}_amd64.deb"
sudo apt install "./splitty_${version}_amd64.deb"
```

Start Splitty from the application menu or as `splitty-gpui`. The same
commands update it; `sudo apt remove splitty` removes it. The package installs
to `/opt/splitty` and names the libraries it needs.

## Fedora, openSUSE

x86_64. On Fedora:

```bash
version=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/npgrosser/splitty/releases/latest | sed 's|.*/v||')
sudo dnf install "https://github.com/npgrosser/splitty/releases/download/v$version/splitty-$version-1.x86_64.rpm"
```

On openSUSE (the package is not signed):

```bash
version=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/npgrosser/splitty/releases/latest | sed 's|.*/v||')
sudo zypper install --allow-unsigned-rpm "https://github.com/npgrosser/splitty/releases/download/v$version/splitty-$version-1.x86_64.rpm"
```

Start Splitty from the application menu or as `splitty-gpui`. The same
commands update it; `sudo dnf remove splitty` or `sudo zypper remove splitty`
removes it.

## Any other Linux

x86_64 with glibc 2.35 or later, for example Arch. The installer downloads the
release's tar.gz, checks it against the release's checksums and installs it
for your user under `~/.local`, with a desktop entry, without `sudo`:

```bash
curl -fsSL https://raw.githubusercontent.com/npgrosser/splitty/main/packaging/linux/native/install.sh | sh
```

Start Splitty from the application menu or as `~/.local/bin/splitty-gpui`.
The same command updates it, and this one removes it:

```bash
curl -fsSL https://raw.githubusercontent.com/npgrosser/splitty/main/packaging/linux/native/install.sh | sh -s -- --uninstall
```

The app needs libxcb, libxkbcommon with its X11 part, Wayland, EGL, the Vulkan
loader and a Vulkan driver, which a desktop system usually has; the installer
says so when the app does not start. On Arch:

```bash
sudo pacman -S --needed libxcb libxkbcommon libxkbcommon-x11 wayland libglvnd vulkan-icd-loader
```

The same installer is in the tar.gz, for a download of your own:
`tar -xzf splitty-<version>-linux-x86_64.tar.gz` and then
`./splitty-<version>-linux-x86_64/install.sh`. `--prefix DIR` installs
somewhere else and `--version VERSION` another release. The app also runs
from the unpacked folder without installing (`./splitty-gpui`), as long as
the `splitty` command and `shell-integration/` stay next to it.

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

## Not supported

There is no desktop app for these, and the commands above do not work there:

| System | What you can do |
|---|---|
| Linux on aarch64 (ARM) | No package. Such a machine works as a remote host of a supported desktop ([Remote development](remote.md)). |
| Linux with a glibc before 2.35 (Ubuntu 20.04, Debian 11, RHEL 9 and earlier) | The packages do not start. The only route is a [build from source](#building-from-source) on that system. |
| Windows without WSL 2 | The app needs WSL 2 for its local terminals. |
| Windows on ARM | No package. |
| FreeBSD and other systems | No package and no build. |

Remote hosts are a separate matter: any Linux or macOS machine on x86_64 or
aarch64 that you reach with `ssh` works as one, whatever its distribution.

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

Terminals of the app and on remote hosts have the command already. The
installer for [any other Linux](#any-other-linux), the Nix packages and
`make install` also link it into their `bin` directory. The other packages do
not put it on the `PATH` of other terminals; take it from the archive of the
same release:

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
