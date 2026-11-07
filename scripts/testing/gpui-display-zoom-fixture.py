#!/usr/bin/env python3
"""Owned raw PTY for the private native display-zoom window check."""

import fcntl
import json
import os
from pathlib import Path
import select
import struct
import sys
import termios
import tty


LINK = "https://example.invalid/zoom-7319"


def write(data):
    fd = sys.stdout.fileno()
    while data:
        data = data[os.write(fd, data):]


def size(fd):
    rows, cols, width, height = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, bytes(8)))
    return {"rows": rows, "cols": cols, "pixelWidth": width, "pixelHeight": height}


def draw():
    write(b"\x1b[2J\x1b[H")
    write((f"\x1b]8;;{LINK}\x1b\\ZOOM_LINK\x1b]8;;\x1b\\\r\n" +
           "SELECTWORD7319\r\n" + "MMMMMMMMMMMM\r\n" * 3).encode())
    write(b"\x1b[?1003h\x1b[?1006h\x1b[?25l")


def main():
    owned = Path(sys.argv[1]).resolve(strict=True)
    pid = os.getpid()
    stdin = sys.stdin.fileno()
    original = termios.tcgetattr(stdin)
    control = owned / f"control-{pid}.json"
    ack = owned / f"ack-{pid}.json"
    sequence = 0
    last_size = None
    with (owned / f"input-{pid}.bin").open("wb", buffering=0) as received, (owned / f"sizes-{pid}.jsonl").open("w", buffering=1) as sizes:
        try:
            tty.setraw(stdin)
            write(b"HISTORY7319\r\n")
            for index in range(80):
                write((f"SCROLLBACK7319-{index:02d}\r\n").encode())
            draw()
            (owned / f"ready-{pid}.json").write_text(json.dumps({"pid": pid}))
            while True:
                current = size(stdin)
                if current != last_size:
                    sizes.write(json.dumps(current) + "\n")
                    last_size = current
                if control.exists():
                    request = json.loads(control.read_text())
                    if request["seq"] > sequence:
                        sequence = request["seq"]
                        action = request["action"]
                        if action == "stop":
                            break
                        if action == "redraw":
                            draw()
                        elif action == "emit":
                            write(("\r\nLIVE7319-" + str(sequence) + "\r\n").encode())
                        else:
                            raise ValueError(f"unknown action: {action}")
                        next_ack = owned / f"ack-{pid}.next"
                        next_ack.write_text(json.dumps({"seq": sequence, "action": action}))
                        next_ack.replace(ack)
                ready, _, _ = select.select([stdin], [], [], .03)
                if ready:
                    data = os.read(stdin, 65536)
                    if not data:
                        break
                    received.write(data)
        finally:
            try:
                write(b"\x1b[?1003l\x1b[?1006l\x1b[?25h")
            except OSError:
                pass
            termios.tcsetattr(stdin, termios.TCSANOW, original)


if __name__ == "__main__":
    main()
