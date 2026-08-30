#!/usr/bin/env python3
"""Build derived faceplate bundles from their `based_on` parent.

ADR-0005: one face source, one orientation constant, one build. A
faceplate whose declaration carries `based_on: <parent>` is generated
here — the source of the parent is read, the orientation constant is
flipped, the descriptor names the derived id, and mpy-cross compiles
the bytecode. The generated bundle is regenerated, never hand-edited.

Usage: python tools/build_faceplates.py [name ...]   (default: all derived)
"""

import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FACEPLATES = ROOT / "faceplates"


def derived_bundles():
    """Every faceplate.yaml that declares based_on."""
    out = []
    for decl in sorted(FACEPLATES.glob("*/*/faceplate.yaml")):
        text = decl.read_text(encoding="utf-8")
        m = re.search(r"^based_on:\s*(\S+)", text, re.M)
        if m:
            out.append((decl.parent, m.group(1)))
    return out


def build(child: Path, parent_name: str) -> bool:
    name = child.name
    parent = FACEPLATES / child.parent.name / parent_name
    src = parent / "face.py"
    if not src.exists():
        print(f"  {name}: parent source missing ({src}) — skipped")
        return False

    # The flip: the constant, and the descriptor's own name.
    text = src.read_text(encoding="utf-8")
    if "INVERT = False" not in text:
        print(f"  {name}: parent face.py has no INVERT constant — skipped")
        return False
    text = text.replace("INVERT = False", "INVERT = True", 1)
    text = re.sub(r'(d\["faceplate"\] = ")[^"]*(")',
                  rf"\g<1>{name}\g<2>", text, count=1)

    child.mkdir(parents=True, exist_ok=True)
    (child / "face.py").write_text(text, encoding="utf-8")

    # Shared cargo: everything the parent bundles that the derived
    # declaration does not redefine — assets and the bootstrap.
    for item in ("main.py", "digits_bebas.bin", "icons.bin"):
        if not (child / item).exists() and (parent / item).exists():
            shutil.copy2(parent / item, child / item)

    r = subprocess.run(
        [sys.executable, "-m", "mpy_cross", "-march=xtensa",
         str(child / "face.py"), "-o", str(child / "face.mpy")],
        capture_output=True, text=True)
    if r.returncode != 0:
        print(f"  {name}: mpy-cross failed:\n{r.stdout}{r.stderr}")
        return False
    print(f"  {name}: built from {parent_name} (face.mpy "
          f"{(child / 'face.mpy').stat().st_size} B)")
    return True


def main():
    wanted = set(sys.argv[1:])
    ok = True
    for child, parent_name in derived_bundles():
        if wanted and child.name not in wanted:
            continue
        ok = build(child, parent_name) and ok
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
