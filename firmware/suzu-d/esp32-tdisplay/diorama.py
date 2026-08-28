"""
Firefly T-Display Diorama Renderer

Three-panel layout for TENSTAR T-Display ESP32-D0WD (ST7789 135x240 RGB565).
Requires russhughes/st7789_mpy firmware with blit_buffer support.

Layout:
  Identity bar (5px left edge, full height)
  HEAD  y=0..95   — stone name, health, gauges
  SCENE y=96..167  — sky gradient (simplified; sprites added later)
  FOOT  y=168..239 — offerings list, capability icons
"""

import gc

try:
    import vga1_8x8 as font
except ImportError:
    font = None

# ═══════════════════════════════════════════════════════
# LAYOUT
# ═══════════════════════════════════════════════════════
W = 135
H = 240
BAR = 5           # Identity bar width
HEAD_H = 96
SCENE_H = 72
FOOT_H = 72
SCENE_T = HEAD_H           # 96
SCENE_B = HEAD_H + SCENE_H # 168

CX = BAR + 7       # 12 — content left margin
CW = W - CX - 6    # 117 — content width

# Gauge layout
G_LX = CX           # 12 — label x
G_TX = CX + 22      # 34 — track x
G_TW = CW - 24      # 93 — track width
G_TH = 3            # track height
G_Y0 = 57           # first gauge row y
G_SP = 9            # gauge row spacing

# Offerings
OFF_HDR_Y = SCENE_B + 5   # 173
OFF_LST_Y = SCENE_B + 16  # 184
OFF_SP = 9
OFF_MAX = 4

# Icons
ICN_Y = H - 16   # 224
ICN_SZ = 8
ICN_GAP = 10

# ═══════════════════════════════════════════════════════
# COLOR UTILITIES
# ═══════════════════════════════════════════════════════

def rgb565(r, g, b):
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)

def lerp565(c1, c2, t):
    """Interpolate two RGB565 colors. t: 0..100."""
    r1 = (c1 >> 11) & 0x1F; g1 = (c1 >> 5) & 0x3F; b1 = c1 & 0x1F
    r2 = (c2 >> 11) & 0x1F; g2 = (c2 >> 5) & 0x3F; b2 = c2 & 0x1F
    r = r1 + (r2 - r1) * t // 100
    g = g1 + (g2 - g1) * t // 100
    b = b1 + (b2 - b1) * t // 100
    return (r << 11) | (g << 5) | b

