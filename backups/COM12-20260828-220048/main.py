# suzu — portrait numerals faceplate (esp8266-oled-v2, suzu/1).
# Portrait composition, 64 wide (u) x 128 tall (v); pixel(u,v)->oled(v,63-u).
# Right edge: the yellow name band. Left: CPU/GPU/MEM big numerals,
# 1-px pulse dividers lit by the audio.level lane. Frames (newline-term):
#   I -> OK,{descriptor}*hh | K -> OK | G,report,<cpu>,<mem>,<gpu> (255=dash)
#   A,audio.level,<v> | J,{"name":...} | S,<name> | X | R,... -> ring blink.
# `*hh` checksums verified; 10 s without frames -> the face rests (dim).
# Full contract: README.md. Keep this file SMALL — the ESP8266 compiles
# it into an 80 KB heap; the ancestor face fit at ~11 KB source.

import gc, sys, time, select, ujson
from machine import Timer, UART

W, H = 64, 128            # portrait: u 0..63 across, v 0..127 down
BAND_U = 48               # the yellow band starts here (16 px wide)
AREA_H = 42               # 3 areas x 42 + 2 dividers = 128
NUM_H = 34                # numeral zone inside an area
REST_MS = 10000           # frames since last_rx before the face rests

u = UART(0, 115200)
oled = None
tick = None
last_rx = None
resting = False
name = "suzu"
values = {"cpu": 255, "mem": 255, "gpu": 255}
pulse_target = 0
pulse_lit = 0

# ── numerals: sprites from the declared font module. If it is ever
# missing, the face degrades to dashes rather than crash — it never
# carries a second font table; RAM here is 80 KB and spoken for. ──

try:
    from digits_bebas import DIGITS as _SPR, H as _SPR_H
    NUM_H = _SPR_H                     # the font's cap height
except ImportError:
    _SPR = {"-": (4, 4, (0, 0, 15, 0))}
    NUM_H = 4

# ── microglyphs: 3x5, upright (rows -> +v, cols -> +u) ──
GLYPHS = {
    "A": (2, 5, 7, 5, 5), "B": (6, 5, 6, 5, 6), "C": (3, 4, 4, 4, 3),
    "D": (6, 5, 5, 5, 6), "E": (7, 4, 6, 4, 7), "F": (7, 4, 6, 4, 4),
    "G": (3, 4, 5, 5, 3), "H": (5, 5, 7, 5, 5), "I": (7, 2, 2, 2, 7),
    "K": (5, 5, 6, 5, 5), "L": (4, 4, 4, 4, 7), "M": (5, 7, 7, 5, 5),
    "N": (6, 5, 5, 5, 5), "O": (2, 5, 5, 5, 2), "P": (6, 5, 6, 4, 4),
    "R": (6, 5, 6, 5, 5), "S": (3, 4, 2, 1, 6), "T": (7, 2, 2, 2, 2),
    "U": (5, 5, 5, 5, 7), "V": (5, 5, 5, 5, 2), "W": (5, 5, 7, 7, 5),
    "X": (5, 5, 2, 5, 5), "Y": (5, 5, 2, 2, 2), "Z": (7, 1, 2, 4, 7),
    "0": (7, 5, 5, 5, 7), "1": (2, 6, 2, 2, 7), "2": (5, 1, 2, 4, 7),
    "3": (7, 1, 3, 1, 7), "4": (5, 5, 7, 1, 1), "5": (7, 7, 6, 1, 6),
    "6": (3, 7, 5, 5, 3), "7": (7, 1, 2, 2, 2), "8": (7, 5, 7, 5, 7),
    "9": (6, 5, 7, 1, 6), "-": (0, 0, 7, 0, 0), " ": (0, 0, 0, 0, 0),
}

# portrait mapping: visual (u,v) -> native (x=v, y=63-u)

def px(u, v, on=1):
    oled.pixel(v, 63 - u, on)

def rect(u, v, w, h, on=1):
    """w runs along u, h along v."""
    oled.fill_rect(v, 63 - u - w + 1, h, w, on)

def r(msg, checksum=False):
    if checksum:
        x = 0
        for c in msg:
            x ^= ord(c)
        msg += "*%02x" % x
    u.write(msg + "\n")
    time.sleep_ms(2)

def descriptor():
    from machine import unique_id
    import ubinascii
    d = {}
    try:
        with open("/suzu.json") as f:
            d = ujson.loads(f.read())
    except (OSError, ValueError):
        pass
    d["proto"] = "suzu/1"
    d["version"] = "1.0.0"
    d["faceplate"] = "portrait-numerals"
    d["coverage"] = {
        "grounds": ["report"],
        "slots": {"report": ["cpu", "mem", "gpu"]},
        "extras": ["audio.level"],
    }
    try:
        d["hardware_id"] = "esp8266-" + ubinascii.hexlify(unique_id()).decode()
    except Exception:
        pass
    return ujson.dumps(d)

# ── composition ──

def glyph(u, v, ch, on=1):
    g = GLYPHS.get(ch)
    if not g:
        return
    for row in range(5):
        for col in range(3):
            if g[row] & (4 >> col):
                px(u + col, v + row, on)

def draw_band():
    """The yellow hardware zone: black name stacked down the band."""
    rect(BAND_U, 0, 16, H, 1)
    x = BAND_U + 6                    # glyphs 3 wide, centered at u=56
    for i, ch in enumerate(name.upper()[:20]):   # 5 + 19*6 <= 127
        glyph(x, 5 + i * 6, ch, 0)

