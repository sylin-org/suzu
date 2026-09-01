# suzu — Aurora faceplate (tdisplay-esp32-ch9102, suzu/1).
# Landscape console, 240 wide (u) x 135 tall (v), RGB565.
#
# [header: device name, device color, and activity indicator]
# [CPU | gauge]  [GPU | gauge]  [MEM | gauge]
# [message area: event label while the overlay is active]
# [spectral field — 48 x 5 cells]
#
# During normal operation, a cursor updates one color column per tick
# and previous columns dim. An event colors the field according to
# urgency and displays its label. A latched alert remains until
# `allclear` or `X`.
#
# After 10 seconds without host data, show the idle star animation.
# Any new frame returns to the active display.
#
# Uses HSL interpolation, sine lookup, deterministic device colors,
# and the idle star animation from the earlier prototype.

import gc
import select
import sys
import time

from machine import Pin, SPI, unique_id

import ujson
import ubinascii

# ── faceplate metadata ──
FACEPLATE_ID = "aurora"
FACEPLATE_MOUNT = "down"
FACEPLATE_VERSION = "1.0.1"

W = 240
H = 135

RIBBON_H = 16
G_Y0 = 20
LINE_H = 18
MSG_Y = 78
FIELD_Y = 114
COLS = 48
CELL_W = 4
ROWS = 5
CELL_H = 4

OVERLAY_MS = 5000
REST_MS = 10000
MARKER_MS = 300
TICK_MS = 100

# the say hues (ADR-0001: urgency as color) — DEGREES, the field's
# Field columns store HSL hue slots rather than RGB tuples.
OVERLAY_HUES = {"info": 215, "ok": 120, "warn": 30, "exception": 0}
HUE_INFO = 215
HUE_OK = 120
HUE_WARN = 30
HUE_CRIT = 0

WHITE = (236, 232, 224)
DIMC = (120, 120, 120)
BLACK = (0, 0, 0)
MIDNIGHT = (5, 6, 26)
TRACK = (14, 14, 14)
TRACK_EDGE = (40, 40, 40)

_SIN = (0, 10, 20, 29, 38, 47, 56, 63, 70, 77, 83, 88, 92, 96, 98, 100,
        100, 100, 98, 96, 92, 88, 83, 77, 70, 63, 56, 47, 38, 29, 20, 10,
        0, -10, -20, -29, -38, -47, -56, -63, -70, -77, -83, -88, -92, -96, -98, -100,
        -100, -100, -98, -96, -92, -88, -83, -77, -70, -63, -56, -47, -38, -29, -20, -10)
_SIN_LEN = 64

# ── state ──
tft = None
capture_buffer = None              # framebuffer used for capture responses;
                           # the display hardware itself is write-only
last_rx = None
label = "suzu"
values = {"cpu": 255, "mem": 255, "gpu": 255}
overlay = None               # None or "overlay"
overlay_hue = HUE_INFO
overlay_kind = "info"        # info | warn | exception
ring_label = None
ring_until = None
latch = False
ring_seq = "0"
activity_indicator_lit = False
field = None               # COLS entries of [hue, k] — k is brightness 0..100
caret = 0
idle = False
idle_init = False
idle_t = 0
stars = None
flies = None
fly_prev = None

# ── color ──

def rgb(c):
    return ((c[0] & 0xF8) << 8) | ((c[1] & 0xFC) << 3) | (c[2] >> 3)

def pack(c):
    return ((c[0] & 0xF8) << 8) | ((c[1] & 0xFC) << 3) | (c[2] >> 3)

