import gc, sys, time, select
from machine import Timer, UART
W = 128
H = 64
Y = 16
BH = 48
S_BOOT = 0
S_READY = 1
S_NC = 2
S_CONN = 3
S_IDLE = 4
TO = 10000
TICK = 100
SIN = (0, 38, 70, 92, 100, 92, 70, 38, 0, -38, -70, -92, -100, -92, -70, -38)
FX = 0
FP = 1
FS = 2
FA = 3
FB = 4
FD = 5
FDN = 6
u = UART(0, 115200)
fnt = None

def _ns():
    pass
d = None
tm = None
needs = False
state = S_BOOT
last_rx = None
ff = []
ff_t0 = 0
ff_last = 0
dash = 1
ht = 0
dash_init = False

def r(msg):
    u.write(msg + "\n")
    time.sleep_ms(2)

def tcb(t):
    global needs
    needs = True

def init_display():
    global d, tm, fnt
    try:
        gc.collect()
        from firefly_oled import FireflyOLED
        d = FireflyOLED()
        fo = sys.modules.get('firefly_oled')
        fnt = fo.font if fo else None
        tm = Timer(-1)
        tm.init(period=200, mode=Timer.PERIODIC, callback=tcb)
        return True
    except Exception as e:
        r("ERR,display_init:%s" % e)
        return False

def tw(s):
    if fnt:
        return fnt.text_width(s)
    return len(s) * 8

def msg(title, line1, line2=None):
    d.oled.contrast(255)
    d.oled.fill_rect(0, 0, W, Y, 0)
    t = title.upper()
    w = tw(t)
    x = 2 if w >= W else (W - w) // 2
    if fnt:
        fnt.draw(d.oled, t, x, 3)
    else:
        d.text(t, x, 3)
    d.oled.fill_rect(0, Y, W, BH, 0)
    if line2 is None:
        lines = [line1]
        ys = [32]
    else:
        lines = [line1, line2]
        ys = [24, 40]
    for i in range(len(lines)):
        s = lines[i]
        w = tw(s)
        x = 2 if w >= W else (W - w) // 2
        if fnt:
            fnt.draw(d.oled, s, x, ys[i])
        else:
            d.text(s, x, ys[i])
    d.show()

def hdr(show=True):
    global ht
    d.oled.fill_rect(0, 0, W, Y, 0)
    name = d.stone_name.upper()
    w = tw(name)
    hw = 120
    if w <= hw:
        x = 2
        if fnt:
            fnt.draw(d.oled, name, x, 3)
        else:
            d.text(name, x, 3)
    else:
        scroll_max = w - hw + 4
        pause = 20
        scroll_ticks = scroll_max // 2 if scroll_max > 0 else 1
        cycle = 2 * pause + 2 * scroll_ticks
        t = ht % cycle
        if t < pause:
            sx = 0
        elif t < pause + scroll_ticks:
            sx = (t - pause) * 2
        elif t < 2 * pause + scroll_ticks:
            sx = scroll_max
        else:
            sx = scroll_max - (t - 2 * pause - scroll_ticks) * 2
        if sx < 0:
            sx = 0
        if sx > scroll_max:
            sx = scroll_max
        if fnt:
            fnt.draw(d.oled, name, 2 - sx, 3)
        else:
            d.text(name, 2 - sx, 3)
        ht += 1
    d.oled.fill_rect(0, Y, W, BH, 0)
    if show:
        d.show()

def fade(ms=500, steps=10):
    if steps <= 0:
        steps = 1
    delay = ms // steps
    for i in range(steps):
        c = 255 - int((i + 1) * 255 / steps)
        d.oled.contrast(c)
        time.sleep_ms(delay)
    d.oled.fill(0)
    d.show()

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
    hdr(False)
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
        if y < Y + 1:
            y = Y + 1
        if y > H - 2:
            y = H - 2
        x = int(f[FX])
        if 0 <= x < W and 0 <= y < H:
            d.oled.pixel(x, int(y), 1)
    d.show()
    if mode == "dash":
        dash = dash * 2
        dash_init = False
        return all(f[FDN] for f in ff)
    return False

