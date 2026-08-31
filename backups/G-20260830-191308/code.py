# Firefly V0 Firmware for Waveshare RP2040-Matrix
# Zen Garden Project - https://github.com/zen-garden
#
# Protocol: Text-based serial commands (115200 baud)
# Commands:
#   P,x,y,r,g,b   - Set pixel at (x,y) to RGB color
#   F,r,g,b       - Fill all pixels with RGB color
#   C             - Clear (all off)
#   B,percent     - Set brightness (0-100)
#   A,name        - Play animation (rainbow|pulse|chase|sparkle)
#   S             - Stop animation
#   T,status      - Show status (healthy|warning|error|offline)
#   I             - Info (returns device info)
#   ?             - Help

import board
import neopixel
import time
import supervisor
import sys

# Configuration
NUM_LEDS = 25
ROWS = 5
COLS = 5
LED_PIN = board.GP16  # WS2812 data pin on RP2040-Matrix

# Initialize NeoPixels
pixels = neopixel.NeoPixel(LED_PIN, NUM_LEDS, brightness=0.3, auto_write=False)

# State
current_animation = None
animation_frame = 0
last_frame_time = 0
blink_state = True

# Color presets for status
STATUS_COLORS = {
    "healthy": (0, 180, 0),
    "warning": (200, 150, 0),
    "error": (200, 0, 0),
    "offline": (0, 0, 0),
}

def xy_to_index(x, y):
    """Convert x,y coordinates to LED index (row-major, top-left origin)."""
    if 0 <= x < COLS and 0 <= y < ROWS:
        return y * COLS + x
    return None

def fill_color(r, g, b):
    """Fill all pixels with a color."""
    pixels.fill((r, g, b))
    pixels.show()

def set_pixel(x, y, r, g, b):
    """Set a single pixel."""
    idx = xy_to_index(x, y)
    if idx is not None:
        pixels[idx] = (r, g, b)
        pixels.show()
        return True
    return False

def clear():
    """Turn off all pixels."""
    pixels.fill((0, 0, 0))
    pixels.show()

def set_brightness(percent):
    """Set global brightness (0-100)."""
    pixels.brightness = max(0, min(100, percent)) / 100.0
    pixels.show()

def show_status(status):
    """Show a status indicator."""
    global current_animation, blink_state
    color = STATUS_COLORS.get(status.lower(), (100, 100, 100))
    if status.lower() == "error":
        current_animation = ("blink", color)
        blink_state = True
    else:
        current_animation = None
        fill_color(*color)

def wheel(pos):
    """Generate rainbow colors across 0-255 positions."""
    if pos < 85:
        return (pos * 3, 255 - pos * 3, 0)
    elif pos < 170:
        pos -= 85
        return (255 - pos * 3, 0, pos * 3)
    else:
        pos -= 170
        return (0, pos * 3, 255 - pos * 3)

