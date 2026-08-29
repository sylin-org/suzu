# suzu firefly matrix - rp2040-matrix, suzu/1
# The lake. The machine's numbers are fireflies that breathe with its
# load; moments land as raindrops - a bright impact, then a colored
# ring expanding and fading in the category's hue. Light adds: ripples
# and fireflies compose without rules. The face contract lives in
# docs/the-face-contract.md.
#
# Frames (suzu/1 suzu-t, newline-terminated):
#   I                -> OK,{descriptor}
#   K                -> OK
#   G,report,c,m,g   -> ground: three atom fireflies breathe with the values
#   A,audio.level,v  -> the pulse: the lake's brightness follows
#   R,sig,urg,0,1,seq[,words] -> a raindrop: hue by verb, speed by urgency;
#                      alert latches - the lake keeps ringing at its spot
#   X                -> allclear lands at the wound; the ground resumes
#   S,words          -> set the label (persisted; reserved in the contract)

import board
import json
import neopixel
import random
import supervisor
import sys
import time
import microcontroller

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


# ── the lake state ──
mode = "idle"                  # idle | work
ground = [10, 10, 10]          # the machine's numbers, as the fireflies breathe
latched = False
latch_center = None
last_rx = None
boot_t = time.monotonic()

# ── atom fireflies: the report's slots, one pixel each, breathing with
# the values. value drives the breathing period; >=80 blinks at the top;
# after each cycle the firefly pops to a new spot (guard-spaced). ──
atoms = []
for i in range(3):
    atoms.append({
        "pos": 6 + i * 7,               # spread around the lake
        "value": 10,
        "period": 6.0,
        "t": i * 1.7,                   # staggered phase
    })


def atom_period(v):
    return 6.0 - 5.0 * (v / 100.0) ** 2   # 10 pct -> ~6 s, 100 pct -> 1 s


# ── raindrops: [pos, born, (r, g, b), urgency] - impact flash, ring ──
drops = []
DROP_LIFE = 1.1


def drop_at(pos, color, urgency):
    drops.append([pos, time.monotonic(), color, urgency])
    if len(drops) > 4:
        drops.pop(0)


# ── fireflies (idle): three wanderers, the poc's garden ──
flies = [[6, 1, 20], [18, -1, 90], [12, 1, 160]]


def add(buf, pos, color):
    r, g, b = buf[pos]
    buf[pos] = (min(255, r + color[0]), min(255, g + color[1]),
                min(255, b + color[2]))


def render(t, dt):
    buf = [(0, 0, 0)] * NUM

    # the ground: atom fireflies breathe with the machine's numbers
    if mode == "work":
        for a in atoms:
            a["t"] += dt
            if a["t"] >= a["period"]:
                a["pos"] = random.randrange(NUM)
                for other in atoms:
                    if other is not a and chebyshev(a["pos"], other["pos"]) < 2:
                        a["pos"] = random.randrange(NUM)
                a["t"] = 0.0
            f = (a["t"] / a["period"]) % 1.0
            if f < 0.30:                     # ramp up
                k = f / 0.30
            elif f < 0.45:                   # stay
                if a["value"] >= 80:         # past the threshold: blink
                    k = 1.0 if int(t * 8) % 2 == 0 else 0.55
                else:
                    k = 1.0
            elif f < 0.75:                   # fade out
                k = 1.0 - (f - 0.45) / 0.30
            else:                            # quiet
                k = 0.0
            if k > 0:
                x, y = xy(a["pos"])
                warm = (255, 150, 30)
                add(buf, a["pos"], (int(warm[0] * k),
                                    int(warm[1] * k),
                                    int(warm[2] * k)))

    # idle: the wandering fireflies
    if mode == "idle":
        for fly in flies:
            fly[0] += fly[1]
            if fly[0] >= NUM or fly[0] < 0:
                fly[1] = -fly[1]
                fly[0] = max(0, min(NUM - 1, fly[0]))
            glow = 100 + int(100 * abs((t % 2.0) - 1))
            add(buf, fly[0], (glow // 3, glow, fly[2] // 4))

    # raindrops: impact flash + expanding, fading rings (light adds)
    for d in drops:
        age = t - d[1]
        color = d[2]
        urgency = d[3]
        cx, cy = xy(d[0])
        radius = age * (3.0 + urgency)
        fade = max(0.0, 1.0 - age / DROP_LIFE)
        if fade <= 0:
            continue
        if age < 0.12:                       # the impact flash
            add(buf, d[0], tuple(min(255, v * 2) for v in color))
        for i in range(NUM):
            x, y = xy(i)
            dist = max(abs(x - cx), abs(y - cy))
            if abs(dist - radius) <= 0.75:
                k = fade * max(0.0, 1.0 - dist / 6.0)
                add(buf, i, (int(color[0] * k), int(color[1] * k),
                             int(color[2] * k)))

    for i in range(NUM):
        pixels[i] = buf[i]
    pixels.show()


def r(msg):
    print(msg)


def process(line):
    global mode, ground, label, latched, latch_center
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
        ground = vals
        for i, atom in enumerate(atoms):
            atom["value"] = ground[i]
            atom["period"] = 6.0 - 5.0 * (atom["value"] / 100.0) ** 2
        mode = "work"
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
        if verb.startswith("alert"):
            latched = True
            latch_center = (cx, cy)
        if verb.startswith("allclear"):
            latched = False
            if latch_center is not None:
                cx, cy = latch_center       # the heal lands at the wound
        drops.append([cy * COLS + cx, time.monotonic(), color, max(1, urgency)])
        if len(drops) > 4:
            drops.pop(0)
        r("OK," + (a[4] if len(a) > 4 else "0"))
    elif c == "X":
        latched = False
        r("OK")
    elif c == "S":
        label = ",".join(a) or label
        try:
            with open(LABEL_FILE, "w") as f:
                f.write(label)
        except OSError:
            pass
        r("OK")
    else:
        r("ERR,unknown:" + c)


def main():
    global last_rx, mode
    r("suzu firefly matrix - rp2040-matrix, suzu/1")
    r("OK," + _descriptor())
    mode = "idle"

    buf = ""
    while True:
        t = time.monotonic()
        if supervisor.runtime.serial_bytes_available:
            last_rx = t
            ch = sys.stdin.read(1)
            if ch:
                if ch == "\r" or ch == "\n":
                    if len(buf) > 1:
                        process(buf.strip())
                    buf = ""
                else:
                    buf += ch
        else:
            if (last_rx is not None and t - last_rx > IDLE_AFTER) or (
                last_rx is None and t - boot_t > IDLE_AFTER
            ):
                if not latched:         # an alert never idles away
                    mode = "idle"

        render(t, TICK)
        time.sleep(TICK)


boot_t = time.monotonic()
main()
