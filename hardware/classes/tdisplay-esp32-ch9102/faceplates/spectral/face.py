# suzu — Spectral faceplate (tdisplay-esp32-ch9102, suzu/1).
# Landscape instrument, 240 wide (u) x 135 tall (v), RGB565.
#
# [header: device name, underline, activity indicator]
# [CPU | rainbow bar | value]  x3
# [message strip: event icon and label while the overlay is active]
# [spectrum: 36 x 14 segments, hue descending purple -> red, white
#  peaks falling under gravity, a scanline sweeping display hardware]
#
# The bars are a rainbow anchored to the FULL width and revealed
# left to right — filling to 45% shows the left 45% of the spectrum,
# matching the HTML mock's clip-path. The left third uses CPU, the
# middle uses memory, and the right uses GPU, with jitter and falling peaks.
#
# An event temporarily colors the name, underline, indicator, bars,
# numerals, and spectrum according to urgency (ADR-0001). A latched
# exception remains until `allclear` or `X`.
#
# After 10 seconds without host data, show a dim grid and scanline.
# Any new frame returns to the active display.
#
# Keep this file SMALL.

import gc
import select
import sys
import time

from machine import Pin, SPI, unique_id

import ujson
import ubinascii

# ── faceplate metadata ──
FACEPLATE_ID = "spectral"
FACEPLATE_MOUNT = "down"
FACEPLATE_VERSION = "1.0.0"

W = 240
H = 135

HEADER_H = 18
M_Y0 = 24
M_PITCH = 18
BAR_X = 26
BAR_W = 170
BAR_H = 7
MSG_Y = 80
FIELD_Y = 93
COLS = 36
SEG_W = 4
COL_PITCH = 6
G_X0 = 13
ROWS = 14
SEG_H = 2
ROW_PITCH = 3

OVERLAY_MS = 5000
REST_MS = 10000
MARKER_MS = 300
TICK_MS = 150

# Notification colors (ADR-0001), expressed as hue degrees.
# Spectral's palette: teal instrument, amber warning, red exception.
OVERLAY_HUES = {"info": 165, "ok": 120, "warn": 48, "exception": 350}
HUE_INFO = 165
HUE_OK = 120
HUE_WARN = 48
HUE_CRIT = 350

WHITE = (236, 232, 224)
TEAL = (0, 255, 204)
MUTED = (74, 119, 112)
DIMC = (120, 120, 120)
BG = (5, 5, 16)
TRACK = (26, 47, 43)

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
overlay_qual = ""            # cpu | gpu | mem — a qualified say's numeral
ring_label = None
ring_until = None
latch = False
ring_seq = "0"
splash_k = 80
activity_indicator_lit = False
dot_lit = False
lit = None                 # COLS lit heights (segment rows)
peaks = None               # COLS falling peak positions (float)
_jx = 137                  # the jitter walk
idle = False
idle_init = False
idle_t = 0
beam_y = 0

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
# bits are each channel's TOP bits (rrrrrggggggbbbbb -> rrrgggbb):
# r >> 13, g >> 8, b >> 3. A wrong shift reads a NEIGHBOR's bits —
# the blue >> 5 read green's low bits and bright blues vanished
# into reds (reproduced offline, pixel for pixel).
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

def hlinef(x, y, w, color):
    fillf(x, y, w, 1, color)

def vlinef(x, y, h, color):
    fillf(x, y, 1, h, color)

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

def col_hue(i):
    """The spectrum's descent: purple 280 at the left to red 0 at the
    right — the mock's one equation."""
    return 280 - i * 280 // (COLS - 1)

def bar_hue(x):
    """The bars carry the same rainbow, anchored to the FULL bar
    width: filling to N% reveals the left N% of the spectrum."""
    return 280 - x * 280 // BAR_W

# ── microglyphs (3x5, shared bits) ──
GLYPH_KEYS = "ABCDEFGHIKLMNOPRSTUVWXYZ0123456789-% "
GLYPH_BITS = b"+\xedk\xae9#kny\xa7y\xa49k[\xedt\x97[\xadI'_\xedkm+jk\xa4k\xad8\x8et\x92[o[j[\xfdZ\xadZ\x92r\xa7{o,\x97R\xa7r\xcf[\xc9\x7f\x8e?kr\x92{\xefk\xce\x01\xc0\x00\x00"
# the percent sign appended: two dots and a slash in 3x5
GLYPH_BITS += b"\x6c\x83"