def hsl565(h, s, l):
    """HSL to RGB565. h:0-359, s:0-100, l:0-100. Integer math only."""
    if s == 0:
        v = l * 255 // 100
        return rgb565(v, v, v)
    s2 = s / 100; l2 = l / 100
    c = (1 - abs(2 * l2 - 1)) * s2
    x = c * (1 - abs((h / 60) % 2 - 1))
    m = l2 - c / 2
    h6 = (h // 60) % 6
    if h6 == 0:   r1, g1, b1 = c, x, 0
    elif h6 == 1: r1, g1, b1 = x, c, 0
    elif h6 == 2: r1, g1, b1 = 0, c, x
    elif h6 == 3: r1, g1, b1 = 0, x, c
    elif h6 == 4: r1, g1, b1 = x, 0, c
    else:         r1, g1, b1 = c, 0, x
    return rgb565(int((r1 + m) * 255), int((g1 + m) * 255), int((b1 + m) * 255))

def stone_hue(name):
    """Deterministic hue 0-359 from stone name (matches JSX stoneHue)."""
    h = 0
    for ch in name:
        h = ord(ch) + ((h << 5) - h)
    return abs(h) % 360

def gauge_color(val):
    """Cold-to-hot gauge ramp. val:0-100 -> RGB565."""
    if val < 15:
        return rgb565(80, 80, 80)
    elif val < 30:
        t = (val - 15) * 100 // 15
        return rgb565(80 - 10 * t // 100, 80 + 50 * t // 100, 80 + 50 * t // 100)
    elif val < 50:
        t = (val - 30) * 100 // 20
        return rgb565(70 + 70 * t // 100, 130 + 50 * t // 100, 130 - 80 * t // 100)
    elif val < 70:
        t = (val - 50) * 100 // 20
        return rgb565(140 + 70 * t // 100, 180 - 20 * t // 100, 50 - 30 * t // 100)
    elif val < 85:
        t = (val - 70) * 100 // 15
        return rgb565(210 + 30 * t // 100, 160 - 70 * t // 100, 20)
    else:
        t = min(100, (val - 85) * 100 // 15)
        return rgb565(240 + 15 * t // 100, 90 - 50 * t // 100, 20 - 15 * t // 100)

# ═══════════════════════════════════════════════════════
# SINE TABLE (64 entries, -100..100) for smooth animation
# sin(i * 2π / 64) * 100, rounded to int
# ═══════════════════════════════════════════════════════
SIN64 = (
    0,   10,  20,  29,  38,  47,  56,  63,  70,  77,  83,  88,  92,  96,  98, 100,
  100,  100,  98,  96,  92,  88,  83,  77,  70,  63,  56,  47,  38,  29,  20,  10,
    0,  -10, -20, -29, -38, -47, -56, -63, -70, -77, -83, -88, -92, -96, -98,-100,
 -100, -100, -98, -96, -92, -88, -83, -77, -70, -63, -56, -47, -38, -29, -20, -10,
)
_SIN_LEN = 64

# ═══════════════════════════════════════════════════════
# NAMED COLORS
# ═══════════════════════════════════════════════════════
BLACK = 0x0000
WHITE = 0xFFFF
SAGE = rgb565(0x84, 0xA5, 0x9D)
HONEY = rgb565(0xC4, 0xB0, 0x60)
CLAY = rgb565(0xD4, 0xA3, 0x73)
DIM = rgb565(0x4A, 0x4A, 0x4A)
NAME_CLR = rgb565(0xEC, 0xE8, 0xE0)
OFF_NAME = rgb565(0x9A, 0x96, 0x90)
OFF_MORE = rgb565(0x3A, 0x3A, 0x3A)
TRACK_BG = rgb565(12, 12, 12)

# Midnight sky colors (NO_COMM ambient)
SKY_MID = rgb565(5, 6, 26)

# Star brightness levels (bright, medium, dim, faint)
_STAR_B = (
    rgb565(0xE8, 0xE4, 0xD8),  # bright
    rgb565(0xA0, 0x9C, 0x90),  # medium
    rgb565(0x60, 0x5E, 0x54),  # dim
    rgb565(0x38, 0x36, 0x30),  # faint
)

# Ambient firefly palette — 3 warm hues (amber, gold, warm orange)
_FF_CORE = (rgb565(255, 220, 140), rgb565(255, 200, 100), rgb565(255, 180, 80))
_FF_MID  = (rgb565(140, 100, 40),  rgb565(130, 90, 30),   rgb565(140, 80, 20))
_FF_OUT  = (rgb565(60, 40, 15),    rgb565(55, 35, 12),    rgb565(60, 30, 8))

# ═══════════════════════════════════════════════════════
# SKY LOOKUP
# ═══════════════════════════════════════════════════════
# Sun position 0-100 per hour (peak at noon, 0 at night)
# max(0, sin(((h-6)/12)*pi)) * 100
SUN_POS = (0,0,0,0,0,0, 0,26,50,71,87,97, 100,97,87,71,50,26, 0,0,0,0,0,0)

# Sky color triplets (top, mid, bottom) for each regime
_SKY_N = (rgb565(5,6,26),   rgb565(10,13,34),  rgb565(16,19,42))
_SKY_DK = (rgb565(40,24,56),  rgb565(74,42,64),  rgb565(176,96,48))
_SKY_D = (rgb565(58,88,120), rgb565(106,138,168), rgb565(152,176,192))

def sky_colors(hour):
    """Return (top, mid, bot) sky RGB565 colors for given hour."""
    sp = SUN_POS[hour % 24]
    if sp < 15:
        return _SKY_N
    elif sp < 35:
        t = (sp - 15) * 100 // 20
        return (lerp565(_SKY_N[0], _SKY_DK[0], t),
                lerp565(_SKY_N[1], _SKY_DK[1], t),
                lerp565(_SKY_N[2], _SKY_DK[2], t))
    else:
        t = min(100, (sp - 35) * 100 // 40)
        return (lerp565(_SKY_DK[0], _SKY_D[0], t),
                lerp565(_SKY_DK[1], _SKY_D[1], t),
                lerp565(_SKY_DK[2], _SKY_D[2], t))

# ═══════════════════════════════════════════════════════
# ICON BITMAPS
# ═══════════════════════════════════════════════════════
_SEED = (
    "   ..   ",
    "  .SS.  ",
    " .S..S. ",
    "  .SS.  ",
    "   ||   ",
    "   ||   ",
    "  .||.  ",
    " ..||.. ",
)
_SEED_P = {".": (0x3D,0x68,0x38), "S": (0x6A,0xAA,0x4E), "|": (0x5A,0x4A,0x32)}

_AI = (
    " ...... ",
    ".AAAAAA.",
    ".A.AA.A.",
    ".AAAAAA.",
    " .AAAA. ",
    "  .AA.  ",
    " .A..A. ",
    ".A.  .A.",
)
_AI_ON = {".": (0x4A,0x5A,0x8A), "A": (0x7A,0x9A,0xCA)}
_AI_OFF = {".": (0x2A,0x2A,0x2A), "A": (0x3A,0x3A,0x3A)}
_AI_ACT = {".": (0x6A,0x5A,0x2A), "A": (0xC4,0xB0,0x60)}

_LNT = (
    "   ..   ",
    "  .LL.  ",
    " .LLLL. ",
    " .LLLL. ",
    " .LLLL. ",
    "  .LL.  ",
    "  ....  ",
    "   ..   ",
)
_LNT_P = {".": (0x6A,0x50,0x20), "L": (0xC4,0xA0,0x40)}

def _render_icon(rows, pal, bg565):
    """Render 8x8 char-grid icon into 128-byte big-endian RGB565 bytearray."""
    buf = bytearray(128)
    idx = 0
    for row in rows:
        for ch in row:
            if ch == " ":
                c = bg565
            else:
                rgb = pal.get(ch)
                c = rgb565(rgb[0], rgb[1], rgb[2]) if rgb else bg565
            buf[idx] = (c >> 8) & 0xFF
            buf[idx + 1] = c & 0xFF
            idx += 2
    return buf


# ═══════════════════════════════════════════════════════
# DIORAMA CLASS
# ═══════════════════════════════════════════════════════

class Diorama:
    def __init__(self, tft):
        self.tft = tft

        # Data state
        self.name = ""
        self.health = "thriving"
        self.cpu = 0
        self.mem = 0
        self.disk = 0
        self.io = 0
        self.gpu = 0
        self.gpu_active = False
        self.has_gpu = False
        self.uptime = 0
        self.hour = 12
        self.is_lantern = False
        self.has_cricket = False
        self.pond_active = False
        self.offerings = []       # [{n:str, h:str}, ...]
        self.off_count = 0
        self.seed_bank = None     # {n, u, t} or None

        # Identity-derived colors (computed once)
        self._hue = 0
        self._bar_clr = 0
        self._bg = 0
        self._lbl_clr = 0
        self._sep_clr = 0
        self._frm_clr = 0
        self._identity_ok = False

        # Pre-rendered sprites
        self._gauge_row = None    # 186 bytes (93 px x 2)
        self._icn_seed = None
        self._icn_ai_on = None
        self._icn_ai_off = None
        self._icn_ai_act = None
        self._icn_lnt = None

        # Animation
        self.tick = 0
        self._head_dirty = True
        self._foot_dirty = True
        self._last_hour = -1

        # Ambient mode (NO_COMM)
        self._amb_init = False
        self._stars = None        # list of (x, y, speed, phase)
        self._ff = None           # list of [x*10, phase, speed, amp, baseY]  (x scaled 10x)
        self._ff_prev = None      # previous firefly pixel coords for erase

    # ─── Identity & Sprites ─────────────────────────────

    def _compute_identity(self, name):
        """Pre-compute all hue-derived colors and sprites. Called once per name."""
        h = stone_hue(name)
        self._hue = h
        self._bar_clr = hsl565(h, 40, 60)
        self._bg = hsl565(h, 6, 8)
        self._lbl_clr = hsl565(h, 25, 25)
        self._sep_clr = hsl565(h, 15, 15)
        self._frm_clr = hsl565(h, 20, 18)

        # Gauge rainbow sprite: 93 x 1 row, big-endian RGB565
        row = bytearray(G_TW * 2)
        for col in range(G_TW):
            val = col * 100 // G_TW
            c = gauge_color(val)
            row[col * 2] = (c >> 8) & 0xFF
            row[col * 2 + 1] = c & 0xFF
        self._gauge_row = row

        # Icon sprites
        bg = self._bg
        self._icn_seed = _render_icon(_SEED, _SEED_P, bg)
        self._icn_ai_on = _render_icon(_AI, _AI_ON, bg)
        self._icn_ai_off = _render_icon(_AI, _AI_OFF, bg)
        self._icn_ai_act = _render_icon(_AI, _AI_ACT, bg)
        self._icn_lnt = _render_icon(_LNT, _LNT_P, bg)

        self._identity_ok = True
        gc.collect()

    # ─── State Updates ──────────────────────────────────

    def apply_snapshot(self, data):
        """Full JSON state from J command."""
        # Reset ambient mode so it re-initializes on next NO_COMM entry
        self._amb_init = False
        self._ff_prev = [None, None, None]

        new_name = data.get("n", self.name)
        if new_name != self.name or not self._identity_ok:
            self.name = new_name
            self._compute_identity(new_name)

        self.health = data.get("h", self.health)
        self.cpu = data.get("c", 0)
        self.mem = data.get("m", 0)
        self.disk = data.get("d", 0)
        self.io = data.get("i", 0)
        self.gpu = data.get("g", 0)
        self.gpu_active = bool(data.get("ga", 0))
        self.has_gpu = bool(data.get("hg", 0))
        self.uptime = data.get("up", 0)
        self.hour = data.get("hr", 12)
        self.is_lantern = bool(data.get("il", 0))
        self.has_cricket = bool(data.get("hc", 0))
        self.pond_active = bool(data.get("pa", 0))
        self.off_count = data.get("sv", 0)
        self.seed_bank = data.get("sb")

        of = data.get("of", [])
        self.offerings = of[:OFF_MAX]
        if not self.off_count:
            self.off_count = len(of)

        self._head_dirty = True
        self._foot_dirty = True

    def apply_load(self, cpu, mem, disk, io, gpu, gpu_active):
        """Incremental load from L command."""
        self.cpu = cpu
        self.mem = mem
        self.disk = disk
        self.io = io
        self.gpu = gpu
        self.gpu_active = gpu_active
        self._head_dirty = True

    def apply_health(self, h):
        self.health = h
        self._head_dirty = True

    def service_started(self, name, hc):
        if len(self.offerings) < OFF_MAX:
            self.offerings.append({"n": name, "h": hc})
        self.off_count = max(self.off_count + 1, len(self.offerings))
        self._foot_dirty = True

    def service_stopped(self, name):
        self.offerings = [o for o in self.offerings if o.get("n") != name]
        self.off_count = max(0, self.off_count - 1)
        self._foot_dirty = True

    def seed_bank_detected(self, name, used, total):
        self.seed_bank = {"n": name, "u": used, "t": total}
        self._foot_dirty = True

    def seed_bank_removed(self):
        self.seed_bank = None
        self._foot_dirty = True

    def tended(self):
        self._head_dirty = True
        self._foot_dirty = True

    # ─── Helpers ────────────────────────────────────────

    @staticmethod
    def _fmt_up(secs):
        """Format uptime: '47d 3h' / '3h 22m' / '22m' / '<1m'."""
        if secs >= 86400:
            return "{}d {}h".format(secs // 86400, (secs % 86400) // 3600)
        if secs >= 3600:
            return "{}h {}m".format(secs // 3600, (secs % 3600) // 60)
        m = secs // 60
        return "{}m".format(m) if m else "<1m"

    def _health_color(self):
        if self.health == "withering": return HONEY
        if self.health == "wilting": return CLAY
        return SAGE

    # ─── HEAD PANEL ─────────────────────────────────────

    def _draw_head(self):
        tft = self.tft
        bg = self._bg

        # Panel background (right of identity bar)
        tft.fill_rect(BAR, 0, W - BAR, HEAD_H, bg)

        # Identity bar — full height, 3 gradient bands
        bc = self._bar_clr
        bright = lerp565(bc, WHITE, 18)
        dark = lerp565(bc, BLACK, 22)
        tft.fill_rect(0, 0, BAR, 80, bright)
        tft.fill_rect(0, 80, BAR, 80, bc)
        tft.fill_rect(0, 160, BAR, 80, dark)

        if not font:
            return

        # "STONE" label
        tft.text(font, "STONE", CX, 5, self._lbl_clr, bg)

        # Stone name (with scrolling for long names)
        dn = self.name
        if dn.startswith("stone-"):
            dn = dn[6:]
        nw = len(dn) * 8

        if nw <= CW:
            tft.text(font, dn, CX, 15, NAME_CLR, bg)
        else:
            sm = nw - CW + 4
            pause = 20
            st = max(1, sm // 2)
            cyc = 2 * pause + 2 * st
            t = self.tick % cyc
            if t < pause:
                sx = 0
            elif t < pause + st:
                sx = (t - pause) * 2
            elif t < 2 * pause + st:
                sx = sm
            else:
                sx = sm - (t - 2 * pause - st) * 2
            sx = max(0, min(sm, sx))
            tft.fill_rect(CX, 15, CW, 8, bg)
            tft.text(font, dn, CX - sx, 15, NAME_CLR, bg)

        # Health dot + text at y=34
        hc = self._health_color()
        tft.fill_rect(CX + 1, 35, 3, 3, hc)
        ht = self.health.upper()
        if len(ht) > 9:
            ht = ht[:9]
        tft.text(font, ht, CX + 8, 34, hc, bg)

        # Uptime right-aligned
        up_s = self._fmt_up(self.uptime)
        uw = len(up_s) * 8
        tft.text(font, up_s, W - 6 - uw, 35, DIM, bg)

        # Separator
        tft.hline(CX, 50, CW, self._sep_clr)

        # Gauge bars
        labels = ("CPU", "MEM", "DSK", "I/O")
        values = (self.cpu, self.mem, self.disk, self.io)
        mv = memoryview(self._gauge_row) if self._gauge_row else None

        for idx in range(4):
            y = G_Y0 + idx * G_SP
            val = values[idx]

            # Label
            tft.text(font, labels[idx], G_LX, y, DIM, bg)

            # Track background
            tft.fill_rect(G_TX, y + 1, G_TW, G_TH, TRACK_BG)

            # Fill from rainbow sprite
            fw = max(0, min(G_TW, val * G_TW // 100))
            if fw > 0 and mv:
                sl = mv[:fw * 2]
                tft.blit_buffer(sl, G_TX, y + 1, fw, 1)
                tft.blit_buffer(sl, G_TX, y + 2, fw, 1)
                tft.blit_buffer(sl, G_TX, y + 3, fw, 1)

            # Value text right-aligned
            vs = str(val)
            vw = len(vs) * 8
            vc = CLAY if val > 80 else DIM
            tft.text(font, vs, CX + CW - vw, y, vc, bg)

    # ─── SCENE PANEL ────────────────────────────────────

    def _draw_scene(self):
        tft = self.tft
        top, mid, bot = sky_colors(self.hour)
        bh = SCENE_H // 3  # 24px per band

        tft.fill_rect(BAR, SCENE_T, W - BAR, bh, top)
        tft.fill_rect(BAR, SCENE_T + bh, W - BAR, bh, mid)
        tft.fill_rect(BAR, SCENE_T + 2 * bh, W - BAR, SCENE_H - 2 * bh, bot)

        # Frame lines
        tft.hline(BAR, SCENE_T, W - BAR, self._frm_clr)
        tft.hline(BAR, SCENE_B - 1, W - BAR, self._frm_clr)

    # ─── FOOT PANEL ─────────────────────────────────────

    def _draw_foot(self):
        tft = self.tft
        bg = self._bg

        # Panel background
        tft.fill_rect(BAR, SCENE_B, W - BAR, FOOT_H, bg)

        if not font:
            return

        # "OFFERINGS" header
        tft.text(font, "OFFERINGS", CX, OFF_HDR_Y, self._lbl_clr, bg)

        # Offering rows
        for idx, off in enumerate(self.offerings[:OFF_MAX]):
            y = OFF_LST_Y + idx * OFF_SP
            oh = off.get("h", "h")
            dc = SAGE if oh == "h" else (HONEY if oh == "w" else CLAY)
            tft.fill_rect(CX + 1, y + 2, 2, 2, dc)
            sn = off.get("n", "?")
            if len(sn) > 12:
                sn = sn[:12]
            tft.text(font, sn, CX + 7, y, OFF_NAME, bg)

        # "+N more"
        if self.off_count > OFF_MAX:
            extra = self.off_count - OFF_MAX
            y = OFF_LST_Y + OFF_MAX * OFF_SP
            tft.text(font, "+{} more".format(extra), CX + 7, y, OFF_MORE, bg)

        # Divider line above icons
        tft.hline(CX, ICN_Y - 5, CW, self._sep_clr)

        # Capability icons
        icons = []
        if self.seed_bank and self._icn_seed:
            icons.append((self._icn_seed, False))
        if self.has_gpu:
            if self.gpu_active and self._icn_ai_act:
                icons.append((self._icn_ai_act, True))
            elif self._icn_ai_on:
                icons.append((self._icn_ai_on, False))
        else:
            has_ollama = any(
                o.get("n", "").startswith("ollama") for o in self.offerings
            )
            if has_ollama and self._icn_ai_off:
                icons.append((self._icn_ai_off, False))
        if self.is_lantern and self._icn_lnt:
            icons.append((self._icn_lnt, False))

        if not icons:
            return

        total_w = len(icons) * ICN_SZ + (len(icons) - 1) * ICN_GAP
        ix = W - 6 - total_w

        for i, (buf, busy) in enumerate(icons):
            tft.blit_buffer(buf, ix, ICN_Y, ICN_SZ, ICN_SZ)
            # Round corners
            tft.pixel(ix, ICN_Y, bg)
            tft.pixel(ix + 7, ICN_Y, bg)
            tft.pixel(ix, ICN_Y + 7, bg)
            tft.pixel(ix + 7, ICN_Y + 7, bg)

            # Scanning underline for busy icons
            if busy:
                sy = ICN_Y + 9
                phase = (self.tick * 3) % (2 * ICN_SZ)
                if phase < ICN_SZ:
                    sx = ix + phase
                else:
                    sx = ix + 2 * ICN_SZ - phase - 1
                tft.pixel(sx, sy, HONEY)
                if ix <= sx - 1 < ix + ICN_SZ:
                    tft.pixel(sx - 1, sy, DIM)

            ix += ICN_SZ + ICN_GAP

    # ─── FRAME LOOP ────────────────────────────────────

    def draw_frame(self):
        """Called every frame in IDLE state (~10 FPS)."""
        self.tick += 1

        if self._head_dirty:
            self._draw_head()
            self._head_dirty = False
        else:
            # Check if name needs scroll update (every 3 ticks)
            if self.tick % 3 == 0:
                dn = self.name
                if dn.startswith("stone-"):
                    dn = dn[6:]
                if len(dn) * 8 > CW:
                    self._head_dirty = True

        if self._foot_dirty:
            self._draw_foot()
            self._foot_dirty = False

        # Scene redraws only when hour changes
        if self.hour != self._last_hour:
            self._draw_scene()
            self._last_hour = self.hour

    def draw_boot(self):
        """Boot splash screen."""
        tft = self.tft
        tft.fill(SKY_MID)
        if font:
            tft.text(font, "suzu", 28, 100, SAGE)
            tft.text(font, "Firefly", 40, 120, HONEY)
            tft.text(font, "Starting..", 30, 150, DIM)

    # ─── AMBIENT MODE (NO_COMM) ──────────────────────────

    def _init_ambient(self):
        """Set up deterministic stars and 3 ambient fireflies."""
        # 12 stars — deterministic positions from seed 42
        # Each: (x, y, base_brightness 0-3, period, phase)
        # period = ticks per full twinkle cycle (30-80 = 3-8 seconds at 10fps)
        seed = 42
        stars = []
        for _ in range(12):
            seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
            sx = seed % W
            seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
            sy = seed % H
            seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
            bright = seed % 4           # base brightness tier 0-3
            seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
            period = 30 + (seed % 50)   # 30-79 ticks (3-8s full cycle)
            seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
            phase = seed % period       # initial phase offset
            stars.append((sx, sy, bright, period, phase))
        self._stars = stars

        # 3 ambient fireflies with independent motion parameters
        # Layout per firefly (12 ints):
        #  [0]  x_10      (unused, position computed from center)
        #  [1]  y_10      (unused)
        #  [2]  x_phase   (0..x_period-1, advances +1/tick)
        #  [3]  y_phase   (0..y_period-1, advances +1/tick)
        #  [4]  x_period  (ticks per full X oscillation)
        #  [5]  y_period  (ticks per full Y oscillation)
        #  [6]  x_amp     (X amplitude in 1/10 pixel)
        #  [7]  y_amp     (Y amplitude in 1/10 pixel)
        #  [8]  cx        (X center in 1/10 pixel)
        #  [9]  cy        (Y center in 1/10 pixel)
        # [10]  pulse_phase (0..pulse_period-1, advances +1/tick)
        # [11]  pulse_period (ticks per full pulse cycle)
        self._ff = [
            # Slow floater — large lazy orbit, upper-left area
            # drift: x=149 ticks (14.9s), y=97 ticks (9.7s)
            # pulse: 31 ticks (3.1s) — slow breathing
            [0, 0,  0, 0,  149, 97,  350, 200,  350, 700,   0, 31],
            # Medium drifter — mid screen
            # drift: x=113 ticks (11.3s), y=79 ticks (7.9s)
            # pulse: 23 ticks (2.3s) — quicker pulse
            [0, 0,  0, 0,  113, 79,  280, 300,  700, 1300,  0, 23],
            # Quick wanderer — tighter path, lower-right
            # drift: x=83 ticks (8.3s), y=127 ticks (12.7s)
            # pulse: 37 ticks (3.7s) — slowest pulse, offset rhythm
            [0, 0,  0, 0,  83, 127,  250, 180,  1000, 1900, 0, 37],
        ]
        self._ff_prev = [None, None, None]

        self._amb_init = True

    def _draw_ff_glow(self, cx, cy, idx, pulse):
        """Draw a firefly with pulsing glow. pulse: -100..100 from sine."""
        tft = self.tft
        # pulse > 30:  full glow (outer + mid + core)
        # pulse > -30: mid + core only
        # pulse <= -30: core only (dim firefly, still visible)
        if pulse > 30:
            tft.fill_rect(cx - 3, cy - 3, 7, 7, _FF_OUT[idx])
        if pulse > -30:
            tft.fill_rect(cx - 2, cy - 2, 5, 5, _FF_MID[idx])
        tft.fill_rect(cx - 1, cy - 1, 3, 3, _FF_CORE[idx])

    def _erase_ff_glow(self, cx, cy):
        """Erase a firefly glow area with sky color."""
        self.tft.fill_rect(cx - 3, cy - 3, 7, 7, SKY_MID)

    def draw_no_comm(self):
        """Ambient mode: midnight sky, twinkling stars, drifting fireflies."""
        tft = self.tft
        self.tick += 1

        if not self._amb_init:
            self._init_ambient()
            # Full midnight sky fill on first frame
            tft.fill(SKY_MID)

        tk = self.tick

        # ── Stars: slow twinkle with individual brightness ──
        for sx, sy, bright, period, phase in self._stars:
            # Position in cycle: 0..period-1
            pos = (tk + phase) % period
            # Map to sine table index
            si = pos * _SIN_LEN // period
            sv = SIN64[si]  # -100..100

            # Combine base brightness with twinkle modulation
            # bright 0 = brightest star, 3 = faintest
            # sv modulates: at peak (100) show full brightness,
            # at trough (-100) go dark
            if sv > 50:
                tft.pixel(sx, sy, _STAR_B[bright])
            elif sv > 0:
                # Show one tier dimmer
                tier = min(3, bright + 1)
                tft.pixel(sx, sy, _STAR_B[tier])
            else:
                tft.pixel(sx, sy, SKY_MID)

        # ── Fireflies: Lissajous drift with glow ──
        for i, ff in enumerate(self._ff):
            # Unpack: [x_10, y_10, x_phase, y_phase,
            #          x_period, y_period, x_amp, y_amp, cx, cy]

            # Erase previous position
            prev = self._ff_prev[i]
            if prev:
                self._erase_ff_glow(prev[0], prev[1])
                # Restore stars under the erased glow
                for sx, sy, bright, period, phase in self._stars:
                    if prev[0] - 4 <= sx <= prev[0] + 4 and prev[1] - 4 <= sy <= prev[1] + 4:
                        pos = (tk + phase) % period
                        si = pos * _SIN_LEN // period
                        if SIN64[si] > 50:
                            tft.pixel(sx, sy, _STAR_B[bright])

            # Advance phases (each at its own rate)
            ff[2] = (ff[2] + 1) % ff[4]   # x drift
            ff[3] = (ff[3] + 1) % ff[5]   # y drift
            ff[10] = (ff[10] + 1) % ff[11] # pulse

            # Compute drift position via sine lookup
            x_si = ff[2] * _SIN_LEN // ff[4]
            y_si = ff[3] * _SIN_LEN // ff[5]
            px10 = ff[8] + ff[6] * SIN64[x_si] // 100
            py10 = ff[9] + ff[7] * SIN64[y_si] // 100

            # Convert to pixels, clamp to safe area
            px = max(4, min(W - 5, px10 // 10))
            py = max(4, min(H - 5, py10 // 10))

            # Compute pulse intensity
            p_si = ff[10] * _SIN_LEN // ff[11]
            pulse = SIN64[p_si]

            # Draw glow with pulse
            self._draw_ff_glow(px, py, i, pulse)
            self._ff_prev[i] = (px, py)
