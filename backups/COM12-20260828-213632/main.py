"""
Firefly OLED v2 — Dense Icon Dashboard

Serial protocol (115200 baud, newline-terminated):
  I                              → device info
  C                              → clear display
  S,<name>                       → set stone name
  H,<health>                     → set health (thriving/withering/wilting)
  M,<cpu>,<mem>,<disk>,<uptime>  → update resource metrics
  G,<offerings>,<stones>,<net_bps>,<seed_bank> → update garden context
  D,<cpu>,<mem>,<disk>,<uptime>,<offerings>,<stones>,<net_bps>,<seed_bank>
                                 → all-in-one dashboard update
  WIPE-IN,<line1>,<line2>        → wipe-in animation (event interrupt)
  WIPE-OUT,<line1>,<line2>       → wipe-out animation (event interrupt)
  R                              → force redraw

State machine:
  S_NC   → no serial communication (firefly float animation)
  S_CONN → first data received (firefly dash → fade → idle)
  S_IDLE → normal dashboard rendering
"""

import gc, sys, time, select
from machine import Timer, UART

W = 128
H = 64
Y = 16
BH = 48

# States
S_BOOT = 0
S_NC = 2
S_CONN = 3
S_IDLE = 4

# Timeouts
TO = 10000    # serial comm timeout (ms)
TICK = 100    # animation tick (ms)

# Firefly particle fields
FX = 0; FP = 1; FS = 2; FA = 3; FB = 4; FD = 5; FDN = 6

# Sine LUT (×100) for float animation
SIN = (0, 38, 70, 92, 100, 92, 70, 38, 0, -38, -70, -92, -100, -92, -70, -38)

u = UART(0, 115200)
fnt = None
d = None
tm = None
needs = False
state = S_BOOT
last_rx = None

# Firefly particles (reused from v1 for boot/no-comm animations)
ff = []
ff_t0 = 0
ff_last = 0
dash = 1
ht = 0
dash_init = False


def r(msg):
    """Send response to host."""
    u.write(msg + "\n")
    time.sleep_ms(2)


def tcb(t):
    """Timer callback — request dashboard redraw."""
    global needs
    needs = True


def init_display():
    """Initialize OLED and timer."""
    global d, tm, fnt
    try:
        gc.collect()
        from firefly_oled_v2 import FireflyOLED
        d = FireflyOLED()
        fnt = __import__('profont_10')
        tm = Timer(-1)
        tm.init(period=200, mode=Timer.PERIODIC, callback=tcb)
        return True
    except Exception as e:
        r("ERR,display_init:%s" % e)
        return False


def tw(s):
    """Text width helper."""
    if fnt:
        return fnt.text_width(s)
    return len(s) * 8


def msg(title, line1, line2=None):
    """Full-screen message (boot/ready screens)."""
    d.oled.contrast(255)
    d.oled.fill_rect(0, 0, W, Y, 0)
    t = title.upper()
    w = tw(t)
    x = 2 if w >= W else (W - w) // 2
    if fnt:
        fnt.draw(d.oled, t, x, 3)
    else:
        d.oled.text(t, x, 3)
    d.oled.fill_rect(0, Y, W, BH, 0)
    if line2 is None:
        lines, ys = [line1], [32]
    else:
        lines, ys = [line1, line2], [24, 40]
    for i in range(len(lines)):
        s = lines[i]
        w = tw(s)
        x = 2 if w >= W else (W - w) // 2
        if fnt:
            fnt.draw(d.oled, s, x, ys[i])
        else:
            d.oled.text(s, x, ys[i])
    d.show()


def fade(ms=500, steps=10):
    """Fade display to black."""
    if steps <= 0:
        steps = 1
    delay = ms // steps
    for i in range(steps):
        c = 255 - int((i + 1) * 255 / steps)
        d.oled.contrast(c)
        time.sleep_ms(delay)
    d.oled.fill(0)
    d.show()


# --- Firefly particles (boot/no-comm animation) ---

