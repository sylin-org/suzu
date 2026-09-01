#!/usr/bin/env python3
"""Reference install procedure: push suzu firmware files over the
MicroPython serial REPL (the esp8266-oled path).

This is the executable form of the procedure checklists in
hardware/classes/*/procedure.yaml — the same steps the Rust resident
implements. Kept as a standalone Python tool because the legacy tooling (esptool,
mpremote) is Python and the flash step may need it first.

Usage: python scripts/push_firmware.py COM24
"""

import base64
import json
import os
import pathlib
import re
import serial
import sys
import time

CHUNK = 192               # base64 chars per chunk-line (144 B binary) —
                          # short lines survive the ESP8266's UART RX FIFO
READ_SLICE = 384          # hexlify doubles it on-device: 384 -> 768, safe
BOOT_WAIT = 2.5


def esc(data):
    """A MicroPython bytes literal for `data`. Kept for reference — the
    write path uses base64 chunks instead: twice on this heap the
    escaped-literal parse died with a 2048-byte MemoryError."""
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


class FramingError(SystemExit):
    pass


class Repl:
    def __init__(self, port):
        self.p = serial.Serial(port, 115200, timeout=0.3)
        time.sleep(BOOT_WAIT)                      # ESP auto-reset on open
        self.raw = False
        self.ensure_raw()
        # The sanity round-trip: the same raw-REPL path every later
        # step needs, proven working BEFORE anything is read or written.
        out = self.exec("print('suzu-ok')")
        if b"suzu-ok" not in out:
            raise FramingError("REPL answered but not sanely: " + repr(out[:80]))
        # The legacy application left the heap fragmented; collect
        # here is the difference between a 1 KB parse fitting or not.
        self.exec("import gc; gc.collect()")

    def ensure_raw(self):
        """Enter raw mode and BELIEVE it only when the device says so.

        The failed first migration ran writes with `raw = True` set
        blindly; half the traffic landed at the friendly prompt (a
        Ctrl-D there reboots the board mid-session). Never again: a
        session without the raw banner is not a session."""
        for attempt in range(3):
            self.p.write(b"\r\x03\x03")            # interrupt any app
            time.sleep(0.7)
            self.drain(0.5)                        # the app's exit reply lands late
            self.p.write(b"\x02")                  # Ctrl-B: friendly, known state
            time.sleep(0.3)
            self.drain(0.3)
            self.p.write(b"\x01")                  # Ctrl-A: raw mode
            time.sleep(0.3)
            banner = self.drain(0.5)
            if b"raw REPL" in banner:
                self.raw = True
                print("  raw REPL confirmed (attempt %d)" % (attempt + 1))
                return
        raise FramingError("could not confirm raw REPL — device untouched")

    def drain(self, secs):
        end = time.time() + secs
        buf = b""
        while time.time() < end:
            n = self.p.in_waiting
            buf += self.p.read(n if n else 1)
        return buf

    def exec(self, code):
        """One raw-REPL round trip. Framing is verified, never assumed;
        a lost frame aborts loudly (blind retries can double-write)."""
        sys.stdout.flush()
        if not self.raw:
            self.ensure_raw()
        self.p.write(code.encode())
        self.p.write(b"\x04")
        # The end marker is the PAIR `\x04>` — a bare `\x04` check only
        # passes when a read happens to split between the two bytes.
        out = b""
        end = time.time() + 20
        while time.time() < end and not out.endswith(b"\x04>"):
            out += self.p.read(512)
        if not out.endswith(b"\x04>"):
            self.raw = False
            raise FramingError(
                "no end-of-reply marker — framing unknown, aborting "
                "(device untouched; re-run re-verifies every file)"
            )
        if b"Traceback" in out:
            raise SystemExit("device raised:\n" + out.decode(errors="replace"))
        return out

    def sync_prompt(self):
        """Hold until the friendly prompt actually answers — the first
        write line must never race the post-interrupt transition."""
        self.p.write(b"\r\n")
        got = b""
        end = time.time() + 3
        while time.time() < end:
            n = self.p.in_waiting
            if n:
                got += self.p.read(n)
            if got.rstrip().endswith(b">>>"):
                return True
            time.sleep(0.05)
        return False

    def write_file(self, name, data):
        """Upload through raw REPL with interrupt, verification,
        Ctrl-B to the friendly prompt, then base64 chunk lines — each
        line dribbled in 16-char bites (the ESP8266's UART RX FIFO
        overruns a 200+ char burst; a truncated line with an open
        quote swallows the session silently) and each line waited out
        to the `>>> ` prompt. Verified afterwards by sliced read-back."""
        self.exec("import gc; gc.collect()")
        self.p.write(b"\r\x03\x03")            # interrupt whatever runs
        time.sleep(0.7)
        self.drain(0.5)
        self.p.write(b"\x02")                  # Ctrl-B: friendly, deliberately
        time.sleep(0.4)
        self.drain(0.4)
        self.raw = False
        if not self.sync_prompt():
            raise SystemExit("friendly prompt not answering — device untouched")

        def line(s):
            payload = s.encode() + b"\r\n"
            for i in range(0, len(payload), 16):
                self.p.write(payload[i : i + 16])
                time.sleep(0.004)
            got = b""
            end = time.time() + 5
            while time.time() < end:
                n = self.p.in_waiting
                if n:
                    got += self.p.read(n)
                if b">>>" in got[-8:]:
                    return
                time.sleep(0.02)
            self.p.write(b"\r\x03\x03")        # unwind a stuck line
            self.drain(0.4)
            raise SystemExit(
                "line never reached the prompt: %r... reply tail: %r"
                % (s[:60], got[-80:])
            )

        line("f = open('%s','wb')" % name)
        line("import ubinascii")
        b64 = base64.b64encode(data).decode()
        for i in range(0, len(b64), CHUNK):
            line("f.write(ubinascii.a2b_base64('%s'))" % b64[i : i + CHUNK])
        line("f.close()")
        line("import os; print('SIZE:', os.stat('%s')[6])" % name)
        time.sleep(0.2)
        # Read-back verify — sliced, never a whole-file hexlify (that
        # doubles the bytes on-device and blows the heap floor).
        self.ensure_raw()
        got = self.read_file(name)
        assert got == data, "verify failed for " + name
        print("  OK %s (%d bytes verified)" % (name, len(data)))

    def list_files(self):
        out = self.exec("import os; print(os.listdir())")
        text = out.decode(errors="replace")
        # Extract the bracket section — a late exit-reply can glue an
        # `OK` (or boot noise) onto the front of the real answer.
        a, b = text.find("["), text.rfind("]")
        if a == -1 or b == -1 or b < a:
            raise FramingError("unparseable list_files reply: " + repr(text[:120]))
        inner = text[a + 1 : b]
        return [s.strip().strip("'\"") for s in inner.split(",") if s.strip()]

    def read_file(self, name):
        """Read a device file back in small slices — hexlify doubles the
        bytes on-device, so the slice obeys the 2 KB heap floor."""
        data = b""
        self.exec("f = open('%s','rb')" % name)
        while True:
            reply = self.exec(
                "import ubinascii; print(ubinascii.hexlify(f.read(%d)))"
                % READ_SLICE
            )
            m = re.search(rb"b'([0-9a-fA-F]*)'", reply)
            assert m, "could not parse backup slice: " + repr(reply[:120])
            chunk = bytes.fromhex(m.group(1).decode())
            if not chunk:
                break
            data += chunk
        self.exec("f.close()")
        return data

    def backup_files(self, names, port):
        """Dump every existing file before any write — the procedure's
        license to touch a working device. Framing errors are NOT
        skippable here: a lost frame means we no longer know state."""
        if not names:
            print("nothing to back up (fresh filesystem)")
            return
        stamp = time.strftime("%Y%m%d-%H%M%S")
        dest = os.path.join("backups", "%s-%s" % (port, stamp))
        os.makedirs(dest, exist_ok=True)
        for name in names:
            try:
                data = self.read_file(name)
            except (AssertionError, OSError) as e:
                print("  backup skipped %s (%s)" % (name, e))
                continue
            with open(os.path.join(dest, name), "wb") as f:
                f.write(data)
            print("  backed up %s (%d bytes)" % (name, len(data)))
        print("backup at %s" % dest)

    def soft_reboot(self):
        self.raw = False
        self.p.write(b"\x02")                      # friendly prompt
        time.sleep(0.4)
        self.p.reset_input_buffer()
        self.p.write(b"\x04")                      # soft reboot -> main.py
        time.sleep(2.5)
        self.p.close()


