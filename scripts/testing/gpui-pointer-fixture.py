#!/usr/bin/env python3
"""Raw PTY endpoint for gpui-pointer-input.py; all files stay in its owned directory."""

import json
import os
from pathlib import Path
import select
import sys
import termios
import time
import tty


MARKER = "POINTERMARK4826"
MODES = (9, 1000, 1002, 1003, 1005, 1006, 1015, 1016, 2004)


def write(data):
    os.write(sys.stdout.fileno(), data)


def configure(modes):
    # Explicitly reset every mode used by the driver between stages.
    write(b"".join(f"\x1b[?{mode}l".encode() for mode in MODES))
    write(b"".join(f"\x1b[?{mode}h".encode() for mode in modes))


def main():
    directory = Path(sys.argv[1]).resolve(strict=True)
    control = directory / "control.json"
    ack = directory / "ack.json"
    received = directory / "received.bin"
    stdin = sys.stdin.fileno()
    original = termios.tcgetattr(stdin)
    sequence = 0
    with received.open("wb", buffering=0) as log:
        try:
            tty.setraw(stdin)
            write(b"\x1b[?25l\x1b[2J\x1b[H")
            write(f"\x1b[3;1H{MARKER}\x1b[5;1HPointer fixture ready".encode())
            (directory / "ready").write_text(str(os.getpid()))
            while True:
                if control.exists():
                    command = json.loads(control.read_text())
                    if command["seq"] > sequence:
                        sequence = command["seq"]
                        if command.get("stop"):
                            break
                        modes = command.get("modes", [])
                        if any(mode not in MODES for mode in modes):
                            raise ValueError(f"unsupported fixture mode: {modes}")
                        configure(modes)
                        # ack follows the terminal mode writes in PTY order.
                        next_ack = directory / "ack.next"
                        next_ack.write_text(json.dumps({"seq": sequence, "modes": modes}))
                        next_ack.replace(ack)
                readable, _, _ = select.select([stdin], [], [], .02)
                if readable:
                    data = os.read(stdin, 65536)
                    if not data:
                        break
                    log.write(data)
        finally:
            try:
                configure([])
                write(b"\x1b[?25h\x1b[2J\x1b[H")
            except OSError:
                # The owner can close the PTY while the fixture is exiting.
                pass
            termios.tcsetattr(stdin, termios.TCSANOW, original)


if __name__ == "__main__":
    main()
