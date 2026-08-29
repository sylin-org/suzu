#!/usr/bin/env python3
"""Screenshot a suzu face — a copy of the screen while it lives.

The capture rides the suzu/1 contract itself: `J,{\"shot\":1}` (the
complex-value escape's snapshot form) makes the face write its own
frame buffer to /shot.tmp WITHOUT stopping — pulse and all. The tool
then lifts the file with the install path's sliced read-back, renders
both orientations host-side, and finally interrupts + reboots only to
clean the scratch file away.

    python tools/screenshot.py COM12 out.png
"""

import os
import re
import sys
import time

import serial

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "scripts"))
import importlib.util

_spec = importlib.util.spec_from_file_location(
    "pf", os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "..", "scripts", "push_firmware.py")
)
pf = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(pf)

OUT_W, OUT_H = 128, 64


def grab(port):
    # 0. Known start: the recovery line boots (or re-boots) the face —
    #    whatever state the board was left in, main.py runs again. The
    #    face needs dressing afterwards (name, ground, pulse), which is
    #    what step 1 seeds; a face in service gets its truth from the
    #    Resident, and serve holds the port anyway.
    p = serial.Serial(port, 115200, timeout=0.3)
    time.sleep(2.5)                          # boot wait if just plugged
    p.write(b"\r\x03\x03")
    time.sleep(0.7)
    p.write(b"\x02\x04")                     # friendly, then soft reboot
    time.sleep(3.0)
    p.reset_input_buffer()

    # 1. Dress the face exactly as the Resident would, then ask for the
    #    shot. Replies must arrive as whole lines AFTER our send (the
    #    boot hello also says OK — never mistake it for the ack).
    def send_line(s):
        data = s.encode()
        for i in range(0, len(data), 16):    # dribble — no RX overrun
            p.write(data[i : i + 16])
            time.sleep(0.004)
        p.write(b"\n")

    def ask(frame):
        send_line("\r")                      # face ignores empty lines
        time.sleep(0.2)
        send_line(frame)
        deadline = time.time() + 5
        line = b""
        while time.time() < deadline:
            if p.in_waiting:
                line += p.read(p.in_waiting)
                while b"\n" in line:
                    reply, line = line.split(b"\n", 1)
                    reply = reply.strip()
                    if reply.startswith((b"OK", b"ERR")):
                        if reply.startswith(b"ERR"):
                            print("face said:", reply.decode(errors="replace"))
                        return reply.startswith(b"OK")
            else:
                time.sleep(0.05)
        return False

    for frame in (
        'J,{"name":"stone-leaded-sparkle"}',
        "G,report,42,61,255",
        "A,audio.level,80",
        "A,audio.level,35",
        'J,{"shot":1}',
    ):
        if not ask(frame):
            p.close()
            raise SystemExit("face did not answer: %s" % frame)
    p.close()

    # 2. Lift /shot.tmp through the proven sliced read-back.
    r = pf.Repl(port)
    if "shot.tmp" not in r.list_files():
        raise SystemExit("face acked but wrote no /shot.tmp")
    frame = r.read_file("shot.tmp")
    assert len(frame) == OUT_W * OUT_H // 8, "short frame: %d" % len(frame)

    # 3. Clean the scratch file, then bring the face straight back up.
    r.exec("import os; os.remove('/shot.tmp')")
    r.soft_reboot()
    return frame


def render(frame, out_png):
    from PIL import Image

    native = Image.new("1", (OUT_W, OUT_H), 0)
    for page in range(OUT_H // 8):
        for col in range(OUT_W):
            bits = frame[page * OUT_W + col]
            for b in range(8):
                if bits & (1 << b):
                    native.putpixel((col, page * 8 + b), 1)

    base, _ = os.path.splitext(out_png)
    native.resize((512, 256), Image.NEAREST).save(base + "-native.png")

    # The panel stands on its long edge: portrait(u,v) -> native(v, 63-u).
    portrait = Image.new("1", (64, 128), 0)
    for u in range(64):
        for v in range(128):
            if native.getpixel((v, 63 - u)):
                portrait.putpixel((u, v), 1)
    portrait.resize((192, 384), Image.NEAREST).save(out_png)
    print("wrote %s (+%s)" % (out_png, base + "-native.png"))


def main():
    port = sys.argv[1]
    out_png = sys.argv[2] if len(sys.argv) > 2 else "screenshot.png"
    render(grab(port), out_png)


if __name__ == "__main__":
    main()
