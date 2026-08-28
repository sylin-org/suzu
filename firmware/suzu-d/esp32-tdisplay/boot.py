# boot.py - Firefly T-Display boot script
# Runs before main.py on ESP32 T-Display (ST7789 135x240)
#
# Fireflies are USB-tethered by design: the host (suzu) sends all
# commands over serial. Disabling WiFi (and Bluetooth on ESP32) saves power
# and removes an unused attack surface.
import network
try:
    network.WLAN(network.STA_IF).active(False)
    network.WLAN(network.AP_IF).active(False)
except Exception:
    pass

# ESP32 also has Bluetooth — deinit if present
try:
    import bluetooth
    bt = bluetooth.BLE()
    if bt.active():
        bt.active(False)
except Exception:
    pass

import gc
gc.collect()
