"""
Firefly T-Display Main Loop

Hardware: TENSTAR T-Display ESP32-D0WD
  - ST7789 TFT 135x240 RGB565
  - SPI: MOSI=19, SCLK=18, CS=5, DC=16, RST=23, BL=4
  - Buttons: GPIO35, GPIO0
  - USB-UART: CH9102 (VID 0x1a86)

Serial protocol (115200 baud, \\n terminated):
  I             → OK,firefly-tdisplay,esp32,135x240
  C             → Clear display
  J,<json>      → Full state JSON push
  L,c,m,d,i,g,ga → Incremental load update
  H,<health>    → Health change
  +,<name>,<h>  → Service started
  -,<name>      → Service stopped
  T,<by>,<host> → Stone tended
  SD,<n>,<u>,<t>→ Seed bank detected
  SR            → Seed bank removed

State machine:
  BOOT → NO_COMM → CONNECT (dash animation) → IDLE (full diorama)
  IDLE → NO_COMM on 10s timeout

Requires: russhughes/st7789_mpy firmware (or compatible st7789 driver)
"""

import gc
import sys
import time
import json
import select

from machine import Pin, SPI, unique_id
import ubinascii


# firmware-side runtime truth only. Everything else
# (family, variant, display, capabilities) lives in /suzu.json
# — written at provisioning time by NewFirefly.ps1.
_FW_VERSION = "1.0.0"
_PROCESSOR = "esp32"


def _load_descriptor():
    try:
        with open("/suzu.json", "r") as f:
            return json.loads(f.read())
    except (OSError, ValueError):
        return {}


def _hardware_id():
    try:
        return "esp32-" + ubinascii.hexlify(unique_id()).decode("ascii")
    except Exception:
        return ""


_DESCRIPTOR = _load_descriptor()


def descriptor_json():
    """Merge runtime-truth fields into the provisioned descriptor."""
    d = dict(_DESCRIPTOR)
    d["hardware_id"] = _hardware_id()
    d["version"] = _FW_VERSION
    d["proto"] = "suzu/1"
    d["processor"] = _PROCESSOR
    return json.dumps(d)


def hello_frame():
    """suzu-identity unsolicited HELLO frame."""
    return "* HELLO," + descriptor_json()

# Try to import st7789 driver
try:
    import st7789
except ImportError:
    st7789 = None

from diorama import Diorama

# Display pins (TENSTAR T-Display)
TFT_MOSI = 19
TFT_SCLK = 18
TFT_CS = 5
TFT_DC = 16
TFT_RST = 23
TFT_BL = 4

# States
S_BOOT = 0
S_NO_COMM = 1
S_CONNECT = 2
S_IDLE = 3

# Timeouts
COMM_TIMEOUT_MS = 10000    # 10s before reverting to NO_COMM
FRAME_INTERVAL_MS = 100    # ~10 FPS target
CONNECT_ANIM_MS = 1500     # Connection animation duration

# UART
BAUD = 115200

# Globals
tft = None
scene = None
state = S_BOOT
last_rx = None
tick = 0
connect_start = 0


def uart_reply(msg):
    """Send reply over UART (stdin/stdout for REPL-based MicroPython)."""
    sys.stdout.write(msg + "\n")


def init_display():
    """Initialize the ST7789 TFT display and backlight."""
    global tft

    if st7789 is None:
        # Fallback: no display driver available
        uart_reply("ERR,no_st7789_driver")
        return False

    try:
        # Match russhughes tft_config.py exactly
        spi = SPI(2, baudrate=40000000, sck=Pin(TFT_SCLK), mosi=Pin(TFT_MOSI), miso=None)
        tft = st7789.ST7789(
            spi,
            135,
            240,
            reset=Pin(TFT_RST, Pin.OUT),
            cs=Pin(TFT_CS, Pin.OUT),
            dc=Pin(TFT_DC, Pin.OUT),
            backlight=Pin(TFT_BL, Pin.OUT),
            rotation=0,
        )
        tft.init()
        # Brief green flash proves display is alive
        tft.fill(st7789.GREEN)
        time.sleep_ms(150)
        tft.fill(0)
        return True
    except Exception as e:
        uart_reply("ERR,display_init:%s" % e)
        return False


def enter_state(new_state):
    """Transition to a new state."""
    global state, connect_start
    state = new_state

    if new_state == S_BOOT:
        if scene:
            scene.draw_boot()
    elif new_state == S_NO_COMM:
        # Dark sky with drifting fireflies
        pass
    elif new_state == S_CONNECT:
        connect_start = time.ticks_ms()
    elif new_state == S_IDLE:
        if scene:
            scene._head_dirty = True
            scene._foot_dirty = True


