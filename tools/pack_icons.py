#!/usr/bin/env python3
"""Pack the harvested Open Iconic set into a faceplate icons.bin.

Source: firmware/suzu-d/esp8266-oled-v2/icons.py — the ancestor's
Open Iconic bitmaps (8 bytes per icon, MSB-left rows, MIT licensed).
The bin is raw sprite data: 8 bytes per icon in the order of KEYS,
read straight off the filesystem by the face at draw time.

    python tools/pack_icons.py > faceplates/.../icons.bin   # (writes binary)

Usage:
    python tools/pack_icons.py --out faceplates/.../icons.bin
"""

import argparse
import re

SOURCE = "firmware/suzu-d/esp8266-oled-v2/icons.py"

# key -> ICON_ variable in the source file
SELECT = {
    "disk": "ICON_DSK",
    "usb": "ICON_USB",
    "gear": "ICON_GEAR",
    "net": "ICON_NET",
    "clock": "ICON_CLOCK",
    "heart": "ICON_THRIVING",
    "warn": "ICON_WITHERING",
    "wilt": "ICON_WILTING",
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    ns: dict = {}
    exec(open(SOURCE).read(), ns)  # our own harvested, reviewed file

    out = bytearray()
    keys = []
    for key, var in SELECT.items():
        data = bytes(ns[var])
        assert len(data) == 8, f"{var}: expected 8 bytes, got {len(data)}"
        out += data
        keys.append(key)

    with open(args.out, "wb") as f:
        f.write(out)
    print("keys: '{}'".format(" ".join(keys)))
    print("wrote {}: {} bytes ({} icons x 8)".format(args.out, len(out), len(keys)))


if __name__ == "__main__":
    main()
