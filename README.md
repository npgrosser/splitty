# Splitty

A graphical terminal multiplexer for Linux, macOS and Windows.

![Several screens with terminals in splits and tabs](https://github.com/npgrosser/splitty/raw/media/overview.webp)

## Install

Download the package for your system from the
[latest release](https://github.com/npgrosser/splitty/releases/latest):
Windows 10/11 (with WSL 2), macOS (Apple silicon and Intel) and Linux (deb,
rpm, tar.gz). The notes for each system are in [Install](docs/install.md).

> A personal project in active development (0.x), used daily on all three
> systems. Expect rough edges and changes between versions.

## A terminal

![One terminal with a Git graph, a test run and a diff](https://github.com/npgrosser/splitty/raw/media/terminal.webp)

Written in Rust. Terminal emulation by [libghostty](https://ghostty.org).

## Full keyboard control over splits and tabs

![Splits and tabs opened with keys, a pane maximized and restored](https://github.com/npgrosser/splitty/raw/media/splits.webp)

Every key can be changed: [Keyboard](docs/keyboard.md).

## Or arrange them with the mouse

![Tabs dragged out of a group into splits, splits resized](https://github.com/npgrosser/splitty/raw/media/pointer.webp)

## Multiple screens, each with its own layout

![Named screens in the drawer, switched from the keyboard and the pointer](https://github.com/npgrosser/splitty/raw/media/screens.webp)

## Remote hosts beside your local screens

![A screen of an SSH host opened next to the local ones](https://github.com/npgrosser/splitty/raw/media/remote.webp)

Splitty connects with your `ssh` and installs itself on the host:
[Remote development](docs/remote.md).

## Remote sessions survive a disconnect

![A host is closed and opened again; its programs kept running](https://github.com/npgrosser/splitty/raw/media/reconnect.webp)

Sessions on the host keep running without tmux or screen.

## Remote ports are detected and forwarded with a click

![A port a program opened on the host is forwarded and reached locally](https://github.com/npgrosser/splitty/raw/media/ports.webp)

A remote terminal's folder also opens in VS Code or Zed on your computer:
[Files and editors](docs/files-and-editors.md).

## Customize themes and keys

![Themes change the whole window; the settings list every key](https://github.com/npgrosser/splitty/raw/media/customize.webp)

[Themes](docs/themes.md) · [Keyboard](docs/keyboard.md)

## More

| | |
|---|---|
| [Guide](docs/guide.md) | First steps, templates, how it works |
| [Install](docs/install.md) | Packages, the `splitty` command, building from source |
| [Remote development](docs/remote.md) | Remote screens, ports, host requirements |
| [Files and editors](docs/files-and-editors.md) | Uploads, screenshots, `splitty download`, `splitty open`, editors |
| [The `splitty` command](docs/cli.md) · [Managed jobs](docs/cli-jobs.md) | `attach`, `daemon`, in-pane commands, `splitty run` |
| [Keyboard](docs/keyboard.md) · [Themes](docs/themes.md) | Every command and its keys; themes and palettes |
| [Security](SECURITY.md) | Threat model and guarantees |
| [Known issues](docs/known-issues.md) | Platform quirks, with workarounds |
| [Architecture](docs/architecture.md) · [Networking](docs/networking.md) · [Development](docs/development.md) | How it is built |

Bugs and ideas are welcome as
[GitHub issues](https://github.com/npgrosser/splitty/issues). MIT, see
[LICENSE](LICENSE).