def handle_command(line):
    """Parse and execute a serial command."""
    global last_rx, state

    line = line.strip()
    if not line:
        return

    last_rx = time.ticks_ms()

    # If we're in NO_COMM and receive data, transition to CONNECT
    if state == S_NO_COMM:
        enter_state(S_CONNECT)
        if scene:
            scene._head_dirty = True
            scene._foot_dirty = True

    # Split command and arguments
    idx = line.find(",")
    if idx >= 0:
        cmd = line[:idx].upper()
        args = line[idx + 1:]
    else:
        cmd = line.upper()
        args = ""

    try:
        if cmd == "I":
            uart_reply("OK," + descriptor_json())
            return

        if cmd == "C":
            if tft:
                tft.fill(0)
            uart_reply("OK")
            return

        if cmd == "J":
            # Full JSON state push
            try:
                data = json.loads(args)
                if scene:
                    scene.apply_snapshot(data)
                    if state != S_IDLE:
                        enter_state(S_IDLE)
                uart_reply("OK")
            except ValueError as e:
                uart_reply("ERR,json:%s" % e)
            return

        if cmd == "L":
            # Incremental load: L,cpu,mem,disk,io,gpu,gpu_active
            parts = args.split(",")
            if len(parts) >= 6 and scene:
                scene.apply_load(
                    int(parts[0]),
                    int(parts[1]),
                    int(parts[2]),
                    int(parts[3]),
                    int(parts[4]),
                    bool(int(parts[5])),
                )
            # No reply (L is high-frequency, sent via send_command_no_wait on companion)
            return

        if cmd == "H":
            # Health change
            if scene and args:
                scene.apply_health(args.lower())
            # No reply (high-frequency possible)
            return

        if cmd == "+":
            # Service started: +,name,health_char
            parts = args.split(",", 1)
            name = parts[0] if parts else ""
            hc = parts[1] if len(parts) > 1 else "h"
            if scene:
                scene.service_started(name, hc)
            return

        if cmd == "-":
            # Service stopped: -,name
            if scene:
                scene.service_stopped(args)
            return

        if cmd == "T":
            # Stone tended: T,by,host
            if scene:
                scene.tended()
            return

        if cmd == "SD":
            # Seed bank detected: SD,name,used_gb,total_gb
            parts = args.split(",")
            if len(parts) >= 3 and scene:
                scene.seed_bank_detected(
                    parts[0],
                    int(parts[1]),
                    int(parts[2]),
                )
            return

        if cmd == "SR":
            # Seed bank removed
            if scene:
                scene.seed_bank_removed()
            return

        if cmd == "B":
            # Brightness: B,percent (on/off only without PWM)
            if tft:
                pct = max(0, min(100, int(args)))
                if pct > 0:
                    tft.on()
                else:
                    tft.off()
            uart_reply("OK")
            return

        uart_reply("ERR,unknown_cmd:%s" % cmd)

    except Exception as e:
        uart_reply("ERR,%s" % e)


def main():
    """Main loop: state machine + serial polling + rendering."""
    global scene, last_rx, tick, state

    uart_reply("Firefly T-Display starting...")
    # unsolicited HELLO so the bus identifies us during
    # the ESP32 boot window without needing an active `I` probe.
    uart_reply(hello_frame())

    if not init_display():
        uart_reply("ERR,failed_to_init_display")
        # Continue anyway — we can still handle serial commands
        # and the diorama will just not render

    # Create the diorama scene (works even without display for state tracking)
    scene = Diorama(tft)

    enter_state(S_NO_COMM)
    uart_reply("OK,ready")

    # Set up stdin polling for serial commands
    poll = select.poll()
    poll.register(sys.stdin, select.POLLIN)

    last_frame = time.ticks_ms()

    while True:
        try:
            now = time.ticks_ms()

            # Check communication timeout
            if last_rx is not None and state in (S_IDLE, S_CONNECT):
                if time.ticks_diff(now, last_rx) > COMM_TIMEOUT_MS:
                    enter_state(S_NO_COMM)

            # Render frame at target FPS
            if time.ticks_diff(now, last_frame) >= FRAME_INTERVAL_MS:
                last_frame = now
                tick += 1

                if tft and scene:
                    if state == S_BOOT:
                        scene.draw_boot()
                    elif state == S_NO_COMM:
                        scene.draw_no_comm()
                    elif state == S_CONNECT:
                        # Brief connection animation, then go to idle
                        scene.draw_no_comm()  # Keep showing something
                        if time.ticks_diff(now, connect_start) > CONNECT_ANIM_MS:
                            enter_state(S_IDLE)
                    elif state == S_IDLE:
                        scene.draw_frame()

            # Poll for serial input (non-blocking)
            events = poll.poll(0)
            if events:
                line = sys.stdin.readline()
                if line:
                    handle_command(line)
            else:
                time.sleep_ms(5)

            # Periodic GC
            if tick % 100 == 0:
                gc.collect()

        except KeyboardInterrupt:
            uart_reply("OK,interrupted")
            break
        except Exception as e:
            uart_reply("ERR,%s" % e)
            time.sleep_ms(100)


main()
