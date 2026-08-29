# suzu firefly matrix - rp2040-matrix, suzu/1
# The lake, as a proper machine state.
#
# States:
#   IDLE  the garden: fireflies drift slowly, glow, fade to black
#   WAKE  the garden gathers: the atoms rise, a brief pop
#   WORK  the ground: three atom fireflies breathe with the machine's
#         numbers (value drives the breathing period; past 80 they
#         blink at the peak, brighter than the ceiling)
#   RING  an alert has latched: the lake keeps ringing at its spot
#
# Signals (what the machine hears):
#   FRAME_G, FRAME_R, FRAME_ALLCLEAR, FRAME_X, FRAME_K,
#   TICK_POP (the wake's rise finished), SILENCE (10 s quiet),
#   LATCH (re-drop the latched alert's rain)
#
# Transitions:
#   IDLE  + FRAME_G     -> WAKE   (the house has numbers to show)
#   WAKE  + TICK_POP    -> WORK   (the pop)
#   WORK  + R(alert)    -> RING   (an alert latches)
#   RING  + allclear/X  -> WORK   (the heal lands at the wound)
#   WORK  + SILENCE     -> IDLE   (the house went quiet)
#   RING  + SILENCE     -> RING   (an alert never idles away)
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
pixels = neopixel.NeoPixel(board.GP16, NUM, brightness=0.3, auto_write=False)

_VERSION = "1.0.0"
TICK = 0.05
IDLE_AFTER = 10.0
LABEL_FILE = "/label.txt"
VERBS = ("alert", "allclear", "completion", "discovery", "begin",
         "departure", "tended", "transition", "heartbeat")

# the say vocabulary: the nine verbs, each with its hue
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

MAX_K = 0.5                  # the gentle ceiling: half brightness
BLINK_K = 1.0                # the threshold blink may exceed it
RISE_S = 0.9                 # the wake's rise before the pop
POP_S = 0.12                 # the pop holds this long
WAKE_TOTAL = RISE_S + POP_S
DROP_LIFE = 1.1

# the label: persisted, restored at boot, never reverted (the contract)
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


# ── the machine ──

IDLE, WAKE, WORK, RING = "idle", "wake", "work", "ring"
state = IDLE
t_state = time.monotonic()

ground = [10, 10, 10]
latched = False
latch_center = None
latch_drop_t = 0.0
last_frame_t = time.monotonic()

# atom fireflies: the report's slots, one pixel each. value drives the
# breathing period (a full cycle is 8 s tops); past 80 they blink at
# the peak - brighter than the ceiling. after each cycle the firefly
# pops to a new guard-spaced spot.
atoms = []
for i in range(3):
    atoms.append({
        "pos": 6 + i * 7,
        "value": 10,
        "period": 8.0,
        "t": i * 2.1,
    })

# idle fireflies: [pos, drift, glow phase, move timer]
flies = [[6, 1, 0.1, 0.0], [18, -1, 0.5, 0.4], [12, 1, 0.9, 0.8]]

# raindrops: [pos, born, (r, g, b)] - impact flash, expanding ring
drops = []
DROP_LIFE = 1.1


def add(buf, pos, color):
    r, g, b = buf[pos]
    buf[pos] = (min(255, r + color[0]), min(255, g + color[1]),
                min(255, b + color[2]))


def drop_at(pos, color, urgency, force=False):
    if not force:                       # not too close to a live drop
        for d in drops:
            if chebyshev(d[0], pos) < 2 and time.monotonic() - d[1] < DROP_LIFE:
                return
    drops.append([pos, time.monotonic(), color, urgency])
    if len(drops) > 4:
        drops.pop(0)


# ── the machine: signals and transitions ──

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
        if state != WAKE:
            transition(WAKE, "the house has numbers to show")
    elif sig == "TICK_POP":
        if state == WAKE:
            transition(WORK, "the pop")
    elif sig == "ALERT":
        if state != RING:
            transition(RING, "an alert latches")
    elif sig == "ALLCLEAR":
        if state == RING:
            transition(WORK, "the heal lands at the wound")
    elif sig == "SILENCE":
        if state != IDLE and not latched:
            transition(IDLE, "the house went quiet")


def machine_tick(t):
    if state == WAKE and t - t_state >= WAKE_TOTAL:
        on_signal("TICK_POP")
    if (state != IDLE and not latched and
            t - last_frame_t > IDLE_AFTER):
        on_signal("SILENCE")
    if latched and t - latch_drop_t > 0.8:
        latch_drop_t = t
        drop_at(latch_center, HUES["alert"], 4, force=True)


# ── state enter/tick/render ──

def enter_idle():
    pass                                # the garden needs no preparation


def enter_wake():
    for a in atoms:
        a["t"] = 0.0                    # the rise restarts


def enter_work():
    pass                                # the atoms are already breathing


def enter_ring():
    pass                                # the rain is already falling


ENTER = {IDLE: enter_idle, WAKE: enter_wake, WORK: enter_work, RING: enter_ring}


