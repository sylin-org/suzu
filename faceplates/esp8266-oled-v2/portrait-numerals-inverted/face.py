# suzu — portrait numerals faceplate (esp8266-oled-v2, suzu/1).
# Portrait composition, 64 wide (u) x 128 tall (v); pixel(u,v)->oled(v,63-u).
# Right edge: the yellow label band. Left: CPU/GPU/MEM big numerals,
# 1-px pulse dividers lit by the audio.level lane. Frames (newline-term):
#   I -> OK,{descriptor}*hh | K -> OK | G,report,<cpu>,<mem>,<gpu> (255=dash)
#   A,audio.level,<v> | J,{"name":...} | S,<name> | X | R,... -> ring blink.
# `*hh` checksums verified; 10 s without frames -> the face rests (dim).
# Full contract: README.md. Keep this file SMALL — the ESP8266 compiles
# it into an 80 KB heap; the ancestor face fit at ~11 KB source.

import gc, sys, time, select
from machine import Timer, UART

W, H = 64, 128            # portrait: u 0..63 across, v 0..127 down
INVERT = True            # the -inverted build flips this (tools/build_faceplates.py):
                          # the composition mirrors along its long axis, so the
                          # board hung connector-up reads exactly as this reads
                          # connector-down. Same art, same words, other hang.
BAND_U = 48               # the yellow band starts here (16 px wide)
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
ring_label = None          # a moment's text, shown in the band briefly
ring_icon = None           # its icon's index, when the signal has one
ring_verb = None           # the objective: alert latches, the rest bloom
latch = False              # True while an alert is up
band_lit = True            # the band's blink phase during a latched alert
last_blink = 0
ring_verb = None           # the ring objective: alert latches, others bloom
values = {"cpu": 255, "mem": 255, "gpu": 255}
pulse_target = 0
pulse_lit = 0

# ── numerals: digits_bebas.bin — raw glyph rows (2 B each, MSB =
# leftmost), one width byte per glyph, 11 glyphs in DIG_CHARS order.
# Read one glyph at a time at draw: the file costs ZERO import RAM,
# which the ESP8266's 80 KB heap cannot spare (a .py font module
# OOMs the display driver). Missing file -> the face degrades to
# dashes, not a crash. ──
DIG_CHARS = "0123456789-"
DIG_H = 24
DIG_STRIDE = 1 + DIG_H * 2
DIG_FILE = "/digits_bebas.bin"
NUM_H = DIG_H

# ── the say vocabulary: the nine rings. Each names its objective —
# alert latches (the only sustained-fast state), allclear heals,
# momentary rings bloom for ~5 s and return. Urgency is the 0-5
# vitality scale rendered as tempo. Icons: the signal's qualifier may
# name one; the objective never depends on it. ──
RING_VERBS = ("alert", "allclear", "completion", "discovery", "begin",
              "departure", "tended", "transition", "heartbeat")

