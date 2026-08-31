#!/usr/bin/env python3
"""Render a TTF's glyphs into a MicroPython sprite module (1-bit rows).

The host-side half of a faceplate: the tool installs what the faceplate
declares, and art is data — never interpreted. This script turns a
display font (Bebas Neue class, condensed) into a `digits_*.py` module
of packed row bitmaps the face blits with fill_rect runs.

Data convention (matches the faceplate's fallback table):
    DIGITS = { char: (width, height, (row_int, ...)), ... }
Each row_int has its MSB = leftmost column. All glyphs share one
vertical window (the digits' cap-height box) so baselines and the
dash's mid-height position survive the crop; horizontal cropping is
per-glyph (proportional widths).

Usage:
    python tools/font2sprites.py --font tools/fonts/BebasNeue-Regular.ttf \
        --out faceplates/esp8266-oled/portrait-numerals/digits_bebas.py \
        --preview tools/fonts/digits_bebas_preview.png
"""

import argparse
import os

from PIL import Image, ImageDraw, ImageFont

# The faceplate shows CPU/MEM/GPU percentages (0..100, 255 = unknown ->
# dash) and nothing else, so this char set is the whole contract.
CHARS = "0123456789-"


def render_glyphs(font_path, target_h, chars):
    """Return {char: (w, rows)} rendered at cap height == target_h."""
    # Find the integer font size whose digit cap box is target_h tall.
    size = target_h
    for _ in range(64):
        probe = ImageFont.truetype(font_path, size)
        top, bottom = probe.getbbox("0")[1], probe.getbbox("0")[3]
        if bottom - top == target_h:
            break
        if bottom - top > target_h:
            size -= 1
            break
        size += 1
    font = ImageFont.truetype(font_path, size)

    # One shared vertical window for every glyph: the digits' ink box.
    # (Cropping per glyph would let the '-' drift off its mid-height.)
    tops, bottoms = [], []
    for ch in chars:
        if ch == "-":
            continue
        b = font.getbbox(ch)
        tops.append(b[1])
        bottoms.append(b[3])
    v0, v1 = min(tops), max(bottoms)
    h = v1 - v0

    out = {}
    for ch in chars:
        # Generous canvas; we crop to ink afterwards.
        img = Image.new("L", (size * 2, size * 2), 0)
        ImageDraw.Draw(img).text((size // 2, size // 2), ch, fill=255, font=font)
        bbox = img.getbbox()  # ink box in canvas space
        cols = range(bbox[0], bbox[2])
        rows = []
        for y in range(v0 + size // 2, v1 + size // 2):
            bits = 0
            for x in cols:
                bits = (bits << 1) | (1 if img.getpixel((x, y)) >= 128 else 0)
            rows.append(bits)
        out[ch] = (len(cols), tuple(rows))
    return out, size


def emit_module(glyphs, height, font_name, font_size):
    lines = [
        "# suzu faceplate sprites — digits from a real display font.",
        "#",
        f"# Source: {font_name} (SIL OFL 1.1 — see BebasNeue-OFL.txt),"
        f" rendered at cap height {height}px by tools/font2sprites.py.",
        "# Data: {{char}} = (width, height, (row ints...)); each row int has"
        " MSB =",
        "# leftmost column. Regenerate rather than hand-edit.",
        "#",
        "# The dash keeps the digits' vertical window so it stays",
        "# mid-height — it is the 'not measured' face for a 255 slot.",
        "",
        "FONT = %r" % font_name,
        f"H = {height}",
        "DIGITS = {",
    ]
    for ch, (w, rows) in glyphs.items():
        lines.append("    %r: (%d, %d, (" % (ch, w, height))
        for i in range(0, len(rows), 8):
            lines.append("        " + ", ".join(map(str, rows[i : i + 8])) + ",")
        lines.append("    )),")
    lines.append("}")
    return "\n".join(lines) + "\n"


def emit_preview(glyphs, height, path):
    pad = 4
    total_w = sum(w for w, _ in glyphs.values()) + pad * (len(glyphs) + 1)
    img = Image.new("L", (total_w, height + 2 * pad), 0)
    x = pad
    for ch, (w, rows) in glyphs.items():
        for y, bits in enumerate(rows):
            for c in range(w):
                if bits & (1 << (w - 1 - c)):
                    img.putpixel((x + c, pad + y), 255)
        x += w + pad
    img.resize((img.width * 4, img.height * 4), Image.NEAREST).save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--preview")
    ap.add_argument("--chars", default=CHARS)
    ap.add_argument("--height", type=int, default=32)
    args = ap.parse_args()

    glyphs, size = render_glyphs(args.font, args.height, args.chars)
    name = " ".join(
        part for part in (ImageFont.truetype(args.font, size).getname())
    )
    with open(args.out, "w", newline="\n") as f:
        f.write(emit_module(glyphs, args.height, name, size))
    print(
        "wrote %s: %d glyphs, cap height %dpx, font size %d"
        % (args.out, len(glyphs), args.height, size)
    )

    # Companion raw bin: the device-side form. A .py module builds real
    # objects in the ESP8266's 80 KB heap at import; a raw file costs
    # nothing until the face reads one glyph at draw time. Layout per
    # glyph: [width byte][BPP B per row, MSB = leftmost] — BPP is
    # uniform across the set, sized for the widest glyph, so the face
    # can seek a fixed stride.
    bin_path = os.path.splitext(args.out)[0] + ".bin"
    rows_per_glyph = len(next(iter(glyphs.values()))[1])
    max_w = max(w for w, _ in glyphs.values())
    bpp = (max_w + 7) // 8
    with open(bin_path, "wb") as f:
        for ch in args.chars:
            f.write(bytes((glyphs[ch][0],)))
            for bits in glyphs[ch][1]:
                f.write(bits.to_bytes(bpp, "big"))
    print(
        "wrote %s: %d B (%d glyphs x (1 + %d rows x %d B), widths <= %d px)"
        % (bin_path, len(args.chars) * (1 + rows_per_glyph * bpp),
           len(args.chars), rows_per_glyph, bpp, bpp * 8)
    )

    if args.preview:
        emit_preview(glyphs, args.height, args.preview)
        print("preview: %s" % args.preview)


if __name__ == "__main__":
    main()
