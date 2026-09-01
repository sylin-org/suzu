"""
Firefly OLED Display Driver for ESP8266 + SSD1306

Hardware: NodeMCU ESP8266 V3 with 0.96" OLED (128×64, SSD1306, I2C)
Display zones:
  - Yellow header: rows 0-15 (16px)
  - Blue body: rows 16-63 (48px)

I2C Pins (from datasheet):
  - SCL: GPIO14 (D5)
  - SDA: GPIO12 (D6)
"""

from machine import Pin, SoftI2C, unique_id
import ssd1306
import time
import ubinascii
import ujson
import profont_10 as font


# Firmware runtime fields. Everything else (family,
# variant, display shape, capabilities) lives in /suzu.json —
# written at provisioning time by NewFirefly.ps1. Firmware overlays
# these two fields before emitting the HELLO frame.
_FW_VERSION = "0.2.0"
_PROCESSOR = "esp8266"


def _load_descriptor():
    """Read the operator-provisioned descriptor once at module load.

    Returns an empty dict if /suzu.json is missing or malformed
    so the bus can classify the device as unprovisioned distinctly
    from 'no firefly-style response at all'.
    """
    try:
        with open("/suzu.json", "r") as f:
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
    """Merge runtime fields into the provisioned descriptor."""
    d = dict(_DESCRIPTOR)
    d["hardware_id"] = _hardware_id()
    d["version"] = _FW_VERSION
    d["proto"] = "suzu/1"
    d["processor"] = _PROCESSOR
    return ujson.dumps(d)


def hello_frame():
    """suzu-identity unsolicited HELLO frame emitted on boot."""
    return "* HELLO," + descriptor_json()

# Hardware configuration (from module datasheet)
I2C_SCL_PIN = 12  # D6
I2C_SDA_PIN = 14  # D5
I2C_FREQ = 400000

DISPLAY_WIDTH = 128
DISPLAY_HEIGHT = 64

# Display zones (hardware color zones)
YELLOW_ZONE_HEIGHT = 16  # Top 16 rows are yellow LEDs
BLUE_ZONE_START = 16     # Blue zone starts at row 16

# Health status icons (8×8 bitmaps)
ICON_THRIVING = bytearray([
    0b00111100,
    0b01111110,
    0b11111111,
    0b11111111,
    0b11111111,
    0b11111111,
    0b01111110,
    0b00111100,
])

ICON_WITHERING = bytearray([
    0b00111100,
    0b01100110,
    0b11000011,
    0b11000011,
    0b11000011,
    0b11000011,
    0b01100110,
    0b00111100,
])

ICON_WILTING = bytearray([
    0b11000011,
    0b01100110,
    0b00111100,
    0b00011000,
    0b00011000,
    0b00111100,
    0b01100110,
    0b11000011,
])


