# suzu — portrait numerals faceplate (esp8266-oled-v2, suzu/1).
# Portrait composition, 64 wide (u) x 128 tall (v); pixel(u,v)->oled(v,63-u).
# Right edge: the yellow label band. Left: CPU/GPU/MEM big numerals,
# 1-px pulse dividers lit by the audio.level input. Frames (newline-term):
#   I -> OK,{descriptor}*hh | K -> OK | G,report,<cpu>,<mem>,<gpu> (255=dash)
#   A,audio.level,<v> | J,{"name":...} | S,<name> | X | R,... -> ring blink.
# `*hh` checksums verified; 10 s without frames -> the display dims (dim).
# Full contract: README.md. Keep this file SMALL — the ESP8266 compiles
# it into an 80 KB heap; the legacy face fit at ~11 KB source.

import gc, math, sys, time, select
from machine import Timer, UART

math_cos = math.cos             # trigonometry used to draw the overlay circle
math_sin = math.sin

W, H = 64, 128            # portrait: u 0..63 across, v 0..127 down
INVERT = True            # the -inverted build flips this (tools/build_faceplates.py):
                          # Mirror the composition along its long axis so
                          # connector-up and connector-down mounts remain readable.
                          #
BAND_U = 48               # the yellow band starts here (16 px wide)
FACEPLATE_ID = "numerals"             # stable faceplate identifier
FACEPLATE_MOUNT = "up"          # the mount; the build sets it per mount
FACEPLATE_VERSION = "2.0.0"       # this mount's faceplate version; the build sets it per mount
TEXT_FLIP = True         # left-aligned mounts rotate the text area 180°
                          # Keep text upright for rotated mounts.
# The physical yellow strip does not rotate with the board. Inverted mounts
# move the label band to the opposite edge to preserve the layout.
NUM_U = 0                 # the numeral column's left edge
BAND_X = BAND_U           # the strip's left edge (16 px wide)
if INVERT:
    NUM_U = W - BAND_U    # 16: numerals u 16..63
    BAND_X = 0            # the strip re-homes to the panel's other edge
AREA_H = 42               # 3 areas x 42 + 2 dividers = 128
NUM_H = 34                # numeral zone inside an area
REST_MS = 10000           # idle timeout after the last received frame
BOOT_IDLE_MS = 3000       # additional startup delay before idle mode
I2C_SCL, I2C_SDA = 12, 14 # the class's OLED wiring (D6/D5), 400 kHz

u = UART(0, 115200)
oled = None
tick = None
last_rx = None
boot_ms = None
idle = False
idle_particles = ()                   # idle animation particles
label = "suzu"
overlay = None               # the overlay: None (metrics showing), "full",
                           # or the addressed area 0..2 (cpu/gpu/mem)
overlay_glyph = None         # the full overlay's subject: info|warn|exception
notification_label = None          # active notification label
notification_expires_at = None          # when the overlay ends (ticks_ms); None = latched
notification_latched = False              # True while an exception holds the overlay
notification_sequence = "0"             # notification sequence id included in DONE
band_lit = True            # the flash phase while a latched overlay holds
values = {"cpu": 255, "mem": 255, "gpu": 255}
pulse_target = 0
pulse_lit = 0

# ── numerals: digits_bebas.bin — raw glyph rows (2 B each, MSB =
# leftmost), one width byte per glyph, 11 glyphs in DIG_CHARS order.
# Read one glyph at a time at draw: the file costs ZERO import RAM,
# which the ESP8266's 80 KB heap cannot spare (a .py font module
# OOMs the display driver). Missing file -> the display falls back to
# dashes, not a crash. ──
DIG_CHARS = "0123456789-"
DIG_H = 24
DIG_STRIDE = 1 + DIG_H * 2
DIG_FILE = "/digits_bebas.bin"
NUM_H = DIG_H

# ── the overlay grammar (event overlay behavior). Every event takes one of
# three modes selected by the signal first word: info/ok/allclear display the
# encircled I; warn and the other verbs hold the triangle; the
# exception family (alert/crit) latches — it flashes until allclear
# or X. A bare event takes the whole overlay: the panel clears and one
# one large glyph is displayed. A qualifier that names a metric area
# (cpu/gpu/mem) addresses only that area: its sprite replaces the
# numeral while the other areas remain visible. Any signal the faceplate
# cannot place uses the full-display overlay. ──
OVERLAY_MS = 5000            # overlay duration; latched overlays do not expire

# ── notification icons: icons.bin — 8x8 sprites, 8 bytes each, MSB-left
# rows, one per metric area, in the areas' own order. Zero boot RAM —
# read at draw, like the digits. ──
ICON_KEYS = "cpu gpu mem"
ICON_FILE = "/icons.bin"

