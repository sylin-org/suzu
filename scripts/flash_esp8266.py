#!/usr/bin/env python3
"""Disposable flash+install script for the ESP8266 OLED v2 bench unit.

Fast iteration tool — everything here gets codified into the Rust
procedure engine once it survives three clean runs.

Phases:
  1. recover   — escalate (Enter → Ctrl-C → Ctrl-B → soft reboot) until
                 the board answers a prompt ping; proves it is READING.
  2. raw       — enter raw REPL, verify with a listdir.
  3. push      — write the suzu files, read-back verified per file.
  4. verify    — soft reboot, handshake must answer proto suzu/1 with
                 the preserved device_id.
"""

import json
import serial
import sys
import time

PORT = sys.argv[1] if len(sys.argv) > 1 else "COM24"
DEVICE_ID = sys.argv[2] if len(sys.argv) > 2 else "019d9460-4561-7196-a17d-ff53458fb039"
BASE = "firmware/suzu-d/esp8266-oled-v2/"
FACEPLATE = "hardware/classes/esp8266-oled-v2/faceplates/numerals/down-mount/main.py"

p = serial.Serial(PORT, 115200, timeout=0.1)


def stamp():
    return "[%7.2f]" % (time.time() % 100)


def read_for(secs):
    got = b""
    end = time.time() + secs
    while time.time() < end:
        got += p.read(512)
    return got


def say(msg):
    print("%s %s" % (stamp(), msg))
    sys.stdout.flush()


def write_bytes(data):
    p.write(data)
    p.flush()


def esc(data):
    s = "b'"
    for b in data:
        if b == 0x5C:
            s += "\\\\"
        elif b == 0x27:
            s += "\\'"
        elif b == 0x0D:
            s += "\\r"
        elif b == 0x0A:
            s += "\\n"
        elif 0x20 <= b <= 0x7E:
            s += chr(b)
        else:
            s += "\\x" + format(b, "02x")
    return s + "'"


# ── phase 1: recover until the board proves it is READING ──

def prompt_ping():
    """Send Enter; a receptive board answers with its prompt."""
    p.reset_input_buffer()
    write_bytes(b"\r\n")
    got = read_for(1.2)
    return b">>>" in got or b">" in got


def recover():
    escalations = [
        (b"", "plain Enter"),
        (b"\x03\x03", "Ctrl-C x2 (interrupt app)"),
        (b"\x02", "Ctrl-B (leave raw)"),
        (b"\x02\x04", "soft reboot"),
        (b"\x04", "soft reboot (friendly)"),
    ]
    for data, what in escalations:
        write_bytes(data)
        say("recovery: " + what)
        time.sleep(1.0)
        read_for(0.5)
        for _ in range(2):
            if prompt_ping():
                say("recovered via: " + what)
                return True
        time.sleep(0.5)
    return False


say("phase 1 — recover until the board reads its UART")
alive = prompt_ping()
if not alive:
    alive = recover()
if not alive:
    # last resort: esptool-grade hard reset via RTS/DTR dance
    say("last resort — RTS/DTR hard reset")
    p.dtr = False
    p.rts = True
    time.sleep(0.1)
    p.dtr = True
    p.rts = False
    time.sleep(0.1)
    p.dtr = False
    time.sleep(2.5)
    read_for(1.0)
    alive = prompt_ping()
if not alive:
    raise SystemExit("board never proved it reads — power-cycle it and retry")

say("board is reading. entering raw REPL")
p.reset_input_buffer()
write_bytes(b"\x01")
got = read_for(2.0)
if b"raw REPL" not in got:
    raise SystemExit("raw entry failed: %r" % got[:120])
say("raw REPL entered")


def exec_cmd(code):
    # raw REPL ends every response with \x04> — wait for the PAIR,
    # never for a bare \x04 (that is never the final byte).
    p.reset_input_buffer()
    write_bytes(code.encode() + b"\x04")
    out = b""
    end = time.time() + 10
    while time.time() < end:
        out += p.read(512)
        if out.endswith(b"\x04>"):
            break
    if b"Traceback" in out:
        raise RuntimeError("device raised:\n" + out.decode(errors="replace"))
    return out


def write_file(name, data):
    say("writing %s (%d bytes)" % (name, len(data)))
    exec_cmd("f = open('%s','wb')" % name)
    for i in range(0, len(data), 256):
        exec_cmd("f.write(%s)" % esc(data[i : i + 256]))
    exec_cmd("f.close()")
    exec_cmd("import gc; gc.collect()")
    verify_file(name, data)
    say("verified %s" % name)


def verify_file(name, data):
    # ESP8266 heap lesson: never hexlify a whole file (23 KB alloc on an
    # 80 KB heap = MemoryError). Verify in 1 KB slices — ~2 KB peak.
    CH = 1024
    for off in range(0, len(data), CH):
        piece = data[off : off + CH]
        reply = exec_cmd(
            "import ubinascii; f=open('%s','rb'); f.seek(%d); print(ubinascii.hexlify(f.read(%d))); f.close()"
            % (name, off, len(piece))
        )
        import re

        m = re.search(rb"b'([0-9a-fA-F]*)'", reply)
        assert m, "could not parse hexlify slice: " + repr(reply[:120])
        assert bytes.fromhex(m.group(1).decode()) == piece, (
            "verify mismatch at offset %d of %s" % (off, name)
        )


# ── phase 2: identity + files ──

say("listdir: %r" % exec_cmd("import os; print(os.listdir())").strip())

suzu = {
    "proto": "suzu/1",
    "companion": "firefly",
    "family": "esp8266-oled",
    "variant": "oled-v2",
    "device_id": DEVICE_ID,
    "faceplate": "portrait-numerals",
    "adopted": "2026-08-28",
}
payload = [
    ("firefly_oled_v2.py", open(BASE + "firefly_oled_v2.py", "rb").read()),
    ("icons.py", open(BASE + "icons.py", "rb").read()),
    ("profont_10.py", open(BASE + "profont_10.py", "rb").read()),
    ("suzu.json", json.dumps(suzu).encode()),
    ("main.py", open("hardware/classes/esp8266-oled-v2/faceplates/numerals/down-mount/main.py", "rb").read()),
]

for name, data in payload:
    write_file(name, data)

# ── phase 3: soft reboot + handshake verify ──

say("soft reboot")
write_bytes(b"\x02")
time.sleep(0.4)
write_bytes(b"\x04")
time.sleep(2.5)
read_for(0.5)

write_bytes(b"I\n")
got = read_for(4.0)
if b"suzu/1" in got and DEVICE_ID.encode() in got:
    say("HANDSHAKE VERIFIED — proto suzu/1, identity preserved")
    say("the face is up. watch the OLED.")
else:
    say("handshake incomplete: %r" % got[:200])