class FireflyOLED:
    """Firefly OLED display controller."""

    def __init__(self):
        """Initialize I2C and display."""
        self.i2c = SoftI2C(
            scl=Pin(I2C_SCL_PIN),
            sda=Pin(I2C_SDA_PIN),
            freq=I2C_FREQ
        )
        self.oled = ssd1306.SSD1306_I2C(DISPLAY_WIDTH, DISPLAY_HEIGHT, self.i2c)
        self.stone_name = "unknown"
        self.health_state = "thriving"
        self.cpu_percent = 0
        self.mem_percent = 0
        self.uptime = "0h 0m"
        # Header scroll: tick-based (shader approach)
        self._tick = 0
        self._header_width = 120

    def clear(self):
        """Clear the entire display."""
        self.oled.fill(0)
        self.oled.show()

    def fill(self, value=1):
        """Fill entire display with value (0=black, 1=white)."""
        self.oled.fill(value)
        self.oled.show()

    def text(self, message, x, y, use_font=None):
        """Draw text at position using optional custom font."""
        if use_font is None:
            self.oled.text(message, x, y)
        else:
            use_font.draw(self.oled, message, x, y)

    def show(self):
        """Update the display."""
        self.oled.show()

    def draw_icon(self, icon_data, x, y):
        """Draw an 8×8 icon bitmap at position."""
        for row in range(8):
            byte = icon_data[row]
            for col in range(8):
                if byte & (0x80 >> col):
                    self.oled.pixel(x + col, y + row, 1)

    def draw_hline(self, x, y, width, value=1):
        """Draw horizontal line."""
        self.oled.hline(x, y, width, value)

    def draw_rect(self, x, y, w, h, value=1):
        """Draw rectangle outline."""
        self.oled.rect(x, y, w, h, value)

    def draw_fill_rect(self, x, y, w, h, value=1):
        """Draw filled rectangle."""
        self.oled.fill_rect(x, y, w, h, value)

    def draw_progress_bar(self, x, y, width, height, percent):
        """Draw a progress bar."""
        # Outline
        self.oled.rect(x, y, width, height, 1)
        # Fill
        fill_width = int((width - 2) * percent / 100)
        if fill_width > 0:
            self.oled.fill_rect(x + 1, y + 1, fill_width, height - 2, 1)

    def get_health_icon(self, state):
        """Get icon data for health state."""
        if state == "thriving":
            return ICON_THRIVING
        elif state == "withering":
            return ICON_WITHERING
        elif state == "wilting":
            return ICON_WILTING
        else:
            return ICON_WITHERING

    def draw_status_screen(self):
        """Draw the main status overview screen."""
        self.oled.fill(0)

        # Yellow zone: Stone name with tick-based scrolling
        name = self.stone_name.upper()
        name_width = font.text_width(name)

        if name_width <= self._header_width:
            # Short name - just display it
            self.text(name, 2, 3, font)
        else:
            # Long name - calculate scroll from tick (shader approach)
            # Cycle: pause(20) -> scroll right -> pause(20) -> scroll left
            scroll_max = name_width - self._header_width + 4
            pause_ticks = 20  # pause at each end
            scroll_ticks = scroll_max // 2  # ticks to scroll full distance
            cycle = 2 * pause_ticks + 2 * scroll_ticks
            t = self._tick % cycle

            if t < pause_ticks:
                # Pause at start
                scroll_x = 0
            elif t < pause_ticks + scroll_ticks:
                # Scroll right
                scroll_x = (t - pause_ticks) * 2
            elif t < 2 * pause_ticks + scroll_ticks:
                # Pause at end
                scroll_x = scroll_max
            else:
                # Scroll left
                scroll_x = scroll_max - (t - 2 * pause_ticks - scroll_ticks) * 2

            scroll_x = max(0, min(scroll_max, scroll_x))
            self.text(name, 2 - scroll_x, 3, font)
            self._tick += 1

        # Blue zone: Health status with icon
        icon = self.get_health_icon(self.health_state)
        self.draw_icon(icon, 2, 18)

        # Status text
        status_text = self.health_state.upper()
        self.text(status_text, 14, 19, font)

        # Uptime (right aligned)
        self.text(self.uptime, 78, 19, font)

        # CPU bar
        self.text("CPU", 2, 32, font)
        self.draw_progress_bar(28, 34, 76, 6, self.cpu_percent)
        self.text(f"{self.cpu_percent}%", 108, 32, font)

        # Memory bar
        self.text("MEM", 2, 44, font)
        self.draw_progress_bar(28, 46, 76, 6, self.mem_percent)
        self.text(f"{self.mem_percent}%", 108, 44, font)

        self.show()
        return True

    def draw_boot_screen(self):
        """Draw boot splash screen."""
        self.oled.fill(0)

        # Yellow zone
        self.text("suzu", 38, 3, font)

        # Blue zone
        self.text("ZEN GARDEN", 24, 28, font)
        self.text("Initializing...", 18, 48, font)

        self.show()

    def update_metrics(self, cpu=None, mem=None, uptime=None):
        """Update displayed metrics."""
        if cpu is not None:
            self.cpu_percent = min(100, max(0, cpu))
        if mem is not None:
            self.mem_percent = min(100, max(0, mem))
        if uptime is not None:
            self.uptime = uptime

    def set_stone_name(self, name):
        """Set the stone name to display."""
        self.stone_name = name
        self._tick = 0  # Reset scroll position

    def set_health(self, state):
        """Set health state: thriving, withering, wilting."""
        if state in ("thriving", "withering", "wilting", "resting", "offline"):
            self.health_state = state

    def device_info(self):
        """Return the descriptor JSON framed as `OK,{...}` for the `I` command.

        suzu identity protocol: emits the same descriptor as HELLO does on
        boot. Uses descriptor_json() for all identity replies.
        """
        return "OK," + descriptor_json()

    # ==================== ANIMATIONS ====================

    def wipe(self, line1, line2, direction="in", wipe_ms=300, hold_ms=1500):
        """
        Wipe transition: reveal text then clear.
        direction: "in" (left→right) or "out" (right→left)
        """
        bar_width = 12  # ~11 steps for fast wipe
        steps = DISPLAY_WIDTH // bar_width
        delay = wipe_ms // steps // 2  # divide by 2 for reveal + clear phases

        # Prepare text in buffer (hidden)
        self.oled.fill(0)
        # Center text in blue zone
        self.text(line1.upper(), 4, 24, font)
        self.text(line2.upper(), 4, 40, font)

        # Phase 1: Wipe reveals content
        if direction == "in":
            # Left to right
            for x in range(0, DISPLAY_WIDTH, bar_width):
                # Draw white bar at leading edge
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 1)
                self.show()
                time.sleep_ms(delay // 2)
                # Redraw text in revealed area
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 0)
                self.text(line1.upper(), 4, 24, font)
                self.text(line2.upper(), 4, 40, font)
                # Mask unrevealed area
                if x + bar_width < DISPLAY_WIDTH:
                    self.oled.fill_rect(x + bar_width, BLUE_ZONE_START, DISPLAY_WIDTH - x - bar_width, 48, 0)
                self.show()
                time.sleep_ms(delay // 2)
        else:
            # Right to left
            for x in range(DISPLAY_WIDTH - bar_width, -bar_width, -bar_width):
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 1)
                self.show()
                time.sleep_ms(delay // 2)
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 0)
                self.text(line1.upper(), 4, 24, font)
                self.text(line2.upper(), 4, 40, font)
                if x > 0:
                    self.oled.fill_rect(0, BLUE_ZONE_START, x, 48, 0)
                self.show()
                time.sleep_ms(delay // 2)

        # Hold
        time.sleep_ms(hold_ms)

        # Phase 2: Wipe clears to black
        if direction == "in":
            for x in range(0, DISPLAY_WIDTH, bar_width):
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 1)
                self.show()
                time.sleep_ms(delay // 2)
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 0)
                self.show()
                time.sleep_ms(delay // 2)
        else:
            for x in range(DISPLAY_WIDTH - bar_width, -bar_width, -bar_width):
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 1)
                self.show()
                time.sleep_ms(delay // 2)
                self.oled.fill_rect(x, BLUE_ZONE_START, bar_width, 48, 0)
                self.show()
                time.sleep_ms(delay // 2)

    def blink(self, count=3, on_ms=200, off_ms=200):
        """
        Blink the blue zone content.
        Flashes current content on/off.
        """
        # Save current contrast
        for _ in range(count):
            self.oled.fill_rect(0, BLUE_ZONE_START, DISPLAY_WIDTH, 48, 0)
            self.show()
            time.sleep_ms(off_ms)
            self.draw_status_screen()
            time.sleep_ms(on_ms)

    def pulse(self, count=3, step_ms=30):
        """
        Pulse brightness using contrast control.
        Creates a pulsing brightness effect.
        """
        for _ in range(count):
            # Fade out
            for c in range(255, 0, -15):
                self.oled.contrast(c)
                time.sleep_ms(step_ms)
            # Fade in
            for c in range(0, 256, 15):
                self.oled.contrast(c)
                time.sleep_ms(step_ms)
        # Restore full brightness
        self.oled.contrast(255)

    # Removed: scroll, spinner, slide - to save memory on ESP8266
