#!/usr/bin/env python3
"""Owned raw PTY content and byte recorder for gpui-terminal-links.py."""

import fcntl
import json
import os
from pathlib import Path
import select
import struct
import sys
import termios
import tty


OSC_TARGET = "https://example.invalid/osc-target-4826?from=label"
PLAIN_TARGET = "http://example.invalid/plain-4826?x=1"
WRAP_TARGET = "https://example.invalid/softwrap-4826?q=1"
OLD_TARGET = "https://example.invalid/old-4826"
NEW_TARGET = "https://example.invalid/new-4826"


def write(data):
    fd = sys.stdout.fileno()
    while data:
        data = data[os.write(fd, data):]


def osc8(label, target):
    return (f"\x1b]8;;{target}\x1b\\{label}\x1b]8;;\x1b\\").encode()


def initial_content(cols):
    write(b"\x1b[2J\x1b[H")
    write(b"\x1b[1;1H")
    write(osc8("OSC_LABEL", OSC_TARGET))
    write(("\x1b[2;1H" + PLAIN_TARGET).encode())
    # The full URL spans rows 3 and 4 without a hard newline.
    # Whitespace keeps the URL's first byte a real token boundary. Its last
    # bytes then wrap naturally onto row 4 without an explicit newline.
    write(("\x1b[3;1H" + " " * (cols - 15) + WRAP_TARGET).encode())
    write(b"\x1b[5;1H")
    write(osc8("https://example.invalid/decoy-4826", "javascript:alert(1)"))
    write(b"\x1b[6;1Hfile:///tmp/splitty-link-4826 vscode://file/hidden javascript:alert(2)")
    write(b"\x1b[7;1H")
    write(osc8("MUTABLE", OLD_TARGET))
    write(b"\x1b[8;1HSELECTWORD4826 neutral terminal text")
    write(b"\x1b[?1003h\x1b[?1006h\x1b[?25l")


def main():
    owned = Path(sys.argv[1]).resolve(strict=True)
    stdin = sys.stdin.fileno()
    original = termios.tcgetattr(stdin)
    rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(stdin, termios.TIOCGWINSZ, bytes(8)))
    if rows < 12 or cols < 40:
        raise RuntimeError(f"terminal too small: {cols}x{rows}")
    (owned / "geometry.json").write_text(json.dumps({"cols": cols, "rows": rows}))
    control = owned / "control.json"
    ack = owned / "ack.json"
    sequence = 0
    with (owned / "received.bin").open("wb", buffering=0) as received:
        try:
            tty.setraw(stdin)
            initial_content(cols)
            (owned / "ready").write_text(str(os.getpid()))
            while True:
                if control.exists():
                    request = json.loads(control.read_text())
                    if request["seq"] > sequence:
                        sequence = request["seq"]
                        action = request["action"]
                        if action == "stop":
                            break
                        if action == "mutate":
                            write(b"\x1b[7;1H")
                            write(osc8("MUTABLE", NEW_TARGET))
                        else:
                            raise ValueError(f"unknown fixture action: {action}")
                        next_ack = owned / "ack.next"
                        next_ack.write_text(json.dumps({"seq": sequence, "action": action}))
                        next_ack.replace(ack)
                ready, _, _ = select.select([stdin], [], [], .02)
                if ready:
                    data = os.read(stdin, 65536)
                    if not data:
                        break
                    received.write(data)
        finally:
            try:
                write(b"\x1b[?1003l\x1b[?1006l\x1b[?25h\x1b[2J\x1b[H")
            except OSError:
                pass
            termios.tcsetattr(stdin, termios.TCSANOW, original)


if __name__ == "__main__":
    main()
