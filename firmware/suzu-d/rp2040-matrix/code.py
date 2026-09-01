# Suzu RP2040 matrix firmware, suzu/1.
#
# States:
#   IDLE   low-intensity particle animation
#   WAKE   transition from idle to metric indicators
#   WORK   three metric indicators; each metric controls its cycle time
#   ALERT  persistent alert animation at the selected pixel
#
# State-machine signals:
#   FRAME_G, FRAME_R, FRAME_ALLCLEAR, FRAME_X, FRAME_K,
#   WAKE_COMPLETE, SILENCE (10 seconds without host data), and LATCH.
#
# Transitions:
#   IDLE  + FRAME_G       -> WAKE
#   WAKE  + WAKE_COMPLETE -> WORK
#   WORK  + R(alert)      -> ALERT
#   ALERT + allclear/X    -> WORK
#   WORK  + SILENCE       -> IDLE
#   ALERT + SILENCE       -> ALERT
#
# The face contract lives in docs/the-face-contract.md.

import board
import json
import neopixel
import random
import supervisor
import sys
import time
import microcontroller

try:
    supervisor.disable_autoreload()   # a replug must not reload-loop
except AttributeError:
    pass

NUM = 25
COLS = 5
ROWS = 5
# These LEDs are RGB-wired; CircuitPython's neopixel silently defaults
# to GRB, which swaps red and green on the physical board while the J
# capture buffer (logical RGB) remains unchanged; the preview differed until
# this was set. Confirmed empirically 2026-08-30: GRB order showed
# amber frames as green pixels.
pixels = neopixel.NeoPixel(board.GP16, NUM, brightness=0.3, auto_write=False,
                           pixel_order=neopixel.RGB)

_VERSION = "1.0.0"
TICK = 0.05
IDLE_AFTER = 10.0
LABEL_FILE = "/label.txt"
VERBS = ("alert", "allclear", "completion", "discovery", "begin",
         "departure", "tended", "transition", "heartbeat")

# Protocol event colors.
HUES = {
    "alert": (255, 25, 0),
    "allclear": (0, 255, 90),
    "completion": (0, 170, 255),
    "discovery": (170, 60, 255),
    "begin": (0, 200, 120),
    "departure": (120, 120, 120),
    "tended": (255, 150, 0),
    "transition": (140, 90, 200),
    "heartbeat": (0, 70, 25),
    "info": (0, 170, 255),
    "warn": (255, 190, 0),
}

MAX_K = 0.5                  # normal maximum brightness
BLINK_K = 1.0                # high-metric blink brightness
WAKE_RISE_S = 0.9            # idle-to-work transition
WAKE_FLASH_S = 0.12          # final transition flash
WAKE_TOTAL = WAKE_RISE_S + WAKE_FLASH_S
DROP_LIFE = 1.1

# Persisted device label.
label = "suzu"
try:
    with open(LABEL_FILE) as f:
        label = f.read().strip() or label
except OSError:
    pass


def _load_descriptor():
    try:
        with open("/suzu.json") as f:
            return json.loads(f.read())
    except (OSError, ValueError):
        return {}


def _descriptor():
    d = dict(_DESCRIPTOR)
    d["proto"] = "suzu/1"
    d["version"] = _VERSION
    try:
        uid = microcontroller.cpu.uid
        d["hardware_id"] = "rp2040-" + "".join("{:02x}".format(b) for b in uid)
    except AttributeError:
        pass
    d["label"] = label
    d["coverage"] = {
        "grounds": ["report"],
        "slots": {"report": ["cpu", "mem", "gpu"]},
        "extras": ["audio.level"],
        "rings": list(VERBS),
    }
    return json.dumps(d)


_DESCRIPTOR = _load_descriptor()


def xy(pos):
    return pos % COLS, pos // COLS


def chebyshev(a, b):
    ax, ay = xy(a)
    bx, by = xy(b)
    return max(abs(ax - bx), abs(ay - by))


# ── state machine ──

IDLE, WAKE, WORK, ALERT = "idle", "wake", "work", "alert"
state = IDLE
t_state = time.monotonic()

metrics = [10, 10, 10]
latched = False
latch_center = None
latch_drop_t = 0.0
last_frame_t = time.monotonic()

# One indicator per reported metric. Positions change only between
# brightness cycles, after the previous pixel has faded out.
def timings_for(value):
    total = 2.0 + 6.0 * (1.0 - value / 100.0)   # 8 s tops, 2 s floor
    return total * 0.30, total * 0.20, total * 0.30, total * 0.20

