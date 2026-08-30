# Census — Waveshare RP2040-Matrix population study
#
# Started 2026-08-28. One row per individual; evidence/ holds the dated
# raw captures. The class signature is what never varies across rows.

## Signature invariants (4/4 units)

- VID/PID `239a:80f4` — CircuitPython CDC (Adafruit VID, usbser driver)
- product "USB Serial Device", manufacturer reported as Microsoft
- serial always `E66…` — RP2040 flash-derived
- silent to `I` — stock CircuitPython, no pre-suzu identity on any unit

## Individuals

| unit | serial | lot prefix | sessions | state |
|---|---|---|---|---|
| 1 | `E6625887D3331037` | A `E6625887D3` | COM15 ×2 | CP; one wedged-CDC episode (os-error-22), clean after RESET |
| 2 | `E6625C05E71C4F25` | B `E6625C05E7` | COM16 | CP, clean |
| 3 | `E6625887D3807037` | A | COM18 | CP, clean |
| 4 | `E6625C05E70C4C25` | B | COM20 | CP, clean |
| 5 | `E6625C05E7355A25` | B | COM21 | CP, clean |
| 6 | `E6625C05E7776A23` | B | COM22 | CP, clean |
| 7 | `E6625C05E7958826` | B | COM23 | CP, clean |

## Lot hypothesis

Two production lots, currently 2 A + 5 B. Serial prefix = lot marker,
pending more samples.

## Operational notes

- The wedged-CDC state (write → os-error-22) recovered with the board
  RESET button; procedures should treat os-error-22 as "ask for RESET,
  retry once" rather than failure.
- All units take the `circuitpy-drive-copy` upgrade path — none has
  pre-suzu firmware.


## LED order (2026-08-30, the golden/green mismatch)

The 5x5's SK6812/WS2812 pixels are **GRB-wired**, but CircuitPython's
`neopixel` library silently defaults to RGB — so every hue the face
displayed was red/green-scrambled on the physical board while the J
shot (logical rgb) stayed truthful. Symptom: work atoms golden in the
workbench preview, green on the board. Fix: `pixel_order=neopixel.GRB`
in the NeoPixel constructor. Rule for this class: the preview is the
frame buffer's truth; when eyes and frame disagree, suspect the wire
order first.