# ── moment icons: icons.bin — 8x8 sprites, 8 bytes each, MSB-left
# rows, in ICON_KEYS order. A ring's signal names one; the icon rides
# the overlay. Zero boot RAM — read at draw, like the digits. ──
ICON_KEYS = "disk usb gear net clock heart warn wilt"
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
    d["version"] = "1.0.0"
    d["faceplate"] = "portrait-numerals-inverted" if INVERT else "portrait-numerals"
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
    runs along it; the top of each letter faces the band's outer
    edge, and the inverted build reads bottom-up."""
    i = GLYPH_KEYS.find(ch)
    if i < 0:
        return
    bits = (GLYPH_BITS[i * 2] << 8) | GLYPH_BITS[i * 2 + 1]
    for row in range(5):
        for col in range(3):
            if bits & (1 << (14 - row * 3 - col)):
                if INVERT:
                    px(u + row, v + col, on)
                else:
                    px(u + (4 - row), v + col, on)

def draw_band():
    """The yellow strip, kept dark: the label as glowing spine text,
    reading top -> bottom. Lit letters outread black cutouts at 1 px."""
    x = BAND_U + 5                    # rotated glyphs are 5 across
    text = ring_label if ring_label else label
    for i, ch in enumerate(text.upper()[:30]):   # 4 + 29*4 <= 127
        band_glyph(x, 4 + i * 4, ch, 1)

def draw_divider(v):
    """1-px divider; the lit run hangs off the label band, growing left."""
    rect(0, v, BAND_U, 1, 0)
    if pulse_lit:
        rect(BAND_U - pulse_lit, v, pulse_lit, 1, 1)

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
        g = bytearray(DIG_STRIDE)      # honest "not measured" dash
        g[0] = 10
        g[1 + (DIG_H // 2) * 2] = 0x0F
        return (10, bytes(g))
    return (g[0], g[1:])

def draw_num(v0, text):
    """Big numerals centered in the 48-px column, below v0."""
    rect(0, v0, BAND_U, NUM_H + 2, 0)
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
    x = max(0, (BAND_U - total) // 2)
    y = v0 + 1
    for (w, rowbytes) in sprs:
        blit(x, y, w, rowbytes)
        x += w + gap

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
    global pulse_lit, idle, ring_label, ring_icon, ring_until
    if ring_until is not None and time.ticks_diff(time.ticks_ms(), ring_until) > 0:
        ring_label = None             # the moment passed; the house returns
        ring_icon = None
        ring_until = None
        redraw()
    if pulse_lit > pulse_target:      # decay exponential toward the target
        pulse_lit = pulse_target + (pulse_lit - pulse_target) * 3 // 4
        draw_divider(AREA_H - 1)
        draw_divider(AREA_H * 2 - 1)
        oled.show()
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
        (24, 0, 1, 6, -2, 0),
        (34, 4, 2, 8, 40, 1000),
        (20, 10, 1, 5, 84, 2000),
    )]

def idle_step():
    rect(0, 0, BAND_U, H, 0)
    now = time.ticks_ms()
    for f in ff:
        if time.ticks_diff(now, ff_t0) < f[5]:
            continue                    # staggered entrance, as the poc
        f[4] += f[2]                    # drift down at the poc's speeds
        if f[4] > 129:
            f[4] = -1
        f[1] = (f[1] + f[2]) % 16       # the poc's bob tempo
        x = f[0] + (f[3] * SIN[f[1]]) // 100   # ±amp, not ±25
        px(max(1, min(46, x)), f[4], 1)
    oled.show()

def draw_icon(u, v, i):
    try:
        with open(ICON_FILE, "rb") as f:
            f.seek(i * 8)
            rows = f.read(8)
    except OSError:
        return
    for r in range(8):
        for c in range(8):
            if rows[r] & (0x80 >> c):
                px(u + c * 2, v + r * 2, 1)
                px(u + c * 2 + 1, v + r * 2, 1)
                px(u + c * 2, v + r * 2 + 1, 1)
                px(u + c * 2 + 1, v + r * 2 + 1, 1)

def ring_draw():
    """The overlay: the moment's icon centered in the column, its
    words glowing on the band."""
    rect(0, 0, BAND_U, H, 0)
    if ring_icon is not None:
        draw_icon((BAND_U - 16) // 2, 20, ring_icon)
    draw_band()
    oled.show()

def wake():
    global idle
    if idle:
        idle = False
        redraw()

# ── frames ──

def cmd(line):
    global last_rx, label, values
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
            r("OK")
        elif c == "S":                # compat alias: set the band
            if a and a != label:
                label = a
                save_label()
                draw_band()
                oled.show()
            r("OK")
        elif c == "X":
            r("OK")                   # nothing overlaid yet; ground is showing
        elif c == "R":
            global ring_label, ring_until, ring_icon, ring_verb, band_lit, latch
            p = a.split(",")
            signal = p[0].lower()
            verb = next((v for v in RING_VERBS if signal.startswith(v)), "transition")
            ring_verb = verb
            qual = signal.split(".", 1)[1] if "." in signal else ""
            keys = ICON_KEYS.split()
            ring_icon = keys.index(qual) if qual in keys else -1
            ring_label = " ".join(p[6:])[:30] or None
            if verb == "alert":
                latch = True                    # alert latches until allclear
                ring_until = None
            else:
                latch = False
                ring_until = time.ticks_add(time.ticks_ms(), 5000)
            band_lit = True
            ring_draw()
            oled.show()
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
