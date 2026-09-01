#!/usr/bin/env python3
"""Back up a T-Display and install an Aurora faceplate while preserving metadata.

The T-Display already runs MicroPython (legacy firmware on the
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
from push_firmware import Repl, resolve_faceplate  # noqa: E402

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

    faceplate_dir, faceplate_name, faceplate_mount, faceplate_version = \
        resolve_faceplate(faceplate)

    repl = Repl(port)
    # Opening the port pulses DTR and resets the board; the faceplate needs
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

    # Persist faceplate metadata on the device (ADR-0005).
    suzu = {
        "proto": "suzu/1",
        "companion": "firefly",
        "family": "firefly",
        "variant": "tdisplay",
        "faceplate": faceplate_name,
        "mount": faceplate_mount,
        "dress_version": faceplate_version,
        "adopted": time.strftime("%Y-%m-%d"),
    }
    if not device_id:
        # Preserve the device's existing identity when none was supplied.
        # The file may be spaced (this script's json.dumps) or compact
        # (the faceplate's ujson) — accept both JSON layouts.
        import re as _re
        raw = repl.exec(
            "print(open('suzu.json').read())").decode(errors="replace")
        m = _re.search(r'"device_id":\s*"([^"]+)"', raw)
        if m:
            device_id = m.group(1)
        elif "suzu.json" in files:
            raise SystemExit(
                "suzu.json exists but gave no device_id — refusing to "
                "write a faceplate without preserving a known device ID (pass the ID "
                "explicitly, or --fresh after a real wipe)")
    if device_id:
        suzu["device_id"] = device_id
        print("identity preserved:", device_id)
    # If neither side has an ID, the Resident assigns one through its session.

    payload = [("suzu.json", json.dumps(suzu).encode())]
    for name in BUNDLE_FILES:
        p = pathlib.Path(faceplate_dir) / name
        payload.append((name, p.read_bytes()))

    for name, data in payload:
        repl.write_file(name, data)

    # A leftover face.mpy from an older push would shadow the fresh
    # source with the wrong vintage — remove it before the reboot.
    repl.exec("import os; os.remove('face.py') if 'face.py' in os.listdir() else None")

    print("pushed %d files — restarting the faceplate" % len(payload))
    # soft_reboot closes the port; the device restarts independently.
    repl.soft_reboot()
    time.sleep(2.0)
    print("rebooted — waiting for the faceplate HELLO response")


def resolve_faceplate(faceplate):
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
