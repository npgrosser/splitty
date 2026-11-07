#!/usr/bin/env python3
"""Owned raw PTY for the native Screen-transfer window smoke."""

import fcntl
import json
import os
from pathlib import Path
import select
import struct
import sys
import termios
import tty


def write(data):
    fd = sys.stdout.fileno()
    while data:
        data = data[os.write(fd, data):]


def size(fd):
    rows, cols, width, height = struct.unpack(
        "HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, bytes(8)))
    return {"rows": rows, "cols": cols, "pixelWidth": width, "pixelHeight": height}


def main():
    owned = Path(sys.argv[1]).resolve(strict=True)
    pid = os.getpid()
    pane = os.environ["SPLITTY_PANE_ID"]
    session = os.environ["SPLITTY_SESSION_ID"]
    fd = sys.stdin.fileno()
    original = termios.tcgetattr(fd)
    ready = owned / f"ready-{pid}.json"
    control = owned / f"control-{pid}.json"
    ack = owned / f"ack-{pid}.json"
    sequence = 0
    with (owned / f"input-{pid}.bin").open("wb", buffering=0) as received, (
            owned / f"sizes-{pid}.jsonl").open("w", buffering=1) as sizes:
        try:
            tty.setraw(fd)
            write((f"HISTORY_{pid}\r\n" +
                   "".join(f"SCROLLBACK_{pid}_{i:02d}\r\n" for i in range(80))).encode())
            write(b"\x1b[2J\x1b[H")
            write((f"PANE_{pid}\r\nSELECT_{pid}\r\n").encode())
            write(b"\x1b[?1003h\x1b[?1006h\x1b[?25l")
            ready.write_text(json.dumps({"pid": pid, "pane": pane, "session": session}))
            last_size = None
            while True:
                current = size(fd)
                if current != last_size:
                    sizes.write(json.dumps(current) + "\n")
                    last_size = current
                if control.exists():
                    request = json.loads(control.read_text())
                    if request["seq"] > sequence:
                        sequence = request["seq"]
                        if request["action"] == "stop":
                            break
                        if request["action"] == "emit":
                            write((f"\r\nLIVE_{pid}_{sequence}\r\n").encode())
                        else:
                            raise ValueError(request["action"])
                        next_ack = owned / f"ack-{pid}.next"
                        next_ack.write_text(json.dumps({"seq": sequence}))
                        next_ack.replace(ack)
                readable, _, _ = select.select([fd], [], [], .03)
                if readable:
                    data = os.read(fd, 65536)
                    if not data:
                        break
                    received.write(data)
        finally:
            try:
                write(b"\x1b[?1003l\x1b[?1006l\x1b[?25h")
            except OSError:
                pass
            termios.tcsetattr(fd, termios.TCSANOW, original)


if __name__ == "__main__":
    main()