# ── microglyphs: 3x5, upright (rows -> +v, cols -> +u). Packed as a
# 36-char key strip + 2 bytes (15 bits) per glyph — ~150 RAM bytes
# where a dict of tuples costs ~3 KB this board doesn't have. ──
GLYPH_KEYS = "ABCDEFGHIKLMNOPRSTUVWXYZ0123456789- "
GLYPH_BITS = b"+\xedk\xae9#kny\xa7y\xa49k[\xedt\x97[\xadI'_\xedkm+jk\xa4k\xad8\x8et\x92[o[j[\xfdZ\xadZ\x92r\xa7{o,\x97R\xa7r\xcf[\xc9\x7f\x8e?kr\x92{\xefk\xce\x01\xc0\x00\x00"

# Sine lookup table (scaled by 100) used by idle particles.
SIN = (0, 38, 70, 92, 100, 92, 70, 38, 0, -38, -70, -92, -100, -92, -70, -38)

# Persist the label so resets do not discard text already sent by the Resident.
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
    """Load the persistent device identity from suzu.json (ADR-0003)."""
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
# and the numeral order inverts with the mount.

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
    d["version"] = FACEPLATE_VERSION   # faceplate version from faceplate.yaml
    d["faceplate"] = FACEPLATE_ID
    d["mount"] = FACEPLATE_MOUNT            # the orientation: down | up | left | right
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
    """A microglyph rotated 90° — the text rotation convention. The letter's
    5-row height spans the band across (u 0..4), its 3-column width
    runs along it; the top of each letter faces the band's outer
    edge, and the inverted build's mirrored columns keep the rendered
    view reading exactly like the parent's."""
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

def draw_band(inverted=False):
    """Draw the notification or persistent label in the mount orientation."""
    if TEXT_FLIP:
        x, v0, step = 6, H - 5, -4
    else:
        x, v0, step = BAND_U + 5, 4, 4
    rect(BAND_X, 0, W - BAND_U, H, 1 if inverted else 0)
    text = notification_label if notification_label else label
    for i, ch in enumerate(text.upper()[:30]):   # 4 + 29*4 <= 127
        band_glyph(x, v0 + i * step, ch, 0 if inverted else 1)

def draw_divider(v):
    """1-px divider; the lit run extends from the label band, growing
    away from it (left in the parent mount, right in the inverted)."""
    rect(NUM_U, v, BAND_U, 1, 0)
    if pulse_lit:
        run_u = NUM_U if INVERT else BAND_U - pulse_lit
        rect(run_u, v, pulse_lit, 1, 1)