indicators = []
for i in range(3):
    rise, stay, fall, wait = timings_for(10)
    indicators.append({"pos": 6 + i * 7, "value": 10, "pending": 10,
                  "phase": "quiet", "pt": 0.0,
                  "rise": rise, "stay": stay, "fall": fall, "wait": wait})


def retimings(indicator):
    total = 2.0 + 6.0 * (1.0 - indicator["value"] / 100.0)
    indicator["rise"], indicator["stay"] = total * 0.30, total * 0.20
    indicator["fall"], indicator["wait"] = total * 0.30, total * 0.20


def area_dark(x, y):
    """True when the pixel and its 1-px guard are all dark."""
    for dx in (-1, 0, 1):
        for dy in (-1, 0, 1):
            xx, yy = x + dx, y + dy
            if 0 <= xx < COLS and 0 <= yy < ROWS:
                r, g, b = pixels[yy * COLS + xx]
                if r or g or b:
                    return False
    return True


# Idle particles: [position, direction, brightness phase, move timer].
idle_particles = [[6, 1, 0.1, 0.0], [18, -1, 0.5, 0.4], [12, 1, 0.9, 0.8]]

# Event effects: [position, start time, color, urgency].
event_effects = []
DROP_LIFE = 1.1


def add(buf, pos, color):
    r, g, b = buf[pos]
    buf[pos] = (min(255, r + color[0]), min(255, g + color[1]),
                min(255, b + color[2]))


def add_event_effect(pos, color, urgency, force=False):
    if not force:                       # Avoid overlapping recent effects.
        for d in event_effects:
            if chebyshev(d[0], pos) < 2 and time.monotonic() - d[1] < DROP_LIFE:
                return
    event_effects.append([pos, time.monotonic(), color, urgency])
    if len(event_effects) > 4:
        event_effects.pop(0)


# ── signals and transitions ──

def set_state(s):
    global state, t_state
    state = s
    t_state = time.monotonic()
    enter = ENTER.get(s)
    if enter:
        enter()


def transition(to, why=""):
    print("[machine] {} -> {} {}".format(state, to,
                                         ("(" + why + ")") if why else ""))
    set_state(to)
    print("[machine] in " + to)


def on_signal(sig):
    if sig == "FRAME_G":
        if state == IDLE:   # only idle wakes; work takes the numbers
            transition(WAKE, "metrics received")
    elif sig == "WAKE_COMPLETE":
        if state == WAKE:
            transition(WORK, "wake transition complete")
    elif sig == "ALERT":
        if state != ALERT:
            transition(ALERT, "alert latched")
    elif sig == "ALLCLEAR":
        if state == ALERT:
            transition(WORK, "alert cleared")
    elif sig == "SILENCE":
        if state != IDLE and not latched:
            transition(IDLE, "host data timeout")


def machine_tick(t):
    global latch_drop_t
    if state == WAKE and t - t_state >= WAKE_TOTAL:
        on_signal("WAKE_COMPLETE")
    if (state != IDLE and not latched and
            t - last_frame_t > IDLE_AFTER):
        on_signal("SILENCE")
    if latched and t - latch_drop_t > 0.8:
        latch_drop_t = t
        add_event_effect(latch_center, HUES["alert"], 4, force=True)


# ── state enter/tick/render ──

def enter_idle():
    pass


def enter_wake():
    for a in indicators:
        a["t"] = 0.0                    # the rise restarts


def enter_work():
    pass


def enter_alert():
    pass


ENTER = {IDLE: enter_idle, WAKE: enter_wake, WORK: enter_work, ALERT: enter_alert}


def tick_idle(t, dt):
    for particle in idle_particles:
        particle[3] -= dt
        if particle[3] <= 0:
            particle[0] += particle[1]
            if particle[0] >= NUM or particle[0] < 0:
                particle[1] = -particle[1]
                particle[0] = max(0, min(NUM - 1, particle[0]))
            particle[3] = 1.2 + random.random() * 0.8   # a slow, lazy drift
    buf = [(0, 0, 0)] * NUM
    for particle in idle_particles:
        cyc = ((t / 2.6) + particle[2]) % 1.0
        if cyc < 0.35:                  # fade in
            k = (cyc / 0.35) * MAX_K
        elif cyc < 0.6:                 # gentle hold
            k = MAX_K
        elif cyc < 0.85:                # fade out - to black
            k = (1.0 - (cyc - 0.6) / 0.25) * MAX_K
        else:                           # a dark rest
            k = 0.0
        if k > 0:
            add(buf, particle[0], (int(70 * k), int(190 * k), int(50 * k)))
    return buf


