"""Real child-pipe fixture for hub tests; speaks the published native v1 wire."""
import json
import os
import struct
import sys

reader, writer = sys.stdin.buffer, sys.stdout.buffer


def read_exact(size):
    data = reader.read(size)
    if len(data) != size:
        raise EOFError()
    return data


def read_frame():
    kind, size = struct.unpack(">BI", read_exact(5))
    assert size <= 16 * 1024 * 1024 - 5
    return kind, read_exact(size)


def write_frame(kind, body):
    writer.write(struct.pack(">BI", kind, len(body)) + body)
    writer.flush()


def control(kind, value):
    write_frame(kind, json.dumps(value, separators=(",", ":")).encode())


ready = json.loads(sys.argv[1])
generation = ready["generation"]
pending = set()


def data(kind, body):
    receipt = os.urandom(16)
    pending.add(receipt.hex())
    write_frame(kind, struct.pack(">I", generation) + body + receipt)


try:
    assert read_exact(8) == b"SPLITTY\x01"
    kind, body = read_frame()
    assert kind == 0 and json.loads(body) == {"transportVersion": 1}
    if sys.argv[2] == "__REVOKE_DURING_PROGRESS__":
        # One write makes Progress and Ready available in the same read poll.
        # Ignore stdin/EOF so the owner's failure path must reap the process.
        import time
        def encoded(kind, value):
            body = json.dumps(value, separators=(",", ":")).encode()
            return struct.pack(">BI", kind, len(body)) + body
        writer.write(b"SPLITTY\x01" + encoded(3, {"transportVersion": 1}) +
                     encoded(11, {"phase": "connecting", "message": "prepared"}) +
                     encoded(4, ready))
        writer.flush()
        time.sleep(30)
        sys.exit(0)
    writer.write(b"SPLITTY\x01")
    control(3, {"transportVersion": 1})
    control(4, ready)
    kind, body = read_frame()
    assert kind == 13 and json.loads(body) == {
        "generation": generation, "instance": ready["instance"]
    }
    if sys.argv[2] == "__OWNER_CLEANUP__":
        # Real child EOF cleanup lasts longer than an ordinary relay's grace.
        # The owner must let cleanup finish, while a relay may be killed.
        while True:
            read_frame()
    if sys.argv[2] == "__IGNORE_EOF__":
        import time
        time.sleep(30)
        sys.exit(0)
    if sys.argv[2] == "__WELCOME_THEN_IGNORE_EOF__":
        import time
        data(1, b'{"type":"mux.welcome"}')
        time.sleep(30)
        sys.exit(0)
    if sys.argv[2] == "__INVALID_FRAME_EXIT_255__":
        # A valid Ready followed by invalid framing must take precedence over
        # a simultaneous generic SSH failure status.
        write_frame(255, b"")
        sys.exit(255)
    data(1, sys.argv[2].encode())
    while True:
        kind, body = read_frame()
        if kind == 8:
            control(9, json.loads(body))
        elif kind == 10:
            credit = json.loads(body)
            assert credit["generation"] == generation
            pending.remove(credit["receipt"])
        else:
            assert kind == 1 and struct.unpack(">I", body[:4])[0] == generation
            control(10, {"generation": generation, "receipt": body[-16:].hex()})
            message = json.loads(body[4:-16])
            if message.get("type") == "input" and message.get("id") == "__emit__":
                # The test speaks for the daemon: its data goes out as an event.
                data(1, message["data"].encode())
                continue
            data(1, body[4:-16])
            if json.loads(body[4:-16]).get("type") == "input":
                data(2, bytes([0, 1, 2, 255]))
except EOFError:
    if sys.argv[2] == "__OWNER_CLEANUP__":
        import time
        time.sleep(0.75)
        with open(sys.argv[3], "w") as output:
            output.write("cleanup completed")
