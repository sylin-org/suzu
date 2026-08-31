# firmware/ — suzu device firmware

Harvested from the ancestor project's proof-of-concept firmware, then
swept into suzu's language: same faces, same grammar, suzu identity.

## Layout — tier `suzu-d`, one folder per board class

| Folder | Board class | Runtime | Faces |
|---|---|---|---|
| `esp8266-oled-v2/` | NodeMCU ESP8266 + 0.96" OLED (dual-zone) | MicroPython | dense icon dashboard, wipes, spinner, no-comm fireflies |
| `esp8266-oled-v1/` | NodeMCU ESP8266 + 0.96" OLED (classic) | MicroPython | name/health/metrics status screen |
| `esp32-tdisplay/` | T-Display ESP32 1.14" ST7789 | MicroPython (russhughes st7789) | three-panel diorama, gauges, sky, starfield idle |
| `rp2040-matrix/` | Waveshare RP2040-Matrix 5×5 | CircuitPython | host-composed frames + built-in animations |

`suzu-a` (the ember tier) has no ancestor — it will be written fresh.

## Identity

Each folder carries a `suzu.json` descriptor template. Adoption writes
the real descriptor (minting a device_id, or preserving one from
ancestor provisioning); every family's `descriptor_json()` merges
`"proto": "suzu/1"` into the response, so migrated boards answer the
handshake as suzu while keeping their identity.

## Install paths

- MicroPython boards (ESP8266, ESP32): REPL push — chunked escaped
  writes over the serial REPL (see the resident's procedure engine).
- RP2040 (CircuitPython): drive copy onto CIRCUITPY; blank boards first
  take the CircuitPython UF2 via BOOTSEL.

Production pushes pre-compiled `.mpy` artifacts (mpy-cross); this repo
harvests the readable `.py` sources. Font and icon assets keep their
upstream licenses (see `OPEN-ICONIC-LICENSE`).