def scale(c, k):
    return (c[0] * k // 100, c[1] * k // 100, c[2] * k // 100)


def _c(color):
    """Normalize: a (r,g,b) tuple packs; an int is already RGB565."""
    return rgb(color) if isinstance(color, tuple) else color


# RGB565 to RGB332: 3-3-2 bits, one byte per pixel.
# color. The capture_buffer is 32.4 KB, not 64.8 (which does not fit). The
# bits are each channel's TOP bits (rrrrrggggggbbbbb → rrrgggbb):
# r >> 13, g >> 8, b >> 3. A wrong shift reads a NEIGHBOR's bits —
# the blue >> 5 read green's low bits and saturated blues drifted
# (reproduced offline, pixel for pixel, on the Spectral bench).
def to332(c):
    return ((c >> 13) & 0x07) << 5 | ((c >> 8) & 0x07) << 2 | (c >> 3) & 0x03


def m_set(x, y, c):
    if 0 <= x < W and 0 <= y < H:
        capture_buffer[y * W + x] = to332(c)


def fillf(x, y, w, h, color):
    """Draw a filled rectangle to the display and capture buffer."""
    c = _c(color)
    tft.fill_rect(x, y, w, h, c)
    row = bytes((to332(c),)) * w
    for yy in range(max(0, y), min(y + h, H)):
        x0 = max(0, x)
        ww = min(x + w, W) - x0
        if ww > 0:
            i = yy * W + x0
            capture_buffer[i:i + ww] = row[x0 - x:x0 - x + ww]


def pixelf(x, y, color):
    c = _c(color)
    tft.pixel(x, y, c)
    m_set(x, y, c)


def rectf(x, y, w, h, color):
    """1-px outline on both surfaces."""
    fillf(x, y, w, 1, color)
    fillf(x, y + h - 1, w, 1, color)
    fillf(x, y, 1, h, color)
    fillf(x + w - 1, y, 1, h, color)


def hlinef(x, y, w, color):
    fillf(x, y, w, 1, color)


def fillall(color):
    c = _c(color)
    tft.fill(c)
    row = bytes((to332(c),)) * W
    for yy in range(H):
        i = yy * W
        capture_buffer[i:i + W] = row


def hsl(h, s, l):
    s2 = s / 100
    l2 = l / 100
    c = (1 - abs(2 * l2 - 1)) * s2
    x = c * (1 - abs((h / 60) % 2 - 1))
    m = l2 - c / 2
    h6 = (h // 60) % 6
    if h6 == 0:
        r1, g1, b1 = c, x, 0
    elif h6 == 1:
        r1, g1, b1 = x, c, 0
    elif h6 == 2:
        r1, g1, b1 = 0, c, x
    elif h6 == 3:
        r1, g1, b1 = 0, x, c
    elif h6 == 4:
        r1, g1, b1 = x, 0, c
    else:
        r1, g1, b1 = c, 0, x
    return (int((r1 + m) * 255), int((g1 + m) * 255), int((b1 + m) * 255))

def stone_hue(name):
    h = 0
    for ch in name:
        h = ord(ch) + ((h << 5) - h)
    return abs(h) % 360

# ── microglyphs (3x5, shared bits) ──
GLYPH_KEYS = "ABCDEFGHIKLMNOPRSTUVWXYZ0123456789- "
GLYPH_BITS = b"+\xedk\xae9#kny\xa7y\xa49k[\xedt\x97[\xadI'_\xedkm+jk\xa4k\xad8\x8et\x92[o[j[\xfdZ\xadZ\x92r\xa7{o,\x97R\xa7r\xcf[\xc9\x7f\x8e?kr\x92{\xefk\xce\x01\xc0\x00\x00"

def glyph(x, y, ch, color, scale=1):
    i = GLYPH_KEYS.find(ch)
    if i < 0:
        return
    bits = (GLYPH_BITS[i * 2] << 8) | GLYPH_BITS[i * 2 + 1]
    col = pack(color)
    for row in range(5):
        for c in range(3):
            if bits & (1 << (14 - row * 3 - c)):
                if scale == 1:
                    pixelf(x + c, y + row, col)
                else:
                    fillf(x + c * scale, y + row * scale,
                                  scale, scale, col)

def text(x, y, s, color, scale=1):
    for i, ch in enumerate(s.upper()):
        glyph(x + i * 4 * scale, y, ch, color, scale)

def text_w(s, scale=1):
    return len(s) * 4 * scale

# ── the ribbon ──

def draw_ribbon():
    """The name on its hue — a filled ribbon, notch at the right end
    that fills while traffic is fresh."""
    h = stone_hue(label)
    base = hsl(h, 55, 42)
    fillf(0, 0, W, RIBBON_H, rgb(base))
    fillf(0, 0, W, 2, rgb(hsl(h, 55, 58)))
    name = label
    if name.startswith("stone-"):
        name = name[6:]
    name = name.upper()[:26]
    tw = text_w(name, 2)
    if tw <= W - 8:
        text((W - tw) // 2, 3, name, WHITE, 2)
    else:
        # harvested scroll: pause, run, pause, run back
        sm = tw - (W - 8)
        pause = 20
        st = max(1, sm // 2)
        cyc = 2 * pause + 2 * st
        t2 = (time.ticks_ms() // 40) % cyc
        if t2 < pause:
            sx = 0
        elif t2 < pause + st:
            sx = (t2 - pause) * 2
        elif t2 < 2 * pause + st:
            sx = sm
        else:
            sx = sm - (t2 - 2 * pause - st) * 2
        sx = max(0, min(sm, sx))
        fillf(0, 3, W - 6, 10, rgb(base))
        text(2 - sx, 3, name, WHITE, 2)
    # the notch: dark while idle, filled on fresh traffic
    fresh = last_rx is not None and \
        time.ticks_diff(time.ticks_ms(), last_rx) < MARKER_MS
    fillf(W - 5, RIBBON_H - 5, 3, 3,
                  rgb(base) if fresh else pack(hsl(h, 30, 14)))

# ── the gauge lines ──

def draw_gauge_line(idx, y):
    name = ("CPU", "GPU", "MEM")[idx]
    key = ("cpu", "gpu", "mem")[idx]
    val = values[key]
    text(2, y + 1, name, DIMC, 1)
    filled = 0 if val == 255 else (val * 5 + 50) // 100
    gx = 16
    for s in range(5):
        gw = 14
        fillf(gx, y, gw, 9, rgb(TRACK))
        if s < filled:
            fillf(gx + 1, y + 1, gw - 2, 7,
                          rgb(scale(SAY_HUE_OK, 40 + s * 15)))
        else:
            rectf(gx, y, gw, 9, rgb(TRACK_EDGE))
        gx += gw + 3
    vs = "-" if val == 255 else str(val)
    text(W - 4 - text_w(vs, 2), y, vs, WHITE, 2)

# ── the message area ──

def draw_message():
    fillf(0, MSG_Y, W, FIELD_Y - MSG_Y - 2, rgb(BLACK))
    hlinef(0, MSG_Y, W, rgb(TRACK_EDGE))
    if ring_label:
        words = ring_label.upper()[:52]
        text(2, MSG_Y + 4, words[:26], WHITE, 2)
        if len(words) > 26:
            text(2, MSG_Y + 16, words[26:], WHITE, 2)
    else:
        text(2, MSG_Y + 4, "-", DIMC, 1)

# ── the spectral field ──

def field_home(c):
    return c * 360 // COLS

def field_init():
    global field, caret
    field = []
    for c in range(COLS):
        field.append([field_home(c), 30 + (c * 7) % 25])
    caret = 0
    field_draw_full()

def field_draw_column(c):
    hue, k = field[c]
    col = rgb(hsl(hue, 70, 20 + k // 3))
    for r in range(ROWS):
        fade = 100 - r * 12
        x = c * (CELL_W + 1)
        y = FIELD_Y + r * (CELL_H + 1)
        fillf(x, y, CELL_W, CELL_H, rgb(scale(hsl(hue, 70, 20 + k // 3), max(20, fade))))

def field_draw_full():
    fillf(0, FIELD_Y, W, H - FIELD_Y, rgb(BLACK))
    for c in range(COLS):
        field_draw_column(c)

def field_sweep_tick():
    """Idle: the caret regenerates one column, gently, and dims the
    one behind it."""
    global caret
    field[caret] = [field_home(caret), 45 + (time.ticks_ms() // 700) % 25]
    field_draw_column(caret)
    behind = (caret - 1) % COLS
    field[behind][1] = max(12, field[behind][1] - 6)
    field_draw_column(behind)
    caret = (caret + 1) % COLS

def field_splash_tick(now):
    """Update a temporary event overlay until it expires."""
    spent = 0
    if ring_until is not None:
        total = OVERLAY_MS
        left = max(0, time.ticks_diff(ring_until, now))
        spent = 100 - left * 100 // total
    k = max(30, splash_k - spent // 2)
    for c in range(COLS):
        field[c][0] = overlay_hue_c
        field[c][1] = min(100, k + (c * 11) % 20)
        field_draw_column(c)

# ── the overlay ──

overlay_hue_c = HUE_INFO
splash_k = 80

def overlay_draw(now):
    """Render the overlay below the header. Latched exceptions alternate brightness."""
    dark = latch and (now // 400) % 2 == 0
    fillf(0, RIBBON_H, W, H - RIBBON_H, rgb(BLACK))
    if dark:
        # Dim phase for a latched exception.
        k = 30
        for c in range(COLS):
            hue, _ = field[c]
            field[c][1] = k
            col = rgb(hsl(hue, 70, 12))
            for r in range(ROWS):
                fillf(c * (CELL_W + 1),
                              FIELD_Y + r * (CELL_H + 1),
                              CELL_W, CELL_H - 1, col)
        text(2, MSG_Y + 4, (ring_label or "").upper()[:26], DIMC, 2)
        return
    if overlay == "overlay" and overlay_kind == "exception":
        # Bright phase for a latched exception.
        for c in range(COLS):
            field[c] = [overlay_hue_c, 90 + (c * 7) % 10]
    if overlay == "overlay":
        for c in range(COLS):
            hue, k = field[c]
            col = rgb(hsl(hue, 75, 25 + k // 3))
            for r in range(ROWS):
                fillf(c * (CELL_W + 1),
                              FIELD_Y + r * (CELL_H + 1),
                              CELL_W, CELL_H - 1, col)
    draw_ribbon()
    if ring_label:
        words = ring_label.upper()[:26]
        text(2, MSG_Y + 4, words, WHITE, 2)
        if len(ring_label) > 26:
            text(2, MSG_Y + 16, ring_label.upper()[26:52], WHITE, 2)

# ── the field's pulse state ──

def decay_tick(now):
    global ring_until, ring_label, overlay, latch
    if latch:
        # A latched exception remains active and alternates brightness.
        phase = (now // 400) % 2 == 1
        if (not band_phase) == phase or band_phase is None:
            pass
        overlay_draw(now)
    elif ring_until is not None and time.ticks_diff(now, ring_until) > 0:
        ring_until = None
        ring_label = None
        overlay = None
        redraw_idle()
    elif overlay is not None:
        # Update a temporary overlay until it expires.
        field_splash_tick(now)

band_phase = False

# ── identity ──
DESCRIPTOR_FILE = "/suzu.json"

def store_identity(dev_id):
    try:
        d = {}
        try:
            with open(DESCRIPTOR_FILE) as f:
                d = ujson.loads(f.read())
        except (OSError, ValueError):
            pass
        if d.get("device_id") != dev_id:
            d["device_id"] = dev_id
            with open(DESCRIPTOR_FILE, "w") as f:
                f.write(ujson.dumps(d))
    except OSError:
        pass

def descriptor():
    d = {}
    try:
        with open(DESCRIPTOR_FILE) as f:
            d = ujson.loads(f.read())
    except (OSError, ValueError):
        pass
    d["proto"] = "suzu/1"
    d["version"] = FACEPLATE_VERSION
    d["faceplate"] = FACEPLATE_ID
    d["mount"] = FACEPLATE_MOUNT
    try:
        d["hardware_id"] = "esp32-" + ubinascii.hexlify(unique_id()).decode()
    except Exception:
        pass
    return ujson.dumps(d)

# ── the wire ──

def r(msg, checksum=False):
    if checksum:
        x = 0
        for c in msg:
            x ^= ord(c)
        msg += "*%02x" % x
    sys.stdout.write(msg + "\n")
    time.sleep_ms(2)

def cmd(line):
    global last_rx, label, ring_label, ring_until, overlay, overlay_hue_c, latch
    line = line.strip()
    if not line:
        return
    if "*" in line:
        i = line.rfind("*")
        body, hexsum = line[:i], line[i + 1:]
        if len(hexsum) == 2:
            x = 0
            for c in body:
                x ^= ord(c)
            if "%02x" % x != hexsum:
                # the wire contract: no request goes unanswered — a
                # silent drop reads on the host as a dead face
                r("ERR,checksum")
                return
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
                r("OK")
                return
            for i, key in enumerate(("cpu", "mem", "gpu")):
                if len(p) > i + 1 and p[i + 1]:
                    values[key] = int(p[i + 1])
            if overlay is not None:
                r("OK")       # Defer metric rendering while an overlay is active.
                return
            for i in range(3):
                draw_gauge_line(i, G_Y0 + i * LINE_H)
            r("OK")
        elif c == "A":
            r("OK")           # any arrival relights the ribbon notch
        elif c == "J":
            ctx = ujson.loads(a)
            if isinstance(ctx, dict):
                if ctx.get("shot"):
                    # Return the capture buffer in chunks to limit memory use.
                    import ubinascii
                    sys.stdout.write("OK,")
                    x = 0
                    for ch in b"OK,":
                        x ^= ch
                    mv = memoryview(capture_buffer)
                    for i in range(0, len(capture_buffer), 510):
                        chunk = ubinascii.b2a_base64(mv[i:i + 510])[:-1]
                        sys.stdout.write(chunk)
                        for ch in chunk:
                            x ^= ch
                        time.sleep_ms(1)
                    sys.stdout.write("*%02x" % (x & 0xFF))
                    sys.stdout.write("\n")
                    time.sleep_ms(2)
                    return
                if ctx.get("name") and ctx["name"] != label:
                    label = ctx["name"]
                    save_label()
                    if overlay is None:
                        draw_ribbon()
                if ctx.get("device_id"):
                    store_identity(ctx["device_id"])
            r("OK")
        elif c == "S":
            if a and a != label:
                label = a
                save_label()
                if overlay is None:
                    draw_ribbon()
            r("OK")
        elif c == "X":
            overlay = None
            overlay_hue_c = HUE_INFO
            latch = False
            ring_until = None
            ring_label = None
            redraw_idle()
            r("OK")
        elif c == "R":
            p = a.split(",")
            signal = p[0].lower()
            word = signal.split(".", 1)[0]
            qual = signal.split(".", 1)[1] if "." in signal else ""
            if word[:5] == "alert" or word[:4] == "crit" or word[:9] == "exception":
                kind = "exception"
            elif word[:4] == "info" or word[:2] == "ok" or word[:8] == "allclear":
                kind = "info"
            else:
                kind = "warn"
            ring_label = " ".join(p[5:])[:52] or None
            if len(p) > 4:
                ring_seq = p[4]
            overlay = "overlay"
            overlay_kind = kind
            overlay_hue_c = OVERLAY_HUES.get(kind, HUE_WARN)
            latch = kind == "exception"
            ring_until = (None if latch
                          else time.ticks_add(time.ticks_ms(), OVERLAY_MS))
            overlay_draw(time.ticks_ms())
            ack = "OK," + p[4] if len(p) > 4 else "OK"
            r(ack, checksum=True)
        else:
            r("ERR,unknown:%s" % c)
    except (ValueError, IndexError) as e:
        r("ERR,%s" % e)
    resume_display()

def resume_display():
    global idle, idle_init
    if idle:
        idle = False
        idle_init = False
        redraw_idle()

# ── idle star animation ──
MIDNIGHT = (5, 6, 26)
_SIN = (0, 10, 20, 29, 38, 47, 56, 63, 70, 77, 83, 88, 92, 96, 98, 100,
        100, 100, 98, 96, 92, 88, 83, 77, 70, 63, 56, 47, 38, 29, 20, 10,
        0, -10, -20, -29, -38, -47, -56, -63, -70, -77, -83, -88, -92, -96, -98, -100,
        -100, -100, -98, -96, -92, -88, -83, -77, -70, -63, -56, -47, -38, -29, -20, -10)
_SIN_LEN = 64
idle = False
idle_init = False
idle_t = 0
stars = None
flies = None
fly_prev = None

def midnight_init():
    global stars, flies, fly_prev, idle_init
    seed = 42
    stars = []
    for _ in range(12):
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        sx = seed % W
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        sy = seed % H
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        tier = seed % 4
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        period = 30 + (seed % 50)
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        phase = seed % period
        stars.append((sx, sy, tier, period, phase))
    flies = [
        [0, 0, 0, 0, 149, 97, 350, 200, 350, 700, 0, 31],
        [0, 0, 0, 0, 113, 79, 280, 300, 700, 1300, 0, 23],
        [0, 0, 0, 0, 83, 127, 250, 180, 1000, 1900, 0, 37],
    ]
    fly_prev = [None, None, None]
    idle_init = True

def midnight_draw():
    global fly_prev
    t = idle_t
    for sx, sy, tier, period, phase in stars:
        pos = (t + phase) % period
        sv = _SIN[pos * _SIN_LEN // period]
        if sv > 50:
            pixelf(sx, sy, rgb((232, 228, 216)))
        elif sv > 0:
            pixelf(sx, sy, rgb((96, 94, 84)))
        else:
            pixelf(sx, sy, rgb(MIDNIGHT))
    for idx, particle in enumerate(flies):
        prev = fly_prev[idx]
        if prev:
            fillf(prev[0] - 3, prev[1] - 3, 7, 7, rgb(MIDNIGHT))
            for sx, sy, tier, period, phase in stars:
                if prev[0] - 4 <= sx <= prev[0] + 4 and \
                        prev[1] - 4 <= sy <= prev[1] + 4:
                    sv = _SIN[((t + phase) % period) * _SIN_LEN // period]
                    if sv > 50:
                        pixelf(sx, sy, rgb((232, 228, 216)))
        particle[2] = (particle[2] + 1) % particle[4]
        particle[3] = (particle[3] + 1) % particle[5]
        particle[10] = (particle[10] + 1) % particle[11]
        x_si = particle[2] * _SIN_LEN // particle[4]
        y_si = particle[3] * _SIN_LEN // particle[5]
        px10 = particle[8] + particle[6] * _SIN[x_si] // 100
        py10 = particle[9] + particle[7] * _SIN[y_si] // 100
        px = max(4, min(W - 5, px10 // 10))
        py = max(4, min(H - 5, py10 // 10))
        p_si = particle[10] * _SIN_LEN // particle[11]
        pulse = _SIN[p_si]
        if pulse > 30:
            fillf(px - 3, py - 3, 7, 7, rgb((60, 40, 15)))
        if pulse > -30:
            fillf(px - 2, py - 2, 5, 5, rgb((140, 100, 40)))
        fillf(px - 1, py - 1, 3, 3, rgb((255, 220, 140)))
        fly_prev[idx] = (px, py)

# ── label persistence ──
LABEL_FILE = "/label.txt"

def save_label():
    try:
        with open(LABEL_FILE, "w") as f:
            f.write(label)
    except OSError:
        pass

def load_label():
    global label
    try:
        with open(LABEL_FILE) as f:
            n = f.read().strip()
        if n:
            label = n
    except OSError:
        pass

# ── the composite states ──

def redraw_idle():
    fillall(0)
    draw_ribbon()
    for i in range(3):
        draw_gauge_line(i, G_Y0 + i * LINE_H)
    draw_message()
    field_draw_full()
    draw_marker_tick()

def draw_marker_tick():
    global activity_indicator_lit
    fresh = last_rx is not None and \
        time.ticks_diff(time.ticks_ms(), last_rx) < MARKER_MS
    if fresh != activity_indicator_lit:
        activity_indicator_lit = fresh
        draw_ribbon()

# ── init & main loop ──

def init_display():
    global tft
    try:
        spi = SPI(2, baudrate=40000000, sck=Pin(18), mosi=Pin(19), miso=None)
        import st7789
        # The constructor takes the native portrait dimensions (135x240),
        # and rotation=1 selects the landscape coordinate system. Passing
        # (240,135) creates an invalid address window and displays only part
        # of the capture buffer.
        tft = st7789.ST7789(
            spi, 135, 240,
            reset=Pin(23, Pin.OUT), cs=Pin(5, Pin.OUT), dc=Pin(16, Pin.OUT),
            backlight=Pin(4, Pin.OUT), rotation=1)
        tft.init()
        fillall(0)
        tft.on()
        return True
    except Exception as e:
        r("ERR,display_init:%s" % e)
        return False

band_phase = False

def decay_tick(now):
    global ring_until, ring_label, overlay, latch, band_phase
    if latch:
        # the exception holds: the field blinks between burn and dim
        phase = (now // 400) % 2 == 1
        if phase != band_phase:
            band_phase = phase
            overlay_draw(now)
    elif ring_until is not None and time.ticks_diff(now, ring_until) > 0:
        ring_until = None
        ring_label = None
        overlay = None
        redraw_idle()

def main():
    global idle, idle_init, idle_t, last_rx, capture_buffer
    gc.collect()
    # The capture_buffer first, before fragmentation: one contiguous
    # 32.4 KB (rgb332, a byte a pixel) — the camera's film, priced
    # to this heap (rgb565's 64.8 KB does not fit).
    capture_buffer = bytearray(W * H)
    # the console is the wire on ESP32 (sys.stdin/stdout); a UART(0)
    # re-init kills the REPL console — the harvested PoC knew it
    load_label()
    has_display = init_display()
    if not has_display:
        r("ERR,failed_to_init_display — answering the wire headless")
    field_init()
    redraw_idle()
    r("suzu aurora — tdisplay console, suzu/1")
    r("OK," + descriptor(), checksum=True)
    poll = select.poll()
    poll.register(sys.stdin, select.POLLIN)
    last_frame = time.ticks_ms()
    while True:
        try:
            now = time.ticks_ms()
            # idle transition: start the particle animation after 10 s without input
            if last_rx is not None and \
                    time.ticks_diff(now, last_rx) > REST_MS and not idle:
                idle = True
                idle_init = False
                idle_t = 0
                fillall(rgb(MIDNIGHT))
            if idle and last_rx is not None and \
                    time.ticks_diff(now, last_rx) < REST_MS:
                idle = False
                redraw_idle()
            if time.ticks_diff(now, last_frame) >= TICK_MS and tft is not None:
                last_frame = now
                idle_t += 1
                if idle:
                    if not idle_init:
                        midnight_init()
                        fillall(rgb(MIDNIGHT))
                    midnight_draw()
                elif overlay is not None:
                    decay_tick(now)
                    if overlay is not None and not latch:
                        field_splash_tick(now)
                else:
                    field_sweep_tick()
            events = poll.poll(0)
            # Drain all available input. Processing one line per tick can
            # overflow the RX ring during expensive draws and corrupt commands.
            # This module owns the event loop, so it must empty the queue before
            # entering idle mode.
            while events:
                line = sys.stdin.readline()
                if line:
                    cmd(line)
                events = poll.poll(0)
            if not events:
                time.sleep_ms(5)
            if idle_t % 100 == 0:
                gc.collect()
        except KeyboardInterrupt:
            break
        except Exception as e:
            r("ERR,%s" % e)
            time.sleep_ms(100)

main()
