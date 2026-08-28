# suzu — portrait numerals faceplate  (esp8266-oled-v2, suzu/1)
#
# The display is composed in portrait: 64 wide x 128 tall.
#   - right edge (u 48..64): the hardware yellow zone — the name band,
#     text flowing top -> down
#   - the rest (u 0..48): three stacked areas — CPU / GPU / MEM — each
#     with big suzu numerals and a 3x5 label
#   - 1-px pulse dividers between areas, lit by the audio.level lane
#
# Native mapping: pixel(u,v) -> oled(v, 63-u)  (flip lands the yellow
# zone on the visual right).
#
# Frames (suzu/1, newline-terminated):
#   I                 -> OK,{descriptor}
#   S,<name>          -> set the name band
#   G,<cpu>,<mem>,<gpu> -> big numbers (255 = unknown, drawn as a dash)
#   P,audio,<level>   -> pulse divider target (0..100)
#   R                 -> redraw everything
# Ten seconds without frames -> the face rests (contrast dims).

import gc, sys, time, select
from machine import Timer, UART

W, H = 64, 128            # portrait
BAND_U = 48               # yellow band starts here (visual width 16)
AREA_H = 42               # 3 areas x 42 + 2 dividers = 128

# ── suzu numerals: 4x7, one int per row, MSB = leftmost column ──
DIGITS = {
    "0": (6, 9, 9, 9, 9, 9, 6),
    "1": (2, 6, 2, 2, 2, 2, 7),
    "2": (14, 1, 1, 6, 8, 8, 15),
    "3": (14, 1, 1, 6, 1, 1, 14),
    "4": (1, 3, 5, 9, 15, 1, 1),
    "5": (15, 8, 14, 1, 1, 9, 6),
    "6": (6, 8, 8, 14, 9, 9, 6),
    "7": (15, 1, 1, 2, 2, 4, 4),
    "8": (6, 9, 6, 9, 9, 9, 6),
    "9": (6, 9, 9, 7, 1, 1, 6),
    "-": (0, 0, 0, 15, 0, 0, 0),
}

# ── microglyphs: 3x5, for labels and the name band ──
GLYPHS = {
    "A": (2, 5, 7, 5, 5), "B": (6, 5, 6, 5, 6), "C": (3, 4, 4, 4, 3),
    "D": (6, 5, 5, 5, 6), "E": (7, 4, 6, 4, 7), "F": (7, 4, 6, 4, 4),
    "G": (3, 4, 5, 5, 3), "H": (5, 5, 7, 5, 5), "I": (7, 2, 2, 2, 7),
    "K": (5, 5, 6, 5, 5), "L": (4, 4, 4, 4, 7), "M": (5, 7, 7, 5, 5),
    "N": (6, 5, 5, 5, 5), "O": (2, 5, 5, 5, 2), "P": (6, 5, 6, 4, 4),
    "R": (6, 5, 6, 5, 5), "S": (3, 4, 2, 1, 6), "T": (7, 2, 2, 2, 2),
    "U": (5, 5, 5, 5, 7), "V": (5, 5, 5, 5, 2), "-": (0, 0, 7, 0, 0),
    "0": (7, 5, 5, 5, 7), "1": (2, 6, 2, 2, 7), "3": (7, 1, 3, 1, 7),
    "4": (5, 5, 7, 1, 1), "7": (7, 1, 2, 2, 2), "8": (7, 5, 7, 5, 7),
    " ": (0, 0, 0, 0, 0),
}

u = UART(0, 115200)
oled = None
tick = None
needs = True
last_rx = None
name = "suzu"
values = {"cpu": 255, "mem": 255, "gpu": 255}
pulse_target = 0
pulse_lit = 0

# portrait mapping: visual (u,v) -> native (x=v, y=63-u)

def px(u, v, on=1):
    oled.pixel(v, 63 - u, on)

def rect(u, v, w, h, on=1):
    oled.fill_rect(v, 63 - u - w + 1, h, w, on)

def glyph(u, v, ch, scale=1, on=1):
    g = GLYPHS.get(ch)
    if not g:
        return
    for r in range(5):
        for c in range(3):
            if g[r] & (4 >> c):
                if scale == 1:
                    px(u + r, v + c, on)
                else:
                    rect(u + r * scale, v + c * scale, scale, scale, on)

def vtext(u, v, text, on=1):
    """Vertical text, top -> down, 1 px glyphs."""
    for i, ch in enumerate(text.upper()):
        glyph(u, v + i * 4, ch, 1, on)

def label(u, v, text):
    for i, ch in enumerate(text.upper()):
        glyph(u, v + i * 4, ch, 1)