def tick_idle(t, dt):
    for fly in flies:
        fly[3] -= dt
        if fly[3] <= 0:
            fly[0] += fly[1]
            if fly[0] >= NUM or fly[0] < 0:
                fly[1] = -fly[1]
                fly[0] = max(0, min(NUM - 1, fly[0]))
            fly[3] = 1.2 + random.random() * 0.8   # a slow, lazy drift
    buf = [(0, 0, 0)] * NUM
    for fly in flies:
        cyc = ((t / 2.6) + fly[2]) % 1.0
        if cyc < 0.35:                  # fade in
            k = (cyc / 0.35) * MAX_K
        elif cyc < 0.6:                 # gentle hold
            k = MAX_K
        elif cyc < 0.85:                # fade out - to black
            k = (1.0 - (cyc - 0.6) / 0.25) * MAX_K
        else:                           # a dark rest
            k = 0.0
        if k > 0:
            add(buf, fly[0], (int(70 * k), int(190 * k), int(50 * k)))
    return buf


def tick_wake(t, dt):
    f = min(1.0, (t - t_state) / RISE_S)
    buf = [(0, 0, 0)] * NUM
    for i, a in enumerate(atoms):
        x, y = xy(a["pos"])
        k = f * BLINK_K                 # rise through the ceiling: the pop
        warm = (255, 150 + i * 20, 30)
        add(buf, a["pos"], tuple(int(v * k) for v in warm))
    return buf


def tick_work(t, dt):
    buf = [(0, 0, 0)] * NUM
    for a in atoms:
        a["t"] += dt
        if a["t"] >= a["period"]:
            a["pos"] = random.randrange(NUM)
            for other in atoms:
                if other is not a and chebyshev(a["pos"], other["pos"]) < 2:
                    a["pos"] = random.randrange(NUM)
            a["t"] = 0.0
        f = (a["t"] / a["period"]) % 1.0
        hot = a["value"] >= 80
        if f < 0.30:                    # ramp up to the gentle ceiling
            k = (f / 0.30) * MAX_K
        elif f < 0.50:                  # stay
            if hot:                     # past the threshold: blink,
                k = (BLINK_K if int(t * 8) % 2 == 0  # allowed brighter
                     else MAX_K * 0.45)
            else:
                k = MAX_K
        elif f < 0.80:                  # fade out - all the way to black
            k = (1.0 - (f - 0.50) / 0.30) * MAX_K
        else:                           # the quiet beat: truly dark
            k = 0.0
        if k > 0:
            x, y = xy(a["pos"])
            warm = (255, 150, 30)
            add(buf, a["pos"], (int(warm[0] * k), int(warm[1] * k),
                                int(warm[2] * k)))
    return buf


def enter_ring():
    pass


ENTER = {IDLE: enter_idle, WAKE: enter_wake, WORK: enter_work, RING: enter_ring}


def tick_ring(t, dt):
    """A latched alert: the lake dims and keeps ringing at the wound."""
    buf = [(0, 0, 0)] * NUM
    base = 12
    for i in range(NUM):
        buf[i] = (base // 6, base // 3, base // 6)
    if t - latch_drop_t > 0.8:          # the lake keeps ringing there
        latch_drop_t = t
        drop_at(latch_center, HUES["alert"], 4, force=True)
    for d in drops:
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


TICKS = {IDLE: tick_idle, WAKE: tick_wake, WORK: tick_work, RING: tick_ring}


def render():
    buf = TICKS[state](time.monotonic(), TICK)
    # the raindrop layer lands in every state: moments reach the face
    # wherever it is
    for d in drops:
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
        pixels[i] = buf[i]
    pixels.show()


# ── frames ──

def process(line):
    global latched, latch_center, latch_drop_t
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
        ground[:] = vals
        for i, atom in enumerate(atoms):
            atom["value"] = ground[i]
            atom["period"] = 2.0 + 6.0 * (1.0 - atom["value"] / 100.0)
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
        drop_at(cy * COLS + cx, color, max(1, urgency))
        if verb.startswith("alert"):
            latched = True
            latch_center = cy * COLS + cx
            latch_drop_t = time.monotonic()
        if verb.startswith("allclear"):
            latched = False
            if latch_center is not None:
                cx, cy = xy(latch_center)   # the heal lands at the wound
        r("OK," + (a[4] if len(a) > 4 else "0"))
    elif c == "X":
        latched = False
        latch_center = None
        r("OK")
    elif c == "S":
        words = ",".join(a)
        if words:
            label = words
            try:
                with open(LABEL_FILE, "w") as f:
                    f.write(label)
            except OSError:
                pass
        r("OK")
    else:
        r("ERR,unknown:" + c)


def r(msg):
    print(msg)


def main():
    r("suzu firefly matrix - rp2040-matrix, suzu/1")
    r("OK," + _descriptor())
    set_state(IDLE)

    buf = ""
    last = time.monotonic()
    while True:
        t = time.monotonic()
        if supervisor.runtime.serial_bytes_available:
            ch = sys.stdin.read(1)
            if ch:
                if ch == "\r" or ch == "\n":
                    if len(buf) > 1:
                        process(buf.strip())
                        last_frame_t = time.monotonic()
                    buf = ""
                else:
                    buf += ch
        machine_tick(t)
        render()
        time.sleep(TICK)


main()
