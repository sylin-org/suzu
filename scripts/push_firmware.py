#!/usr/bin/env python3
"""Reference install procedure: push suzu firmware files over the
MicroPython serial REPL (the esp8266-oled-v2 path).

This is the executable form of the procedure checklists in
hardware/classes/*/procedure.yaml — the same steps the Rust resident
implements. Kept in Python because the ancestor tooling (esptool,
mpremote) is Python and the flash step may need it first.

Usage: python scripts/push_firmware.py COM24
"""

import json
import serial
import sys
import time

CHUNK = 256
BOOT_WAIT = 2.5


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


class Repl:
    def __init__(self, port):
        self.p = serial.Serial(port, 115200, timeout=0.3)
        time.sleep(BOOT_WAIT)                      # ESP auto-reset on open
        self.p.write(b"\r\x03\x03")                # interrupt any app
        time.sleep(0.7)
        self.p.reset_input_buffer()
        self.p.write(b"\x01")                      # raw REPL
        time.sleep(0.5)
        self.p.reset_input_buffer()
        self.raw = True

    def exec(self, code):
        if not self.raw:
            self.enter_raw()
        self.p.write(code.encode())
        self.p.write(b"\x04")
        out = b""
        end = time.time() + 20
        while time.time() < end and not out.endswith(b"\x04"):
            out += self.p.read(512)
        end = time.time() + 5
        while time.time() < end:
            c = self.p.read(1)
            if c == b">":
                break
        if b"Traceback" in out:
            raise SystemExit("device raised:\n" + out.decode(errors="replace"))
        return out

    def write_file(self, name, data):
        self.exec("f = open('%s','wb')" % name)
        for i in range(0, len(data), CHUNK):
            self.exec("f.write(%s)" % esc(data[i : i + CHUNK]))
        self.exec("f.close()")
        # read-back verify — never trust a blind write
        reply = self.exec(
            "import ubinascii; print(ubinascii.hexlify(open('%s','rb').read()))" % name
        )
        hexs = "".join(c for c in reply.decode(errors="replace") if c in "0123456789abcdefABCDEF")
        assert bytes.fromhex(hexs) == data, "verify failed for " + name
        print("  OK %s (%d bytes verified)" % (name, len(data)))

    def list_files(self):
        out = self.exec("import os; print(os.listdir())")
        inner = out.decode(errors="replace").strip().strip("[]")
        return [s.strip().strip("'\"") for s in inner.split(",") if s.strip()]

    def soft_reboot(self):
        self.p.write(b"\x02")                      # friendly prompt
        time.sleep(0.4)
        self.p.reset_input_buffer()
        self.p.write(b"\x04")                      # soft reboot -> main.py
        time.sleep(2.5)
        self.p.close()


def main():
    port = sys.argv[1] if len(sys.argv) > 1 else "COM24"
    base = "firmware/suzu-d/esp8266-oled-v2/"
    device_id = sys.argv[2] if len(sys.argv) > 2 else None

    repl = Repl(port)
    files = repl.list_files()
    print("device files:", files)

    suzu = {
        "proto": "suzu/1",
        "companion": "firefly",
        "family": "esp8266-oled",
        "variant": "oled-v2",
        "faceplate": "portrait-numerals",
        "adopted": "2026-08-28",
    }
    if device_id:
        suzu["device_id"] = device_id              # preserve identity
        print("identity preserved:", device_id)

    payload = [
        ("boot.py", open(base + "boot.py", "rb").read()),
        ("firefly_oled_v2.py", open(base + "firefly_oled_v2.py", "rb").read()),
        ("icons.py", open(base + "icons.py", "rb").read()),
        ("profont_10.py", open(base + "profont_10.py", "rb").read()),
        ("suzu.json", json.dumps(suzu).encode()),
        ("main.py", open("faceplates/esp8266-oled-v2/portrait-numerals/main.py", "rb").read()),
    ]
    print("pushing %d files to %s ..." % (len(payload), port))
    for name, data in payload:
        repl.write_file(name, data)

    repl.soft_reboot()
    print("rebooted into suzu — run `suzu scan` to verify the handshake")


if __name__ == "__main__":
    main()