def tick_wake(t, dt):
    f = min(1.0, (t - t_state) / WAKE_RISE_S)
    buf = [(0, 0, 0)] * NUM
    for i, a in enumerate(indicators):
        x, y = xy(a["pos"])
        k = f * BLINK_K                 # rise through the ceiling: the pop
        warm = (255, 150 + i * 20, 30)
        add(buf, a["pos"], tuple(int(v * k) for v in warm))
    return buf


def step_atom(a, dt):
    """One indicator's lifecycle: rise -> stay -> fall -> wait -> rise
    somewhere new. The position only ever changes in the dark wait, so
    a lit pixel never teleports; a value change waits for the dark too."""
    a["pt"] += dt

    if a["phase"] == "rise":
        k = min(1.0, a["pt"] / a["rise"]) * MAX_K
        if a["pt"] >= a["rise"]:
            a["phase"], a["pt"] = "stay", 0.0
        return k

    if a["phase"] == "stay":
        if a["value"] >= 80:            # past the threshold: blink at ~4 Hz
            k = (BLINK_K if int(a["pt"] * 8) % 2 == 0 else MAX_K * 0.45)
        else:
            k = MAX_K
        if a["pt"] >= a["stay"]:
            a["phase"], a["pt"] = "fall", 0.0
        return k

    if a["phase"] == "fall":
        k = max(0.0, 1.0 - a["pt"] / a["fall"]) * MAX_K
        if a["pt"] >= a["fall"]:
            a["phase"], a["pt"] = "wait", 0.0
        return k

    # the dark beat: the walk happens here - one step to a dark
    # adjacent pixel (wrapping), the pending value taken, nothing lit
    # ever jumps
    if "pending" in a:
        a["value"] = a.pop("pending")
        retimings(a)
    x, y = xy(a["pos"])
    cands = []
    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        nx, ny = (x + dx) % COLS, (y + dy) % ROWS
        if area_dark(nx, ny):
            cands.append((nx, ny))
    if cands:
        nx, ny = random.choice(cands)
        a["pos"] = ny * COLS + nx
    a["phase"], a["pt"] = "rise", 0.0
    return 0.0


def tick_work(t, dt):
    buf = [(0, 0, 0)] * NUM
    for a in indicators:
        k = step_atom(a, dt)
        if k > 0:
            x, y = xy(a["pos"])
            warm = (255, 150, 30)
            add(buf, a["pos"], (int(warm[0] * k), int(warm[1] * k),
                                int(warm[2] * k)))
    return buf


def enter_alert():
    pass


ENTER = {IDLE: enter_idle, WAKE: enter_wake, WORK: enter_work, ALERT: enter_alert}


