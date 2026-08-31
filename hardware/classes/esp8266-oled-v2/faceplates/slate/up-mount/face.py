# suzu — Slate faceplate (esp8266-oled-v2, suzu/1).
# Portrait composition, 64 wide (u) x 128 tall (v); pixel(u,v)->oled(v,63-u).
# A console readout: block digits state the fact, a segmented gauge
# carries the proportion, solid rules divide the areas, and the name
# is embossed on a filled label strip whose end dot pulses with
# traffic. Frames (newline-term):
#   I -> OK,{descriptor}*hh | K -> OK | G,report,<cpu>,<mem>,<gpu> (255=dash)
#   A,audio.level,<v> | J,{"name":...} | S,<name> | X | R,... -> the stage.
# `*hh` checksums verified; 10 s without frames -> the face rests (dim).
# Full contract: README.md. Keep this file SMALL — the ESP8266 compiles
# it into an 80 KB heap; the ancestor face fit at ~11 KB source.

import gc, math, sys, time, select
from machine import Timer, UART

math_cos = math.cos             # the stage's circle, one lookup each
math_sin = math.sin

W, H = 64, 128            # portrait: u 0..63 across, v 0..127 down
INVERT = True            # the -inverted build flips this (tools/build_faceplates.py):
                          # the composition mirrors along its long axis, so the
                          # board hung connector-up reads exactly as this reads
                          # connector-down. Same art, same words, other hang.
BAND_U = 48               # the yellow band starts here (16 px wide)
# The glass's painted yellow strip is fixed to the glass: it does not
# move when the board is hung the other way. The 180° remap alone
# would set the drawn strip against the numerals — numbers over the
# paint, words deep in the blue. So the inverted hang re-homes the
# composition: band strip at the panel's other edge, numerals after.
NUM_U = 0                 # the numeral column's left edge
BAND_X = BAND_U           # the strip's left edge (16 px wide)
DRESS_ID = "slate-up"            # the variant's wire id; the build sets it per mount
TEXT_FLIP = True         # left-aligned mounts rotate the text area 180°
                          # — the words would stand on their head otherwise
if INVERT:
    NUM_U = W - BAND_U    # 16: numerals u 16..63
    BAND_X = 0            # the strip re-homes to the panel's other edge
AREA_H = 42               # 3 areas x 42 + 2 dividers = 128
NUM_H = 34                # numeral zone inside an area
REST_MS = 10000           # frames since last_rx before the face idles
BOOT_IDLE_MS = 3000       # no host at boot -> fireflies instead of dashes
I2C_SCL, I2C_SDA = 12, 14 # the class's OLED wiring (D6/D5), 400 kHz

u = UART(0, 115200)
oled = None
tick = None
last_rx = None
boot_ms = None
idle = False
ff = ()                   # the fireflies: [u0, phase, speed, v] each
label = "suzu"
stage = None               # the stage: None (ground showing), "full",
                           # or the addressed area 0..2 (cpu/gpu/mem)
stage_glyph = None         # the full stage's subject: info|warn|exception
ring_label = None          # a moment's words, glowing on the band
ring_until = None          # when the stage ends (ticks_ms); None = latched
latch = False              # True while an exception holds the stage
ring_seq = "0"             # the stage's ring: DONE carries it, so a
                           # dropped animation can never answer for
                           # the one that replaced it
band_lit = True            # the flash phase while a latched stage holds
marker_lit = False         # the heartbeat dot: lit while traffic is fresh
values = {"cpu": 255, "mem": 255, "gpu": 255}

# ── numerals: digits_slate.bin — raw glyph rows (2 B each, MSB =
# leftmost), one width byte per glyph, 11 glyphs in DIG_CHARS order.
# Read one glyph at a time at draw: the file costs ZERO import RAM,
# which the ESP8266's 80 KB heap cannot spare (a .py font module
# OOMs the display driver). Missing file -> the face degrades to
# dashes, not a crash. ──
DIG_CHARS = "0123456789-"
DIG_H = 25
DIG_STRIDE = 1 + DIG_H * 2          # widths <= 16: two bytes a row
DIG_FILE = "/digits_slate.bin"
NUM_H = DIG_H

