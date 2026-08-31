# boot.py - Firefly OLED v2 boot script
# Runs before main.py
#
# Fireflies are USB-tethered by design: the host (garden-firefly) sends all
# commands over serial. Disabling WiFi saves ~70mA and removes an unused
# attack surface.
import network
try:
    network.WLAN(network.STA_IF).active(False)
    network.WLAN(network.AP_IF).active(False)
except Exception:
    pass  # If WiFi subsystem isn't present, skip silently.

import gc
gc.collect()  # Free memory before importing main