def r(msg):
    u.write(msg + "\n")
    time.sleep_ms(2)

def descriptor():
    import ujson, ubinascii
    from machine import unique_id
    d = {}
    try:
        with open("/suzu.json") as f:
            d = ujson.loads(f.read())
    except OSError:
        pass
    d["proto"] = "suzu/1"
    d["version"] = "1.0.0"
    try:
        d["hardware_id"] = "esp8266-" + ubinascii.hexlify(unique_id()).decode()
    except Exception:
        pass
    return ujson.dumps(d)

# ── composition ──

def draw_band():
    """The yellow hardware zone, as the name band on the visual right."""
    oled.fill_rect(0, 0, 128, 16, 1)          # native: light the yellow zone
    vtext(50, 3, name.upper(), 0)             # carve the name in black

def draw_divider(v):
    """1-px divider, lit length = pulse level, growing from the bottom."""
    oled.fill_rect(v, 16, 1, 48, 0)
    if pulse_lit:
        oled.fill_rect(v, 63 - pulse_lit, 1, pulse_lit, 1)

def draw_num(area, text):
    """Big numerals for one area: adaptive scale, dirty per digit."""
    u0 = area * AREA_H
    scale = 5 if len(text) <= 2 else 3
    dw = 4 * scale
    total = len(text) * dw + (len(text) - 1) * scale
    v0 = (AREA_H - total) // 2
    for i, ch in enumerate(text):
        g = DIGITS.get(ch)
        if not g:
            continue
        for rr in range(7):
            for cc in range(4):
                if g[rr] & (8 >> cc):
                    rect(u0 + 2 + rr * scale, v0 + i * (dw + scale) + cc * scale,
                         scale, scale, 1)

def draw_label(area, text):
    label(area * AREA_H + AREA_H - 8, 2, text)

def redraw():
    oled.fill(0)
    draw_divider(AREA_H - 1)
    draw_divider(AREA_H * 2 - 1)
    for i, label_text in enumerate(("CPU", "GPU", "MEM")):
        key = ("cpu", "gpu", "mem")[i]
        val = values[key]
        draw_num(i, "-" if val == 255 else str(val))
        draw_label(i, label_text)
    draw_band()
    oled.show()

def set_pulse(v):
    global pulse_target, pulse_lit
    pulse_target = v
    # attack instant, decay one step per divider draw
    if v >= pulse_lit:
        pulse_lit = v
    else:
        pulse_lit = (pulse_lit + v) // 2
    draw_divider(AREA_H - 1)
    draw_divider(AREA_H * 2 - 1)
    oled.show()

def decay():
    global pulse_lit
    if pulse_lit > pulse_target:
        pulse_lit = max(pulse_target, pulse_lit - 3)
        draw_divider(AREA_H - 1)
        draw_divider(AREA_H * 2 - 1)
        oled.show()

# ── frames ──

def cmd(line):
    global last_rx, name, values
    line = line.strip()
    if not line:
        return
    parts = line.split(",", 1)
    c = parts[0].upper()
    a = parts[1] if len(parts) > 1 else ""
    last_rx = time.ticks_ms()

    if c == "I":
        r("OK," + descriptor())
    elif c == "S":
        if a and a != name:
            name = a
            draw_band()
            oled.show()
        r("OK")
    elif c == "G":
        p = a.split(",")
        for i, key in enumerate(("cpu", "mem", "gpu")):
            if len(p) > i and p[i]:
                values[key] = int(p[i])
        redraw()
        r("OK")
    elif c == "P":
        p = a.split(",")
        if len(p) >= 2:
            set_pulse(int(p[1]))
        r("OK")
    elif c == "R":
        redraw()
        r("OK")
    else:
        r("ERR,unknown:%s" % c)

def tcb(t):
    decay()

def init():
    global oled, tick
    gc.collect()
    from firefly_oled_v2 import FireflyOLED
    oled = FireflyOLED().oled
    tick = Timer(-1)
    tick.init(period=100, mode=Timer.PERIODIC, callback=tcb)
    return True

def main():
    r("suzu firefly oled v2 — portrait numerals, suzu/1")
    if not init():
        r("ERR,display_init")
        return
    draw_band()
    oled.show()

    poll = select.poll()
    poll.register(sys.stdin, select.POLLIN)

    while True:
        try:
            events = poll.poll(50)
            if events:
                line = sys.stdin.readline()
                if line:
                    cmd(line)
            else:
                time.sleep_ms(10)
        except KeyboardInterrupt:
            break
        except Exception as e:
            r("ERR,%s" % e)

main()
