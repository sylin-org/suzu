#!/usr/bin/env python3
"""Build the faceplate dresses from their manifests.

ADR-0005, reorganized: a faceplate directory holds a manifest and one
bundle directory per declared hang (`down-mount/`, `up-mount/`, ...).
A single-type faceplate (no `variants:` in the manifest) bundles at
its own root. The parent hang (usb-down) carries the human source;
every other hang is derived from it — the mount chooses the canvas
flip (up, right) and the text-area flip (up, left), and the
descriptor names the variant. Regenerate; never hand-edit a derived
face.py.

    python tools/build_faceplates.py [faceplate ...]   (default: all)
"""

import re
import shutil
import subprocess
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
FACEPLATES = ROOT / "faceplates"

CANVAS_FLIP = {"usb-up", "usb-right"}
TEXT_FLIP = {"usb-up", "usb-left"}


def mount_dir(mount):
    side = mount.removeprefix("usb-")
    return f"{side}-mount"


def build_variant(face_dir, faceplate_name, variant):
    mount = variant["mount"]
    vdir = face_dir / mount_dir(mount)
    parent = next(v for v in variants_of(face_dir) if v["mount"] == "usb-down")
    src = face_dir / mount_dir(parent["mount"]) / "face.py"
    if not src.exists():
        print(f"  {faceplate_name}/{mount_dir(mount)}: no parent source — skipped")
        return False

    text = src.read_text(encoding="utf-8")
    if mount in CANVAS_FLIP:
        text = text.replace("INVERT = False", "INVERT = True", 1)
    if mount in TEXT_FLIP:
        text = text.replace("TEXT_FLIP = False", "TEXT_FLIP = True", 1)
    text = re.sub(r'(d\["faceplate"\] = ")[^"]*(")',
                  rf"\g<1>{variant['id']}\g<2>", text, count=1)
    vdir.mkdir(parents=True, exist_ok=True)
    (vdir / "face.py").write_text(text, encoding="utf-8")

    # Shared cargo: everything the parent bundles that the variant
    # needs — assets and the bootstrap.
    for item in ("main.py", "digits_bebas.bin", "digits_slate.bin", "icons.bin"):
        if not (vdir / item).exists() and (src.parent / item).exists():
            shutil.copy2(src.parent / item, vdir / item)

    r = subprocess.run(
        [sys.executable, "-m", "mpy_cross", "-march=xtensa",
         str(vdir / "face.py"), "-o", str(vdir / "face.mpy")],
        capture_output=True, text=True)
    if r.returncode != 0:
        print(f"  {faceplate_name}/{mount_dir(mount)}: mpy-cross failed:\n"
              f"{r.stdout}{r.stderr}")
        return False
    print(f"  {mount_dir(mount)}: built from {parent['mount']} "
          f"(face.mpy {(vdir / 'face.mpy').stat().st_size} B)")
    return True


def variants_of(face_dir):
    mf = face_dir / "faceplate.yaml"
    face = yaml.safe_load(mf.read_text(encoding="utf-8")) or {}
    return face.get("variants") or []


def main():
    wanted = set(sys.argv[1:])
    ok = True
    for class_dir in sorted(FACEPLATES.iterdir()):
        if not class_dir.is_dir():
            continue
        for face_dir in sorted(class_dir.iterdir()):
            mf = face_dir / "faceplate.yaml"
            if not mf.exists():
                continue
            name = face_dir.name
            if wanted and name not in wanted:
                continue
            face = yaml.safe_load(mf.read_text(encoding="utf-8")) or {}
            variants = face.get("variants")
            if not variants:
                # Single type: the faceplate bundles at its own root.
                # A manifest without a poured source is a declaration
                # of intent, not a failure — skip it with a word.
                if not (face_dir / "face.py").exists():
                    print(f"  {name}: declared, no source poured yet — skipped")
                    continue
                r = subprocess.run(
                    [sys.executable, "-m", "mpy_cross", "-march=xtensa",
                     str(face_dir / "face.py"), "-o", str(face_dir / "face.mpy")],
                    capture_output=True, text=True)
                print(f"  {name}: {'built' if r.returncode == 0 else 'FAILED'}")
                ok = ok and r.returncode == 0
                continue
            print(f"  {name}:")
            for v in variants:
                if v["mount"] == "usb-down":
                    # The parent hang: the human source, compiled in place.
                    src = face_dir / mount_dir("usb-down") / "face.py"
                    if not src.exists():
                        print(f"  {name}: parent source missing — skipped")
                        ok = False
                        continue
                    r = subprocess.run(
                        [sys.executable, "-m", "mpy_cross", "-march=xtensa",
                         str(src), "-o", str(face_dir / mount_dir("usb-down") / "face.mpy")],
                        capture_output=True, text=True)
                    print(f"  {mount_dir('usb-down')}: "
                          f"{'built' if r.returncode == 0 else 'FAILED'}")
                    ok = ok and r.returncode == 0
                else:
                    ok = build_variant(face_dir, name, v) and ok
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