def resolve_faceplate(faceplate):
    """Map a faceplate ID to its bundle and persisted metadata:
    faceplate name, mount, and version.
    Variant-type faceplates declare mounts in the manifest;
    single-type faceplates bundle at their own root."""
    import yaml
    root = pathlib.Path("hardware/classes/esp8266-oled/faceplates")
    for mf in sorted(root.glob("*/faceplate.yaml")):
        face = yaml.safe_load(mf.read_text(encoding="utf-8")) or {}
        for v in face.get("variants") or []:
            if v.get("id") == faceplate:
                side = v["mount"].removeprefix("usb-")
                version = v.get("version") or face.get("version") or "0.0.0"
                return (str(mf.parent / (side + "-mount")),
                        face["name"], side, version)
        if face.get("name") == faceplate and not face.get("variants"):
            return (str(mf.parent), face["name"], None,
                    face.get("version") or "0.0.0")
    raise SystemExit(f"faceplate {faceplate!r} is not declared anywhere — "
                     "check the manifests under faceplates/")


def main():
    port = sys.argv[1] if len(sys.argv) > 1 else "COM24"
    base = "firmware/suzu-d/esp8266-oled-v2/"
    device_id = sys.argv[2] if len(sys.argv) > 2 else None
    fresh = "--fresh" in sys.argv
    # A declared faceplate carries its own metadata (ADR-0005).
    # main.py / face.mpy / art bins, and its id goes into suzu.json.
    # Variant-type faceplates bundle one directory per mount beside
    # their manifest; the id resolves through the manifest's variants.
    faceplate = "numerals"
    if "--faceplate" in sys.argv:
        faceplate = sys.argv[sys.argv.index("--faceplate") + 1]
    faceplate_dir, faceplate_name, faceplate_mount, faceplate_version = resolve_faceplate(faceplate)

    repl = Repl(port)
    files = repl.list_files()
    print("device files:", files)
    if not files and not fresh:
        # The failed first migration "backed up" an empty list into
        # silence — an unreadable filesystem is a diagnosis, not a
        # blank check. Nothing gets written on top of an unknown state.
        raise SystemExit(
            "filesystem listing came back empty — refusing to write "
            "(pass --fresh after erase_flash + write_flash, never on a guess)"
        )
    # Rule zero: never modify a working device without a proven
    # rollback. The backup provides file-level rollback; the legacy
    # installer (erase -> flash -> provision) remains the heavy one.
    repl.backup_files(files, port)

    # Persist faceplate name, mount, and version (ADR-0005). The descriptor answers
    # the same three; the flattened install id is the doors' business.
    suzu = {
        "proto": "suzu/1",
        "companion": "firefly",
        "family": "esp8266-oled",
        "variant": "oled-v2",
        "faceplate": faceplate_name,
        "adopted": "2026-08-28",
        "dress_version": faceplate_version,
    }
    if faceplate_mount:
        suzu["mount"] = faceplate_mount
    if device_id:
        suzu["device_id"] = device_id              # preserve identity
        print("identity preserved:", device_id)

    # The faceplate bundle: its bootstrap, its bytecode, its art.
    # Validate every source file before writing to the device.
    faceplate_files = ["main.py", "face.mpy"] + sorted(
        p.name for p in pathlib.Path(faceplate_dir).glob("*.bin"))
    payload = [
        ("boot.py", open(base + "boot.py", "rb").read()),
        ("firefly_oled_v2.py", open(base + "firefly_oled_v2.py", "rb").read()),
        ("icons.py", open(base + "icons.py", "rb").read()),
        ("profont_10.py", open(base + "profont_10.py", "rb").read()),
        ("suzu.json", json.dumps(suzu).encode()),
        # The face ships as BYTECODE: a 13.7 KB source recompiled its
        # parse tree past this 80 KB-heap board's boot (MemoryError at
        # 436 B). mpy-cross -march=xtensa, ~5 KB, zero parse peak.
        # This firmware auto-runs main.py only, so main.py is a
        # two-line bootstrap importing face (face.mpy).
    ]
    for name in faceplate_files:
        payload.append((name, open(f"{faceplate_dir}/{name}", "rb").read()))
    for stale in ("main.mpy", "face.py"):
        if stale in repl.list_files():
            print("  removing stale %s ..." % stale)
            repl.exec("import os; os.remove('%s')" % stale)
    print("pushing %d files to %s ... (faceplate: %s)" % (len(payload), port, faceplate))
    for name, data in payload:
        repl.exec("import gc; gc.collect()")       # fresh heap per file
        repl.write_file(name, data)

    repl.soft_reboot()
    print("rebooted into suzu — run `suzu scan` to verify the handshake")


if __name__ == "__main__":
    main()