def draw_divider(v):
    """1-px divider; the lit run hangs off the name band, growing left."""
    rect(0, v, BAND_U, 1, 0)
    if pulse_lit:
        rect(BAND_U - pulse_lit, v, pulse_lit, 1, 1)

def blit(u, v, w, rows, scale=1):
    """Draw a sprite (native-width row ints, MSB = leftmost) at (u,v);
    horizontal runs become fill_rects."""
    for rix, bits in enumerate(rows):
        col = 0
        while col < w:
            if bits & (1 << (w - 1 - col)):
                run = 1
                while col + run < w and bits & (1 << (w - 1 - col - run)):
                    run += 1
                rect(u + col * scale, v + rix * scale, run * scale, scale, 1)
                col += run
            else:
                col += 1

def spr_for(ch):
    """-> (native_width, rows, scale). Fallback table renders 4x."""
    s = _SPR.get(ch)
    if s is None:
        return None
    w, h, rows = s
    return (w, rows, 4 if h == 7 else 1)

def draw_num(v0, text):
    """Big numerals centered in the 48-px column, below v0."""
    rect(0, v0, BAND_U, NUM_H + 2, 0)
    sprs = []
    for ch in text:
        s = spr_for(ch)
        if not s:
            continue
        sprs.append(s)
    gap = 2
    total = sum(s[0] * s[2] for s in sprs) + gap * (len(sprs) - 1)
    if total > BAND_U:                # belt and braces: tighten, then clamp
        gap = 1
        total = sum(s[0] * s[2] for s in sprs) + gap * (len(sprs) - 1)
    x = max(0, (BAND_U - total) // 2)
    y = v0 + 1
    for (w, rows, scale) in sprs:
        blit(x, y, w, rows, scale)
        x += w * scale + gap

def draw_label(v0, text):
    for i, ch in enumerate(text.upper()):
        glyph(2 + i * 4, v0, ch)

def draw_area(area, label_text):
    v0 = area * AREA_H
    val = values[("cpu", "gpu", "mem")[area]]
    draw_num(v0 + 1, "-" if val == 255 else str(val))
    draw_label(v0 + AREA_H - 7, label_text)

def redraw():
    oled.fill(0)
    for i, label_text in enumerate(("CPU", "GPU", "MEM")):
        draw_area(i, label_text)
    draw_divider(AREA_H - 1)
    draw_divider(AREA_H * 2 - 1)
    draw_band()
    oled.show()

def set_pulse(v):
    global pulse_target, pulse_lit
    v = max(0, min(48, v))
    pulse_target = v
    if v >= pulse_lit:                # attack instant
        pulse_lit = v
        draw_divider(AREA_H - 1)
        draw_divider(AREA_H * 2 - 1)
        oled.show()

def decay():
    global pulse_lit, resting
    if resting:
        return
    if pulse_lit > pulse_target:      # decay exponential toward the target
        pulse_lit = pulse_target + (pulse_lit - pulse_target) * 3 // 4
        draw_divider(AREA_H - 1)
        draw_divider(AREA_H * 2 - 1)
        oled.show()
    now = time.ticks_ms()
    if last_rx is not None and time.ticks_diff(now, last_rx) > REST_MS:
        rest()

def rest():
    global resting
    resting = True
    oled.contrast(10)

def wake():
    global resting
    if resting:
        resting = False
        oled.contrast(255)

# ── frames ──

def cmd(line):
    global last_rx, name, values
    line = line.strip()
    if not line:
        return
    if "*" in line:                   # `*hh` xor checksum, if present
        body, _, hexsum = line.rpartition("*")
        if len(hexsum) == 2:
            x = 0
            for c in body:
                x ^= ord(c)
            if "%02x" % x != hexsum:
                return                # bad checksum: drop; state self-heals
            line = body
    last_rx = time.ticks_ms()
    wake()
    parts = line.split(",", 1)
    c = parts[0].upper()
    a = parts[1] if len(parts) > 1 else ""

    try:
        if c == "I":
            r("OK," + descriptor(), checksum=True)
        elif c == "K":
            r("OK")
        elif c == "G":
            p = a.split(",")
            if p[0].lower() != "report":
                r("OK")               # a ground this face doesn't declare
                return
            for i, key in enumerate(("cpu", "mem", "gpu")):
                if len(p) > i + 1 and p[i + 1]:
                    values[key] = int(p[i + 1])
            for i, label_text in enumerate(("CPU", "GPU", "MEM")):
                draw_area(i, label_text)
            oled.show()
            r("OK")
        elif c == "A":
            p = a.split(",")
            if len(p) >= 2 and p[0] == "audio.level":
                set_pulse(int(p[1]))
            r("OK")
        elif c == "J":
            ctx = ujson.loads(a)
            if isinstance(ctx, dict) and ctx.get("name") and ctx["name"] != name:
                name = ctx["name"]
                draw_band()
                oled.show()
            r("OK")
        elif c == "S":                # compat alias: set the band
            if a and a != name:
                name = a
                draw_band()
                oled.show()
            r("OK")
        elif c == "X":
            r("OK")                   # nothing overlaid yet; ground is showing
        elif c == "R":
            p = a.split(",")
            set_pulse(48)             # a ring blinks the dividers, once
            pulse_target = 0
            ack = "OK," + p[4] if len(p) > 4 else "OK"   # echo the seq
            r(ack, checksum=True)
        else:
            r("ERR,unknown:%s" % c)
    except (ValueError, IndexError) as e:
        r("ERR,%s" % e)

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
    r("OK," + descriptor(), checksum=True)   # unsolicited hello
    if not init():
        r("ERR,display_init")
        return
    redraw()

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