# ── the stage grammar (the keeper's design). Every say takes one of
# three modes, by its signal's first word: info/ok/allclear bloom the
# encircled I; warn and the other verbs hold the triangle; the
# exception family (alert/crit) latches — it flashes until allclear
# or X. A bare say takes the whole stage: the panel clears and one
# big glyph speaks, alone. A qualifier that names a ground area
# (cpu/gpu/mem) addresses only that area: its sprite replaces the
# numeral while the other areas keep breathing. Anything the face
# cannot place degrades to the full stage — never disconnected. ──
STAGE_MS = 5000            # a bloom's length; a latch knows no clock

# ── moment icons: icons.bin — 8x8 sprites, 8 bytes each, MSB-left
# rows, one per ground area, in the areas' own order. Zero boot RAM —
# read at draw, like the digits. ──
ICON_KEYS = "cpu gpu mem"
ICON_FILE = "/icons.bin"

# ── microglyphs: 3x5, upright (rows -> +v, cols -> +u). Packed as a
# 36-char key strip + 2 bytes (15 bits) per glyph — ~150 RAM bytes
# where a dict of tuples costs ~3 KB this board doesn't have. ──
GLYPH_KEYS = "ABCDEFGHIKLMNOPRSTUVWXYZ0123456789- "
GLYPH_BITS = b"+\xedk\xae9#kny\xa7y\xa49k[\xedt\x97[\xadI'_\xedkm+jk\xa4k\xad8\x8et\x92[o[j[\xfdZ\xadZ\x92r\xa7{o,\x97R\xa7r\xcf[\xc9\x7f\x8e?kr\x92{\xefk\xce\x01\xc0\x00\x00"

# the poc's sine table (x100) — the idle fireflies bob on it
SIN = (0, 38, 70, 92, 100, 92, 70, 38, 0, -38, -70, -92, -100, -92, -70, -38)

# the label persists on the filesystem: a face that reboots (a
# screenshot's probe, a power blink) keeps its band text instead of
# reverting to "suzu" while the resident's session still believes it
# already sent one.
LABEL_FILE = "/label.txt"

def load_label():
    global label
    try:
        with open(LABEL_FILE) as f:
            n = f.read().strip()
        if n:
            label = n
    except OSError:
        pass

def save_label():
    try:
        with open(LABEL_FILE, "w") as f:
            f.write(label)
    except OSError:
        pass

def store_identity(dev_id):
    """The house names a face at adoption; the name outlives the
    session that minted it (ADR-0003). suzu.json is the deed."""
    import ujson
    try:
        d = {}
        try:
            with open("/suzu.json") as f:
                d = ujson.loads(f.read())
        except (OSError, ValueError):
            pass
        if d.get("device_id") != dev_id:
            d["device_id"] = dev_id
            with open("/suzu.json", "w") as f:
                f.write(ujson.dumps(d))
    except OSError:
        pass

# portrait mapping: visual (u,v) -> native (x=v, y=63-u);
# inverted builds mirror the long axis: (u,v) -> native (127-v, u) —
# the band stays on the panel's right, the reading runs bottom-up,
# and the numeral order inverts with the hang.

def px(u, v, on=1):
    if INVERT:
        oled.pixel(127 - v, u, on)
    else:
        oled.pixel(v, 63 - u, on)

def rect(u, v, w, h, on=1):
    """w runs along u, h along v."""
    if INVERT:
        oled.fill_rect(127 - v - h + 1, u, h, w, on)
    else:
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
    import ujson
    from machine import unique_id
    import ubinascii
    d = {}
    try:
        with open("/suzu.json") as f:
            d = ujson.loads(f.read())
    except (OSError, ValueError):
        pass
    d["proto"] = "suzu/1"
    d["version"] = "1.0.0"             # the faceplate.yaml version: the currency gate reads it
    d["faceplate"] = DRESS_ID
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
    i = GLYPH_KEYS.find(ch)
    if i < 0:
        return
    bits = (GLYPH_BITS[i * 2] << 8) | GLYPH_BITS[i * 2 + 1]
    for row in range(5):
        for col in range(3):
            if bits & (1 << (14 - row * 3 - col)):
                px(u + col, v + row, on)