def ff_init():
    global ff, ff_t0, ff_last
    ff_t0 = time.ticks_ms()
    ff_last = ff_t0
    ff = [
        [-6, 0, 1, 6, 24, 0, 0],
        [-8, 4, 2, 8, 34, 1000, 0],
        [-10, 8, 1, 5, 44, 2000, 0],
    ]


def ff_step(mode="float"):
    global ff_last, dash, dash_init
    now = time.ticks_ms()
    if time.ticks_diff(now, ff_last) < TICK:
        return False
    ff_last = now

    # Redraw header
    d.oled.fill_rect(0, 0, W, Y, 0)
    name = d.stone_name.upper()
    w = tw(name)
    if w <= 120:
        if fnt:
            fnt.draw(d.oled, name, 2, 3)
        else:
            d.oled.text(name, 2, 3)
    d.oled.fill_rect(0, Y, W, BH, 0)

    for f in ff:
        if f[FDN]:
            continue
        if time.ticks_diff(now, ff_t0) < f[FD]:
            continue
        if mode == "dash" and dash_init:
            f[FD] = 0
            f[FDN] = 0
            if f[FX] < 0:
                f[FX] = 0
            elif f[FX] > W - 2:
                f[FX] = W - 2
        if mode == "dash":
            f[FX] += dash
        else:
            f[FX] += f[FS]
            f[FP] = (f[FP] + f[FS]) % len(SIN)
        if f[FX] > W + 2:
            if mode == "dash":
                f[FDN] = 1
            else:
                f[FX] = -2
                f[FP] = 0
        sv = SIN[f[FP]]
        y = f[FB] + (f[FA] * sv) // 100
        y = max(Y + 1, min(H - 2, y))
        x = int(f[FX])
        if 0 <= x < W and 0 <= y < H:
            d.oled.pixel(x, int(y), 1)
    d.show()
    if mode == "dash":
        dash = dash * 2
        dash_init = False
        return all(f[FDN] for f in ff)
    return False


# --- State machine ---

def enter(s):
    global state, needs, dash, dash_init, ff_last
    state = s
    if s == S_BOOT:
        msg("Zen Garden", "Firefly v2")
    elif s == S_CONN:
        if not ff:
            ff_init()
        needs = False
        d.oled.contrast(255)
        dash = 1
        dash_init = True
        ff_last = 0
    elif s == S_NC:
        needs = False
        d.oled.contrast(255)
        ff_init()
    elif s == S_IDLE:
        needs = True


