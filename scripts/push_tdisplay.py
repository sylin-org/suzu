#!/usr/bin/env python3
"""Adopt a T-Display: back up its files, push the Aurora face, keep
the dress tuple.

The tdisplay already runs MicroPython (firefly/tdisplay on the
russhughes st7789 firmware — the C display driver stays frozen in
place, we only replace the application files). The transfer is the
proven REPL dance from push_firmware.py: interrupt, friendly prompt,
base64 chunks dribbled 16 bytes at a time, read-back verify.

Usage: python scripts/push_tdisplay.py COM14 <device_id>
"""

import base64
import json
import os
import pathlib
import re
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from push_firmware import Repl, resolve_dress  # noqa: E402

CLASS_ROOT = "hardware/classes/tdisplay-esp32-ch9102/faceplates"
# The face ships as BYTECODE (vintage-matched: this board runs
# MicroPython v1.20 — compile with `mpy_cross -march=xtensawin -b 1.20`).
# A 25 KB source's parse tree at import eats the heap peak the 64.8 KB
# mirror needs (the esp8266's lesson, earned again here); bytecode
# imports flat. The push removes any stale face.py so nothing shadows.
BUNDLE_FILES = ["main.py", "face.mpy"]


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    port = args[0]
    device_id = args[1] if len(args) > 1 else None
    faceplate = "aurora"
    if "--faceplate" in sys.argv:
        faceplate = sys.argv[sys.argv.index("--faceplate") + 1]

    dress_dir, dress_name, dress_mount, dress_version = \
        resolve_dress_for(faceplate)

    repl = Repl(port)
    # Opening the port pulses DTR and resets the board; the face needs
    # its boot before it can answer a file read. Settle first — a read
    # that races the boot comes back empty or stale.
    repl.drain(3.0)
    files = repl.list_files()
    print("device files:", files)
    if not files and "--fresh" not in sys.argv:
        raise SystemExit(
            "filesystem listing came back empty — refusing to write "
            "(pass --fresh after erase_flash + write_flash, never on a guess)"
        )
    repl.backup_files(files, port)

    # The durable word on the device: the dress tuple (ADR-0005).
    suzu = {
        "proto": "suzu/1",
        "companion": "firefly",
        "family": "firefly",
        "variant": "tdisplay",
        "faceplate": dress_name,
        "mount": dress_mount,
        "dress_version": dress_version,
        "adopted": time.strftime("%Y-%m-%d"),
    }
    if not device_id:
        # never wipe a deed by silence: keep what the device carries.
        # The file may be spaced (this script's json.dumps) or compact
        # (the face's ujson) — match both, never a format guess.
        import re as _re
        raw = repl.exec(
            "print(open('suzu.json').read())").decode(errors="replace")
        m = _re.search(r'"device_id":\s*"([^"]+)"', raw)
        if m:
            device_id = m.group(1)
        elif "suzu.json" in files:
            raise SystemExit(
                "suzu.json exists but gave no device_id — refusing to "
                "write an idless dress over a known device (pass the id "
                "explicitly, or --fresh after a real wipe)")
    if device_id:
        suzu["device_id"] = device_id
        print("identity preserved:", device_id)
    # no id on either side: the house mints and deeds through its session

    payload = [("suzu.json", json.dumps(suzu).encode())]
    for name in BUNDLE_FILES:
        p = pathlib.Path(dress_dir) / name
        payload.append((name, p.read_bytes()))

    for name, data in payload:
        repl.write_file(name, data)

    # A leftover face.mpy from an older push would shadow the fresh
    # source with the wrong vintage — remove it before the reboot.
    repl.exec("import os; os.remove('face.py') if 'face.py' in os.listdir() else None")

    print("pushed %d files — soft reboot into the face" % len(payload))
    # soft_reboot closes the port; the face is on its own from here.
    repl.soft_reboot()
    time.sleep(2.0)
    print("rebooted — the face should answer its HELLO on the bus")


def resolve_dress_for(faceplate):
    """id -> (bundle dir, faceplate name, mount side, version) from the
    class's faceplate manifests."""
    import yaml
    root = pathlib.Path(CLASS_ROOT)
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
    raise SystemExit(f"faceplate {faceplate!r} is not declared — "
                     f"check the manifests under {CLASS_ROOT}/")


if __name__ == "__main__":
    main()