def animate_rainbow(frame):
    """Rainbow cycle animation."""
    for i in range(NUM_LEDS):
        pixel_index = (i * 256 // NUM_LEDS + frame) % 256
        pixels[i] = wheel(pixel_index)
    pixels.show()

def animate_pulse(frame):
    """Breathing/pulse animation."""
    # Sine-ish brightness curve using frame
    brightness = abs((frame % 100) - 50) / 50.0
    brightness = 0.1 + brightness * 0.5  # Range 0.1 to 0.6
    pixels.brightness = brightness
    if frame == 0:
        pixels.fill((0, 180, 0))  # Green pulse
    pixels.show()

def animate_chase(frame):
    """Single LED chasing around perimeter."""
    # Perimeter indices: top row, right col, bottom row reversed, left col reversed
    perimeter = [0, 1, 2, 3, 4, 9, 14, 19, 24, 23, 22, 21, 20, 15, 10, 5]
    pixels.fill((0, 0, 0))
    idx = perimeter[frame % len(perimeter)]
    pixels[idx] = (0, 150, 255)
    # Trail
    trail_idx = perimeter[(frame - 1) % len(perimeter)]
    pixels[trail_idx] = (0, 50, 80)
    trail_idx2 = perimeter[(frame - 2) % len(perimeter)]
    pixels[trail_idx2] = (0, 20, 30)
    pixels.show()

def animate_sparkle(frame):
    """Random sparkle effect."""
    import random
    # Dim all pixels slightly
    for i in range(NUM_LEDS):
        r, g, b = pixels[i]
        pixels[i] = (max(0, r - 20), max(0, g - 20), max(0, b - 20))
    # Add random sparkles
    for _ in range(3):
        idx = random.randint(0, NUM_LEDS - 1)
        pixels[idx] = (255, 255, 255)
    pixels.show()

def animate_blink(frame, color):
    """Blinking animation for error status."""
    global blink_state
    if frame % 10 == 0:
        blink_state = not blink_state
    if blink_state:
        pixels.fill(color)
    else:
        pixels.fill((0, 0, 0))
    pixels.show()

def start_animation(name):
    """Start a named animation."""
    global current_animation, animation_frame
    name = name.lower()
    if name in ("rainbow", "pulse", "chase", "sparkle"):
        current_animation = (name, None)
        animation_frame = 0
        return True
    return False

def stop_animation():
    """Stop current animation."""
    global current_animation
    current_animation = None
    clear()

def update_animation():
    """Update animation frame if one is running."""
    global animation_frame, last_frame_time

    if current_animation is None:
        return

    now = time.monotonic()
    frame_delay = 0.03  # ~30fps

    if now - last_frame_time < frame_delay:
        return

    last_frame_time = now
    anim_type, anim_data = current_animation

    if anim_type == "rainbow":
        animate_rainbow(animation_frame)
    elif anim_type == "pulse":
        animate_pulse(animation_frame)
    elif anim_type == "chase":
        animate_chase(animation_frame)
    elif anim_type == "sparkle":
        animate_sparkle(animation_frame)
    elif anim_type == "blink":
        animate_blink(animation_frame, anim_data)

    animation_frame += 1

def parse_color(value):
    """Parse color value - either hex (ff0000) or decimal (255)."""
    value = value.strip()
    if len(value) == 6:
        # Hex color
        try:
            r = int(value[0:2], 16)
            g = int(value[2:4], 16)
            b = int(value[4:6], 16)
            return r, g, b
        except ValueError:
            pass
    return None

def process_command(line):
    """Process a single command line."""
    global current_animation

    line = line.strip()
    if not line:
        return None

    parts = line.split(",")
    cmd = parts[0].upper()
    args = parts[1:] if len(parts) > 1 else []

    try:
        if cmd == "P" and len(args) >= 5:
            # Pixel: P,x,y,r,g,b
            x, y = int(args[0]), int(args[1])
            r, g, b = int(args[2]), int(args[3]), int(args[4])
            current_animation = None
            if set_pixel(x, y, r, g, b):
                return "OK"
            return "ERR,invalid coordinates"

        elif cmd == "F" and len(args) >= 3:
            # Fill: F,r,g,b
            r, g, b = int(args[0]), int(args[1]), int(args[2])
            current_animation = None
            fill_color(r, g, b)
            return "OK"

        elif cmd == "C":
            # Clear
            current_animation = None
            clear()
            return "OK"

        elif cmd == "B" and len(args) >= 1:
            # Brightness: B,percent
            percent = int(args[0])
            set_brightness(percent)
            return "OK"

        elif cmd == "A" and len(args) >= 1:
            # Animate: A,name
            name = args[0].strip()
            if start_animation(name):
                return "OK"
            return "ERR,unknown animation"

        elif cmd == "S":
            # Stop animation
            stop_animation()
            return "OK"

        elif cmd == "T" and len(args) >= 1:
            # Status: T,status
            status = args[0].strip()
            show_status(status)
            return "OK"

        elif cmd == "I":
            # Info
            return "OK,firefly-v0,rp2040-matrix,5x5"

        elif cmd == "?":
            # Help
            return "OK,P|F|C|B|A|S|T|I|?"

        else:
            return "ERR,unknown command"

    except (ValueError, IndexError) as e:
        return f"ERR,parse error: {e}"

def boot_animation():
    """Play a brief boot animation."""
    # Quick rainbow sweep
    for frame in range(50):
        animate_rainbow(frame * 5)
        time.sleep(0.02)

    # Fade to green
    for brightness in range(60, 10, -5):
        pixels.brightness = brightness / 100.0
        pixels.fill((0, 180, 0))
        pixels.show()
        time.sleep(0.05)

    # Settle to dim green (idle)
    pixels.brightness = 0.2
    pixels.fill((0, 80, 0))
    pixels.show()
    time.sleep(0.3)
    clear()

# Main
print("Firefly V0 - Zen Garden LED Controller")
print("Ready. Send ? for help.")

boot_animation()

# Input buffer
input_buffer = ""

while True:
    # Update animation if running
    update_animation()

    # Check for serial input (non-blocking)
    if supervisor.runtime.serial_bytes_available:
        char = sys.stdin.read(1)
        if char == "\n" or char == "\r":
            if input_buffer:
                response = process_command(input_buffer)
                if response:
                    print(response)
                input_buffer = ""
        else:
            input_buffer += char

    # Small delay to prevent busy-waiting
    time.sleep(0.001)