def glyph(x, y, ch, color, gs=1):
    i = GLYPH_KEYS.find(ch)
    if i < 0:
        return
    bits = (GLYPH_BITS[i * 2] << 8) | GLYPH_BITS[i * 2 + 1]
    col = pack(color)
    for row in range(5):
        for c in range(3):
            if bits & (1 << (14 - row * 3 - c)):
                if gs == 1:
                    pixelf(x + c, y + row, col)
                else:
                    fillf(x + c * gs, y + row * gs, gs, gs, col)

def text(x, y, s, color, gs=1):
    for i, ch in enumerate(s.upper()):
        glyph(x + i * 4 * gs, y, ch, color, gs)

def text_w(s, gs=1):
    return len(s) * 4 * gs

# ── the overlay markers (the grammar's reserved shapes) ──
CIRCLED_I = (
    "...###...",
    "..#...#..",
    ".#.###.#.",
    "#...#...#",
    "#...#...#",
    "#...#...#",
    ".#.###.#.",
    "..#...#..",
    "...###...")

TRIANGLE = (
    "....#....",
    "....#....",
    "...#.#...",
    "...#.#...",
    "..#...#..",
    ".#.....#.",
    ".#.....#.",
    "#.......#",
    "#########")

def draw_bitmap(x, y, rows, color):
    col = pack(color)
    for j, row in enumerate(rows):
        for i, ch in enumerate(row):
            if ch == "#":
                pixelf(x + i, y + j, col)

def draw_marker(x, y, color):
    draw_bitmap(x, y,
                CIRCLED_I if overlay_kind == "info" else TRIANGLE, color)

# ── the header ──

