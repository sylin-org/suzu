"""
Firefly OLED v2 — Dense Icon Dashboard

Hardware: NodeMCU ESP8266 V3 with 0.96" OLED (128x64, SSD1306, I2C)
Display zones (hardware):
  - Yellow header: rows 0-15 (16px)
  - Blue body:     rows 16-63 (48px)

Layout:
  Yellow: [stone name (scroll)]              [health 8x8]
  Blue:   [CPU icon] |████░░| 42%    [gear]  7
          [MEM icon] |██████| 82%    [net]   1.2M
          [DSK icon] |██░░░░| 30%    [clock] 3h
           ^ optional [USB] icon when seed-bank present

I2C Pins (NodeMCU V3 datasheet):
  SCL: GPIO12 (D6)
  SDA: GPIO14 (D5)
"""

from machine import Pin, SoftI2C, unique_id
import ssd1306
import ubinascii
import ujson
import profont_10 as font


# FIREFLY-0004: firmware-side runtime truth only. Everything else
# lives in /zen-garden.json (written by NewFirefly.ps1 at
# provisioning time).
_FW_VERSION = "2.0.0"
_PROCESSOR = "esp8266"


def _load_descriptor():
    try:
        with open("/zen-garden.json", "r") as f:
            return ujson.loads(f.read())
    except (OSError, ValueError):
        return {}


def _hardware_id():
    try:
        return "esp8266-" + ubinascii.hexlify(unique_id()).decode("ascii")
    except Exception:
        return ""


_DESCRIPTOR = _load_descriptor()


def descriptor_json():
    """Merge runtime-truth fields into the provisioned descriptor."""
    d = dict(_DESCRIPTOR)
    d["hardware_id"] = _hardware_id()
    d["version"] = _FW_VERSION
    d["processor"] = _PROCESSOR
    return ujson.dumps(d)


def hello_frame():
    """FIREFLY-0004 unsolicited HELLO emitted on boot."""
    return "* HELLO," + descriptor_json()
from icons import (
    ICON_CPU, ICON_MEM, ICON_DSK, ICON_USB,
    ICON_GEAR, ICON_NET, ICON_CLOCK, ICON_STONES,
    ICON_THRIVING, ICON_WITHERING, ICON_WILTING,
)

# --- Hardware ---
I2C_SCL = 12   # D6
I2C_SDA = 14   # D5
I2C_FREQ = 400000
W = 128
H = 64
Y_BLUE = 16    # blue zone start row

# --- Layout constants ---
# Left panel: resource meters
ICON_X = 1         # icon left edge
BAR_X = 12         # bar start (after icon + 2px gap)
BAR_W = 48         # bar inner width (each pixel ~2%)
BAR_END = BAR_X + BAR_W + 1  # bar end cap x
PCT_X = BAR_END + 3  # percentage text x
# Disk row with seed-bank icon shifts bar right
BAR_X_SB = 22      # bar start when seed-bank icon shown
BAR_W_SB = 38      # narrower bar when seed-bank icon shown
BAR_END_SB = BAR_X_SB + BAR_W_SB + 1

# Right panel: info column
DIV_X = 84         # vertical divider x
INFO_ICON_X = 88   # right-panel icon x
INFO_TEXT_X = 98    # right-panel text x

# Row y positions (icon top edge; bar is icon_y+2 to icon_y+4)
ROW_Y = (19, 33, 47)  # CPU, MEM, DSK

# Bar cap height (3px tall vertical bracket)
CAP_H = 3

# Activity spinner: 2x3 pixel group, bottom-right of blue zone.
# Each SSE event advances the lit pixel clockwise one step:
#
#    1 2        (124,58) (125,58)
#    6 3   =    (124,59) (125,59)
#    5 4        (124,60) (125,60)
#
# Order below follows that clockwise traversal: 1 → 2 → 3 → 4 → 5 → 6 → 1 ...
SPINNER_POSITIONS = (
    (124, 58),  # 1 - top-left
    (125, 58),  # 2 - top-right
    (125, 59),  # 3 - middle-right
    (125, 60),  # 4 - bottom-right
    (124, 60),  # 5 - bottom-left
    (124, 59),  # 6 - middle-left
)

