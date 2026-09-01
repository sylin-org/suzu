"""
Status-overlay sprites for the esp8266-oled-v2 faceplate.

Three 8x8 sprites, one per metric area. A qualified event replaces
the numeral for that metric. Redrawn by hand in
the style of the discovered sets — `cpu` after Bootstrap Icons'
`cpu`, `gpu` after Bootstrap Icons' `gpu-card` (MIT), `mem` after
Lucide's `memory-stick` (ISC). The large overlay glyphs (encircled I,
warning triangle) are code-drawn geometry in face.py, not sprites.

The legacy Open Iconic pack (pulse, layers, hdd, bolt, gear,
arrows, clock, stones, heart, withering, wilt) rested here; only
only the icons used by the current overlay remain.

Each icon is an 8-byte MSB-left bitmap (8x8 pixels).
"""

# chip die with pins and center dot
ICON_CPU = bytearray([
    0x48,  # .#..#...
    0xF8,  # .#####..
    0x88,  # .#...#..
    0xA8,  # .#.#.#..
    0x88,  # .#...#..
    0xF8,  # .#####..
    0x48,  # .#..#...
    0x00,  # ........
])

# card with fan
ICON_GPU = bytearray([
    0xFF,  # ########
    0x81,  # #......#
    0xB9,  # #.###..#
    0xB1,  # #.#.#..#
    0xB9,  # #.###..#
    0x81,  # #......#
    0xFF,  # ########
    0x48,  # .#..#...
])

# RAM stick: slats above, pin row below
ICON_MEM = bytearray([
    0xFC,  # ######..
    0x84,  # #....#..
    0xB4,  # #.##.#..
    0xB4,  # #.##.#..
    0x84,  # #....#..
    0xFC,  # ######..
    0x04,  # ....#...
    0x7C,  # .#####..
])