def draw_header(tint=None):
    accent = tint or TEAL
    fillf(0, 0, W, HEADER_H, rgb(BG))
    name = label
    if name.startswith("stone-"):
        name = name[6:]
    text(4, 4, name.upper()[:26], accent, 2)
    hlinef(0, HEADER_H - 1, W, rgb(scale(accent, 30)))
    # the heartbeat dot: solid on fresh traffic, blinking otherwise
    fresh = last_rx is not None and \
        time.ticks_diff(time.ticks_ms(), last_rx) < MARKER_MS
    on = fresh or (time.ticks_ms() // 500) % 2 == 0
    _dot(on, accent)

def _dot(on, accent):
    global dot_lit
    dot_lit = on
    fillf(W - 9, 6, 4, 4, rgb(accent) if on else rgb(BG))

# ── the metric rows ──

def draw_metric_row(idx, tint=None):
    accent = tint
    y = M_Y0 + idx * M_PITCH
    name = ("CPU", "GPU", "MEM")[idx]
    key = ("cpu", "gpu", "mem")[idx]
    val = values[key]
    text(4, y + 1, name, MUTED, 1)
    fillf(BAR_X, y, BAR_W, BAR_H, rgb(TRACK))
    if val != 255:
        fw = val * BAR_W // 100
        if accent is not None:
            # a say re-tints the bars with its hue, solid
            fillf(BAR_X, y, fw, BAR_H, rgb(accent))
        else:
            for cx in range(0, fw - 1, 2):
                fillf(BAR_X + cx, y, 2, BAR_H, rgb(hsl(bar_hue(cx), 100, 50)))
    fillf(200, y - 1, 40, 12, rgb(BG))
    vs = "-" if val == 255 else str(val) + "%"
    vc = accent
    if vc is None:
        vc = TEAL if val != 255 else MUTED
    if overlay is not None and overlay_qual == key:
        vc = hsl(overlay_hue, 100, 55)
    text(W - 4 - text_w(vs, 2), y, vs, vc, 2)

def draw_metrics(tint=None):
    for i in range(3):
        draw_metric_row(i, tint)

# ── the message strip ──

def draw_strip(phase_bright=True):
    fillf(0, MSG_Y - 2, W, FIELD_Y - MSG_Y, rgb(BG))
    if not ring_label:
        return
    words = ring_label.upper()[:26]
    wc = WHITE if phase_bright else DIMC
    draw_marker(4, MSG_Y, wc)
    text(17, MSG_Y, words, wc, 2)

# ── the spectrum ──

def graph_init():
    global lit, peaks
    lit = [0] * COLS
    peaks = [0.0] * COLS

def influence(i):
    """Left third CPU, middle MEM, right GPU — the mock's mapping."""
    if i < COLS // 3:
        v = values["cpu"]
    elif i < COLS * 2 // 3:
        v = values["mem"]
    else:
        v = values["gpu"]
    return 0 if v == 255 else v

def draw_graph_col(i, height, hue=None, floor_l=9):
    """One column: dim segments below are ALWAYS faintly there (the
    mock's 0.15 opacity), lit segments bright, the peak marker
    white."""
    gx = G_X0 + i * COL_PITCH
    hu = col_hue(i) if hue is None else hue
    off = rgb(hsl(hu, 60, floor_l))
    on = rgb(hsl(hu, 100, 55))
    gy = FIELD_Y
    for j in range(ROWS):
        fillf(gx, gy, SEG_W, SEG_H, on if j < height else off)
        gy += ROW_PITCH

def draw_graph_full():
    fillf(0, FIELD_Y - 1, W, H - FIELD_Y + 1, rgb(BG))
    for i in range(COLS):
        draw_graph_col(i, lit[i])
        if peaks[i] >= 1:
            pj = int(peaks[i]) - 1
            fillf(G_X0 + i * COL_PITCH, FIELD_Y + pj * ROW_PITCH,
                  SEG_W, SEG_H, rgb(WHITE))

def graph_tick():
    """Update spectrum heights from metrics, jitter, peaks, and scanline."""
    global _jx, beam_y
    boost = 0
    hu = None
    if overlay is not None:
        boost = max(0, splash_k - 60)
        hu = overlay_hue
    for i in range(COLS):
        _jx = (_jx * 109 + 47) & 0xFF
        v = influence(i) + _jx % 31 - 15 + boost
        if v < 0:
            v = 0
        elif v > 100:
            v = 100
        h = v * ROWS // 100
        lit[i] = h
        if h >= peaks[i]:
            peaks[i] = h
        else:
            p = peaks[i] - 0.4
            peaks[i] = p if p > 0 else 0
        draw_graph_col(i, h, hu)
        if peaks[i] >= 1:
            pj = int(peaks[i]) - 1
            fillf(G_X0 + i * COL_PITCH, FIELD_Y + pj * ROW_PITCH,
                  SEG_W, SEG_H, rgb(WHITE))
    # the scanline rides on top; every segment repaints each tick, so
    # the old line is erased by the redraw itself — never smears
    beam_y = FIELD_Y + (beam_y - FIELD_Y + 2) % (H - FIELD_Y)
    hlinef(0, beam_y, W, rgb(hsl(overlay_hue if overlay else 165, 70, 55)))

# ── the overlay ──

def overlay_draw(now):
    """Render the event overlay. Latched exceptions alternate bright and dim states."""
    global _jx
    bright = not (latch and (now // 400) % 2 == 0)
    tint = hsl(overlay_hue, 100, 50) if bright else hsl(overlay_hue, 50, 14)
    draw_header(tint)
    draw_metrics(tint)
    draw_strip(bright)
    if latch:
        if bright:
            for i in range(COLS):
                _jx = (_jx * 109 + 47) & 0xFF
                h = (85 + _jx % 15) * ROWS // 100
                draw_graph_col(i, h, overlay_hue, floor_l=12)
        else:
            for i in range(COLS):
                draw_graph_col(i, 0, overlay_hue, floor_l=4)

def overlay_splash_tick(now):
    """Update a temporary overlay until it expires."""
    graph_tick()

def decay_tick(now):
    global ring_until, ring_label, overlay, overlay_qual, latch, band_phase
    if latch:
        phase = (now // 400) % 2 == 1
        if phase != band_phase:
            band_phase = phase
            overlay_draw(now)
    elif ring_until is not None and time.ticks_diff(now, ring_until) > 0:
        ring_until = None
        ring_label = None
        overlay = None
        overlay_qual = ""
        redraw_idle()

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
    global last_rx, label, ring_label, ring_until, overlay, overlay_hue
    global latch, overlay_kind, overlay_qual, splash_k, ring_seq
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
    resume_display()                    # awake before anything draws
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
            draw_metrics()
            r("OK")
        elif c == "A":
            r("OK")           # any arrival relights the dot
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
                        draw_header()
                if ctx.get("device_id"):
                    store_identity(ctx["device_id"])
            r("OK")
        elif c == "S":
            if a and a != label:
                label = a
                save_label()
                if overlay is None:
                    draw_header()
            r("OK")
        elif c == "X":
            overlay = None
            overlay_hue = HUE_INFO
            overlay_qual = ""
            latch = False
            ring_until = None
            ring_label = None
            redraw_idle()
            r("OK")
        elif c == "R":
            p = a.split(",")
            signal = p[0].lower()
            word = signal.split(".", 1)[0]
            overlay_qual = signal.split(".", 1)[1] if "." in signal else ""
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
            overlay_hue = OVERLAY_HUES.get(kind, HUE_WARN)
            latch = kind == "exception"
            splash_k = min(100, 60 + int(p[1] or 0) * 8)
            ring_until = (None if latch
                          else time.ticks_add(time.ticks_ms(), OVERLAY_MS))
            overlay_draw(time.ticks_ms())
            ack = "OK," + p[4] if len(p) > 4 else "OK"
            r(ack, checksum=True)
        else:
            r("ERR,unknown:%s" % c)
    except (ValueError, IndexError) as e:
        r("ERR,%s" % e)

def resume_display():
    global idle, idle_init
    if idle:
        idle = False
        idle_init = False
        redraw_idle()

# ── idle mode ──

def rest_init():
    global idle_init, beam_y
    idle_init = True
    beam_y = 0
    fillall(rgb(BG))
    g = rgb(hsl(165, 50, 6))
    for x in range(0, W, 12):
        vlinef(x, 0, H, g)
    for y in range(0, H, 12):
        hlinef(0, y, W, g)

def rest_tick():
    global beam_y
    fillf(0, beam_y, W, 1, rgb(BG))
    beam_y = (beam_y + 2) % H
    hlinef(0, beam_y, W, rgb(hsl(165, 60, 35)))

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
    global overlay_qual
    overlay_qual = ""
    fillall(rgb(BG))
    draw_header()
    draw_metrics()
    draw_strip()
    draw_graph_full()

def dot_tick():
    global activity_indicator_lit
    fresh = last_rx is not None and \
        time.ticks_diff(time.ticks_ms(), last_rx) < MARKER_MS
    if fresh != activity_indicator_lit:
        activity_indicator_lit = fresh
        if overlay is None:
            draw_header()

# ── init & main loop ──

def init_display():
    global tft
    try:
        spi = SPI(2, baudrate=40000000, sck=Pin(18), mosi=Pin(19), miso=None)
        import st7789
        # the constructor takes the NATIVE portrait panel (135x240)
        # with the turn applied after — against any other geometry the
        # address window wraps and display hardware shows a slice
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

def main():
    global idle, idle_init, idle_t, last_rx, capture_buffer, beam_y
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
    graph_init()
    redraw_idle()
    r("suzu spectral — tdisplay console, suzu/1")
    r("OK," + descriptor(), checksum=True)
    poll = select.poll()
    poll.register(sys.stdin, select.POLLIN)
    last_frame = time.ticks_ms()
    while True:
        try:
            now = time.ticks_ms()
            # idle transition: enter idle mode after 10 s without input
            if last_rx is not None and \
                    time.ticks_diff(now, last_rx) > REST_MS and not idle:
                idle = True
                idle_init = False
                idle_t = 0
                fillall(rgb(BG))
            if idle and last_rx is not None and \
                    time.ticks_diff(now, last_rx) < REST_MS:
                idle = False
                redraw_idle()
            if time.ticks_diff(now, last_frame) >= TICK_MS and tft is not None:
                last_frame = now
                idle_t += 1
                if idle:
                    if not idle_init:
                        rest_init()
                    rest_tick()
                elif overlay is not None:
                    decay_tick(now)
                    if overlay is not None and not latch:
                        overlay_splash_tick(now)
                else:
                    graph_tick()
                    dot_tick()
            events = poll.poll(0)
            # Drain all available input. Processing one line per tick can
            # overflow the RX ring during expensive draws and corrupt commands
            # (observed as ERR,unknown:AA). This module owns the event loop, so
            # it must empty the queue before entering idle mode.
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