def band_glyph(u, v, ch, on=1):
    """A microglyph rotated 90° — the spine convention. The letter's
    5-row height spans the band across (u 0..4), its 3-column width
    runs along it. The mount chooses the layout: the parent hang and
    the right-aligned hang read one way, the up and left hangs —
    where the words would stand on their head — carry the rotated
    text area."""
    i = GLYPH_KEYS.find(ch)
    if i < 0:
        return
    bits = (GLYPH_BITS[i * 2] << 8) | GLYPH_BITS[i * 2 + 1]
    for row in range(5):
        for col in range(3):
            if bits & (1 << (14 - row * 3 - col)):
                if INVERT:
                    if TEXT_FLIP:
                        px(u + row, v - col, on)
                    else:
                        px(u + row, v + col, on)
                else:
                    if TEXT_FLIP:
                        px(u + 1 + row, v - col, on)
                    else:
                        px(u + (4 - row), v + col, on)

def draw_band(dark=False):
    """The label: a filled strip with the name knocked out — embossed
    tape, this face's voice. Cleared first; glyphs never overlay
    glyphs. A latched stage flashes the polarity: the whole label
    blinks, never merely dims."""
    if TEXT_FLIP:
        x, v0, step = 6, H - 5, -4
    else:
        x, v0, step = BAND_U + 5, 4, 4
    rect(BAND_X, 0, W - BAND_U, H, 0 if dark else 1)
    text = ring_label if ring_label else label
    for i, ch in enumerate(text.upper()[:30]):   # 4 + 29*4 <= 127
        band_glyph(x, v0 + i * step, ch, 1 if dark else 0)

def draw_marker():
    """The heartbeat: a notch at the strip's far end that fills while
    traffic is fresh and fades when the house goes quiet."""
    global marker_lit
    fresh = last_rx is not None and         time.ticks_diff(time.ticks_ms(), last_rx) < 300
    if fresh != marker_lit:
        marker_lit = fresh
        rect(BAND_X + 2, 3 if TEXT_FLIP else H - 8, 2, 2,
             0 if fresh else 1)
        oled.show()

def draw_divider(v):
    """A solid rule between areas — the lane it used to carry now
    lives in the label strip's end dot."""
    rect(NUM_U, v, BAND_U, 1, 1)