def cmd(line):
    """Parse and execute a serial command."""
    global last_rx
    line = line.strip()
    if not line:
        return
    parts = line.split(",", 1)
    c = parts[0].upper()
    a = parts[1] if len(parts) > 1 else ""

    # During transitions, only accept state/data updates
    if state != S_IDLE and c not in ("I", "S", "H", "M", "G", "D", "R", "WIPE-IN", "WIPE-OUT"):
        r("OK")
        return

    last_rx = time.ticks_ms()

    # Advance the activity spinner on every valid command. One pixel in the
    # bottom-right corner moves clockwise — confirms the host→USB→firmware
    # pipeline is live without needing any extra serial traffic.
    d.spinner_pos = (d.spinner_pos + 1) % 6

    if state == S_NC and c in ("S", "H", "M", "G", "D", "R"):
        enter(S_CONN)

    try:
        if c == "I":
            r(d.device_info())

        elif c == "C":
            d.clear()
            r("OK")

        elif c == "S":
            d.set_stone_name(a)
            if state == S_IDLE:
                d.oled.contrast(255)
                d.draw_dashboard()
            r("OK")

        elif c == "H":
            d.set_health(a.lower())
            if state == S_IDLE:
                d.oled.contrast(255)
                d.draw_dashboard()
            r("OK")

        elif c == "M":
            # M,cpu,mem,disk,uptime
            p = a.split(",")
            cpu = int(p[0]) if len(p) > 0 and p[0] else None
            mem = int(p[1]) if len(p) > 1 and p[1] else None
            disk = int(p[2]) if len(p) > 2 and p[2] else None
            up = p[3] if len(p) > 3 else None
            d.update_metrics(cpu=cpu, mem=mem, disk=disk, uptime=up)
            r("OK")

        elif c == "G":
            # G,offerings,stones,net_bps,seed_bank
            p = a.split(",")
            off = int(p[0]) if len(p) > 0 and p[0] else None
            sto = int(p[1]) if len(p) > 1 and p[1] else None
            net = int(p[2]) if len(p) > 2 and p[2] else None
            sb = int(p[3]) if len(p) > 3 and p[3] else None
            d.update_garden(offerings=off, stones=sto, net_bps=net, seed_bank=sb)
            r("OK")

        elif c == "D":
            # D,cpu,mem,disk,uptime,offerings,stones,net_bps,seed_bank
            p = a.split(",")
            if len(p) >= 4:
                d.update_metrics(
                    cpu=int(p[0]),
                    mem=int(p[1]),
                    disk=int(p[2]),
                    uptime=p[3],
                )
            if len(p) >= 8:
                d.update_garden(
                    offerings=int(p[4]),
                    stones=int(p[5]),
                    net_bps=int(p[6]),
                    seed_bank=int(p[7]),
                )
            r("OK")

        elif c == "R":
            if state == S_IDLE:
                d.oled.contrast(255)
                d.draw_dashboard()
            r("OK")

        elif c in ("WIPE-IN", "WIPE-OUT"):
            p = a.split(",", 1)
            l1 = p[0] if len(p) > 0 else ""
            l2 = p[1] if len(p) > 1 else ""
            d.wipe(l1, l2, "in" if c == "WIPE-IN" else "out")
            r("OK")

        elif c == "BLINK":
            cnt = int(a) if a else 3
            for _ in range(cnt):
                d.oled.fill_rect(0, Y, W, BH, 0)
                d.show()
                time.sleep_ms(200)
                d.draw_dashboard()
                time.sleep_ms(200)
            r("OK")

        elif c == "PULSE":
            cnt = int(a) if a else 3
            for _ in range(cnt):
                for cv in range(255, 0, -15):
                    d.oled.contrast(cv)
                    time.sleep_ms(30)
                for cv in range(0, 256, 15):
                    d.oled.contrast(cv)
                    time.sleep_ms(30)
            d.oled.contrast(255)
            r("OK")

        else:
            r("ERR,unknown_cmd:%s" % c)

    except Exception as e:
        r("ERR,%s" % e)


def main():
    global needs, last_rx
    r("Firefly OLED v2 starting...")
    # FIREFLY-0004: unsolicited HELLO frame so the bus identifies us
    # before the `I` fallback timeout elapses.
    from firefly_oled_v2 import hello_frame
    r(hello_frame())
    if not init_display():
        r("ERR,failed_to_init_display")
        return
    d.set_stone_name("Zen Garden")
    enter(S_NC)
    r("OK,ready")

    poll = select.poll()
    poll.register(sys.stdin, select.POLLIN)

    while True:
        try:
            now = time.ticks_ms()

            # Comm timeout → no-comm state
            if last_rx is not None and state in (S_IDLE, S_CONN):
                if time.ticks_diff(now, last_rx) > TO:
                    enter(S_NC)

            # State rendering
            if state == S_CONN:
                if ff_step("dash"):
                    fade(500)
                    enter(S_IDLE)
                    needs = True
            elif state == S_NC:
                ff_step("float")

            if needs and state == S_IDLE:
                needs = False
                d.oled.contrast(255)
                d.draw_dashboard()

            # Poll for serial input
            events = poll.poll(0)
            if events:
                line = sys.stdin.readline()
                if line:
                    cmd(line)
            else:
                time.sleep_ms(10)

        except KeyboardInterrupt:
            r("OK,interrupted")
            break
        except Exception as e:
            r("ERR,%s" % e)


main()