def blit(u, v, w, rowbytes):
    """Draw one glyph's rows at (u,v); horizontal runs become fill_rects."""
    for rix in range(len(rowbytes) // 2):
        bits = (rowbytes[rix * 2] << 8) | rowbytes[rix * 2 + 1]
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
        g = bytearray(DIG_STRIDE)      # explicit "not measured" dash
        g[0] = 10
        g[1 + (DIG_H // 2) * 2] = 0x0F
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
    global pulse_lit, idle, notification_expires_at, notification_label, overlay, overlay_glyph, notification_latched, band_lit
    now = time.ticks_ms()
    if notification_latched:
        # Flash a latched exception by inverting every 400 ms.
        phase = (now // 400) % 2 == 1
        if band_lit != phase:
            band_lit = phase
            overlay_draw()
    elif notification_expires_at is not None and time.ticks_diff(now, notification_expires_at) > 0:
        notification_expires_at = None             # The notification expired; restore metrics and the persistent label.
        notification_label = None
        overlay = None
        overlay_glyph = None
        redraw()
    if pulse_lit > pulse_target and overlay != "full":
        pulse_lit = pulse_target + (pulse_lit - pulse_target) * 3 // 4
        draw_divider(AREA_H - 1)
        draw_divider(AREA_H * 2 - 1)
        oled.show()
    now = time.ticks_ms()
    quiet_for = (REST_MS if last_rx is not None
                 else BOOT_IDLE_MS + REST_MS)
    anchor = last_rx if last_rx is not None else boot_ms
    if time.ticks_diff(now, anchor) > quiet_for and not idle and not notification_latched:
        idle_start()

def idle_start():
    """Initialize three vertically moving idle particles."""
    global idle, idle_particles, boot_ms, idle_started_at
    idle = True
    boot_ms = None
    idle_started_at = time.ticks_ms()
    draw_band()                   # Draw the persistent label during the idle animation.
    idle_particles = [list(p) for p in (
        # [u0, phase, speed, amplitude, v, delay]. Start particles at
        # staggered 0, 1000, and 2000 ms offsets.
        (NUM_U + 24, 0, 1, 6, -2, 0),
        (NUM_U + 34, 4, 2, 8, 40, 1000),
        (NUM_U + 20, 10, 1, 5, 84, 2000),
    )]

def idle_step():
    rect(NUM_U, 0, BAND_U, H, 0)
    now = time.ticks_ms()
    for particle in idle_particles:
        if time.ticks_diff(now, idle_started_at) < particle[5]:
            continue                    # Wait for the particle.s configured delay.
        particle[4] += particle[2]                    # Advance the particle vertically.
        if particle[4] > 129:
            particle[4] = -1
        particle[1] = (particle[1] + particle[2]) % 16       # advance the horizontal oscillation
        x = particle[0] + (particle[3] * SIN[particle[1]]) // 100   # ±amp, not ±25
        px(max(NUM_U + 1, min(NUM_U + BAND_U - 2, x)), particle[4], 1)
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

# ── the overlay's big glyphs: code-drawn geometry, not sprites — they
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
    """The encircled I — the event that carries no alarm."""
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

def overlay_draw():
    """The overlay: a bare event clears the panel and one big glyph
    is displayed; a qualified event replaces only that area's numeral
    while other metrics remain visible. The band clears before showing the label
    and flashes by inversion while a notification is latched."""
    flash = notification_latched and (time.ticks_ms() // 400) % 2 == 1
    if overlay == "full":
        rect(NUM_U, 0, BAND_U, H, 0)
        if overlay_glyph == "exception":
            draw_warn(flash)
        elif overlay_glyph == "warn":
            draw_warn(False)
        else:
            draw_info()
    elif overlay is not None:
        v0 = overlay * AREA_H
        rect(NUM_U, v0 + 1, BAND_U, NUM_H + 2, 0)
        u = NUM_U + (BAND_U - 16) // 2
        v = v0 + 1 + (NUM_H - 16) // 2
        if flash:
            rect(u, v, 16, 16, 1)
            draw_icon(u, v, overlay, 0)
        else:
            draw_icon(u, v, overlay, 1)
    draw_band(flash)
    oled.show()

def resume_display():
    global idle
    if idle:
        idle = False
        if overlay is not None:
            overlay_draw()              # preserve the active overlay
        else:
            redraw()

# ── frames ──

def cmd(line):
    global last_rx, label, values, notification_expires_at, notification_label
    global overlay, overlay_glyph, notification_latched, band_lit, notification_sequence
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
                r("OK")               # a metrics group this face does not declare
                return
            for i, key in enumerate(("cpu", "mem", "gpu")):
                if len(p) > i + 1 and p[i + 1]:
                    values[key] = int(p[i + 1])
            if overlay == "full":
                r("OK")               # defer metrics while the overlay is active
                return
            if overlay is not None:
                # A qualified overlay replaces the addressed metric area;
                # the other two remain visible.
                for i in range(3):
                    if i != overlay:
                        draw_area(i, ("CPU", "GPU", "MEM")[i])
                oled.show()
                r("OK")
                return
            for i, label_text in enumerate(("CPU", "GPU", "MEM")):
                draw_area(i, label_text)
            oled.show()
            r("OK")
        elif c == "A":
            p = a.split(",")
            if len(p) >= 2 and p[0] == "audio.level" and overlay is None:
                set_pulse(int(p[1]))  # the overlay also replaces the dividers
            r("OK")
        elif c == "J":
            import ujson
            ctx = ujson.loads(a)
            if isinstance(ctx, dict):
                if ctx.get("shot"):
                    # Return a base64 copy of the display in the acknowledgement:
                    # one write of approximately 120 ms without rebooting.
                    # This supports capture while the session remains active.
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
            # Clear the latched overlay.
            overlay = None
            overlay_glyph = None
            notification_latched = False
            notification_expires_at = None
            notification_label = None
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
            notification_label = " ".join(p[5:])[:30] or None
            if len(p) > 4:
                notification_sequence = p[4]
            keys = ICON_KEYS.split()
            prev = overlay
            overlay = keys.index(qual) if qual in keys else "full"
            overlay_glyph = glyph
            if overlay != "full":
                # Clear the previous overlay, redraw stored metrics, and
                # render the new overlay in its selected area.
                rect(NUM_U, 0, BAND_U, H, 0)
                for i in range(3):
                    if i != overlay:
                        draw_area(i, ("CPU", "GPU", "MEM")[i])
            notification_latched = glyph == "exception"    # Alert and critical notifications remain active.
            notification_expires_at = (None if notification_latched     # Non-latched notifications expire.
                          else time.ticks_add(time.ticks_ms(), OVERLAY_MS))
            band_lit = False
            overlay_draw()
            ack = "OK," + p[4] if len(p) > 4 else "OK"   # echo the seq
            r(ack, checksum=True)
        else:
            r("ERR,unknown:%s" % c)
    except (ValueError, IndexError) as e:
        r("ERR,%s" % e)
    resume_display()

def tcb(t):
    if idle:
        idle_step()
    else:
        decay()

def init():
    global oled, tick
    gc.collect()
    # The frozen ssd1306 driver, driven directly — NOT the legacy's
    # dashboard class. Importing that 11.5 KB .py means compiling it on
    # this 80 KB-heap board, and its parse tree alone exhausts the heap
    # during import. The faceplate
    # uses only the native framebuffer API.
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
    idle_start()                  # start the idle animation; the first
                                  # host data resumes the active display

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
