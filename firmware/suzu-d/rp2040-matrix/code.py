# suzu firefly matrix - rp2040-matrix, suzu/1
#
# The light-sentence face: 25 RGB pixels, per-pixel color and intensity.
# Hue carries valence, intensity carries urgency, tempo carries time -
# the gloss (5-4 breath, 3 pulse, 2 blink, 1 strobe, 0 dark) rendered
# literally. Text lives on other faces; this one tells with light.
#
# Frames (suzu/1 suzu-t, newline-terminated):
#   I                     -> OK,{descriptor}*hh
#   K                     -> OK (keepalive; also feeds the idle clock)
#   G,report,<cpu>,<mem>,<gpu> -> ground: the breath, hue by health fold
#   A,audio.level,<0-100> -> the pulse lane: brightness follows the level
#   R,<signal>,<urgency>,0,1,<seq>[,<words>] -> a ring: hue by valence,
#                            tempo by urgency; alert latches, the rest
#                            render ~5 s and the ground resumes
#   X                     -> restore ground after a ring
# Unknown frames -> ERR. Silence -> the fireflies come out (idle).
# Boot opens with the fireflies too: the garden before the house.

import board
import json
import neopixel
import os
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
    d["coverage"] = {
        "grounds": ["report"],
        "slots": {"report": ["cpu", "mem", "gpu"]},
        "extras": ["audio.level"],
    }
    return json.dumps(d)


_DESCRIPTOR = _load_descriptor()

LABEL_FILE = "/label.txt"
label = "suzu"
try:
    with open(LABEL_FILE) as f:
        label = f.read().strip() or label
except OSError:
    pass

mode = "idle"
ring_hue = (200, 10, 0)
ring_latch = False
ring_until = 0.0
ground = (50, 50, 50)
last_rx = None
boot_t = time.monotonic()
ff_t0 = boot_t

flies = [[6, 1, 20], [18, -1, 90], [12, 1, 160]]

TICK = 0.1
IDLE_AFTER = 10.0


def r(msg):
    print(msg)


def breath_color():
    worst = max(ground)
    if worst < 60:
        return (0, 120, 30)
    if worst < 85:
        return (160, 110, 0)
    return (180, 20, 0)


def ring_hue_for(signal):
    s = signal.lower()
    if s.startswith("alert"):
        return (200, 10, 0)
    if s.startswith("allclear"):
        return (0, 180, 40)
    if s.startswith("completion"):
        return (0, 120, 200)
    if s.startswith("discovery"):
        return (120, 60, 200)
    return (200, 140, 0)


def render_breath(t):
    hue = breath_color()
    phase = (t % 4.0) / 4.0
    k = 0.35 + 0.4 * abs(phase * 2 - 1)
    c = tuple(int(v * k) for v in hue)
    pixels.fill(c)
    pixels.show()


def render_ring(t):
    left = ring_until - t
    on = True
    if ring_latch:
        on = int(t * 4) % 2 == 0
    elif int(t * 5) % 2 == 0:
        on = left % 0.4 < 0.25
    if on:
        k = max(0.15, min(1.0, 0.15 + 4 * 0.17))
        pixels.fill(tuple(int(v * k) for v in ring_hue))
    else:
        pixels.fill((0, 0, 0))
    pixels.show()


def render_idle(t):
    px_new = [None] * NUM
    for fly in flies:
        fly[0] += fly[1]
        if fly[0] >= NUM or fly[0] < 0:
            fly[1] = -fly[1]
            fly[0] = max(0, min(NUM - 1, fly[0]))
        age = (t * 40 + fly[2]) % 255
        g = 120 + int(90 * abs((t % 2) - 1))
        px_new[fly[0]] = (g // 3, g, age // 3)
    for i in range(NUM):
        r, g, b = pixels[i]
        pixels[i] = (max(0, r - 30), max(0, g - 30), max(0, b - 30))
    for i in range(NUM):
        if px_new[i] is not None:
            pixels[i] = px_new[i]
    pixels.show()


def with_checksum(frame):
    x = 0
    for b in frame.encode():
        x ^= b
    return "{0}*{1:02x}".format(frame, x)


def process(line):
    global mode, ground, ring_hue, ring_latch, ring_until, last_rx, label
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
        mode = "breath"
        r("OK")
    elif c == "A" and len(a) >= 2 and a[0] == "audio.level":
        pulse = max(0, min(100, int(a[1])))
        pixels.brightness = 0.1 + (pulse / 100.0) * 0.5
        r("OK")
    elif c == "R":
        signal = a[0].lower() if a else "transition"
        urgency = int(a[1]) if len(a) > 1 and a[1].isdigit() else 3
        seq_field = a[4] if len(a) > 4 else "0"
        words = " ".join(a[5:])[:30]
        ring_hue = ring_hue_for(signal)
        ring_latch = signal.startswith("alert")
        ring_until = time.monotonic() + (3600.0 if ring_latch else 5.0)
        mode = "ring"
        r("OK," + seq_field)
    elif c == "X":
        ring_latch = False
        mode = "breath"
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
    pixels.fill((0, 60, 20))
    pixels.show()
    time.sleep(0.4)

    while True:
        t = time.monotonic()
        if supervisor.runtime.serial_bytes_available:
            last_rx = t
            line = ""
            while True:
                ch = sys.stdin.read(1)
                if ch == "\r" or ch == "\n":
                    break
                line += ch
            if line.strip():
                process(line.strip())
        else:
            if (last_rx is not None and t - last_rx > IDLE_AFTER) or (
                last_rx is None and t - boot_t > IDLE_AFTER
            ):
                mode = "idle"

        if mode == "idle":
            render_idle(t)
        elif mode == "ring":
            if now() < ring_until or ring_latch:
                render_ring(t)
            else:
                mode = "breath"
        elif mode == "breath":
            render_breath(t)
        time.sleep(TICK)


boot_t = time.monotonic()
main()
