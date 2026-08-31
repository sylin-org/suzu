#!/usr/bin/env python3
"""Pack the stage sprite set into a faceplate icons.bin.

Source: firmware/suzu-d/esp8266-oled-v2/icons.py — the stage set
drawn for the keeper's stage grammar (8 bytes per icon, MSB-left
rows). The bin is raw sprite data: 8 bytes per icon in the order of
KEYS, read straight off the filesystem by the face at draw time.
The keys are the face's ground areas: a qualified say names one and
the face replaces that area's numeral with its sprite.

    python tools/pack_icons.py --out faceplates/.../icons.bin
"""

import argparse
import re

SOURCE = "firmware/suzu-d/esp8266-oled-v2/icons.py"

# key -> ICON_ variable in the source file; the keys are the areas
SELECT = {
    "cpu": "ICON_CPU",
    "gpu": "ICON_GPU",
    "mem": "ICON_MEM",
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