# 8x8 icon bitmaps are imported from icons.py (Open Iconic, MIT license).
# Re-generate by running: python firmware/firefly/tools/convert_icons.py


class FireflyOLED:
    """Firefly OLED v2 display controller — dense icon dashboard."""

    def __init__(self):
        self.i2c = SoftI2C(scl=Pin(I2C_SCL), sda=Pin(I2C_SDA), freq=I2C_FREQ)
        self.oled = ssd1306.SSD1306_I2C(W, H, self.i2c)

        # State
        self.stone_name = "unknown"
        self.health = "thriving"
        self.cpu = 0
        self.mem = 0
        self.disk = 0
        self.uptime = "0s"
        self.offerings = 0
        self.stones = 0
        self.net_bps = 0
        self.seed_bank = False

        # Header scroll tick
        self._tick = 0

        # Activity spinner: advances on each SSE event (via tick() method)
        self.spinner_pos = 0

    # --- Public API ---

    def clear(self):
        self.oled.fill(0)
        self.oled.show()

    def show(self):
        self.oled.show()

    def set_stone_name(self, name):
        self.stone_name = name
        self._tick = 0

    def set_health(self, state):
        if state in ("thriving", "withering", "wilting", "resting", "offline"):
            self.health = state

    def update_metrics(self, cpu=None, mem=None, disk=None, uptime=None):
        if cpu is not None:
            self.cpu = min(100, max(0, cpu))
        if mem is not None:
            self.mem = min(100, max(0, mem))
        if disk is not None:
            self.disk = min(100, max(0, disk))
        if uptime is not None:
            self.uptime = uptime

    def update_garden(self, offerings=None, stones=None, net_bps=None, seed_bank=None):
        if offerings is not None:
            self.offerings = offerings
        if stones is not None:
            self.stones = stones
        if net_bps is not None:
            self.net_bps = net_bps
        if seed_bank is not None:
            self.seed_bank = bool(seed_bank)

    def device_info(self):
        """FIREFLY-0004 descriptor framed as `OK,{...}` for the `I` command."""
        return "OK," + descriptor_json()

    # --- Dashboard rendering ---

    def draw_dashboard(self):
        """Render the full dense dashboard (called on every tick)."""
        self.oled.fill(0)
        self._draw_header()
        self._draw_meters()
        self._draw_info_panel()
        self._draw_spinner()
        self.oled.show()

    def _draw_spinner(self):
        """Draw the activity spinner — a single lit pixel that advances
        clockwise around a 2x3 region on each incoming serial command.

        One pixel is always lit (confirms firmware alive); it moves only
        when the host sends data (confirms SSE pipeline alive end-to-end).
        """
        x, y = SPINNER_POSITIONS[self.spinner_pos % len(SPINNER_POSITIONS)]
        self.oled.pixel(x, y, 1)

    def _draw_header(self):
        """Yellow zone: stone name (scrolling) + health icon."""
        name = self.stone_name.upper()
        nw = font.text_width(name)
        avail = W - 12  # reserve right side for health icon

        if nw <= avail:
            font.draw(self.oled, name, 2, 3)
        else:
            # Tick-based scroll: pause → scroll → pause → scroll back
            scroll_max = nw - avail + 4
            pause = 20
            scroll_t = max(scroll_max // 2, 1)
            cycle = 2 * pause + 2 * scroll_t
            t = self._tick % cycle
            if t < pause:
                sx = 0
            elif t < pause + scroll_t:
                sx = (t - pause) * 2
            elif t < 2 * pause + scroll_t:
                sx = scroll_max
            else:
                sx = scroll_max - (t - 2 * pause - scroll_t) * 2
            sx = max(0, min(scroll_max, sx))
            font.draw(self.oled, name, 2 - sx, 3)
            self._tick += 1

        # Health icon (top-right corner)
        icon = self._health_icon()
        self._draw_icon(icon, W - 10, 4)

    def _draw_meters(self):
        """Blue zone left panel: CPU/MEM/DSK meters with icons and bars."""
        meters = [
            (ICON_CPU, self.cpu),
            (ICON_MEM, self.mem),
            (ICON_DSK, self.disk),
        ]
        for i, (icon, pct) in enumerate(meters):
            y = ROW_Y[i]
            # Icon
            self._draw_icon(icon, ICON_X, y)

            # Seed-bank icon next to disk
            if i == 2 and self.seed_bank:
                self._draw_icon(ICON_USB, ICON_X + 10, y)
                bx = BAR_X_SB
                bw = BAR_W_SB
                be = BAR_END_SB
            else:
                bx = BAR_X
                bw = BAR_W
                be = BAR_END

            # Bar: start cap | fill | end cap
            bar_mid = y + 3  # vertical center of 8px icon
            cap_top = bar_mid - 1
            self.oled.vline(bx, cap_top, CAP_H, 1)      # start cap
            self.oled.vline(be, cap_top, CAP_H, 1)       # end cap
            fill_w = (bw * pct) // 100
            if fill_w > 0:
                self.oled.hline(bx + 1, bar_mid, fill_w, 1)  # fill

            # Percentage text
            txt = str(pct)
            tx = be + 3
            font.draw(self.oled, txt, tx, y + 1)

    def _draw_info_panel(self):
        """Blue zone right panel: offerings, network, uptime."""
        # Dotted vertical divider
        for y in range(Y_BLUE + 2, H - 2, 3):
            self.oled.pixel(DIV_X, y, 1)

        rows = [
            (ICON_GEAR,  self._fmt_offerings()),
            (ICON_NET,   self._fmt_net()),
            (ICON_CLOCK, self.uptime),
        ]
        for i, (icon, text) in enumerate(rows):
            y = ROW_Y[i]
            self._draw_icon(icon, INFO_ICON_X, y)
            font.draw(self.oled, text, INFO_TEXT_X, y + 1)

    # --- Helpers ---

    def _draw_icon(self, data, x, y):
        """Draw an 8x8 MSB-left bitmap."""
        for row in range(8):
            byte = data[row]
            for col in range(8):
                if byte & (0x80 >> col):
                    self.oled.pixel(x + col, y + row, 1)

    def _health_icon(self):
        if self.health == "thriving":
            return ICON_THRIVING
        if self.health == "wilting":
            return ICON_WILTING
        return ICON_WITHERING

    def _fmt_offerings(self):
        return str(self.offerings)

    def _fmt_net(self):
        b = self.net_bps
        if b <= 0:
            return "idle"
        if b < 1024:
            return "%dB" % b
        if b < 1048576:
            k = b / 1024
            return "%.0fK" % k if k >= 10 else "%.1fK" % k
        m = b / 1048576
        return "%.0fM" % m if m >= 10 else "%.1fM" % m

    # --- Animations (kept from v1 for event interrupts) ---

    def wipe(self, line1, line2, direction="in", wipe_ms=300, hold_ms=1500):
        """Wipe transition: reveal text then clear."""
        bar_w = 12
        steps = W // bar_w
        delay = wipe_ms // steps // 2

        self.oled.fill(0)
        font.draw(self.oled, line1.upper(), 4, 24)
        font.draw(self.oled, line2.upper(), 4, 40)

        rng = range(0, W, bar_w) if direction == "in" else range(W - bar_w, -bar_w, -bar_w)
        import time

        # Reveal phase
        for x in rng:
            self.oled.fill_rect(x, Y_BLUE, bar_w, 48, 1)
            self.oled.show()
            time.sleep_ms(delay // 2)
            self.oled.fill_rect(x, Y_BLUE, bar_w, 48, 0)
            font.draw(self.oled, line1.upper(), 4, 24)
            font.draw(self.oled, line2.upper(), 4, 40)
            if direction == "in" and x + bar_w < W:
                self.oled.fill_rect(x + bar_w, Y_BLUE, W - x - bar_w, 48, 0)
            elif direction != "in" and x > 0:
                self.oled.fill_rect(0, Y_BLUE, x, 48, 0)
            self.oled.show()
            time.sleep_ms(delay // 2)

        time.sleep_ms(hold_ms)

        # Clear phase
        rng2 = range(0, W, bar_w) if direction == "in" else range(W - bar_w, -bar_w, -bar_w)
        for x in rng2:
            self.oled.fill_rect(x, Y_BLUE, bar_w, 48, 1)
            self.oled.show()
            time.sleep_ms(delay // 2)
            self.oled.fill_rect(x, Y_BLUE, bar_w, 48, 0)
            self.oled.show()
            time.sleep_ms(delay // 2)