def wipe(line1, line2, direction=1):
    msg("suzu", line1, line2)
    y = H - 1
    step = 8
    steps = W // step
    delay = 500 // (steps if steps > 0 else 1)
    if direction >= 0:
        for x in range(0, W + 1, step):
            d.oled.hline(0, y, x, 1)
            d.show()
            time.sleep_ms(delay)
    else:
        for x in range(W, -1, -step):
            d.oled.hline(x, y, W - x, 1)
            d.show()
            time.sleep_ms(delay)
    time.sleep_ms(1000)
    if direction >= 0:
        for x in range(0, W + 1, step):
            d.oled.hline(0, y, x, 0)
            d.show()
            time.sleep_ms(delay)
    else:
        for x in range(W, -1, -step):
            d.oled.hline(x, y, W - x, 0)
            d.show()
            time.sleep_ms(delay)

def enter(s):
    global state, needs, dash, dash_init
    state = s
    if s == S_BOOT:
        msg("suzu", "Firefly Initializing...")
    elif s == S_READY:
        msg("suzu", "Firefly ready!")
    elif s == S_CONN:
        if not ff:
            ff_init()
        needs = False
        d.oled.contrast(255)
        hdr()
        dash = 1
        dash_init = True
        global ff_last
        ff_last = 0
    elif s == S_NC:
        needs = False
        d.oled.contrast(255)
        hdr()
        ff_init()
    elif s == S_IDLE:
        needs = True

def cmd(line):
    global last_rx
    line = line.strip()
    if not line:
        return
    parts = line.split(",", 1)
    c = parts[0].upper()
    a = parts[1] if len(parts) > 1 else ""
    # Transitions override rendering: only accept state/data updates while transitioning.
    if state != S_IDLE and c not in ("I", "S", "H", "M", "R", "WIPE-IN", "WIPE-OUT"):
        r("OK")
        return
    last_rx = time.ticks_ms()
    if state == S_NC and c in ("S", "H", "M", "R"):
        enter(S_CONN)
    try:
        if c == "I":
            r(d.device_info())
        elif c == "C":
            d.clear(); r("OK")
        elif c == "S":
            global ht
            ht = 0
            d.set_stone_name(a)
            if state == S_IDLE:
                d.oled.contrast(255); d.draw_status_screen()
            r("OK")
        elif c == "H":
            d.set_health(a.lower())
            if state == S_IDLE:
                d.oled.contrast(255); d.draw_status_screen()
            r("OK")
        elif c == "M":
            p = a.split(",")
            cpu = int(p[0]) if len(p) > 0 else None
            mem = int(p[1]) if len(p) > 1 else None
            up = p[2] if len(p) > 2 else None
            d.update_metrics(cpu=cpu, mem=mem, uptime=up)
            r("OK")
        elif c == "R":
            if state == S_IDLE:
                d.oled.contrast(255); d.draw_status_screen()
            r("OK")
        elif c == "WIPE-IN" or c == "WIPE-OUT":
            p = a.split(",", 1)
            l1 = p[0] if len(p) > 0 else ""
            l2 = p[1] if len(p) > 1 else ""
            wipe(l1, l2, 1 if c == "WIPE-IN" else -1)
            r("OK")
        else:
            r("ERR,unknown_cmd:%s" % c)
    except Exception as e:
        r("ERR,%s" % e)

def main():
    global needs, last_rx
    r("Firefly OLED starting...")
    # emit the unsolicited HELLO frame as soon as the
    # serial port is ready — the bus is listening with a ~3s timeout.
    from firefly_oled import hello_frame
    r(hello_frame())
    if not init_display():
        r("ERR,failed_to_init_display")
        return
    d.set_stone_name("suzu")
    enter(S_NC)
    r("OK,ready")
    poll = select.poll()
    poll.register(sys.stdin, select.POLLIN)
    while True:
        try:
            now = time.ticks_ms()
            if last_rx is not None and state in (S_IDLE, S_CONN):
                if time.ticks_diff(now, last_rx) > TO:
                    enter(S_NC)
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
                d.draw_status_screen()
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
