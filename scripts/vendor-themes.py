#!/usr/bin/env python3
"""Refresh the third-party themes bundled with the native app.

Usage: vendor-themes.py [--kit <checkout>] [--palettes <checkout>]

Without a checkout the script clones the upstream repository (the themes
directory only) into a temporary directory.

- UI themes: every theme set in the toolkit repository's `themes/`.
- Terminal palettes: the Ghostty-format palettes of iTerm2-Color-Schemes whose
  entry in that repository's `licenses.yml` names a license from `ALLOWED`.
  Palettes with an unknown, non-free or copyleft license are left out; a user
  can still place them in the themes directory.

The Splitty themes (`ui/splitty.json`, `terminal/splitty/`) are authored here
and are not touched.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

KIT_URL = "https://github.com/longbridge/gpui-kit.git"
PALETTES_URL = "https://github.com/mbadolato/iTerm2-Color-Schemes.git"
ALLOWED = {
    "0BSD",
    "Apache-2.0",
    "CC0-1.0",
    "ISC",
    "MIT",
    "MIT OR Apache-2.0",
    "Unlicense",
    "WTFPL",
    "Zlib",
}
THEMES = Path(__file__).resolve().parent.parent / "crates/splitty-gpui/themes"


def checkout(url: str, directory: str, into: Path) -> Path:
    target = into / directory
    subprocess.run(
        ["git", "clone", "-q", "--depth", "1", "--filter=blob:none", "--sparse", url, str(target)],
        check=True,
    )
    subprocess.run(["git", "-C", str(target), "sparse-checkout", "set", directory], check=True)
    return target


def revision(repo: Path) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    ).stdout.strip()


def licenses(path: Path) -> dict[str, tuple[str, str]]:
    """`licenses.yml` as name -> (license, source). The file is a flat mapping
    of names to a few scalar fields, which this reads without a YAML library."""
    result: dict[str, tuple[str, str]] = {}
    name = None
    fields: dict[str, str] = {}

    def unquote(value: str) -> str:
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
            value = value[1:-1].replace("''", "'") if value[0] == "'" else value[1:-1]
        return value

    def flush() -> None:
        if name is not None:
            result[name] = (fields.get("license", "unknown"), fields.get("source", ""))

    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if not line.startswith(" "):
            flush()
            name, fields = unquote(line.rsplit(":", 1)[0]), {}
        else:
            match = re.match(r"\s+(\w+):\s*(.*)$", line)
            if match:
                fields[match.group(1)] = unquote(match.group(2))
    flush()
    return result


def vendor_kit(repo: Path) -> None:
    target = THEMES / "ui/gpui-kit"
    shutil.rmtree(target, ignore_errors=True)
    target.mkdir(parents=True)
    files = sorted((repo / "themes").glob("*.json"))
    for file in files:
        shutil.copyfile(file, target / file.name)
    shutil.copyfile(repo / "LICENSE-APACHE", target / "LICENSE-APACHE")
    (target / "SOURCE.md").write_text(
        f"UI theme sets from `themes/` of <{KIT_URL[:-4]}> at revision `{revision(repo)}`,\n"
        "unchanged, under the Apache License 2.0 (`LICENSE-APACHE`). Each file names the\n"
        "author and origin of its color scheme.\n",
        encoding="utf-8",
    )
    print(f"{len(files)} UI theme sets")


def vendor_palettes(repo: Path) -> None:
    target = THEMES / "terminal/iterm2"
    shutil.rmtree(target, ignore_errors=True)
    target.mkdir(parents=True)
    known = licenses(repo / "licenses.yml")
    rows = []
    for file in sorted((repo / "ghostty").iterdir(), key=lambda entry: entry.name.lower()):
        name = file.name.removesuffix(".conf")
        license_, source = known.get(name, ("unknown", ""))
        if not file.is_file() or license_ not in ALLOWED:
            continue
        # A palette is named by its file, and Windows cannot check out a
        # file whose name holds one of these characters.
        if any(char in name for char in '<>:"/\\|?*'):
            continue
        shutil.copyfile(file, target / name)
        rows.append(f"| {name} | {license_} | {source} |")
    shutil.copyfile(repo / "LICENSE", target / "LICENSE")
    (target / "SOURCE.md").write_text(
        f"Terminal palettes from `ghostty/` of <{PALETTES_URL[:-4]}> at revision\n"
        f"`{revision(repo)}`, unchanged. The collection is under the MIT license (`LICENSE`).\n"
        "Only palettes whose own license permits redistribution without further\n"
        "conditions are included, as recorded in the collection's `licenses.yml`, and\n"
        "only those whose name is a file name on every system (Windows allows no `:`):\n\n"
        "| Palette | License | Source |\n|---|---|---|\n" + "\n".join(rows) + "\n",
        encoding="utf-8",
    )
    print(f"{len(rows)} terminal palettes")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--kit", type=Path, help="checkout of the toolkit repository")
    parser.add_argument("--palettes", type=Path, help="checkout of iTerm2-Color-Schemes")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as scratch:
        vendor_kit(args.kit or checkout(KIT_URL, "themes", Path(scratch) / "kit"))
        vendor_palettes(args.palettes or checkout(PALETTES_URL, "ghostty", Path(scratch) / "palettes"))


if __name__ == "__main__":
    main()