def tick_alert(t, dt):
    """Render a persistent alert centered on the selected pixel."""
    global latch_drop_t
    buf = [(0, 0, 0)] * NUM
    base = 12
    for i in range(NUM):
        buf[i] = (base // 6, base // 3, base // 6)
    if t - latch_drop_t > 0.8:          # Repeat the alert effect.
        latch_drop_t = t
        add_event_effect(latch_center, HUES["alert"], 4, force=True)
    for d in event_effects:
        age = t - d[1]
        color = d[2]
        urgency = d[3]
        cx, cy = xy(d[0])
        radius = age * (3.0 + urgency)
        fade = max(0.0, 1.0 - age / DROP_LIFE)
        if fade <= 0:
            continue
        if age < 0.12:                  # the impact flash
            add(buf, d[0], tuple(min(255, v * 2) for v in color))
        for i in range(NUM):
            x, y = xy(i)
            dist = max(abs(x - cx), abs(y - cy))
            if abs(dist - radius) <= 0.75:
                k = fade * max(0.0, 1.0 - dist / 6.0)
                if k > 0:
                    add(buf, i, (int(color[0] * k), int(color[1] * k),
                                 int(color[2] * k)))
    return buf


TICKS = {IDLE: tick_idle, WAKE: tick_wake, WORK: tick_work, ALERT: tick_alert}


frame = bytearray(NUM * 3)         # the shot: flat rgb75, row-major


def render():
    global frame
    buf = TICKS[state](time.monotonic(), TICK)
    # Render event effects over every base state.
    for d in event_effects:
        age = time.monotonic() - d[1]
        if age > DROP_LIFE:
            continue
        cx, cy = xy(d[0])
        radius = age * (3.0 + d[3])
        fade = max(0.0, 1.0 - age / DROP_LIFE)
        if age < 0.12:
            add(buf, d[0], tuple(min(255, v * 2) for v in d[2]))
        for i in range(NUM):
            ix, iy = xy(i)
            dist = max(abs(ix - cx), abs(iy - cy))
            if abs(dist - radius) <= 0.75:
                k = fade * max(0.0, 1.0 - dist / 6.0)
                if k > 0:
                    add(buf, i, (int(d[2][0] * k), int(d[2][1] * k),
                                 int(d[2][2] * k)))
    for i in range(NUM):
        px_ = buf[i]
        frame[i * 3] = px_[0]
        frame[i * 3 + 1] = px_[1]
        frame[i * 3 + 2] = px_[2]
        pixels[i] = px_
    pixels.show()


# ── frames ──

def process(line):
    global latched, latch_center, latch_drop_t, label
    parts = line.split(",")
    c = parts[0].upper()
    a = parts[1:]

    if c == "I":
        r("OK," + _descriptor())
    elif c == "K":
        r("OK")
    elif c == "G" and a and a[0] == "report":
        vals = []
        for v in a[1:4]:
            vals.append(int(v) if v.isdigit() else 0)
        metrics[:] = vals
        for i, indicator in enumerate(indicators):
            # Apply new values between brightness cycles.
            indicator["pending"] = metrics[i]
        on_signal("FRAME_G")
        r("OK")
    elif c == "A" and len(a) >= 2 and a[0] == "audio.level":
        pulse = max(0, min(100, int(a[1])))
        pixels.brightness = 0.1 + (pulse / 100.0) * 0.5
        r("OK")
    elif c == "R":
        signal = a[0].lower() if a else "transition"
        urgency = int(a[1]) if len(a) > 1 and a[1].isdigit() else 2
        verb = signal.split(".")[0]
        color = HUES.get(verb, HUES["transition"])
        cx = random.randrange(COLS)
        cy = random.randrange(ROWS)
        add_event_effect(cy * COLS + cx, color, max(1, urgency))
        if verb.startswith("alert"):
            latched = True
            latch_center = cy * COLS + cx
            latch_drop_t = time.monotonic()
            on_signal("ALERT")
        if verb.startswith("allclear"):
            latched = False
            if latch_center is not None:
                cx, cy = xy(latch_center)
            on_signal("ALLCLEAR")
        r("OK," + (a[4] if len(a) > 4 else "0"))
    elif c == "X":
        latched = False
        latch_center = None
        on_signal("ALLCLEAR")
        r("OK")
    elif c == "S":
        words = ",".join(a)
        if words:
            label = ",".join(a) or label
            try:
                with open(LABEL_FILE, "w") as f:
                    f.write(label)
            except OSError:
                pass
        r("OK")
    elif c == "J":
        # Return the current RGB75 frame as base64 without rebooting.
        # CircuitPython dialect: binascii (no ubinascii alias here)
        import binascii
        payload = str(binascii.b2a_base64(frame)[:-1], "ascii")
        r("OK," + payload, checksum=True)
    else:
        r("ERR,unknown:" + c)


def r(msg, checksum=False):
    if checksum:
        x = 0
        for c in msg:
            x ^= ord(c)
        msg += "*%02x" % x
    print(msg)


def main():
    global last_frame_t
    r("suzu firefly matrix - rp2040-matrix, suzu/1")
    r("OK," + _descriptor())
    set_state(IDLE)

    buf = ""
    while True:
        t = time.monotonic()
        # Drain all available input; scalar updates alone run at
        # 5 Hz, and one char per tick (~15 B/s) drowns the RX ring in
        # seconds — the session dies of a host-side write timeout and
        # Continue processing display updates while other devices run.
        while supervisor.runtime.serial_bytes_available:
            ch = sys.stdin.read(1)
            if not ch:
                break
            if ch == "\r" or ch == "\n":
                # a lone `I`, `K` or `X` is a whole command: skip
                # only empty lines, never short ones
                if buf.strip():
                    process(buf.strip())
                    last_frame_t = time.monotonic()
                buf = ""
            else:
                buf += ch
        machine_tick(t)
        render()
        time.sleep(TICK)


main()