def blit(u, v, w, rowbytes):
    """Draw one glyph's rows at (u,v); horizontal runs become fill_rects.
    Rows are packed MSB-left at (w + 7) // 8 bytes each — big pixel
    fonts need the room."""
    bpp = (w + 7) // 8
    for rix in range(len(rowbytes) // bpp):
        bits = int.from_bytes(rowbytes[rix * bpp:(rix + 1) * bpp], "big")
        col = 0
        while col < w:
            if bits & (1 << (w - 1 - col)):
                run = 1
                while col + run < w and bits & (1 << (w - 1 - col - run)):
                    run += 1
                rect(u + col, v + rix, run, 1, 1)
                col += run
            else:
                col += 1

def spr_for(ch):
    """-> (width, rowbytes) from the font bin; a dash if it's missing."""
    i = DIG_CHARS.find(ch)
    if i < 0:
        return None
    try:
        with open(DIG_FILE, "rb") as f:
            f.seek(i * DIG_STRIDE)
            g = f.read(DIG_STRIDE)
    except OSError:
        g = bytearray(DIG_STRIDE)      # honest "not measured" dash
        g[0] = 10
        g[1 + (DIG_H // 2) * ((10 + 7) // 8)] = 0x0F
        return (10, bytes(g))
    return (g[0], g[1:])

def draw_num(v0, text):
    """Big numerals centered in the 48-px column, below v0."""
    rect(NUM_U, v0, BAND_U, NUM_H + 2, 0)
    sprs = []
    for ch in text:
        s = spr_for(ch)
        if not s or s[0] == 0:
            continue
        sprs.append(s)
    gap = 2
    total = sum(s[0] for s in sprs) + gap * (len(sprs) - 1)
    if total > BAND_U:                # belt and braces: tighten, then clamp
        gap = 1
        total = sum(s[0] for s in sprs) + gap * (len(sprs) - 1)
    x = NUM_U + max(0, (BAND_U - total) // 2)
    y = v0 + 1
    for (w, rowbytes) in sprs:
        blit(x, y, w, rowbytes)
        x += w + gap

def draw_label(v0, text):
    for i, ch in enumerate(text.upper()):
        glyph(NUM_U + 2 + i * 4, v0, ch)

def draw_gauge(u, v, val):
    """Five cells beside the name, filled to the value, hollow past
    it — the fact, at a glance."""
    filled = (val * 5 + 50) // 100
    for s in range(5):
        su = u + s * 7
        rect(su, v, 5, 4, 1)
        if s >= filled:
            rect(su + 2, v + 1, 3, 2, 0)

def draw_dots(u, v):
    for du in range(0, BAND_U, 2):
        px(u + du, v, 1)

def draw_area(area, label_text):
    v0 = area * AREA_H
    val = values[("cpu", "gpu", "mem")[area]]
    draw_num(v0 + 1, "-" if val == 255 else str(val))
    draw_label(v0 + 28, label_text)
    draw_gauge(NUM_U + 13, v0 + 28, 0 if val == 255 else val)
    draw_dots(NUM_U, v0 + 35)

def redraw():
    oled.fill(0)
    for i, label_text in enumerate(("CPU", "GPU", "MEM")):
        draw_area(i, label_text)
    draw_divider(AREA_H - 1)
    draw_divider(AREA_H * 2 - 1)
    draw_band()
    draw_marker()
    oled.show()

def decay():
    global idle, ring_until, ring_label, stage, stage_glyph, latch, band_lit, marker_lit
    now = time.ticks_ms()
    if latch:
        # the exception holds: it flashes by inversion, ~400 ms a phase
        phase = (now // 400) % 2 == 1
        if band_lit != phase:
            band_lit = phase
            stage_draw()
    elif ring_until is not None and time.ticks_diff(now, ring_until) > 0:
        ring_until = None             # the moment passed: the words go
        ring_label = None             # with it, the name returns, and the
        stage = None                  # substrate fills the gap on its next
        stage_glyph = None            # frame
        redraw()
    draw_marker()
    now = time.ticks_ms()
    quiet_for = (REST_MS if last_rx is not None
                 else BOOT_IDLE_MS + REST_MS)
    anchor = last_rx if last_rx is not None else boot_ms
    if time.ticks_diff(now, anchor) > quiet_for and not idle and not latch:
        idle_start()

def idle_start():
    """The poc's idle: three fireflies drift down the numeral column,
    bobbing on the same sine table, in the same 100 ms tick."""
    global idle, ff, boot_ms, ff_t0
    idle = True
    boot_ms = None
    ff_t0 = time.ticks_ms()
    draw_band()                   # the label glows while they float
    ff = [list(p) for p in (
        # [u0, phase, speed, amp, v, delay] — the poc's particles,
        # portrait-turned: they drift DOWN the column, bob ±amp on the
        # sine table, and enter staggered (0/1000/2000 ms).
        (NUM_U + 24, 0, 1, 6, -2, 0),
        (NUM_U + 34, 4, 2, 8, 40, 1000),
        (NUM_U + 20, 10, 1, 5, 84, 2000),
    )]

def idle_step():
    rect(NUM_U, 0, BAND_U, H, 0)
    now = time.ticks_ms()
    for f in ff:
        if time.ticks_diff(now, ff_t0) < f[5]:
            continue                    # staggered entrance, as the poc
        f[4] += f[2]                    # drift down at the poc's speeds
        if f[4] > 129:
            f[4] = -1
        f[1] = (f[1] + f[2]) % 16       # the poc's bob tempo
        x = f[0] + (f[3] * SIN[f[1]]) // 100   # ±amp, not ±25
        px(max(NUM_U + 1, min(NUM_U + BAND_U - 2, x)), f[4], 1)
    oled.show()

def draw_icon(u, v, i, on=1):
    try:
        with open(ICON_FILE, "rb") as f:
            f.seek(i * 8)
            rows = f.read(8)
    except OSError:
        return
    for r in range(8):
        for c in range(8):
            if rows[r] & (0x80 >> c):
                px(u + c * 2, v + r * 2, on)
                px(u + c * 2 + 1, v + r * 2, on)
                px(u + c * 2, v + r * 2 + 1, on)
                px(u + c * 2 + 1, v + r * 2 + 1, on)

# ── the stage's big glyphs: code-drawn geometry, not sprites — they
# must read across the room at 1 bit, and shapes cost no heap. ──

def line(x0, y0, x1, y1, on=1):
    dx = abs(x1 - x0)
    sx = 1 if x0 < x1 else -1
    dy = -abs(y1 - y0)
    sy = 1 if y0 < y1 else -1
    err = dx + dy
    while True:
        px(x0, y0, on)
        if x0 == x1 and y0 == y1:
            return
        e2 = 2 * err
        if e2 >= dy:
            err += dy
            x0 += sx
        if e2 <= dx:
            err += dx
            y0 += sy

def circle(cx, cy, rad, on=1):
    steps = rad * 6
    prev = None
    for i in range(steps + 1):
        a = 6.2832 * i / steps
        pt = (cx + int(rad * math_cos(a)), cy + int(rad * math_sin(a)))
        if prev:
            line(prev[0], prev[1], pt[0], pt[1], on)
        prev = pt

def draw_info():
    """The encircled I — the say that carries no alarm."""
    cx = NUM_U + 24
    circle(cx, 64, 17)
    rect(cx - 3, 54, 6, 21, 1)
    rect(cx - 6, 51, 12, 3, 1)
    rect(cx - 6, 75, 12, 3, 1)

def tri_edges(cx, cy, w, h, v):
    """half-width of the triangle at row v"""
    top = cy - h
    base = cy + 2 * h // 3
    if v < top or v > base:
        return None
    return (v - top) * w // (base - top)

def draw_warn(inverted=False):
    """The exclamation triangle — attention, held steady. Inverted
    (lit fill, dark mark) is the exception's flash phase."""
    cx, cy, w, h = NUM_U + 24, 62, 22, 19
    top, base = cy - h, cy + 2 * h // 3
    if inverted:
        for v in range(top, base + 1):
            t = tri_edges(cx, cy, w, h, v)
            if t is not None:
                rect(cx - t, v, 2 * t + 1, 1, 1)
    else:
        line(cx, top, cx - w, base, 1)
        line(cx, top, cx + w, base, 1)
        line(cx - w, base, cx + w, base, 1)
    ink = 0 if inverted else 1
    rect(cx - 1, cy - 6, 3, 9, ink)
    rect(cx - 1, cy + 5, 3, 3, ink)

def stage_draw():
    """The stage: a bare say clears the panel and one big glyph
    speaks, alone; a qualified say replaces only that area's numeral
    while the rest keeps breathing. The band clears before it speaks
    and flashes by inversion while a latch holds."""
    flash = latch and (time.ticks_ms() // 400) % 2 == 1
    if stage == "full":
        rect(NUM_U, 0, BAND_U, H, 0)
        if stage_glyph == "exception":
            draw_warn(flash)
        elif stage_glyph == "warn":
            draw_warn(False)
        else:
            draw_info()
    elif stage is not None:
        v0 = stage * AREA_H
        rect(NUM_U, v0 + 1, BAND_U, NUM_H + 2, 0)
        u = NUM_U + (BAND_U - 16) // 2
        v = v0 + 1 + (NUM_H - 16) // 2
        if flash:
            rect(u, v, 16, 16, 1)
            draw_icon(u, v, stage, 0)
        else:
            draw_icon(u, v, stage, 1)
    draw_band(flash)
    draw_marker()
    oled.show()

def wake():
    global idle
    if idle:
        idle = False
        if stage is not None:
            stage_draw()              # the say that woke the face stays up
        else:
            redraw()

# ── frames ──

def cmd(line):
    global last_rx, label, values, ring_until, ring_label
    global stage, stage_glyph, latch, band_lit, ring_seq
    line = line.strip()
    if not line:
        return
    if "*" in line:                   # `*hh` xor checksum, if present
        i = line.rfind("*")           # (rfind: no rpartition on this port)
        body, hexsum = line[:i], line[i + 1:]
        if len(hexsum) == 2:
            x = 0
            for c in body:
                x ^= ord(c)
            if "%02x" % x != hexsum:
                return                # bad checksum: drop; state self-heals
            line = body
    last_rx = time.ticks_ms()
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
            if stage == "full":
                r("OK")               # the stage owns the panel; ground waits
                return
            if stage is not None:
                # a qualified stage: the addressed area is spoken for,
                # the other two keep breathing
                for i in range(3):
                    if i != stage:
                        draw_area(i, ("CPU", "GPU", "MEM")[i])
                oled.show()
                r("OK")
                return
            for i, label_text in enumerate(("CPU", "GPU", "MEM")):
                draw_area(i, label_text)
            oled.show()
            r("OK")
        elif c == "A":
            r("OK")               # any arrival relights the end dot
        elif c == "J":
            import ujson
            ctx = ujson.loads(a)
            if isinstance(ctx, dict):
                if ctx.get("shot"):
                    # a copy of the screen rides the ack itself:
                    # base64 buffer, one ~120 ms write, no reboot —
                    # the house can screenshot a live face.
                    import ubinascii
                    payload = str(
                        ubinascii.b2a_base64(oled.buffer)[:-1], "ascii")
                    r("OK," + payload, checksum=True)
                    return
                if ctx.get("name") and ctx["name"] != label:
                    label = ctx["name"]
                    save_label()
                    draw_band()
                    oled.show()
                if ctx.get("device_id"):
                    store_identity(ctx["device_id"])
            r("OK")
        elif c == "S":                # compat alias: set the band
            if a and a != label:
                label = a
                save_label()
                draw_band()
                oled.show()
            r("OK")
        elif c == "X":
            # the host stands the stage down (the latch's other key)
            stage = None
            stage_glyph = None
            latch = False
            ring_until = None
            ring_label = None
            redraw()
            r("OK")
        elif c == "R":
            p = a.split(",")
            signal = p[0].lower()
            word = signal.split(".", 1)[0]
            qual = signal.split(".", 1)[1] if "." in signal else ""
            if word[:5] == "alert" or word[:4] == "crit" or word[:9] == "exception":
                glyph = "exception"
            elif word[:4] == "info" or word[:2] == "ok" or word[:8] == "allclear":
                glyph = "info"
            else:
                glyph = "warn"
            ring_label = " ".join(p[5:])[:30] or None
            if len(p) > 4:
                ring_seq = p[4]
            keys = ICON_KEYS.split()
            prev = stage
            stage = keys.index(qual) if qual in keys else "full"
            stage_glyph = glyph
            if stage != "full":
                # stepping between stages: the old mark must not haunt
                # the areas — clear the column, repaint the rest from
                # the stored truth, and let the stage own its area
                rect(NUM_U, 0, BAND_U, H, 0)
                for i in range(3):
                    if i != stage:
                        draw_area(i, ("CPU", "GPU", "MEM")[i])
            latch = glyph == "exception"    # the exception latches; the
            ring_until = (None if latch     # rest blooms and returns
                          else time.ticks_add(time.ticks_ms(), STAGE_MS))
            band_lit = False
            stage_draw()
            ack = "OK," + p[4] if len(p) > 4 else "OK"   # echo the seq
            r(ack, checksum=True)
        else:
            r("ERR,unknown:%s" % c)
    except (ValueError, IndexError) as e:
        r("ERR,%s" % e)
    wake()

def tcb(t):
    if idle:
        idle_step()
    else:
        decay()

def init():
    global oled, tick
    gc.collect()
    # The frozen ssd1306 driver, driven directly — NOT the ancestor's
    # dashboard class. Importing that 11.5 KB .py means compiling it on
    # this 80 KB-heap board, and its parse tree alone exhausts the heap
    # (the silent session-killer; twice proven on the bench). The face
    # draws with the native framebuffer API and declares nothing else.
    from machine import SoftI2C, Pin
    import ssd1306
    i2c = SoftI2C(scl=Pin(I2C_SCL), sda=Pin(I2C_SDA), freq=400000)
    oled = ssd1306.SSD1306_I2C(128, 64, i2c)
    tick = Timer(-1)
    tick.init(period=100, mode=Timer.PERIODIC, callback=tcb)
    return True

def main():
    global boot_ms
    boot_ms = time.ticks_ms()
    load_label()
    r("suzu firefly oled v2 — portrait numerals, suzu/1")
    r("OK," + descriptor(), checksum=True)   # unsolicited hello
    if not init():
        r("ERR,display_init")
        return
    idle_start()                  # fireflies open the show; the first
                                  # frame from the house wakes the face

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
