#!/usr/bin/env python3
"""Host-side preview of a faceplate composition — no hardware touched.

Executes the faceplate's main.py against a fake ssd1306 framebuffer and
a scripted frame feed, then renders what the device would show (the
portrait view, after the u/v -> native rotation) to a PNG. The same
path validates the fallback numeral table by hiding digits_bebas.

Usage:
    python tools/preview_faceplate.py \
        faceplates/esp8266-oled/portrait-numerals [out.png] [--fallback]
"""

import io
import os
import sys
import types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)) + "/fonts")

from PIL import Image  # noqa: E402


def build_env(frame_lines, native):
    """A MicroPython-shaped world around the faceplate code."""
    t = {"ms": 0}

    time = types.ModuleType("time")
    time.ticks_ms = lambda: t["ms"]
    time.ticks_add = lambda base, delta: base + delta
    time.ticks_diff = lambda a, b: a - b
    time.sleep_ms = lambda ms: t.__setitem__("ms", t["ms"] + ms)

    class Timer:
        PERIODIC = 1

        def __init__(self, *a):
            pass

        def init(self, **kw):
            self.cb = kw.get("callback")

    machine = types.ModuleType("machine")
    machine.Timer = Timer
    machine.unique_id = lambda: b"\x01\x02\x03\x04\x05\x06"
    machine.Pin = lambda *a, **k: None
    machine.SoftI2C = lambda *a, **k: None

    class UART:
        def __init__(self, *a):
            self.sent = []

        def write(self, b):
            self.sent.append(b)
            return len(b)

    machine.UART = UART

    ujson = types.ModuleType("ujson")
    ujson.loads = __import__("json").loads
    ujson.dumps = lambda o, **k: __import__("json").dumps(o, separators=(",", ":"))

    ubinascii = types.ModuleType("ubinascii")
    ubinascii.hexlify = lambda b: b.hex().encode()

    select = types.ModuleType("select")
    select.POLLIN = 1

    class Poll:
        def __init__(self, lines):
            self.lines = lines
            self.total = len(lines)
            self.n = 0

        def register(self, *a):
            pass

        def poll(self, ms):
            self.n += 1
            if self.n <= self.total:
                return [(None, 1)]
            raise KeyboardInterrupt()

    select.poll = lambda: Poll(frame_lines)

    class FireflyOLED:
        def __init__(self):
            self.oled = native

    fw = types.ModuleType("firefly_oled_v2")
    fw.FireflyOLED = FireflyOLED

    ssd = types.ModuleType("ssd1306")
    ssd.SSD1306_I2C = lambda w, h, i2c: native

    stdin = types.SimpleNamespace(
        readline=lambda: frame_lines.pop(0) + "\n"
        if frame_lines
        else ""
    )

    return time, machine, ujson, ubinascii, select, ssd, stdin


class FakeOLED:
    """Just the ssd1306 surface the faceplate uses, on a 128x64 buffer."""

    W, H = 128, 64

    def __init__(self):
        self.buf = bytearray(self.W * self.H // 8)

    def _set(self, x, y, on):
        if 0 <= x < self.W and 0 <= y < self.H:
            i = x + (y // 8) * self.W
            if on:
                self.buf[i] |= 1 << (y % 8)
            else:
                self.buf[i] &= ~(1 << (y % 8))

    def pixel(self, x, y, on=1):
        self._set(x, y, on)

    def fill(self, on):
        for i in range(len(self.buf)):
            self.buf[i] = 0xFF if on else 0

    def fill_rect(self, x, y, w, h, on):
        for yy in range(y, y + h):
            row = yy
            for xx in range(x, x + w):
                self._set(xx, row, on)

    def show(self):
        pass

    def contrast(self, v):
        pass

    def to_portrait_image(self):
        """The view after rotating the panel 90deg CW (64 wide, 128 tall)."""
        img = Image.new("1", (64, 128), 0)
        for u in range(64):
            for v in range(128):
                x, y = v, 63 - u
                if self.buf[x + (y // 8) * self.W] & (1 << (y % 8)):
                    img.putpixel((u, v), 1)
        return img


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    fallback = "--fallback" in sys.argv
    plate_dir = os.path.abspath(args[0])
    out_png = args[1] if len(args) > 1 else os.path.join(plate_dir, "preview.png")

    native = FakeOLED()
    frames = [
        'J,{"name":"leaded-sparkle"}',
        "G,report,42,61,255",
        "A,audio.level,80",
        "A,audio.level,30",
        "R,disk,1,0,1,1,3,Disk at 50%",
    ]
    time_m, machine, ujson, ubinascii, select, ssd, stdin = build_env(frames, native)

    src = open(os.path.join(plate_dir, "face.py"), encoding="utf-8").read()

    glo = {
        "__name__": "__main__",
        "sys": types.SimpleNamespace(stdin=stdin),
        "time": time_m,
        "machine": machine,
        "ujson": ujson,
        "ubinascii": ubinascii,
        "select": select,
        "gc": __import__("gc"),
    }
    code = compile(src, "main.py", "exec")

    # The face reads its font bin from the device root; redirect that
    # absolute path to the package dir so the sim draws the real art.
    real_open = open

    def plate_open(path, *a, **kw):
        if isinstance(path, str) and path.startswith("/"):
            return real_open(os.path.join(plate_dir, path[1:]), *a, **kw)
        return real_open(path, *a, **kw)

    import builtins

    builtins.open = plate_open

    # Swap the stubs in for the run; restore the host's own modules after.
    # `sys` too — the faceplate's `import sys` resolves through
    # sys.modules, which would otherwise hand it the host's real stdin.
    real = {k: sys.modules.get(k) for k in ("sys", "time", "select", "machine")}
    sys.modules["sys"] = types.ModuleType("sys")
    sys.modules["sys"].stdin = stdin
    sys.modules["time"] = time_m
    sys.modules["select"] = select
    sys.modules["machine"] = machine
    sys.modules["ujson"] = ujson
    sys.modules["ubinascii"] = ubinascii
    sys.modules["ssd1306"] = ssd
    if fallback:
        # `None in sys.modules` -> ImportError, exactly what a device
        # without the font module would raise.
        sys.modules["digits_bebas"] = None
    else:
        sys.path.insert(0, plate_dir)

    try:
        exec(code, glo)  # main() runs until the feed is exhausted
    except KeyboardInterrupt:
        pass
    finally:
        builtins.open = real_open
        for k, v in real.items():
            if v is not None:
                sys.modules[k] = v

    uart = glo["u"]
    print("device said:")
    for b in uart.sent:
        if isinstance(b, bytes):
            b = b.decode(errors="replace")
        print("   ", b.rstrip())

    img = native.to_portrait_image()
    img = img.resize((192, 384), Image.NEAREST)
    img.save(out_png)
    print("portrait preview:", out_png, "(fallback)" if fallback else "")


if __name__ == "__main__":
    main()
