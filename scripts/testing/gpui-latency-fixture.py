#!/usr/bin/env python3
"""Quiet raw-PTY reply fixture for normal Workspace input latency."""
import json
import os
from pathlib import Path
import sys
import termios
import tty

out = Path(sys.argv[1])
fd = sys.stdin.fileno()
previous = termios.tcgetattr(fd)
seq = 0
keys = []
try:
    tty.setraw(fd)
    # Clear the shell command, hide the cursor, and mark one fixed cell row.
    # The harness crops only that row, so the reply is its sole changing content.
    os.write(1, b"\x1b[?25l\x1b[2J\x1b[H\x1b[10;5HSEQ:000000:READY")
    (out / "ready").write_text("ready\n")
    while True:
        key = os.read(fd, 1)
        if key == b"q":
            break
        if not key:
            break
        seq += 1
        char = key.decode("ascii", "replace")
        keys.append(char)
        os.write(1, f"\x1b[10;5HSEQ:{seq:06d}:{char}    ".encode())
        temporary = out / "ack.tmp"
        temporary.write_text(json.dumps({"seq": seq, "key": char}))
        temporary.replace(out / "ack.json")
finally:
    os.write(1, b"\x1b[?25h\x1b[0m")
    termios.tcsetattr(fd, termios.TCSANOW, previous)
    (out / "done.json").write_text(json.dumps({"count": seq, "keys": keys}))
