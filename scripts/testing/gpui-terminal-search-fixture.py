#!/usr/bin/env python3
"""Owned raw PTY content and byte recorder for gpui-terminal-search.py."""

import fcntl
import json
import os
from pathlib import Path
import select
import struct
import sys
import termios
import tty


COPY_WORD = "COPYTOKEN4826"


def write(data):
    output = sys.stdout.fileno()
    while data:
        data = data[os.write(output, data):]


def geometry(fd):
    rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, bytes(8)))
    return rows, cols


def initial_content(cols):
    # Real scrollback precedes a stable visible screen. None of these lines is
    # a test-only app hook; the terminal receives ordinary VT output.
    for index in range(60):
        text = f"history-{index:03d} " + ("needle" if index % 10 == 0 else "other")
        write((text + "\r\n").encode())
    write(b"\x1b[2J\x1b[H")
    write(("\x1b[1;1Hneedle visible\x1b[2;1HNEEDLE upper\x1b[3;1H" + COPY_WORD +
           "\x1b[4;1H\u754ce\u0301 needle unicode").encode())
    # No CR/LF divides this word: it must cross a terminal soft wrap.
    write(("\x1b[6;1H" + "x" * (cols - 4) + "softwrap").encode())
    write(b"\x1b[?1003h\x1b[?1006h\x1b[?25l")


def main():
    owned = Path(sys.argv[1]).resolve(strict=True)
    stdin = sys.stdin.fileno()
    original = termios.tcgetattr(stdin)
    rows, cols = geometry(stdin)
    if rows < 12 or cols < 20:
        raise RuntimeError(f"terminal too small for fixture: {cols}x{rows}")
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
                        if action == "emit":
                            write(b"\x1b[9;1Hneedle after output")
                        elif action == "dense":
                            line = ("a" * (cols - 1) + "\r\n").encode()
                            for _ in range(300):
                                write(line)
                        elif action == "reset":
                            # DEC RIS clears the terminal's old screen and
                            # retained history, then leaves one new match.
                            write(b"\x1bc\x1b[1;1Hneedle after reset")
                        elif action == "mouse_on":
                            write(b"\x1b[?1003h\x1b[?1006h")
                        else:
                            raise ValueError(f"unknown fixture action: {action}")
                        next_ack = owned / "ack.next"
                        next_ack.write_text(json.dumps({"seq": sequence, "action": action}))
                        next_ack.replace(ack)
                readable, _, _ = select.select([stdin], [], [], .02)
                if readable:
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
